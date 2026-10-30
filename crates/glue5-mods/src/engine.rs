//! Mods in the engine: each runs in a script thread of its own, so its
//! natives run in a real script context (the entities it creates belong to
//! its thread, and the engine cleans them up when the thread ends).
//!
//! The thread's program is ours, built in wasm memory: `loop: WAIT(0)`.
//! Every frame the engine calls WAIT for every script thread; the hook on
//! it resumes the mod's Lua when the thread is a mod's and is about to wait,
//! and writes how long it waits into WAIT's argument. Starting and killing
//! threads happens from the same hook, in whatever thread waits next, so it
//! all stays on the engine's script update.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

use glue5_game::{Env, Hooks, orig};
use glue5_runtime::memory::Mem;

use crate::vm::{Jobs, SLICE, Step, Vm};
use crate::{Command, LogLevel, ModState, SHARED, log_line, natives};

/// What all the mods together may take of a frame: each gets at most a
/// slice, and while the budget lasts.
const BUDGET: Duration = Duration::from_millis(4);

/// The REPL's thread: `eval` runs there, as a mod of its own.
const REPL: &str = "~eval";

pub(crate) fn hooks(h: &mut Hooks) {
    h.script_wait = Some(script_wait);
}

/// fwTimer's frame count, what GET_FRAME_COUNT returns (read here: every
/// script's WAIT asks, every frame).
const FRAME_COUNT: u64 = 12321252;
/// The program registry's slots (`scrProgram::GetProgram`): 176 u64, and
/// its free list's first slot (176: none left, where `Insert` traps).
const PROGRAMS: u64 = 15406680;
const PROGRAM_SLOTS: u64 = 176;
const PROGRAMS_FREE: u64 = 15406464;
/// `sizeof(scrProgram)`; the fields set below are all the engine reads to
/// start and run a thread.
const PROGRAM_SIZE: u64 = 128;
/// scrThread fields.
const THREAD_ID: u64 = 24;
const THREAD_STATE: u64 = 32;
const STATE_WAITING: u32 = 1;
const THREAD_NAME: u64 = 428;
/// The script threads (`rage::scrThread::GetThread`): an array of
/// pointers, and their count (u16).
const THREADS_ARRAY: u64 = 15409704;
const THREADS_COUNT: u64 = 15409712;
/// Where in the module's thread-local storage the engine keeps the running
/// script thread (`rage::scrThread::GetActiveThread`).
const TLS_ACTIVE_THREAD: u64 = 3088;
/// GTA Online's session script, which every player runs: what a mod makes
/// as it is networked (natives.rs, `Session`).
const SESSION_SCRIPT: &[u8] = b"freemode\0";
/// The stack size of the mods' threads, one of gameconfig.xml's
/// (MICRO): a mod keeps its state in Lua.
const STACK: u32 = 128;
/// Opcodes (the b2802+ table).
const OP_ENTER: u8 = 45;
const OP_NATIVE: u8 = 44;
const OP_PUSH_0: u8 = 113;
const OP_J: u8 = 85;

struct Running {
    name: String,
    thread_id: u32,
    thread: u64,
    vm: Vm,
    /// Lua ended (or failed): the thread is killed from the next wait
    /// outside it.
    ending: bool,
    /// The game ended its thread itself: nothing to kill.
    gone: bool,
    cpu: f32,
    waiting: bool,
}

impl Running {
    /// It ends: its thread is killed from the next wait outside it. What it
    /// stored is written now (a restart's new run reads it next).
    fn end(&mut self) {
        self.ending = true;
        self.vm.flush_storage();
    }
}

#[derive(Default)]
struct Engine {
    /// The program text and natives table every mod program shares.
    code: Option<(u64, u64)>,
    /// Program id by mod name.
    programs: HashMap<String, u32>,
    running: Vec<Running>,
    /// The REPL's snippets.
    jobs: Jobs,
    /// The catalog of names was read (once the game has loaded), and when
    /// that was last checked.
    catalog: bool,
    catalog_checked: Option<Instant>,
    /// The frame (GET_FRAME_COUNT) and the time the mods took in it.
    frame: i32,
    spent: Duration,
    /// Mods that found no budget left: in the last frame (they go first,
    /// whatever is left, so that none starves) and in this one.
    skipped: HashSet<String>,
    skipping: HashSet<String>,
    /// GTA Online's session script thread this frame, if one runs: mods
    /// run as it (natives.rs, `Session`).
    session: Option<u64>,
}

thread_local! {
    static ENGINE: RefCell<Engine> = RefCell::default();
    /// The ids of the mods' script threads (ENGINE is borrowed while a mod
    /// runs; natives.rs asks).
    static THREADS: RefCell<Vec<u32>> = const { RefCell::new(Vec::new()) };
}

/// GTA Online's session script thread, if one runs (none in story mode or a
/// job).
fn session_script(mem: Mem) -> Option<u64> {
    let threads = mem.u64(THREADS_ARRAY);
    (0..u64::from(mem.u16(THREADS_COUNT)))
        .map(|i| mem.u64(threads + 8 * i))
        .find(|&t| t != 0 && mem.u32(t + THREAD_ID) != 0 && mem.bytes(t + THREAD_NAME, SESSION_SCRIPT.len()) == SESSION_SCRIPT)
}

/// The session script's (`host`) context for a mod running in `own`, in
/// GTA Online.
fn session_context(env: *mut Env, host: Option<u64>, own: u64) -> Option<natives::Session> {
    let slot = unsafe { glue5_game::tls_base(env) } + TLS_ACTIVE_THREAD;
    host.map(|host| natives::Session { slot, own, host })
}

/// What the names of the mods' programs and threads start with.
pub(crate) const PREFIX: &str = "glue5:";

/// Whether script thread `id` is a mod's. A mod's thread is ended by the
/// hook, from outside it: a native that ended it from inside (the mod
/// itself, through TERMINATE_THREAD) would leave the hook going on in a
/// dead thread.
pub(crate) fn is_mod_thread(id: u32) -> bool {
    THREADS.with_borrow(|t| t.contains(&id))
}

unsafe extern "C" fn script_wait(env: *mut Env, info: u64) {
    let killed = ENGINE.with(|engine| {
        // A mod's native that waits would come back here: not for us then.
        let Ok(mut engine) = engine.try_borrow_mut() else { return false };
        // A panic must not cross into the engine (that aborts the game):
        // it stops the mod it happened in.
        let caught = glue5_runtime::exit::recoverable(|| {
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| unsafe { engine.wait(env, info) }))
        });
        match caught {
            Ok(killed) => killed,
            Err(panic) => {
                engine.panicked(&*panic);
                false
            }
        }
    });
    if !killed {
        unsafe { orig::script_wait(env, info) }
    }
}

impl Engine {
    /// A panic was caught in `wait`: the mod running then fails and stops.
    fn panicked(&mut self, panic: &(dyn std::any::Any + Send)) {
        let what = panic
            .downcast_ref::<&str>()
            .map(|s| s.to_string())
            .or_else(|| panic.downcast_ref::<String>().cloned())
            .unwrap_or_else(|| "a panic".into());
        let name = std::mem::take(&mut *crate::lock(&crate::RUNNING_MOD));
        log::error!("mods: GLUE 5 failed inside {}: {what}", if name.is_empty() { "the mods' update" } else { &name });
        if let Some(r) = self.running.iter_mut().find(|r| r.name == name && !r.ending) {
            let e = format!("GLUE 5 failed while running this mod ({what}); it was stopped");
            log_line(&r.name, LogLevel::Error, &e);
            set_state(&r.name, ModState::Error, Some(e));
            r.end();
        }
    }

    /// Returns whether the active thread was killed (WAIT must not run).
    unsafe fn wait(&mut self, env: *mut Env, info: u64) -> bool {
        let mem = unsafe { Mem::of(env) };
        let active = unsafe { orig::script_active_thread(env) };
        if SHARED.pending.swap(false, Ordering::Acquire) {
            let commands = std::mem::take(&mut crate::lock(&SHARED.state).commands);
            for command in commands {
                unsafe { self.command(env, command) };
            }
        }
        if !self.catalog && self.catalog_checked.is_none_or(|t| t.elapsed().as_secs() >= 1) {
            self.catalog_checked = Some(Instant::now());
            let ready = unsafe {
                natives::call_int(env, "GET_IS_LOADING_SCREEN_ACTIVE") == Some(0)
                    && natives::call_int(env, "IS_SCREEN_FADED_IN") == Some(1)
                    && natives::call_int(env, "PLAYER_PED_ID").is_some_and(|p| p != 0)
            };
            if ready {
                self.catalog = true;
                crate::world::set_catalog(unsafe { crate::world::read_catalog(env) });
            }
        }
        // Once a frame: threads the game ended itself (TERMINATE_THREAD, a
        // session that starts again) take their mods with them.
        let frame = mem.i32(FRAME_COUNT);
        if frame != self.frame {
            self.frame = frame;
            // In a session that has started (not while it starts).
            let session = session_script(mem).filter(|_| unsafe { natives::call_int(env, "NETWORK_IS_SESSION_STARTED") } == Some(1));
            if session != self.session {
                // What mods made in the last one went with its script.
                natives::forget_made();
                self.session = session;
            }
            self.spent = Duration::ZERO;
            self.skipped = std::mem::take(&mut self.skipping);
            // What the mods show, for the renderer (when one drew anew).
            crate::draw::publish(self.running.iter().filter(|r| !r.ending).filter_map(|r| r.vm.canvas()));
            for r in self.running.iter_mut().filter(|r| !r.ending) {
                r.vm.flush_storage_due();
                if unsafe { orig::script_get_thread(env, r.thread_id) } != r.thread {
                    log_line(&display(&r.name), LogLevel::Error, "its script thread was ended by the game");
                    set_state(&r.name, ModState::Stopped, None);
                    r.end();
                    r.gone = true;
                }
            }
        }
        // Ended mods' threads, from outside them, once they had their last
        // turn.
        let mut i = 0;
        while i < self.running.len() {
            let r = &self.running[i];
            if r.ending && r.thread != active && (r.gone || !r.vm.wants_last_turn()) {
                if !r.gone {
                    unsafe { orig::script_kill_thread(env, r.thread_id) };
                }
                let slot = unsafe { glue5_game::tls_base(env) } + TLS_ACTIVE_THREAD;
                unsafe { natives::give_back(env, &r.name, slot, self.session, active) };
                crate::menus::remove_instance(r.vm.instance());
                unsafe { crate::memory::release(env, r.vm.instance()) };
                let r = self.running.remove(i);
                THREADS.with_borrow_mut(|t| t.retain(|&id| id != r.thread_id));
                // Snippets still queued get an answer (unless a new console
                // started meanwhile: they are its own).
                if r.name == REPL && !self.running.iter().any(|x| x.name == REPL && !x.ending) {
                    for (_, reply) in self.jobs.borrow_mut().drain(..) {
                        let _ = reply.send(Err("the console's script stopped; try again".into()));
                    }
                }
            } else {
                i += 1;
            }
        }
        // A pooled thread object may have gone to another script (in this
        // very frame, before the check above): its id says.
        let Some(r) = self
            .running
            .iter_mut()
            .find(|r| r.thread == active && !r.gone && mem.u32(active + THREAD_ID) == r.thread_id)
        else {
            return false;
        };
        // An ended mod's last turn (`mod.onStop`), on its thread, whether
        // it waits or not.
        if r.ending {
            if r.vm.wants_last_turn() {
                let now = unsafe { natives::call_int(env, "GET_GAME_TIMER") }.unwrap_or(0) as u32 as u64;
                *crate::lock(&crate::RUNNING_MOD) = r.name.clone();
                let session = session_context(env, self.session, active);
                natives::with_env(env, || natives::as_session(env, session, || r.vm.last_turn(now)));
                crate::lock(&crate::RUNNING_MOD).clear();
            }
            return false;
        }
        // WAIT runs every frame while the thread waits; the mod runs when
        // it is about to start one.
        if mem.u32(active + THREAD_STATE) == STATE_WAITING {
            return false;
        }
        // The frame's budget, shared by the mods.
        let slice = if self.skipped.contains(&r.name) { SLICE } else { BUDGET.saturating_sub(self.spent).min(SLICE) };
        if slice < Duration::from_micros(100) {
            // Next frame, first.
            self.skipping.insert(r.name.clone());
            return false;
        }
        let start = Instant::now();
        let now = unsafe { natives::call_int(env, "GET_GAME_TIMER") }.unwrap_or(0) as u32 as u64;
        *crate::lock(&crate::RUNNING_MOD) = r.name.clone();
        let session = session_context(env, self.session, active);
        let step = natives::with_env(env, || natives::as_session(env, session, || r.vm.step(now, slice)));
        crate::lock(&crate::RUNNING_MOD).clear();
        let took = start.elapsed();
        self.spent += took;
        r.cpu = r.cpu * 0.9 + took.as_secs_f32() * 1000.0 * 0.1;
        let ms = match step {
            Step::Wait(ms) => {
                if r.waiting != (ms > 0) {
                    r.waiting = ms > 0;
                    set_waiting(&r.name, r.waiting);
                }
                ms
            }
            Step::Finished => {
                log_line(&display(&r.name), LogLevel::Info, "finished");
                set_state(&r.name, ModState::Stopped, None);
                r.end();
                0
            }
            Step::Failed(e) => {
                log_line(&display(&r.name), LogLevel::Error, &e);
                set_state(&r.name, ModState::Error, Some(e));
                r.end();
                0
            }
        };
        // WAIT's argument, in the thread's stack.
        mem.set_i32(mem.u64(info + 16), ms.min(i32::MAX as u32) as i32);
        if let Some(m) = crate::lock(&SHARED.state).mods.iter_mut().find(|m| m.name == r.name) {
            m.cpu_ms = r.cpu;
            m.memory = r.vm.memory();
        }
        false
    }

    unsafe fn command(&mut self, env: *mut Env, command: Command) {
        match command {
            Command::Start(name) => {
                if self.running.iter().any(|r| r.name == name && !r.ending) {
                    return;
                }
                if let Err(e) = unsafe { self.start(env, &name) } {
                    log_line(&name, LogLevel::Error, &e);
                    set_state(&name, ModState::Error, Some(e));
                }
            }
            Command::Eval(code, reply) => {
                if !self.running.iter().any(|r| r.name == REPL && !r.ending)
                    && let Err(e) = unsafe { self.start(env, REPL) }
                {
                    let _ = reply.send(Err(e));
                    return;
                }
                self.jobs.borrow_mut().push_back((code, reply));
            }
            Command::ClearStorage(name) => {
                if let Some(r) = self.running.iter().find(|r| r.name == name && !r.ending) {
                    r.vm.clear_storage();
                }
            }
            Command::Stop(name) => {
                if let Some(r) = self.running.iter_mut().find(|r| r.name == name && !r.ending) {
                    r.end();
                    log_line(&name, LogLevel::Info, "stopped");
                    set_state(&name, ModState::Stopped, None);
                }
            }
        }
    }

    unsafe fn start(&mut self, env: *mut Env, name: &str) -> Result<(), String> {
        let dir = SHARED.dir.get().ok_or("no mods folder")?.join(name);
        let log_name = if name == REPL { "eval".to_string() } else { name.to_string() };
        let log: Rc<dyn Fn(&str)> = Rc::new(move |text| log_line(&log_name, LogLevel::Info, text));
        let vm = if name == REPL { Vm::repl(self.jobs.clone(), log) } else { Vm::new(name, &dir, log) }
            .map_err(|e| e.to_string())?;
        let program = unsafe { self.program(env, name)? };
        // The program's name, kept (not a copy more at each start).
        let thread_name = match unsafe { orig::script_get_program(env, program) } {
            0 => return Err("its program is gone".into()),
            p => unsafe { Mem::of(env) }.u64(p + 96),
        };
        let thread_id = unsafe { orig::script_start_program(env, program, 0, 0, STACK as i32 as u32, thread_name) };
        if thread_id == 0 {
            return Err("the game has no free script thread for it".into());
        }
        let thread = unsafe { orig::script_get_thread(env, thread_id) };
        if thread == 0 {
            return Err("its script thread is gone".into());
        }
        THREADS.with_borrow_mut(|t| t.push(thread_id));
        self.running.push(Running {
            name: name.to_string(),
            thread_id,
            thread,
            vm,
            ending: false,
            gone: false,
            cpu: 0.0,
            waiting: false,
        });
        if name != REPL {
            log_line(name, LogLevel::Info, "started");
            set_state(name, ModState::Running, None);
        }
        Ok(())
    }

    /// The program of mod `name`'s thread (its name is the thread's), made
    /// once: a copy of a loaded program with our code, natives and name.
    unsafe fn program(&mut self, env: *mut Env, name: &str) -> Result<u32, String> {
        if let Some(&id) = self.programs.get(name) {
            return Ok(id);
        }
        let mem = unsafe { Mem::of(env) };
        let template =
            (0..PROGRAM_SLOTS).map(|i| mem.u64(PROGRAMS + 8 * i)).find(|&p| p != 0).ok_or("no script program is loaded yet")?;
        let (pages, natives) = match self.code {
            Some(code) => code,
            None => {
                let wait = natives::reserved_handler("WAIT").ok_or("no WAIT native")?;
                // ENTER 0 params, 2 locals, no name; loop: PUSH_0; NATIVE
                // 1 argument, no result, natives[0]; J loop.
                let code: [u8; 13] = [OP_ENTER, 0, 2, 0, 0, OP_PUSH_0, OP_NATIVE, 1 << 2, 0, 0, OP_J, 0xF8, 0xFF];
                let text = unsafe { malloc(env, 64)? };
                mem.bytes_mut(text, 64).fill(0);
                mem.bytes_mut(text, code.len()).copy_from_slice(&code);
                let pages = unsafe { malloc(env, 8)? };
                mem.set_u64(pages, text);
                let natives = unsafe { malloc(env, 8)? };
                mem.set_u64(natives, wait);
                self.code = Some((pages, natives));
                (pages, natives)
            }
        };
        if u64::from(mem.u8(PROGRAMS_FREE)) >= PROGRAM_SLOTS {
            return Err("the game's program registry is full".into());
        }
        let id = natives::joaat(&format!("{PREFIX}{name}"));
        let program = unsafe { malloc(env, PROGRAM_SIZE)? };
        // A program of its own from nothing but the class (the template's
        // vtable): none of the loaded one's resource (its page map at +8,
        // globals, breakpoints), which only that one may ever free.
        mem.bytes_mut(program, PROGRAM_SIZE as usize).fill(0);
        mem.set_u64(program, mem.u64(template));
        mem.set_u64(program + 16, pages); // code pages
        mem.set_u32(program + 28, 13); // code size
        mem.set_u32(program + 36, 0); // statics
        mem.set_u64(program + 48, 0);
        mem.set_u32(program + 44, 1); // natives
        mem.set_u64(program + 64, natives);
        mem.set_u32(program + 72, 0);
        mem.set_u32(program + 88, id);
        mem.set_u32(program + 92, 1 << 24); // references: never released
        mem.set_u64(program + 96, unsafe { wasm_string(env, &format!("{PREFIX}{name}")) });
        mem.set_u64(program + 104, 0); // strings
        mem.set_u32(program + 112, 0);
        unsafe { orig::script_insert_program(env, program) };
        if unsafe { orig::script_get_program(env, id) } != program {
            return Err("the game's program registry is full".into());
        }
        self.programs.insert(name.to_string(), id);
        Ok(id)
    }
}

unsafe fn malloc(env: *mut Env, size: u64) -> Result<u64, String> {
    match unsafe { orig::builtin_malloc(env, size) } {
        0 => Err("out of game memory".into()),
        at => Ok(at),
    }
}

/// A NUL-terminated copy of `s` in the game's heap (kept: names of
/// programs and threads).
unsafe fn wasm_string(env: *mut Env, s: &str) -> u64 {
    let Ok(at) = (unsafe { malloc(env, s.len() as u64 + 1) }) else { return 0 };
    unsafe { Mem::of(env) }.write_str(at, s, s.len() + 1);
    at
}

fn set_state(name: &str, state: ModState, error: Option<String>) {
    if let Some(m) = crate::lock(&SHARED.state).mods.iter_mut().find(|m| m.name == name) {
        m.state = state;
        m.error = error;
        m.waiting = false;
        if state != ModState::Running {
            m.cpu_ms = 0.0;
        }
        SHARED.generation.fetch_add(1, Ordering::Relaxed);
    }
}

fn set_waiting(name: &str, waiting: bool) {
    if let Some(m) = crate::lock(&SHARED.state).mods.iter_mut().find(|m| m.name == name) {
        m.waiting = waiting;
        SHARED.generation.fetch_add(1, Ordering::Relaxed);
    }
}

/// The name a mod's log lines go under (the console's is "eval").
fn display(name: &str) -> String {
    if name == REPL { "eval".to_string() } else { name.to_string() }
}
