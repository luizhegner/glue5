//! Luau's type checker (Luau.Analysis, csrc/checker.cpp) with the natives'
//! definitions, for GLUE 5 Studio: diagnostics, completions, hovers. It
//! takes memory (every native's type), so it exists only while something
//! uses it: [`Checker::get`] makes it, [`Checker::release`] drops it.

use std::ffi::{CStr, CString, c_char};
use std::sync::Mutex;

use serde_json::Value;

use crate::natives;

#[repr(C)]
struct Raw {
    _private: [u8; 0],
}

unsafe extern "C" {
    fn glue5_checker_new(definitions: *const c_char, len: usize, error: *mut *mut c_char) -> *mut Raw;
    fn glue5_checker_free(c: *mut Raw);
    fn glue5_string_free(s: *mut c_char);
    fn glue5_checker_set(c: *mut Raw, name: *const c_char, source: *const c_char, len: usize);
    fn glue5_checker_check(c: *mut Raw, name: *const c_char) -> *mut c_char;
    fn glue5_checker_complete(c: *mut Raw, name: *const c_char, line: u32, column: u32) -> *mut c_char;
    fn glue5_checker_hover(c: *mut Raw, name: *const c_char, line: u32, column: u32) -> *mut c_char;
    fn glue5_checker_signature(c: *mut Raw, name: *const c_char, line: u32, column: u32) -> *mut c_char;
}

pub struct Checker {
    raw: *mut Raw,
    /// The files it has.
    known: std::collections::HashSet<String>,
}

// The C++ side has no thread affinity; the Mutex serialises the calls.
unsafe impl Send for Checker {}

static CHECKER: Mutex<Option<Checker>> = Mutex::new(None);
/// When it was last used.
static USED: Mutex<Option<std::time::Instant>> = Mutex::new(None);

impl Checker {
    /// Runs `f` with the checker, made the first time.
    pub fn with<R>(f: impl FnOnce(&mut Checker) -> R) -> Result<R, String> {
        *crate::lock(&USED) = Some(std::time::Instant::now());
        let mut slot = crate::lock(&CHECKER);
        if slot.is_none() {
            let (started, before) = (std::time::Instant::now(), glue5_runtime::memory::footprint().0);
            *slot = Some(Checker::new()?);
            // What it costs, on phones above all.
            let grew = glue5_runtime::memory::footprint().0.zip(before).map(|(a, b)| a.saturating_sub(b) >> 20);
            log::info!(
                "mods: type checker ready in {:.0} ms, +{} MiB",
                started.elapsed().as_secs_f64() * 1000.0,
                grew.map_or("?".into(), |m| m.to_string())
            );
        }
        Ok(f(slot.as_mut().unwrap()))
    }

    /// The file `name` is gone (deleted, renamed): out of the checker, if
    /// there is one (a `require` of it no longer checks).
    pub fn forget(name: &str) {
        if let Some(c) = crate::lock(&CHECKER).as_mut() {
            c.set(name, None);
        }
    }

    /// Drops the checker when it has not been used for `idle` (for users
    /// that do not say when they are done: coding agents over MCP).
    pub fn release_if_idle(idle: std::time::Duration) {
        if crate::lock(&USED).is_some_and(|t| t.elapsed() >= idle) {
            Self::release();
        }
    }

    /// Drops the checker and its memory.
    pub fn release() {
        crate::lock(&CHECKER).take();
    }

    fn new() -> Result<Checker, String> {
        let defs = crate::definitions();
        let mut error: *mut c_char = std::ptr::null_mut();
        let raw = unsafe { glue5_checker_new(defs.as_ptr().cast(), defs.len(), &mut error) };
        if raw.is_null() {
            return Err(take(error).unwrap_or_else(|| "the type checker did not start".into()));
        }
        Ok(Checker { raw, known: Default::default() })
    }

    /// Sets the file `name` (`<mod>/<file>.luau`); `None` removes it.
    pub fn set(&mut self, name: &str, source: Option<&str>) {
        if source.is_some() {
            self.known.insert(name.to_string());
        } else {
            self.known.remove(name);
        }
        let name = cstring(name);
        let (ptr, len) = source.map_or((std::ptr::null(), 0), |s| (s.as_ptr().cast(), s.len()));
        unsafe { glue5_checker_set(self.raw, name.as_ptr(), ptr, len) }
    }

    /// Whether it has file `name`.
    pub fn has(&self, name: &str) -> bool {
        self.known.contains(name)
    }

    /// The file's diagnostics: `[{range: [l0, c0, l1, c1], severity, code, message}]`.
    pub fn check(&mut self, name: &str) -> Value {
        let name = cstring(name);
        json(unsafe { glue5_checker_check(self.raw, name.as_ptr()) })
    }

    /// Completions at a position (0-based): `[{label, kind, detail, doc, deprecated, correct, call}]`.
    pub fn complete(&mut self, name: &str, line: u32, column: u32) -> Value {
        let name = cstring(name);
        json(unsafe { glue5_checker_complete(self.raw, name.as_ptr(), line, column) })
    }

    /// What is at a position: `{type, doc, range}` or null.
    pub fn hover(&mut self, name: &str, line: u32, column: u32) -> Value {
        let name = cstring(name);
        json(unsafe { glue5_checker_hover(self.raw, name.as_ptr(), line, column) })
    }

    /// The call around a position: `{name, params: [{name, type}], ret, active, doc}` or null.
    pub fn signature(&mut self, name: &str, line: u32, column: u32) -> Value {
        let name = cstring(name);
        json(unsafe { glue5_checker_signature(self.raw, name.as_ptr(), line, column) })
    }
}

impl Drop for Checker {
    fn drop(&mut self) {
        unsafe { glue5_checker_free(self.raw) }
    }
}

/// The notes on a documentation symbol of the definitions
/// (`@glue5/global/vehicle.createVehicle`): the natives database's text and
/// the game's name.
pub fn documentation(symbol: &str) -> Option<String> {
    let path = symbol.strip_prefix("@glue5/global/")?;
    let (space, name) = path.split_once('.')?;
    let ns = natives::NAMESPACES.iter().position(|s| *s == space)?;
    let n = &natives::NATIVES[natives::find(ns, name)?];
    Some(if n.doc.is_empty() { format!("`{}`", n.name) } else { format!("{}\n\n`{}`", n.doc, n.name) })
}

fn cstring(s: &str) -> CString {
    CString::new(s.replace('\0', "")).unwrap()
}

fn take(s: *mut c_char) -> Option<String> {
    if s.is_null() {
        return None;
    }
    let text = unsafe { CStr::from_ptr(s) }.to_string_lossy().into_owned();
    unsafe { glue5_string_free(s) };
    Some(text)
}

fn json(s: *mut c_char) -> Value {
    take(s).and_then(|t| serde_json::from_str(&t).ok()).unwrap_or(Value::Null)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SOURCE: &str = "--!strict\nlocal me = player.playerPedId()\nvehicle.setVehicleFixed(me)\nvehicle.setVehicleE\n";

    #[test]
    fn checks_completes_hovers() {
        let started = std::time::Instant::now();
        Checker::with(|c| {
            eprintln!("ready in {:?}", started.elapsed());
            c.set("test/main.luau", Some(SOURCE));
            let t = std::time::Instant::now();
            let diagnostics = c.check("test/main.luau");
            eprintln!("check {:?}: {diagnostics}", t.elapsed());
            let text = diagnostics.to_string();
            assert!(text.contains("Ped") && text.contains("Vehicle"), "{text}");
            let t = std::time::Instant::now();
            let items = c.complete("test/main.luau", 3, 19);
            eprintln!("complete {:?}: {} items", t.elapsed(), items.as_array().map_or(0, Vec::len));
            let engine = items.as_array().unwrap().iter().find(|i| i["label"] == "setVehicleEngineOn").expect("setVehicleEngineOn");
            eprintln!("{engine}");
            assert_eq!(documentation(engine["doc"].as_str().unwrap()).is_some(), true);
            let hover = c.hover("test/main.luau", 1, 21);
            eprintln!("hover {hover}");
            assert!(hover["type"].as_str().unwrap().contains("playerPedId"));
        })
        .unwrap();
    }

    /// A required file's types come from beside the file that asks, as
    /// the runtime loads it (`./`, `../`), never from out of the mod.
    #[test]
    fn requires_are_relative() {
        Checker::with(|c| {
            c.set("req/core/prefs.luau", Some("--!strict\nreturn { speed = 2 }\n"));
            c.set("req/core/bind.luau", Some("--!strict\nlocal prefs = require(\"./prefs\")\nreturn { speed = prefs.speed }\n"));
            c.set("req/features/self.luau", Some("--!strict\nlocal bind = require(\"../core/bind\")\nlocal n: number = bind.speed\nreturn n\n"));
            c.set("req/main.luau", Some("--!strict\nlocal s: number = require(\"./features/self\")\nlocal x = require(\"../out\")\nprint(s, x)\n"));
            assert_eq!(c.check("req/features/self.luau"), serde_json::json!([]));
            let main = c.check("req/main.luau").to_string();
            assert!(main.contains("Unknown require") && !main.contains("features/self"), "{main}");
        })
        .unwrap();
    }
}
