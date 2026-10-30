//! A mod's Luau VM: sandboxed, with the natives as namespace tables, `wait`
//! and `require` (the mod's own files); its main chunk runs as a coroutine
//! the engine resumes once per wait. The REPL's VM (`eval`) is the same
//! with a loop that runs the snippets it is given.

use std::cell::{Cell, RefCell};
use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::mpsc::Sender;
use std::time::{Duration, Instant};

use mlua::{Function, Lua, LuaOptions, MultiValue, StdLib, Table, Thread, Value, VmState};

use crate::storage::Storage;
use crate::{EvalResult, input, json, menus, natives};

/// How long a mod may run before it yields to the game for this frame.
pub const SLICE: Duration = Duration::from_millis(2);
/// The time a mod's `mod.onStop` functions have, all together: they run
/// once, as it stops, and undo what it did (a trainer turns off every
/// feature it has on).
const LAST_TURN: Duration = Duration::from_millis(20);
/// How long a mod may run without calling `wait` before it is stopped.
const RUNAWAY: Duration = Duration::from_secs(2);
/// Memory a mod's VM may use.
const MEMORY: usize = 64 << 20;

/// Snippets for the REPL: code, and where its result goes.
pub type Jobs = Rc<RefCell<VecDeque<(String, Sender<EvalResult>)>>>;

pub struct Vm {
    lua: Lua,
    module: String,
    /// The main chunk, until it ends, and when it is due (game time, ms).
    main: Option<Thread>,
    main_wake: u64,
    main_busy: Duration,
    /// Tasks: `task.spawn`/`task.delay`, menus' callbacks.
    sched: Sched,
    deadline: Rc<Cell<Instant>>,
    /// When this turn's time is up (all its coroutines share it).
    turn_end: Cell<Instant>,
    /// Which goes first this turn, the main chunk or the tasks (they take
    /// turns, so neither starves the other).
    main_first: Cell<bool>,
    /// The mod's `storage` (none for the REPL).
    storage: Option<Rc<Storage>>,
    /// Its database (none for the REPL).
    db: Option<Rc<crate::db::Db>>,
}

/// What `task.*` and the menus share with the VM.
#[derive(Clone, Default)]
struct Sched {
    tasks: Rc<RefCell<Vec<Task>>>,
    /// The game time of the current turn, ms.
    now: Rc<Cell<u64>>,
    handlers: menus::Handlers,
    /// What the mod sees of the keyboard, the mouse and the pad.
    keys: input::Keys,
    /// This run of the mod (its menus are its own).
    instance: u64,
    /// The coroutine the scheduler is running (`Thread::to_pointer`).
    resumed: Rc<Cell<*const std::ffi::c_void>>,
    /// `mod.onStop`'s functions, for the mod's last turn.
    stop: Rc<RefCell<Vec<Function>>>,
}

struct Task {
    thread: Thread,
    wake: u64,
    /// What it starts with (a callback's arguments), the first time.
    args: Option<MultiValue>,
    busy: Duration,
}

/// What a turn of a mod came to.
pub enum Step {
    /// The next thing it does is this many milliseconds away (0: the next
    /// frame).
    Wait(u32),
    Finished,
    Failed(String),
}

/// What a coroutine's run came to.
enum Outcome {
    Wait(u32),
    Preempted,
    Finished,
    Failed(String),
}

impl Vm {
    /// Loads mod `name`'s `main.luau` from `dir`; `log` gets what it prints.
    pub fn new(name: &str, dir: &Path, log: Rc<dyn Fn(&str)>) -> mlua::Result<Vm> {
        let sched = Sched { instance: menus::next_instance(), ..Sched::default() };
        let (lua, deadline) = base(name, log, &sched)?;
        lua.globals().set("require", require(&lua, name, dir)?)?;
        let storage = Storage::open(name);
        storage.install(&lua)?;
        let db = crate::db::Db::new(name);
        db.install(&lua)?;
        crate::folder::install(&lua, name, dir)?;
        crate::draw::set_folder(&lua, name);
        // The mod's code starts once the player is in the game (not on
        // the loading screens, where the player's ped is a stand-in).
        let ready: Function = lua
            .load(
                "local yield = coroutine.yield\n\
                 return function(main) return function(...)\n\
                 while dlc.getIsLoadingScreenActive() or not camera.isScreenFadedIn() \
                 or not player.isPlayerPlaying(player.playerId()) do yield(0) end\n\
                 return main(...)\n\
                 end end",
            )
            .set_name("=ready")
            .eval()?;
        lua.sandbox(true)?;
        let path = dir.join("main.luau");
        let source = std::fs::read_to_string(&path)
            .map_err(|e| mlua::Error::runtime(format!("reading {}: {e}", path.display())))?;
        let chunk = lua.load(source).set_name(format!("@{name}/main.luau")).into_function()?;
        let main = lua.create_thread(ready.call::<Function>(chunk)?)?;
        Ok(Vm {
            lua,
            module: name.to_string(),
            main: Some(main),
            main_wake: 0,
            main_busy: Duration::ZERO,
            sched,
            deadline,
            turn_end: Cell::new(Instant::now()),
            main_first: Cell::new(false),
            storage: Some(storage),
            db: Some(db),
        })
    }

    /// The REPL: runs `jobs`' snippets, as expressions when they are.
    pub fn repl(jobs: Jobs, log: Rc<dyn Fn(&str)>) -> mlua::Result<Vm> {
        let sched = Sched { instance: menus::next_instance(), ..Sched::default() };
        let (lua, deadline) = base("eval", log, &sched)?;
        let waiting: Rc<RefCell<HashMap<i64, Sender<EvalResult>>>> = Rc::default();
        let next = Cell::new(0i64);
        let pending = waiting.clone();
        let take = lua.create_function(move |lua, ()| {
            let Some((code, reply)) = jobs.borrow_mut().pop_front() else { return Ok(MultiValue::new()) };
            let id = next.get() + 1;
            next.set(id);
            let compiled = lua
                .load(format!("return {code}"))
                .set_name("=eval")
                .into_function()
                .or_else(|_| lua.load(code.as_str()).set_name("=eval").into_function());
            Ok(match compiled {
                Ok(f) => {
                    pending.borrow_mut().insert(id, reply);
                    MultiValue::from_vec(vec![Value::Integer(id), Value::Function(f)])
                }
                Err(e) => {
                    let _ = reply.send(Err(error_text(&e)));
                    MultiValue::new()
                }
            })
        })?;
        let reply = lua.create_function(move |_, args: MultiValue| {
            let mut args = args.into_iter();
            let id = match args.next() {
                Some(Value::Integer(id)) => id,
                _ => return Ok(()),
            };
            let ok = matches!(args.next(), Some(Value::Boolean(true)));
            let values: Vec<Value> = args.collect();
            if let Some(sender) = waiting.borrow_mut().remove(&id) {
                let _ = sender.send(if ok {
                    Ok(describe_list(values.iter(), ", ", true))
                } else {
                    Err(values.first().map_or_else(|| "error".into(), describe_plain))
                });
            }
            Ok(())
        })?;
        let main: Function = lua
            .load(
                "local take, reply = ...\n\
                 return function()\n\
                 while true do\n\
                 local id, f = take()\n\
                 if id then reply(id, pcall(f)) else wait() end\n\
                 end end",
            )
            .set_name("=repl")
            .call((take, reply))?;
        lua.sandbox(true)?;
        let main = lua.create_thread(main)?;
        Ok(Vm { lua, module: "eval".into(), main: Some(main), main_wake: 0, main_busy: Duration::ZERO, sched, deadline, turn_end: Cell::new(Instant::now()), main_first: Cell::new(false), storage: None, db: None })
    }

    /// A turn at game time `now` (ms): what the player did with the menus
    /// becomes tasks; the tasks and the main chunk that are due run, until
    /// they wait, end, fail or run out of time.
    pub fn step(&mut self, now: u64, slice: Duration) -> Step {
        self.sched.now.set(now);
        self.turn_end.set(Instant::now() + slice.min(SLICE));
        self.main_first.set(!self.main_first.get());
        if let Some(storage) = &self.storage {
            storage.flush(false);
        }
        // Keys since the last turn: the handlers' calls become tasks.
        match self.sched.keys.take(&self.lua) {
            Ok(calls) => {
                let mut tasks = self.sched.tasks.borrow_mut();
                for (f, args) in calls {
                    match self.lua.create_thread(f) {
                        Ok(thread) => tasks.push(Task { thread, wake: now, args: Some(args), busy: Duration::ZERO }),
                        Err(e) => crate::log_line(&self.module, crate::LogLevel::Error, &error_text(&e)),
                    }
                }
            }
            Err(e) => crate::log_line(&self.module, crate::LogLevel::Error, &error_text(&e)),
        }
        // The players' inputs (the items' values change first) and menus
        // opened or closed: their callbacks become tasks.
        match menus::take_calls(&self.lua, &self.sched.handlers, self.sched.instance) {
            Ok(calls) => {
                let mut tasks = self.sched.tasks.borrow_mut();
                for (f, args) in calls {
                    match self.lua.create_thread(f) {
                        Ok(thread) => tasks.push(Task { thread, wake: now, args: Some(args), busy: Duration::ZERO }),
                        Err(e) => crate::log_line(&self.module, crate::LogLevel::Error, &error_text(&e)),
                    }
                }
            }
            Err(e) => crate::log_line(&self.module, crate::LogLevel::Error, &error_text(&e)),
        }
        if self.main_first.get() {
            if let Some(failed) = self.run_main(now) {
                return failed;
            }
            self.run_tasks(now);
        } else {
            self.run_tasks(now);
            if let Some(failed) = self.run_main(now) {
                return failed;
            }
        }
        // What it drew shows, unless its time ran out before it was done.
        crate::draw::end_turn(&self.lua, Instant::now() < self.turn_end.get());
        // A menu may be used, a key pressed any time: every frame then.
        let player = menus::has_menus(self.sched.instance) || self.sched.keys.listening();
        let tasks = self.sched.tasks.borrow();
        if self.main.is_none() && tasks.is_empty() && !player {
            return Step::Finished;
        }
        if player {
            return Step::Wait(0);
        }
        let next = tasks.iter().map(|t| t.wake).chain(self.main.as_ref().map(|_| self.main_wake)).min().unwrap_or(now);
        Step::Wait(next.saturating_sub(now).min(u64::from(u32::MAX)) as u32)
    }

    /// The tasks that are due, while the turn lasts.
    fn run_tasks(&mut self, now: u64) {
        // The tasks that are due; others may be added while they run.
        let due: Vec<Task> = {
            let mut tasks = self.sched.tasks.borrow_mut();
            let (due, later): (Vec<Task>, Vec<Task>) = tasks.drain(..).partition(|t| t.wake <= now);
            *tasks = later;
            due
        };
        for mut task in due {
            // Out of time this turn: next turn, as it was.
            if Instant::now() >= self.turn_end.get() {
                self.sched.tasks.borrow_mut().push(task);
                continue;
            }
            let args = task.args.take().unwrap_or_default();
            let (outcome, spent) = self.run(&task.thread, args);
            task.busy += spent;
            match outcome {
                Outcome::Wait(ms) => {
                    task.wake = now + u64::from(ms);
                    task.busy = Duration::ZERO;
                }
                Outcome::Preempted if task.busy >= RUNAWAY => {
                    crate::log_line(&self.module, crate::LogLevel::Error, &runaway());
                    continue;
                }
                Outcome::Preempted => task.wake = now,
                Outcome::Finished => continue,
                Outcome::Failed(e) => {
                    // A task's error is logged; the mod goes on.
                    crate::log_line(&self.module, crate::LogLevel::Error, &e);
                    continue;
                }
            }
            self.sched.tasks.borrow_mut().push(task);
        }
    }

    /// The main chunk, if due and the turn lasts; a failure ends the mod.
    fn run_main(&mut self, now: u64) -> Option<Step> {
        if let Some(main) = self.main.clone()
            && self.main_wake <= now
            && Instant::now() < self.turn_end.get()
        {
            let (outcome, spent) = self.run(&main, MultiValue::new());
            self.main_busy += spent;
            match outcome {
                Outcome::Wait(ms) => {
                    self.main_wake = now + u64::from(ms);
                    self.main_busy = Duration::ZERO;
                }
                Outcome::Preempted if self.main_busy >= RUNAWAY => return Some(Step::Failed(runaway())),
                Outcome::Preempted => self.main_wake = now,
                Outcome::Finished => self.main = None,
                Outcome::Failed(e) => return Some(Step::Failed(e)),
            }
        }
        None
    }

    /// Runs a coroutine until it yields, ends or fails; and how long it ran.
    fn run(&self, thread: &Thread, args: MultiValue) -> (Outcome, Duration) {
        if !thread.is_resumable() {
            return (Outcome::Finished, Duration::ZERO);
        }
        let start = Instant::now();
        self.deadline.set(self.turn_end.get());
        self.sched.resumed.set(thread.to_pointer());
        let result = thread.resume::<MultiValue>(args);
        let spent = start.elapsed();
        let outcome = match result {
            Err(e) => Outcome::Failed(error_text(&e)),
            Ok(_) if !thread.is_resumable() => Outcome::Finished,
            // `wait(ms)`.
            Ok(values) => match values.into_iter().next() {
                Some(v) => Outcome::Wait(match v {
                    Value::Integer(i) => i.clamp(0, i32::MAX.into()) as u32,
                    Value::Number(n) if n.is_finite() => n.clamp(0.0, i32::MAX as f64) as u32,
                    _ => 0,
                }),
                // The interrupt: out of time.
                None => Outcome::Preempted,
            },
        };
        (outcome, spent)
    }

    /// Whether the mod has a last turn to take before it ends: functions
    /// it gave `mod.onStop`.
    pub fn wants_last_turn(&self) -> bool {
        !self.sched.stop.borrow().is_empty()
    }

    /// The mod's last turn, at game time `now` (ms), on its thread: its
    /// `mod.onStop` functions run, once, in one slice; one that waits or
    /// runs out of time ends there.
    pub fn last_turn(&mut self, now: u64) {
        let stop = std::mem::take(&mut *self.sched.stop.borrow_mut());
        self.sched.now.set(now);
        self.turn_end.set(Instant::now() + LAST_TURN);
        for f in stop {
            let problem = match self.lua.create_thread(f).map(|thread| self.run(&thread, MultiValue::new()).0) {
                Ok(Outcome::Finished) => continue,
                Ok(Outcome::Failed(e)) => e,
                Ok(Outcome::Wait(_)) => "mod.onStop: a stopping mod does not wait; what came after wait() did not run".into(),
                Ok(Outcome::Preempted) => format!("mod.onStop: cut short, out of time ({} ms)", LAST_TURN.as_millis()),
                Err(e) => error_text(&e),
            };
            crate::log_line(&self.module, crate::LogLevel::Error, &problem);
        }
    }

    /// Memory the VM uses, in bytes.
    pub fn memory(&self) -> usize {
        self.lua.used_memory()
    }

    /// What the mod draws (draw.rs).
    pub(crate) fn canvas(&self) -> Option<std::rc::Rc<crate::draw::Canvas>> {
        crate::draw::canvas(&self.lua)
    }
}

fn runaway() -> String {
    format!("ran for {} s without calling wait(): a loop needs wait() to let the game go on", RUNAWAY.as_secs())
}

/// A VM with the libraries, the natives and the mods' globals (`print`,
/// `wait`, `task`, `ui`, `joaat`, `mod`), and the interrupt that keeps a
/// slice short; not sandboxed yet.
fn base(name: &str, log: Rc<dyn Fn(&str)>, sched: &Sched) -> mlua::Result<(Lua, Rc<Cell<Instant>>)> {
    let lua = Lua::new_with(
        StdLib::COROUTINE
            | StdLib::TABLE
            | StdLib::STRING
            | StdLib::UTF8
            | StdLib::MATH
            | StdLib::BIT
            | StdLib::VECTOR
            | StdLib::BUFFER
            | StdLib::OS,
        LuaOptions::default(),
    )?;
    lua.set_memory_limit(MEMORY)?;
    let g = lua.globals();
    g.set(
        "print",
        lua.create_function(move |_, args: MultiValue| {
            let line = describe_list(args.iter(), " ", false);
            log(&line);
            Ok(())
        })?,
    )?;
    // `wait(ms)`: back to the game until then; plain Lua, so it yields from
    // the running coroutine.
    let wait: Function = lua
        .load("local yield = coroutine.yield\nreturn function(ms) yield(ms or 0) end")
        .set_name("=wait")
        .eval()?;
    g.set("wait", wait.clone())?;
    // `task.spawn(f, ...)`, `task.delay(ms, f, ...)`, `task.wait(ms)`.
    let task = lua.create_table()?;
    task.set("wait", wait)?;
    let s = sched.clone();
    task.set(
        "spawn",
        lua.create_function(move |lua, (f, args): (Function, MultiValue)| {
            let thread = lua.create_thread(f)?;
            s.tasks.borrow_mut().push(Task { thread, wake: s.now.get(), args: Some(args), busy: Duration::ZERO });
            Ok(())
        })?,
    )?;
    let s = sched.clone();
    task.set(
        "delay",
        lua.create_function(move |lua, (ms, f, args): (f64, Function, MultiValue)| {
            let thread = lua.create_thread(f)?;
            let wake = s.now.get() + ms.clamp(0.0, 1e9) as u64;
            s.tasks.borrow_mut().push(Task { thread, wake, args: Some(args), busy: Duration::ZERO });
            Ok(())
        })?,
    )?;
    menus::install(&lua, name, sched.instance, sched.handlers.clone())?;
    input::install(&lua, &sched.keys)?;
    crate::draw::install(&lua)?;
    json::install(&lua)?;
    crate::net::install(&lua)?;
    crate::world::install(&lua)?;
    g.set("joaat", lua.create_function(|_, s: mlua::LuaString| Ok(natives::joaat(&s.to_str()?)))?)?;
    let m = lua.create_table()?;
    m.set("name", name)?;
    let stop = sched.stop.clone();
    m.set(
        "onStop",
        lua.create_function(move |_, f: Function| {
            stop.borrow_mut().push(f);
            Ok(())
        })?,
    )?;
    g.set("mod", m)?;

    // The natives' tables (the scheduler shares `task` with the TASK_
    // natives) and `native.invoke`; the scripts' globals, the game's version;
    // the game's memory.
    natives::install(&lua, &task)?;
    crate::globals::install(&lua)?;
    crate::memory::install(&lua, sched.instance)?;
    crate::offsets::install(&lua)?;
    crate::handling::install(&lua)?;
    // The standard library, Luau over the natives and the tables above.
    crate::stdlib::install(&lua)?;
    // Natives that fill structures, as tables (events, DLC data, nearby peds).
    crate::structs::install(&lua)?;

    let deadline = Rc::new(Cell::new(Instant::now() + SLICE));
    let interrupt_deadline = deadline.clone();
    let resumed = sched.resumed.clone();
    lua.set_interrupt(move |lua| {
        let late = Instant::now().saturating_duration_since(interrupt_deadline.get());
        if late.is_zero() {
            return Ok(VmState::Continue);
        }
        // Past the runaway limit whatever runs: a yield where Luau cannot
        // (a metamethod, a sort's comparator, a module `require` runs) is
        // dropped, so a loop there would never give the game back.
        if late >= RUNAWAY {
            return Err(mlua::Error::runtime(runaway()));
        }
        // Only the coroutine the scheduler resumed yields back to it; one
        // the mod made itself (coroutine.wrap) would hand its resume an
        // empty result. That one runs on, up to the runaway limit.
        if lua.current_thread().to_pointer() == resumed.get() { Ok(VmState::Yield) } else { Ok(VmState::Continue) }
    });
    Ok((lua, deadline))
}

/// `require("./menu")`: `menu.luau` beside the file that asks, as Luau's
/// require by string has it (and the Studio's checker): `./` that file's
/// folder, `../` the one above, never out of the mod's folder. Each file
/// runs once.
fn require(lua: &Lua, name: &str, dir: &Path) -> mlua::Result<Function> {
    let loaded: RefCell<HashMap<PathBuf, Value>> = RefCell::default();
    let (name, dir) = (name.to_string(), dir.to_path_buf());
    lua.create_function(move |lua, path: String| {
        // Where the calling function's chunk is: `@<mod>/core/bind.luau`.
        let from = lua.inspect_stack(1, |d| d.source().source.map(|s| s.into_owned())).flatten().unwrap_or_default();
        let rel = module_path(&name, &from, &path).map_err(|e| mlua::Error::runtime(format!("require(\"{path}\"): {e}")))?;
        let file = dir.join(format!("{rel}.luau"));
        if let Some(v) = loaded.borrow().get(&file) {
            return Ok(v.clone());
        }
        let source = std::fs::read_to_string(&file)
            .map_err(|e| mlua::Error::runtime(format!("require(\"{path}\"): {rel}.luau: {e}")))?;
        let value: Value = lua.load(source).set_name(format!("@{name}/{rel}.luau")).call(())?;
        let value = if value.is_nil() { Value::Boolean(true) } else { value };
        loaded.borrow_mut().insert(file, value.clone());
        Ok(value)
    })
}

/// The file `path` names from the chunk `from` (`@<mod>/<file>.luau`; the
/// mod's folder for another, a console's), relative to the mod's folder
/// and without `.luau`.
fn module_path(name: &str, from: &str, path: &str) -> Result<String, String> {
    let mut at: Vec<&str> = match from.strip_prefix('@').and_then(|f| f.strip_prefix(name)).and_then(|f| f.strip_prefix('/')) {
        Some(file) => file.split('/').collect(),
        None => vec!["main.luau"],
    };
    // The folder of the file that asks.
    at.pop();
    let mut parts = path.split('/');
    match parts.next() {
        Some(".") => {}
        Some("..") => {
            at.pop().ok_or("not in the mod's folder")?;
        }
        _ => return Err("a file of the mod, from the file that asks: \"./name\", \"../core/name\"".into()),
    }
    for p in parts {
        match p {
            ".." => {
                at.pop().ok_or("not in the mod's folder")?;
            }
            "" | "." => return Err("a path of names, as \"./core/name\"".into()),
            p if p.starts_with('.') => return Err("names starting with a dot are not the mod's".into()),
            p => at.push(p),
        }
    }
    if at.is_empty() {
        return Err("a file, not a folder".into());
    }
    Ok(at.join("/"))
}

/// An error as people read it: its message, and where in the mod's files it
/// happened when the message does not say (natives' errors).
pub fn error_text(e: &mlua::Error) -> String {
    let (message, traceback) = match e {
        mlua::Error::CallbackError { traceback, cause } => (error_text(cause), Some(traceback.as_str())),
        // Without the traceback the message carries (its `file:line` is enough).
        mlua::Error::RuntimeError(m) => (m.split("\nstack traceback:").next().unwrap_or(m).to_string(), None),
        mlua::Error::SyntaxError { message, .. } => (message.clone(), None),
        mlua::Error::MemoryError(_) => ("out of memory (a mod may use 64 MB)".into(), None),
        other => (other.to_string(), None),
    };
    // `trainer/main.luau:12: in function ...`: the first frame in a mod's file.
    let at = traceback.and_then(|t| {
        t.lines().map(str::trim).find(|l| l.contains(".luau:")).and_then(|l| {
            let end = l.find(".luau:")? + 6;
            let digits = l[end..].chars().take_while(char::is_ascii_digit).count();
            Some(l[..end + digits].trim_start_matches('@').to_string())
        })
    });
    match at {
        Some(at) if !message.contains(".luau:") => format!("{at}: {message}"),
        _ => message,
    }
}

/// What `print` and the console show at most, tables' contents included,
/// a log line: past it, `…`.
const DESCRIBE_TEXT: usize = 8 << 10;
/// Tables nested deeper show as `{…}`.
const DESCRIBE_DEPTH: usize = 8;
/// Entries shown of each table.
const DESCRIBE_ENTRIES: usize = 16;

/// A value as `print` shows it: strings as they are.
fn describe_plain(v: &Value) -> String {
    describe_list(std::slice::from_ref(v), "", false)
}

/// Values one after the other (`print`'s arguments, the REPL's results),
/// within the one limit.
fn describe_list<'a>(values: impl IntoIterator<Item = &'a Value>, separator: &str, quoted: bool) -> String {
    let mut d = Describe::default();
    for (i, v) in values.into_iter().enumerate() {
        if i > 0 {
            d.push(separator);
        }
        d.value(v, quoted);
    }
    if d.full {
        d.out.push('…');
    }
    d.out
}

/// Writes a value as text within [`DESCRIBE_TEXT`]: the work ends with the
/// text, whatever the tables hold (themselves, the same table many times).
#[derive(Default)]
struct Describe {
    out: String,
    /// The tables being written, outermost first (one inside itself is a
    /// cycle).
    path: Vec<*const std::ffi::c_void>,
    full: bool,
}

impl Describe {
    /// Adds `text`, or what fits of it with the `…` that says it was cut.
    fn push(&mut self, text: &str) {
        if self.full {
            return;
        }
        let room = (DESCRIBE_TEXT - '…'.len_utf8()).saturating_sub(self.out.len());
        if text.len() <= room {
            self.out.push_str(text);
        } else {
            self.out.push_str(&text[..text.floor_char_boundary(room)]);
            self.full = true;
        }
    }

    fn value(&mut self, v: &Value, quoted: bool) {
        if self.full {
            return;
        }
        match v {
            Value::String(s) => {
                // Only what can be shown is copied out of the VM.
                let bytes = s.as_bytes();
                let head = String::from_utf8_lossy(&bytes[..bytes.len().min(DESCRIBE_TEXT + 1)]);
                let cut = head.len() < bytes.len();
                if quoted {
                    self.push(&format!("{head:?}"));
                } else {
                    self.push(&head);
                }
                self.full |= cut;
            }
            Value::Table(t) => self.table(t),
            Value::Nil => self.push("nil"),
            Value::Boolean(b) => self.push(&b.to_string()),
            Value::Integer(i) => self.push(&i.to_string()),
            Value::Number(n) if n.fract() == 0.0 && n.abs() < 1e15 => self.push(&format!("{n:.0}")),
            Value::Number(n) => self.push(&format!("{n}")),
            Value::Vector(v) => self.push(&format!("vector({}, {}, {})", short(v.x()), short(v.y()), short(v.z()))),
            Value::Function(_) => self.push("function"),
            Value::Error(e) => self.push(&error_text(e)),
            other => self.push(other.type_name()),
        }
    }

    fn table(&mut self, t: &Table) {
        let at = t.to_pointer();
        if self.path.contains(&at) {
            return self.push("<cycle>");
        }
        if self.path.len() >= DESCRIBE_DEPTH {
            return self.push("{…}");
        }
        self.path.push(at);
        self.push("{");
        for (i, pair) in t.pairs::<Value, Value>().enumerate() {
            if self.full {
                break;
            }
            if i > 0 {
                self.push(", ");
            }
            if i == DESCRIBE_ENTRIES {
                self.push("…");
                break;
            }
            if let Ok((k, v)) = pair {
                if !matches!(k, Value::Integer(_)) {
                    self.value(&k, false);
                    self.push(" = ");
                }
                self.value(&v, true);
            }
        }
        self.push("}");
        self.path.pop();
    }
}

fn short(x: f32) -> String {
    let s = format!("{x:.2}");
    s.trim_end_matches('0').trim_end_matches('.').to_string()
}

impl Vm {
    /// This run of the mod.
    pub fn instance(&self) -> u64 {
        self.sched.instance
    }

    /// Writes what the mod stored, now.
    pub fn flush_storage(&self) {
        if let Some(storage) = &self.storage {
            storage.flush(true);
        }
    }

    /// Writes what the mod stored when it is time (every couple of
    /// seconds), also while it waits long between its turns.
    pub fn flush_storage_due(&self) {
        if let Some(storage) = &self.storage {
            storage.flush(false);
        }
    }

    /// Empties the mod's storage and deletes its database.
    pub fn clear_storage(&self) {
        if let Some(storage) = &self.storage {
            storage.clear_now();
        }
        if let Some(db) = &self.db {
            db.clear();
        }
    }
}

impl Drop for Vm {
    /// The mod stops: its storage is written now.
    fn drop(&mut self) {
        if let Some(storage) = &self.storage {
            storage.flush(true);
        }
    }
}

#[cfg(test)]
mod tests {
    use mlua::{Lua, Value};

    use super::{DESCRIBE_TEXT, describe_list, describe_plain};

    /// A value as the REPL shows it.
    fn describe(v: &Value) -> String {
        describe_list(std::slice::from_ref(v), "", true)
    }

    fn value(lua: &Lua, code: &str) -> Value {
        lua.load(code).eval().unwrap()
    }

    /// `require` paths go from the file that asks, as Luau's and the
    /// checker's do, and stay in the mod's folder.
    #[test]
    fn require_paths() {
        let m = |from: &str, path: &str| super::module_path("t", from, path);
        assert_eq!(m("@t/main.luau", "./core/bind"), Ok("core/bind".into()));
        assert_eq!(m("@t/core/bind.luau", "./prefs"), Ok("core/prefs".into()));
        assert_eq!(m("@t/features/self.luau", "../core/bind"), Ok("core/bind".into()));
        assert_eq!(m("@t/a/b/c.luau", "../../x"), Ok("x".into()));
        assert_eq!(m("=eval", "./menu"), Ok("menu".into()));
        assert!(m("@t/main.luau", "../x").unwrap_err().contains("not in the mod's folder"));
        assert!(m("@t/a/b.luau", "./../../x").unwrap_err().contains("not in the mod's folder"));
        assert!(m("@t/main.luau", "core/x").is_err());
        assert!(m("@t/main.luau", "./.hidden").is_err());
        assert!(m("@t/main.luau", "./a//b").is_err());
        assert!(m("@t/main.luau", "./..").unwrap_err().contains("not in the mod's folder"));
    }

    #[test]
    fn describe_values() {
        let lua = Lua::new();
        assert_eq!(describe(&value(&lua, "'a'")), r#""a""#);
        assert_eq!(describe_plain(&value(&lua, "'a'")), "a");
        assert_eq!(describe(&value(&lua, "{1, 'x', {true}}")), r#"{1, "x", {true}}"#);
        assert_eq!(describe(&value(&lua, "{k = 2.5}")), "{k = 2.5}");
        let wide = describe(&value(&lua, "local t = {} for i = 1, 40 do t[i] = i end return t"));
        assert!(wide.ends_with(", 16, …}"), "{wide}");
    }

    /// A table inside itself, one held many times over, one nested deep,
    /// a long string: bounded text, quickly.
    #[test]
    fn describe_bounded() {
        let lua = Lua::new();
        let cycle = describe(&value(&lua, "local a, b = {}, {} a.b = b b.a = a return a"));
        assert_eq!(cycle, "{b = {a = <cycle>}}");
        let start = std::time::Instant::now();
        // 16^12 tables if written out.
        let dag = describe(&value(
            &lua,
            "local t = {} for _ = 1, 12 do local u = {} for i = 1, 16 do u[i] = t end t = u end return t",
        ));
        assert!(dag.len() <= DESCRIBE_TEXT, "{}", dag.len());
        let deep = describe(&value(&lua, "local t = {} for _ = 1, 100000 do t = {t} end return t"));
        assert!(deep.contains("{…}") && deep.len() < 64, "{deep}");
        let long = describe_plain(&value(&lua, "string.rep('é', 1000000)"));
        assert!(long.len() <= DESCRIBE_TEXT && long.ends_with('…'));
        let quoted = describe(&value(&lua, "{string.rep('x', 100000)}"));
        assert!(quoted.len() <= DESCRIBE_TEXT && quoted.ends_with('…'));
        let many: mlua::MultiValue = lua.load("local t = {} for i = 1, 5000 do t[i] = string.rep('x', 100) end return unpack(t)").eval().unwrap();
        let many = describe_list(many.iter(), ", ", true);
        assert!(many.len() <= DESCRIBE_TEXT && many.ends_with('…'));
        assert!(start.elapsed().as_secs() < 2, "took {:?}", start.elapsed());
    }
}
