//! The page's state: what the game sent, what is open, the signals the views read.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;

use leptos::prelude::*;
use serde::Deserialize;
use serde_json::Value;

use crate::editor::StudioEditor;
use crate::menus::{Level, MenuInfo};

#[derive(Clone, Deserialize, PartialEq, Debug)]
pub(crate) struct ModInfo {
    pub(crate) name: String,
    pub(crate) state: String,
    pub(crate) error: Option<String>,
    pub(crate) waiting: bool,
    #[serde(rename = "cpuMs")]
    pub(crate) cpu_ms: f64,
    pub(crate) autostart: bool,
    pub(crate) files: Vec<String>,
    /// From its mod.json.
    #[serde(default)]
    pub(crate) title: String,
    #[serde(default)]
    pub(crate) description: String,
    #[serde(default, rename = "restartOnSave")]
    pub(crate) restart_on_save: bool,
}

#[derive(Clone, Deserialize, PartialEq, Debug)]
pub(crate) struct LogLine {
    pub(crate) time: u64,
    #[serde(rename = "mod")]
    pub(crate) module: String,
    pub(crate) level: String,
    pub(crate) text: String,
}

/// A console row: the game's log, or the REPL's input and answers.
#[derive(Clone, PartialEq)]
pub(crate) enum Row {
    Log(LogLine),
    Input(String),
    Value(String),
    Failure(String),
}

#[derive(Clone, Deserialize, PartialEq, Debug)]
pub(crate) struct Diag {
    pub(crate) line: u32,
    pub(crate) column: u32,
    pub(crate) severity: String,
    pub(crate) message: String,
}

#[derive(Clone, PartialEq)]
pub(crate) struct Tab {
    pub(crate) path: String,
    pub(crate) dirty: bool,
    pub(crate) read_only: bool,
}

#[derive(Clone, Copy, PartialEq)]
pub(crate) enum Side {
    Mods,
    Natives,
}

#[derive(Clone, Copy, PartialEq)]
pub(crate) enum Panel {
    Console,
    Problems,
}

/// What a phone shows (one at a time).
#[derive(Clone, Copy, PartialEq)]
pub(crate) enum Screen {
    Mods,
    Editor,
    Console,
    Problems,
}

/// What the page is: the Studio (`/`) or the control page (`/control`, the
/// mods and their menus).
#[derive(Clone, Copy, PartialEq, Debug)]
pub(crate) enum Mode {
    Studio,
    Control,
}

impl Mode {
    /// The scope the page pairs for (the server's names).
    pub(crate) fn scope(self) -> &'static str {
        match self {
            Mode::Studio => "studio",
            Mode::Control => "control",
        }
    }
}

#[derive(Clone, Copy, PartialEq)]
pub(crate) enum Filter {
    All,
    Current,
    Errors,
}

/// Pairing: unknown yet, needed (no or a wrong code), done.
#[derive(Clone, Copy, PartialEq)]
pub(crate) enum Pairing {
    Connecting,
    Needed,
    Paired,
}

#[derive(Clone, Copy)]
pub(crate) struct S {
    pub(crate) pairing: RwSignal<Pairing>,
    pub(crate) connected: RwSignal<bool>,
    pub(crate) hello: RwSignal<Value>,
    pub(crate) mods: RwSignal<Vec<ModInfo>>,
    /// The game told which mods it has (until then the list is unknown, not empty).
    pub(crate) listed: RwSignal<bool>,
    pub(crate) menus: RwSignal<Vec<MenuInfo>>,
    /// What is open in each top-level menu's card (submenus, pickers).
    pub(crate) menu_stacks: RwSignal<HashMap<u32, Vec<Level>>>,
    /// The page has the phone's layout (one screen at a time).
    pub(crate) phone: RwSignal<bool>,
    pub(crate) rows: RwSignal<Vec<(u64, Row)>>,
    pub(crate) tabs: RwSignal<Vec<Tab>>,
    pub(crate) active: RwSignal<Option<String>>,
    pub(crate) expanded: RwSignal<Vec<String>>,
    /// The folders of mods' files the person closed (`<mod>/<dir>`).
    pub(crate) folded: RwSignal<Vec<String>>,
    /// A file or folder being dragged in the tree, and the folder (or mod) it
    /// would drop into.
    pub(crate) dragging: RwSignal<Option<String>>,
    pub(crate) drop_at: RwSignal<Option<String>>,
    pub(crate) side: RwSignal<Side>,
    pub(crate) panel: RwSignal<Panel>,
    pub(crate) screen: RwSignal<Screen>,
    pub(crate) drawer: RwSignal<bool>,
    pub(crate) diags: RwSignal<HashMap<String, Vec<Diag>>>,
    pub(crate) cursor: RwSignal<(u32, u32)>,
    pub(crate) filter: RwSignal<Filter>,
    pub(crate) query: RwSignal<String>,
    pub(crate) found: RwSignal<Vec<Value>>,
    pub(crate) palette: RwSignal<bool>,
    pub(crate) notice: RwSignal<Option<String>>,
    /// The code typed did not work.
    pub(crate) pair_failed: RwSignal<bool>,
    /// A mod's context menu (right click in the tree): the mod, and where.
    pub(crate) context: RwSignal<Option<(String, i32, i32)>>,
}

thread_local! {
    pub(crate) static TOKEN: RefCell<String> = const { RefCell::new(String::new()) };
    pub(crate) static SOCKET: RefCell<Option<web_sys::WebSocket>> = const { RefCell::new(None) };
    pub(crate) static PENDING: RefCell<HashMap<u64, Box<dyn FnOnce(Result<Value, String>)>>> = RefCell::default();
    pub(crate) static NEXT_ID: Cell<u64> = const { Cell::new(1) };
    pub(crate) static EDITOR: RefCell<Option<StudioEditor>> = const { RefCell::new(None) };
    pub(crate) static NEXT_ROW: Cell<u64> = const { Cell::new(1) };
    /// The console's "run this" callback, made once.
    pub(crate) static REPL_RUN: RefCell<Option<wasm_bindgen::closure::Closure<dyn FnMut(String)>>> = const { RefCell::new(None) };
    /// The page was paired with the game once (a lost connection is retried).
    pub(crate) static PAIRED: Cell<bool> = const { Cell::new(false) };
    pub(crate) static MODE: Cell<Mode> = const { Cell::new(Mode::Studio) };
}

pub(crate) fn mode() -> Mode {
    MODE.with(Cell::get)
}

/// Where a page keeps its pairing's token. The control page uses the
/// Studio's when this browser has one (it may do everything).
const STUDIO_TOKEN: &str = "glue5-studio-token";
const CONTROL_TOKEN: &str = "glue5-control-token";

pub(crate) fn stored_token() -> Option<String> {
    let st = storage()?;
    let get = |k: &str| st.get_item(k).ok().flatten().filter(|t| !t.is_empty());
    match mode() {
        Mode::Studio => get(STUDIO_TOKEN),
        Mode::Control => get(STUDIO_TOKEN).or_else(|| get(CONTROL_TOKEN)),
    }
}

/// Keeps the token of a pairing for `scope` ("studio", "control").
pub(crate) fn store_token(scope: &str, token: &str) {
    if let Some(st) = storage() {
        let _ = st.set_item(if scope == "studio" { STUDIO_TOKEN } else { CONTROL_TOKEN }, token);
        // The code kept before pages paired is no longer needed.
        let _ = st.remove_item("glue5-token");
    }
}

/// The token the page used was refused (its pairing was removed): gone.
pub(crate) fn forget_token(token: &str) {
    if let Some(st) = storage() {
        for key in [STUDIO_TOKEN, CONTROL_TOKEN] {
            if st.get_item(key).ok().flatten().as_deref() == Some(token) {
                let _ = st.remove_item(key);
            }
        }
    }
}

pub(crate) fn with_editor(f: impl FnOnce(&StudioEditor)) {
    EDITOR.with(|e| {
        if let Some(e) = e.borrow().as_ref() {
            f(e)
        }
    })
}

pub(crate) fn storage() -> Option<web_sys::Storage> {
    web_sys::window()?.local_storage().ok()?
}

pub(crate) fn host() -> String {
    web_sys::window().and_then(|w| w.location().host().ok()).unwrap_or_default()
}

pub(crate) fn push_row(s: S, row: Row) {
    let n = NEXT_ROW.with(|c| {
        let n = c.get();
        c.set(n + 1);
        n
    });
    s.rows.update(|rows| {
        rows.push((n, row));
        if rows.len() > 1500 {
            rows.drain(..300);
        }
    });
}

pub(crate) fn mod_of(path: &str) -> &str {
    path.strip_prefix(MOD_PAGE).unwrap_or(path).split('/').next().unwrap_or("")
}

/// A tab's path for a mod's page (`mod:trainer`), not a file.
pub(crate) const MOD_PAGE: &str = "mod:";

/// The mod whose page `path` is, if it is one.
pub(crate) fn mod_page(path: &str) -> Option<&str> {
    path.strip_prefix(MOD_PAGE)
}

/// The files the tree shows: not mod.json (the mod's page is it).
pub(crate) fn shown_file(path: &str) -> bool {
    path.split_once('/').is_none_or(|(_, f)| f != "mod.json")
}

/// `trainer/main.luau:12` in a message: the file and line it points at.
pub(crate) fn file_location(text: &str) -> Option<(String, u32, usize, usize)> {
    let end = text.find(".luau:")? + 5;
    let start = text[..end].rfind(|c: char| c.is_whitespace() || c == '"' || c == '(' || c == '[').map_or(0, |i| i + 1);
    let digits: String = text[end + 1..].chars().take_while(char::is_ascii_digit).collect();
    let line = digits.parse().ok()?;
    let path = text[start..end].trim_start_matches('@').to_string();
    path.contains('/').then_some((path, line, start, end + 1 + digits.len()))
}
