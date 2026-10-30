//! Mods: Luau scripts, each in a script thread of the game's own, calling
//! the game's natives.
//!
//! A mod is a folder in the mods folder with a `main.luau` and, optionally,
//! a `mod.json` (`{"autostart": true}`). The application calls [`install`]
//! before the game starts, then [`start`], [`stop`], [`mods`] and [`logs`]
//! from any thread; the mods themselves run on the engine's script update
//! (engine.rs). [`studio`] serves GLUE 5 Studio and the MCP endpoint;
//! [`bundled`] puts the mods that come with GLUE 5 in the mods folder.

pub mod bundled;
mod checker;
mod db;
mod draw;
mod engine;
pub mod files;
mod folder;
mod globals;
mod handling;
mod memory;
pub mod input;
mod json;
mod menus;
pub mod natives;
mod net;
mod socket;
mod offsets;
mod stdlib;
mod storage;
mod structs;
pub mod zip;
pub mod studio;
mod vm;
pub mod world;

use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::SystemTime;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ModState {
    Stopped,
    Running,
    Error,
}

impl ModState {
    pub fn name(self) -> &'static str {
        match self {
            ModState::Stopped => "stopped",
            ModState::Running => "running",
            ModState::Error => "error",
        }
    }
}

/// What a mod says of itself: its `mod.json`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ModInfo {
    /// As people see it (the folder's name when it has none).
    pub title: String,
    pub description: String,
    pub author: String,
    pub version: String,
    /// It starts with the game.
    pub autostart: bool,
    /// Saving one of its files restarts it while it runs.
    pub restart_on_save: bool,
}

/// A mod, as the application shows it.
#[derive(Clone, Debug)]
pub struct Mod {
    pub name: String,
    pub info: ModInfo,
    pub state: ModState,
    /// Why it stopped, when it failed.
    pub error: Option<String>,
    /// Time it takes per frame (smoothed), in milliseconds.
    pub cpu_ms: f32,
    /// Memory of its VM, in bytes.
    pub memory: usize,
    /// It waits (the engine counts the time).
    pub waiting: bool,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum LogLevel {
    Info,
    Error,
}

#[derive(Clone, Debug)]
pub struct LogLine {
    /// Increasing: what a reader has seen is what is below its cursor.
    pub seq: u64,
    pub time: SystemTime,
    pub module: String,
    pub level: LogLevel,
    pub text: String,
}

/// What `eval` gives back: the values as text, or the error.
pub type EvalResult = Result<String, String>;

enum Command {
    Start(String),
    Stop(String),
    Eval(String, Sender<EvalResult>),
    /// Empties a running mod's storage.
    ClearStorage(String),
}

struct State {
    mods: Vec<Mod>,
    commands: Vec<Command>,
    log: VecDeque<LogLine>,
    next_seq: u64,
    /// Files written, deleted or renamed, with a sequence number.
    changes: VecDeque<(u64, String)>,
    next_change: u64,
    /// What the mods notify (for the application).
    notices: VecDeque<(String, String)>,
}

struct Shared {
    dir: OnceLock<PathBuf>,
    state: Mutex<State>,
    /// Commands are waiting for the engine.
    pending: AtomicBool,
    /// Changes when a mod's state does (for readers that push it).
    generation: AtomicU64,
}

static SHARED: Shared = Shared {
    dir: OnceLock::new(),
    state: Mutex::new(State {
        mods: Vec::new(),
        commands: Vec::new(),
        log: VecDeque::new(),
        next_seq: 1,
        changes: VecDeque::new(),
        next_change: 1,
        notices: VecDeque::new(),
    }),
    pending: AtomicBool::new(false),
    generation: AtomicU64::new(0),
};

const LOG_LINES: usize = 2000;
/// The longest log line kept (a mod may print megabytes): past it the
/// line is cut.
const LOG_LINE: usize = 8 << 10;

/// The mod whose turn it is (empty between turns), for the failure note.
static RUNNING_MOD: Mutex<String> = Mutex::new(String::new());

/// Mods kept from starting with the game: they ran when it last stopped.
static HELD_BACK: Mutex<Vec<String>> = Mutex::new(Vec::new());

/// The game has stopped working: which mods ran, and what a mod was doing
/// then (none: no mod was running). Saved, so that the next start does not
/// start them by itself.
pub fn note_failure() -> Option<String> {
    let dir = dir()?;
    let running: Vec<String> =
        mods().into_iter().filter(|m| m.state == ModState::Running).map(|m| m.name).collect();
    let during = RUNNING_MOD.try_lock().map(|m| m.clone()).unwrap_or_default();
    let native = natives::running();
    if running.is_empty() && during.is_empty() {
        return None;
    }
    let mut names = running.clone();
    if !during.is_empty() && !names.contains(&during) {
        names.push(during.clone());
    }
    let note = serde_json::json!({ "mods": names, "during": during, "native": native });
    let _ = std::fs::create_dir_all(dir.join(".glue5"));
    let _ = std::fs::write(dir.join(".glue5/failed.json"), note.to_string());
    Some(match (during.is_empty(), native) {
        (false, Some(n)) => format!("Mod {during} was calling {n}."),
        (false, None) => format!("Mod {during} was running."),
        _ => format!("Mods running: {}.", running.join(", ")),
    })
}

/// The mods kept from starting with the game this time, and why.
pub fn held_back() -> Vec<String> {
    crate::lock(&HELD_BACK).clone()
}

/// The mods folder `dir` (made if missing): its mods listed, their files
/// open to the Studio. Before the game too; they run once [`install`]ed.
pub fn open(dir: &Path) {
    let _ = std::fs::create_dir_all(dir);
    if SHARED.dir.set(dir.to_path_buf()).is_err() {
        return;
    }
    world::load_saved(dir);
    refresh();
}

/// Mods run with the game about to start (the folder [`open`]ed if it was
/// not): the mods marked autostart start once the game runs scripts. Call
/// before `glue5_runtime::start`.
pub fn install(dir: &Path) {
    open(dir);
    if INSTALLED.swap(true, Ordering::AcqRel) {
        return;
    }
    let dir = self::dir().unwrap_or(dir);
    glue5_runtime::add_hooks(engine::hooks);
    // The mods' script threads get stacks of their own.
    glue5_runtime::overlay::more_script_stacks(32);
    log::info!("mods: {} ({})", dir.display(), natives::SUMMARY);
    // The game stopped while these ran: not by themselves this time.
    let failed = dir.join(".glue5/failed.json");
    if let Ok(text) = std::fs::read_to_string(&failed) {
        let _ = std::fs::remove_file(&failed);
        let v: serde_json::Value = serde_json::from_str(&text).unwrap_or_default();
        let names: Vec<String> =
            v["mods"].as_array().into_iter().flatten().filter_map(|m| m.as_str().map(str::to_string)).collect();
        for n in &names {
            log_line(n, LogLevel::Error, "the game stopped while this mod ran: it does not start by itself this time");
        }
        if !names.is_empty() {
            let text = format!("{} did not start: the game stopped while it ran last time.", names.join(", "));
            crate::lock(&SHARED.state).notices.push_back(("mods".into(), text));
        }
        *crate::lock(&HELD_BACK) = names;
    }
    let held = held_back();
    let autostart: Vec<String> =
        mods().into_iter().filter(|m| m.info.autostart && !held.contains(&m.name)).map(|m| m.name).collect();
    for name in autostart {
        start(&name);
    }
}

/// A mutex's value, also after a panic while it was held (the panic was
/// caught and its mod stopped; what it guards stays usable).
pub(crate) fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// The mods folder, once open.
pub fn dir() -> Option<&'static Path> {
    SHARED.dir.get().map(PathBuf::as_path)
}

static INSTALLED: AtomicBool = AtomicBool::new(false);

/// Whether mods run with the game (it started with them).
pub fn installed() -> bool {
    INSTALLED.load(Ordering::Acquire)
}

/// Why a mod cannot be started now, if it cannot.
pub fn cannot_start() -> Option<&'static str> {
    (!installed()).then_some("mods run with the game, and it is not running with them: start it (mods on), or mark the mod to start with the game")
}

/// A mod's file was saved (`path`), from the Studio or by an agent: the
/// mod restarts with it when it runs and its page says so ("Restart when a
/// file is saved"). Whether it restarts.
pub fn saved(path: &str) -> bool {
    let Some((name, file)) = path.split_once('/') else { return false };
    if !file.ends_with(".luau") {
        return false;
    }
    let restarts = mods().iter().any(|m| m.name == name && m.state == ModState::Running && m.info.restart_on_save);
    if restarts {
        restart(name);
    }
    restarts
}

/// What mod `name` keeps in its storage, as JSON (`{}` when nothing).
pub fn stored(name: &str) -> String {
    if !files::valid_name(name) {
        return "{}".into();
    }
    dir()
        .and_then(|d| std::fs::read_to_string(d.join(".glue5/storage").join(format!("{name}.json"))).ok())
        .unwrap_or_else(|| "{}".into())
}

/// Empties mod `name`'s storage and deletes its database (a running
/// mod's too).
pub fn clear_stored(name: &str) {
    if !files::valid_name(name) {
        return;
    }
    if let Some(d) = dir() {
        let _ = std::fs::remove_file(d.join(".glue5/storage").join(format!("{name}.json")));
    }
    // A running mod's is closed and deleted by its VM (here it may be
    // open, which some systems do not delete).
    db::delete(name);
    command(Command::ClearStorage(name.to_string()));
}

/// Whether mod `name` starts with the game.
pub fn set_autostart(name: &str, on: bool) -> Result<(), String> {
    let mut info = files::info(name);
    info.autostart = on;
    files::set_info(name, &info)
}

/// Reads the mods folder again (new or removed mods).
pub fn refresh() {
    let Some(dir) = dir() else { return };
    let mut found = Vec::new();
    if let Ok(entries) = std::fs::read_dir(dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.join("main.luau").is_file() {
                let name = entry.file_name().to_string_lossy().into_owned();
                found.push((name.clone(), files::info_at(&path, &name)));
            }
        }
    }
    found.sort_by(|a, b| a.0.cmp(&b.0));
    let mut s = crate::lock(&SHARED.state);
    let before = s.mods.len();
    s.mods.retain(|m| m.state != ModState::Stopped || found.iter().any(|(n, _)| *n == m.name));
    let mut changed = before != s.mods.len();
    for (name, info) in found {
        match s.mods.iter_mut().find(|m| m.name == name) {
            Some(m) => {
                changed |= m.info != info;
                m.info = info;
            }
            None => {
                changed = true;
                s.mods.push(Mod {
                    name,
                    state: ModState::Stopped,
                    error: None,
                    cpu_ms: 0.0,
                    memory: 0,
                    info,
                    waiting: false,
                })
            }
        }
    }
    // As people read them: by the name shown, numbers as numbers.
    s.mods.sort_by(|a, b| natural(&a.info.title, &b.info.title).then_with(|| a.name.cmp(&b.name)));
    if changed {
        SHARED.generation.fetch_add(1, Ordering::Relaxed);
    }
}

/// Text compared as people expect: case aside, digits as numbers
/// (`filler-2` before `filler-10`).
fn natural(a: &str, b: &str) -> std::cmp::Ordering {
    let (mut a, mut b) = (a.chars().peekable(), b.chars().peekable());
    loop {
        match (a.peek().copied(), b.peek().copied()) {
            (None, None) => return std::cmp::Ordering::Equal,
            (None, _) => return std::cmp::Ordering::Less,
            (_, None) => return std::cmp::Ordering::Greater,
            (Some(x), Some(y)) if x.is_ascii_digit() && y.is_ascii_digit() => {
                let number = |it: &mut std::iter::Peekable<std::str::Chars>| {
                    let mut n = 0u64;
                    while let Some(d) = it.peek().and_then(|c| c.to_digit(10)) {
                        n = n.saturating_mul(10).saturating_add(u64::from(d));
                        it.next();
                    }
                    n
                };
                let ord = number(&mut a).cmp(&number(&mut b));
                if ord.is_ne() {
                    return ord;
                }
            }
            (Some(x), Some(y)) => {
                let ord = x.to_lowercase().cmp(y.to_lowercase());
                if ord.is_ne() {
                    return ord;
                }
                a.next();
                b.next();
            }
        }
    }
}

/// The types the modules share (`Color`), first in the definitions.
pub(crate) const COMMON: &str = include_str!("common.d.luau");

/// The Luau definitions of what a mod uses, for the type checker: the
/// installed game's names, the natives and the globals, the globals the
/// modules add.
pub(crate) fn definitions() -> String {
    [COMMON, world::definitions().as_str(), &offsets::definitions(), natives::DEFINITIONS, menus::DEFINITIONS, structs::DEFINITIONS, &input::definitions(), draw::DEFINITIONS, db::DEFINITIONS, net::DEFINITIONS, handling::DEFINITIONS, stdlib::DEFINITIONS].concat()
}

/// The mods and their state.
pub fn mods() -> Vec<Mod> {
    crate::lock(&SHARED.state).mods.clone()
}

/// Changes whenever [`mods`] would give something else (but the time and
/// memory figures).
pub fn generation() -> u64 {
    SHARED.generation.load(Ordering::Relaxed)
}

/// Starts mod `name` (again, if it ran before).
pub fn start(name: &str) {
    // A folder of the mods folder, nothing else.
    if files::valid_name(name) {
        command(Command::Start(name.to_string()));
    }
}

/// Stops mod `name`: its thread ends, with what it created.
pub fn stop(name: &str) {
    if files::valid_name(name) {
        command(Command::Stop(name.to_string()));
    }
}

/// Stops mod `name` if it runs, then starts it.
pub fn restart(name: &str) {
    stop(name);
    start(name);
}

/// Runs Luau `code` in the game (in a script thread of its own, as a mod),
/// as an expression if it is one. `reply` gets the values as text.
pub fn eval(code: &str, reply: Sender<EvalResult>) {
    if !installed() {
        let _ = reply.send(Err("the game is not running with mods: they run from the next start".into()));
        return;
    }
    command(Command::Eval(code.to_string(), reply));
}

fn command(c: Command) {
    if !installed() {
        return;
    }
    crate::lock(&SHARED.state).commands.push(c);
    SHARED.pending.store(true, Ordering::Release);
}

/// The log lines after `seq` (0: all kept), and the cursor to pass next.
pub fn logs(seq: u64) -> (Vec<LogLine>, u64) {
    let s = crate::lock(&SHARED.state);
    // From the end: the lines are in order, and callers ask for the newest.
    let new = s.log.iter().rev().take_while(|l| l.seq > seq).count();
    let lines: Vec<LogLine> = s.log.iter().skip(s.log.len() - new).cloned().collect();
    (lines, s.next_seq - 1)
}

/// The last `n` log lines.
pub fn recent_logs(n: usize) -> Vec<LogLine> {
    let s = crate::lock(&SHARED.state);
    s.log.iter().skip(s.log.len().saturating_sub(n)).cloned().collect()
}

/// The files changed after `seq` (by any client), and the cursor to pass next.
pub fn changes(seq: u64) -> (Vec<String>, u64) {
    let s = crate::lock(&SHARED.state);
    let paths = s.changes.iter().filter(|(n, _)| *n > seq).map(|(_, p)| p.clone()).collect();
    (paths, s.next_change - 1)
}

pub(crate) fn changed(path: &str) {
    let mut s = crate::lock(&SHARED.state);
    if s.changes.len() == 256 {
        s.changes.pop_front();
    }
    let n = s.next_change;
    s.next_change += 1;
    s.changes.push_back((n, path.to_string()));
}

/// A mod's menu (`ui.menu` in Luau, or `menu:submenu` inside another):
/// what the application, the Studio and MCP show for it.
#[derive(Clone, Debug, PartialEq)]
pub struct Menu {
    pub module: String,
    /// The run of the mod that made it (a restarted mod's menus are its
    /// new run's; the old run's go with it).
    pub instance: u64,
    pub id: u32,
    /// The menu whose submenu row opens this one; None: top-level.
    pub parent: Option<u32>,
    pub title: String,
    /// A top-level menu's line under its title in the list of menus.
    pub note: Option<String>,
    /// Players filter its rows by what they type (a long list's).
    pub search: bool,
    pub items: Vec<MenuItem>,
    /// Bumped by every change to the menu or its items (structure or value).
    pub rev: u64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct MenuItem {
    pub id: u32,
    /// A label item's text, a separator's title.
    pub label: String,
    /// A line under the label.
    pub note: Option<String>,
    /// False: shown dimmed, not usable.
    pub enabled: bool,
    /// False: not shown at all.
    pub visible: bool,
    pub kind: MenuKind,
}

#[derive(Clone, Debug, PartialEq)]
pub enum MenuKind {
    /// `action` is the pill's text (None: "Run").
    Button { action: Option<String> },
    Toggle(bool),
    /// `value` is on the grid `min + k*step`, or `max`; a `unit` of "%"
    /// shows the value times 100.
    Slider { value: f64, min: f64, max: f64, step: f64, unit: Option<String> },
    /// A side without a bound is ±1e15.
    Number { value: f64, min: Option<f64>, max: Option<f64>, step: f64, unit: Option<String> },
    /// `index` is 0-based.
    Choice { options: Arc<[String]>, index: usize },
    /// 0-based, ascending.
    Multi { options: Arc<[String]>, selected: Vec<u16> },
    /// Set when committed (Enter, Done, focus lost), not per keystroke.
    Text { value: String, placeholder: Option<String>, max: u16 },
    /// With `alpha` false, `value.a` is 255.
    Color { value: Rgba, alpha: bool },
    Label,
    Separator,
    /// 0 to 1.
    Progress(f32),
    /// A row that opens the menu of this id.
    Submenu(u32),
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Rgba {
    pub r: u8,
    pub g: u8,
    pub b: u8,
    pub a: u8,
}

impl Rgba {
    /// `#rrggbb`, or `#rrggbbaa` with `alpha`.
    pub fn hex(self, alpha: bool) -> String {
        let Rgba { r, g, b, a } = self;
        if alpha { format!("#{r:02x}{g:02x}{b:02x}{a:02x}") } else { format!("#{r:02x}{g:02x}{b:02x}") }
    }

    /// `#rrggbb` or `#rrggbbaa` (the `#` may be left out).
    pub fn from_hex(text: &str) -> Option<Rgba> {
        menus::validate::parse_hex(text)
    }
}

/// What a player did with a menu's item.
#[derive(Clone, Debug, PartialEq)]
pub enum MenuInput {
    /// A button.
    Press,
    Toggle(bool),
    /// A slider or a number.
    Number(f64),
    /// A choice's option, 0-based.
    Choose(usize),
    /// A multi's options, 0-based.
    Select(Vec<u16>),
    Text(String),
    Color(Rgba),
}

/// The menus (all, submenus included, in creation order) as a shared
/// snapshot, rebuilt only after a change: call it every frame.
pub fn menus() -> Arc<Vec<Menu>> {
    menus::snapshot()
}

/// Whether any mod has a menu.
pub fn has_menus() -> bool {
    menus::any()
}

/// A player (app, Studio, MCP) changed or pressed `item` of mod `module`:
/// checked against the item (kind, range, enabled, visible, its menus
/// shown); on success the shared value changes at once and the mod hears
/// it at its next turn. The error is for people (the Studio shows it).
pub fn menu_input(module: &str, item: u32, input: MenuInput) -> Result<(), String> {
    menus::input(module, item, input)
}

/// A viewer (the app's page, a Studio session, MCP) starts (true) or stops
/// (false) looking at menu `menu`: the first one runs the menu's `onOpen`,
/// the last one gone its `onClose`.
pub fn menu_viewer(module: &str, menu: u32, open: bool) {
    menus::viewer(module, menu, open)
}

/// A mod asked to be shown (`menu:show()`): (module, menu id), once.
pub fn take_menu_show() -> Option<(String, u32)> {
    menus::take_show()
}

/// What the mods notified since the last call: (mod, text).
pub fn take_notices() -> Vec<(String, String)> {
    crate::lock(&SHARED.state).notices.drain(..).collect()
}

/// Whether a native trapped: the game may now misbehave.
pub fn tainted() -> bool {
    natives::TAINTED.load(Ordering::Relaxed)
}

pub(crate) fn log_line(module: &str, level: LogLevel, text: &str) {
    let cut;
    let text = if text.len() > LOG_LINE {
        let head = &text[..text.floor_char_boundary(LOG_LINE)];
        cut = format!("{head}… ({} bytes more)", text.len() - head.len());
        &cut
    } else {
        text
    };
    match level {
        LogLevel::Info => log::info!("mod {module}: {text}"),
        LogLevel::Error => log::error!("mod {module}: {text}"),
    }
    let mut s = crate::lock(&SHARED.state);
    if s.log.len() == LOG_LINES {
        s.log.pop_front();
    }
    let seq = s.next_seq;
    s.next_seq += 1;
    s.log.push_back(LogLine { seq, time: SystemTime::now(), module: module.to_string(), level, text: text.to_string() });
}

#[cfg(test)]
mod tests {
    /// A mod that prints megabytes leaves a line of 8 KB and a note of how
    /// much was cut.
    #[test]
    fn long_log_lines_are_cut() {
        let text = format!("{}tail", "é".repeat(10_000));
        super::log_line("log-test", super::LogLevel::Info, &text);
        let line = super::recent_logs(super::LOG_LINES).into_iter().rev().find(|l| l.module == "log-test").unwrap();
        assert!(line.text.len() < super::LOG_LINE + 64, "{}", line.text.len());
        assert!(line.text.ends_with(&format!("… ({} bytes more)", text.len() - super::LOG_LINE)), "{}", &line.text[line.text.len() - 40..]);
        super::log_line("log-test", super::LogLevel::Info, "short");
        assert_eq!(super::recent_logs(super::LOG_LINES).into_iter().rev().find(|l| l.module == "log-test").unwrap().text, "short");
    }
}
