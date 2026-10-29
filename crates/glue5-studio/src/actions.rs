//! What the buttons and keys do: open, close, save, evaluate.

use leptos::prelude::*;
use serde_json::json;
use wasm_bindgen::prelude::*;

use crate::net::{fetch_text, request};
use crate::state::*;

/// A mod's page, in a tab of its own.
pub(crate) fn open_mod_page(s: S, name: &str) {
    s.screen.set(Screen::Editor);
    s.drawer.set(false);
    let path = format!("{MOD_PAGE}{name}");
    if !s.tabs.with_untracked(|t| t.iter().any(|t| t.path == path)) {
        s.tabs.update(|t| t.push(Tab { path: path.clone(), dirty: false, read_only: true }));
    }
    s.active.set(Some(path));
}

/// Closes the tabs of mod `name` (its page, its files): it went away.
pub(crate) fn close_mod(s: S, name: &str) {
    let page = format!("{MOD_PAGE}{name}");
    let prefix = format!("{name}/");
    let gone: Vec<String> = s.tabs.with_untracked(|t| {
        t.iter().filter(|t| t.path == page || t.path.starts_with(&prefix)).map(|t| t.path.clone()).collect()
    });
    for path in &gone {
        forget_tab(s, path);
    }
}

/// A new mod, asked for by name; its main.luau opens.
pub(crate) fn new_mod(s: S) {
    let Some(name) = web_sys::window().and_then(|w| w.prompt_with_message("New mod's name (lowercase, digits, - and _):").ok().flatten()) else { return };
    let name = name.trim().to_lowercase();
    if name.is_empty() {
        return;
    }
    let n = name.clone();
    request("mods.create", json!({ "name": name }), move |r| match r {
        Ok(_) => {
            s.expanded.update(|e| e.push(n.clone()));
            open_file(s, format!("{n}/main.luau"));
        }
        Err(e) => s.notice.set(Some(e)),
    });
}

/// A new file in `name` (a mod, or a folder of one: `<mod>/<dir>`), asked
/// for by name (`core/prefs.luau` makes it in a folder), then opened.
pub(crate) fn new_file(s: S, name: &str) {
    let Some(file) = web_sys::window()
        .and_then(|w| w.prompt_with_message_and_default("New file's name (in a folder: folder/name.luau):", "module.luau").ok().flatten())
    else {
        return;
    };
    let file = file.trim().to_string();
    if file.is_empty() {
        return;
    }
    let path = format!("{name}/{file}");
    request("files.create", json!({ "path": path }), move |r| match r {
        Ok(_) => open_file(s, path),
        Err(e) => s.notice.set(Some(e)),
    });
}

/// Renames or moves a file or a folder (`<mod>/<dir>`), asked for the new
/// name; its tabs follow.
pub(crate) fn rename_file(s: S, path: &str) {
    let Some((name, file)) = path.split_once('/') else { return };
    let Some(new) = web_sys::window().and_then(|w| w.prompt_with_message_and_default("Rename or move to (folder/name moves it):", file).ok().flatten()) else {
        return;
    };
    let new = new.trim().to_string();
    if new.is_empty() || new == file {
        return;
    }
    move_path(s, path.to_string(), format!("{name}/{new}"));
}

/// Moves (renames) a file or a folder within its mod: dropped in the tree,
/// or renamed. Its tabs follow.
pub(crate) fn move_path(s: S, from: String, to: String) {
    // Unsaved changes go first (what moved opens from disk).
    let inside = format!("{from}/");
    let dirty: Vec<String> = s.tabs.with_untracked(|t| {
        t.iter().filter(|t| t.dirty && (t.path == from || t.path.starts_with(&inside))).map(|t| t.path.clone()).collect()
    });
    if let Some(path) = dirty.into_iter().next() {
        let mut text = String::new();
        with_editor(|e| text = e.text(&path));
        request("files.write", json!({ "path": path, "text": text }), move |r| match r {
            // One at a time: the next dirty one, if any, then the move.
            Ok(_) => {
                s.tabs.update(|t| {
                    if let Some(t) = t.iter_mut().find(|t| t.path == path) {
                        t.dirty = false;
                    }
                });
                move_path(s, from, to);
            }
            Err(e) => s.notice.set(Some(e)),
        });
        return;
    }
    rename_now(s, from, to);
}

fn rename_now(s: S, from: String, to: String) {
    request("files.rename", json!({ "from": from, "to": to }), move |r| match r {
        // The tabs of what moved (a file, or a folder's files) follow it.
        Ok(_) => {
            let inside = format!("{from}/");
            let moved: Vec<String> = s.tabs.with_untracked(|t| {
                t.iter().filter(|t| t.path == from || t.path.starts_with(&inside)).map(|t| t.path.clone()).collect()
            });
            for old in moved {
                forget_tab(s, &old);
                open_file(s, format!("{to}{}", &old[from.len()..]));
            }
        }
        Err(e) => s.notice.set(Some(e)),
    });
}

/// Deletes a file, or a folder with what it holds, once confirmed.
pub(crate) fn delete_file(s: S, path: &str, folder: bool) {
    let what = if folder { format!("{path} and everything in it") } else { path.to_string() };
    let ok = web_sys::window()
        .and_then(|w| w.confirm_with_message(&format!("Delete {what}? It cannot be undone.")).ok())
        .unwrap_or(false);
    if !ok {
        return;
    }
    let p = path.to_string();
    request("files.delete", json!({ "path": p }), move |r| match r {
        Ok(_) => {
            let inside = format!("{p}/");
            let gone: Vec<String> = s.tabs.with_untracked(|t| {
                t.iter().filter(|t| t.path == p || t.path.starts_with(&inside)).map(|t| t.path.clone()).collect()
            });
            for path in gone {
                forget_tab(s, &path);
            }
        }
        Err(e) => s.notice.set(Some(e)),
    });
}

/// A new folder in `name` (a mod or one of its folders), with a first file
/// in it: folders hold files.
pub(crate) fn new_folder(s: S, name: &str) {
    let Some(folder) = web_sys::window().and_then(|w| w.prompt_with_message("New folder's name:").ok().flatten()) else {
        return;
    };
    let folder = folder.trim().trim_matches('/').to_string();
    if folder.is_empty() {
        return;
    }
    new_file(s, &format!("{name}/{folder}"));
}

/// A mod from a .zip the person picks; its page opens.
pub(crate) fn import(s: S) {
    let token = TOKEN.with(|t| t.borrow().clone());
    let done = Closure::<dyn FnMut(JsValue)>::new(move |name: JsValue| {
        if let Some(name) = name.as_string() {
            s.expanded.update(|e| e.push(name.clone()));
            open_mod_page(s, &name);
        }
    });
    let failed = Closure::<dyn FnMut(JsValue)>::new(move |e: JsValue| {
        s.notice.set(Some(format!("Import: {}", e.as_string().unwrap_or_else(|| "it failed".into()))));
    });
    let _ = crate::editor::import_mod(&format!("/import?token={token}")).then2(&done, &failed);
    done.forget();
    failed.forget();
}

pub(crate) fn open_file(s: S, path: String) {
    open_file_at(s, path, None);
}

/// Opens a file, and puts the cursor at (line, column) once it shows (not
/// before: on a slow link the file is read later).
pub(crate) fn open_file_at(s: S, path: String, at: Option<(u32, u32)>) {
    s.screen.set(Screen::Editor);
    s.drawer.set(false);
    let reveal = move || {
        if let Some((line, column)) = at {
            with_editor(|e| e.reveal(line, column));
        }
    };
    if s.tabs.with_untracked(|t| t.iter().any(|t| t.path == path)) {
        s.active.set(Some(path.clone()));
        with_editor(|e| e.open(&path, "", false));
        reveal();
        return;
    }
    let read_only = path == "natives.d.luau";
    let p = path.clone();
    let shown = move |text: String| {
        // Asked twice (a double tap) before the first answer: one tab.
        if !s.tabs.with_untracked(|t| t.iter().any(|t| t.path == p)) {
            s.tabs.update(|t| t.push(Tab { path: p.clone(), dirty: false, read_only }));
        }
        with_editor(|e| e.open(&p, &text, read_only));
        s.active.set(Some(p.clone()));
        reveal();
    };
    if read_only {
        // The definitions, as served (not a mod's file).
        let token = TOKEN.with(|t| t.borrow().clone());
        let url = format!("/natives.d.luau?token={token}");
        fetch_text(url, shown);
    } else {
        request("files.read", json!({ "path": path }), move |r| match r {
            Ok(v) => shown(v["text"].as_str().unwrap_or("").to_string()),
            Err(e) => s.notice.set(Some(e)),
        });
    }
}

/// A tab goes (closed, its file deleted or renamed, its mod gone): out of
/// the editor and the problems; the last tab left shows instead.
pub(crate) fn forget_tab(s: S, path: &str) {
    if mod_page(path).is_none() {
        with_editor(|e| e.close(path));
    }
    s.tabs.update(|t| t.retain(|t| t.path != path));
    s.diags.update(|d| {
        d.remove(path);
    });
    if s.active.get_untracked().as_deref() == Some(path) {
        let next = s.tabs.with_untracked(|t| t.last().map(|t| t.path.clone()));
        s.active.set(next.clone());
        if let Some(n) = next.filter(|n| mod_page(n).is_none()) {
            with_editor(|e| e.open(&n, "", false));
        }
    }
}

pub(crate) fn close_tab(s: S, path: &str) {
    let dirty = s.tabs.with_untracked(|t| t.iter().any(|t| t.path == path && t.dirty));
    if dirty
        && !web_sys::window()
            .and_then(|w| w.confirm_with_message(&format!("{path} has unsaved changes. Close it?")).ok())
            .unwrap_or(false)
    {
        return;
    }
    forget_tab(s, path);
}

/// Code into the open file at the cursor; with no file open (or a mod's
/// page shown), it says so rather than typing into nothing.
pub(crate) fn insert_code(s: S, code: &str) {
    let writable = active_file(s).filter(|p| s.tabs.with_untracked(|t| t.iter().any(|t| &t.path == p && !t.read_only)));
    if writable.is_none() {
        s.notice.set(Some("Open one of the mod's files first: it goes in at the cursor".into()));
        return;
    }
    s.screen.set(Screen::Editor);
    with_editor(|e| e.insert(code));
}

/// The open file, when the active tab is one (not a mod's page).
pub(crate) fn active_file(s: S) -> Option<String> {
    s.active.get_untracked().filter(|p| mod_page(p).is_none())
}

pub(crate) fn save(s: S, path: String) {
    // The definitions and mods' pages are not files to write.
    if mod_page(&path).is_some() || s.tabs.with_untracked(|t| t.iter().any(|t| t.path == path && t.read_only)) {
        return;
    }
    let mut text = String::new();
    with_editor(|e| text = e.text(&path));
    let t = text.clone();
    request("files.write", json!({ "path": path, "text": text }), move |r| match r {
        Ok(_) => with_editor(|e| e.saved(&path, &t)),
        Err(e) => s.notice.set(Some(e)),
    });
}

pub(crate) fn eval(s: S, code: String) {
    if code.trim().is_empty() {
        return;
    }
    push_row(s, Row::Input(code.clone()));
    request("eval", json!({ "code": code }), move |r| match r {
        Ok(v) => push_row(s, Row::Value(v.as_str().unwrap_or("").to_string())),
        Err(e) => push_row(s, Row::Failure(e)),
    });
}
