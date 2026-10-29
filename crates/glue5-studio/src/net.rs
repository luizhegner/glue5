//! The WebSocket to the game (`/studio`): requests and what the game pushes.

use std::cell::{Cell, RefCell};

use leptos::prelude::*;
use serde_json::{Value, json};
use wasm_bindgen::prelude::*;

use crate::state::*;

/// Sends a request to the game; `then` gets its answer.
pub(crate) fn request(method: &str, params: Value, then: impl FnOnce(Result<Value, String>) + 'static) {
    let id = NEXT_ID.with(|n| {
        let id = n.get();
        n.set(id + 1);
        id
    });
    let sent = SOCKET.with(|s| {
        s.borrow()
            .as_ref()
            .and_then(|ws| ws.send_with_str(&json!({ "id": id, "method": method, "params": params }).to_string()).ok())
            .is_some()
    });
    if sent {
        PENDING.with(|p| p.borrow_mut().insert(id, Box::new(then)));
    } else {
        then(Err("not connected to the game".into()));
    }
}

pub(crate) fn connect(s: S) {
    let token = TOKEN.with(|t| t.borrow().clone());
    if token.is_empty() {
        s.pairing.set(Pairing::Needed);
        return;
    }
    let url = format!("ws://{}/studio?token={token}", host());
    let Ok(ws) = web_sys::WebSocket::new(&url) else { return };
    let opened = std::rc::Rc::new(Cell::new(false));
    {
        let opened = opened.clone();
        let on_open = Closure::<dyn FnMut()>::new(move || {
            opened.set(true);
            // Back after a break: open files the game changed meanwhile follow.
            let again = PAIRED.with(Cell::get);
            PAIRED.with(|p| p.set(true));
            s.connected.set(true);
            s.pairing.set(Pairing::Paired);
            s.pair_failed.set(false);
            if again {
                // The game sends again the log it keeps: not shown twice.
                s.rows.update(|r| r.retain(|(_, row)| !matches!(row, Row::Log(_))));
                for path in s.tabs.with_untracked(|t| t.iter().filter(|t| !t.dirty).map(|t| t.path.clone()).collect::<Vec<_>>()) {
                    reload(s, &path);
                }
            }
        });
        ws.set_onopen(Some(on_open.as_ref().unchecked_ref()));
        on_open.forget();
    }
    let on_message = Closure::<dyn FnMut(web_sys::MessageEvent)>::new(move |e: web_sys::MessageEvent| {
        let Some(text) = e.data().as_string() else { return };
        let Ok(msg) = serde_json::from_str::<Value>(&text) else { return };
        if let Some(id) = msg["id"].as_u64() {
            if let Some(then) = PENDING.with(|p| p.borrow_mut().remove(&id)) {
                then(match msg.get("error") {
                    Some(e) if !e.is_null() => Err(e.as_str().unwrap_or("error").to_string()),
                    _ => Ok(msg["result"].clone()),
                });
            }
            return;
        }
        match msg["method"].as_str() {
            Some("hello") => s.hello.set(msg["params"].clone()),
            Some("mods") => {
                if !s.listed.get_untracked() {
                    s.listed.set(true);
                }
                if let Ok(mods) = serde_json::from_value::<Vec<ModInfo>>(msg["params"].clone())
                    && s.mods.with_untracked(|m| *m != mods)
                {
                    // Mods deleted elsewhere (Claude Code, the files): their tabs go.
                    let gone: Vec<String> = s.mods.with_untracked(|old| {
                        old.iter().filter(|o| !mods.iter().any(|m| m.name == o.name)).map(|o| o.name.clone()).collect()
                    });
                    s.mods.set(mods);
                    for name in gone {
                        crate::actions::close_mod(s, &name);
                    }
                }
            }
            Some("menus") => {
                if let Ok(menus) = serde_json::from_value::<Vec<crate::menus::MenuInfo>>(msg["params"].clone()) {
                    s.menus.set(menus);
                }
            }
            Some("log") => {
                if let Ok(lines) = serde_json::from_value::<Vec<LogLine>>(msg["params"].clone()) {
                    for l in lines {
                        push_row(s, Row::Log(l));
                    }
                }
            }
            Some("files") => {
                for path in msg["params"].as_array().into_iter().flatten().filter_map(Value::as_str) {
                    reload(s, path);
                }
            }
            _ => {}
        }
    });
    ws.set_onmessage(Some(on_message.as_ref().unchecked_ref()));
    on_message.forget();
    let on_close = Closure::<dyn FnMut()>::new(move || {
        s.connected.set(false);
        SOCKET.with(|sock| sock.borrow_mut().take());
        PENDING.with(|p| {
            for (_, then) in p.borrow_mut().drain() {
                then(Err("the connection to the game closed".into()));
            }
        });
        if opened.get() {
            // The game went away (or the network did): try again.
            set_timeout(move || connect(s), std::time::Duration::from_secs(2));
        } else {
            // Refused, or no game: a removed pairing and a game away look
            // the same here; the server says which.
            check_pairing(move |answer| match answer {
                Some(false) => needed(s),
                _ => {
                    set_timeout(move || connect(s), std::time::Duration::from_secs(3));
                }
            });
        }
    });
    ws.set_onclose(Some(on_close.as_ref().unchecked_ref()));
    on_close.forget();
    SOCKET.with(|sock| *sock.borrow_mut() = Some(ws));
}

/// The page must pair (again): its token is no more (the pairing was
/// removed in the game) or there was none. The editor stops trying it too.
fn needed(s: S) {
    PAIRED.with(|p| p.set(false));
    with_editor(|e| e.pause());
    let token = TOKEN.with(|t| std::mem::take(&mut *t.borrow_mut()));
    if !token.is_empty() {
        forget_token(&token);
        // Another pairing this browser has (the control page's, when the
        // Studio's went) may still be one.
        if let Some(other) = stored_token() {
            TOKEN.with(|t| *t.borrow_mut() = other);
            connect(s);
            return;
        }
    }
    s.pairing.set(Pairing::Needed);
}

/// Whether the page's token is still a pairing of the game's: `Some(true)`,
/// `Some(false)`, or `None` when the game does not answer.
fn check_pairing(then: impl FnOnce(Option<bool>) + 'static) {
    let Some(window) = web_sys::window() else { return };
    let token = TOKEN.with(|t| t.borrow().clone());
    let then = std::rc::Rc::new(RefCell::new(Some(then)));
    let (ok_then, err_then) = (then.clone(), then);
    let on_response = Closure::<dyn FnMut(JsValue)>::new(move |resp: JsValue| {
        let status = js_sys::Reflect::get(&resp, &"status".into()).ok().and_then(|v| v.as_f64()).unwrap_or(0.0) as u32;
        if let Some(f) = ok_then.borrow_mut().take() {
            f(match status {
                200 => Some(true),
                401 => Some(false),
                _ => None,
            });
        }
    });
    let on_error = Closure::<dyn FnMut(JsValue)>::new(move |_: JsValue| {
        if let Some(f) = err_then.borrow_mut().take() {
            f(None);
        }
    });
    let _ = window.fetch_with_str(&format!("/check?token={token}")).then2(&on_response, &on_error);
    on_response.forget();
    on_error.forget();
}

/// A file changed in the game's folder: an open, unchanged tab follows it.
pub(crate) fn reload(s: S, path: &str) {
    let tab = s.tabs.with_untracked(|t| t.iter().find(|t| t.path == path).cloned());
    let Some(tab) = tab else { return };
    if tab.read_only || mod_page(path).is_some() {
        return;
    }
    if tab.dirty {
        // Saving would overwrite what changed: say so.
        s.notice.set(Some(format!("{path} changed in the game's folder; saving keeps your version")));
        return;
    }
    let p = path.to_string();
    request("files.read", json!({ "path": path }), move |r| {
        if let Ok(v) = r {
            with_editor(|e| e.replace(&p, v["text"].as_str().unwrap_or("")));
        }
    });
}

/// Fetches `url` as text (no futures runtime: the promise's callbacks).
pub(crate) fn fetch_text(url: String, then: impl FnOnce(String) + 'static) {
    let Some(window) = web_sys::window() else { return };
    let then = RefCell::new(Some(then));
    let on_text = Closure::<dyn FnMut(JsValue)>::new(move |text: JsValue| {
        if let Some(f) = then.borrow_mut().take() {
            f(text.as_string().unwrap_or_default());
        }
    });
    let on_response = Closure::<dyn FnMut(JsValue)>::new(move |resp: JsValue| {
        if let Ok(text) = js_sys::Reflect::get(&resp, &"text".into()).and_then(|f| f.dyn_into::<js_sys::Function>())
            && let Ok(p) = text.call0(&resp).and_then(|p| p.dyn_into::<js_sys::Promise>())
        {
            let _ = p.then(&on_text);
        }
    });
    let _ = window.fetch_with_str(&url).then(&on_response);
    on_response.forget();
}
