//! Pairing this browser with the game: the code the game shows, once, for a
//! token of the page's own (the game's `POST /pair`), kept in this
//! browser's storage and sent from then on. A pairing lasts until it is
//! removed in the game (or unused for 90 days): no code at each start.
//!
//! The code comes typed, or in the address (`?pair=`, the game's QR code
//! and its Open buttons), which is cleared at once.

use leptos::prelude::*;
use wasm_bindgen::prelude::*;

use crate::net::connect;
use crate::state::*;

/// Pairs with `code` for the page's scope, then connects; a wrong code says
/// so on the pairing screen.
pub(crate) fn pair(s: S, code: &str) {
    let code: String = code.chars().filter(|c| c.is_ascii_alphanumeric()).collect::<String>().to_ascii_uppercase();
    if code.is_empty() {
        return;
    }
    s.pairing.set(Pairing::Connecting);
    let scope = mode().scope();
    let done = Closure::<dyn FnMut(JsValue)>::new(move |answer: JsValue| {
        let field = |k: &str| js_sys::Reflect::get(&answer, &k.into()).ok().and_then(|v| v.as_string()).unwrap_or_default();
        let (token, granted) = (field("token"), field("scope"));
        store_token(&granted, &token);
        TOKEN.with(|t| *t.borrow_mut() = token);
        s.pair_failed.set(false);
        connect(s);
    });
    let failed = Closure::<dyn FnMut(JsValue)>::new(move |status: JsValue| {
        // 401: a wrong code; else no game to ask.
        s.pair_failed.set(status.as_f64() == Some(401.0));
        if status.as_f64() != Some(401.0) {
            s.notice.set(Some("The game did not answer: is it still running?".into()));
        }
        s.pairing.set(Pairing::Needed);
    });
    let _ = crate::editor::pair_with(&code, scope).then2(&done, &failed);
    done.forget();
    failed.forget();
}

/// The page's start: a code in the address pairs; else the token this
/// browser keeps; else the pairing screen.
pub(crate) fn start(s: S) {
    let window = web_sys::window();
    let params = window
        .as_ref()
        .and_then(|w| w.location().search().ok())
        .and_then(|q| web_sys::UrlSearchParams::new_with_str(&q).ok());
    // `token` is the addresses of before pairing (the code was the token).
    let code = params.as_ref().and_then(|p| p.get("pair").or_else(|| p.get("token")));
    if let (Some(p), Some(w)) = (&params, &window)
        && code.is_some()
    {
        // Out of the address bar (and screenshots and history of it).
        p.delete("pair");
        p.delete("token");
        let query = p.to_string().as_string().unwrap_or_default();
        let path = w.location().pathname().unwrap_or_else(|_| "/".into());
        let url = if query.is_empty() { path } else { format!("{path}?{query}") };
        if let Ok(h) = w.history() {
            let _ = h.replace_state_with_url(&JsValue::NULL, "", Some(&url));
        }
    }
    let token = stored_token();
    match (code, token) {
        // Already paired: the code is not needed (a QR scanned again).
        (_, Some(token)) => {
            TOKEN.with(|t| *t.borrow_mut() = token);
            connect(s);
        }
        (Some(code), None) => pair(s, &code),
        (None, None) => match legacy_code() {
            Some(code) => pair(s, &code),
            None => s.pairing.set(Pairing::Needed),
        },
    }
}

/// The code this browser kept before pages paired (it was the token):
/// used once to pair, then forgotten.
fn legacy_code() -> Option<String> {
    let st = storage()?;
    let code = st.get_item("glue5-token").ok().flatten()?;
    let _ = st.remove_item("glue5-token");
    Some(code)
}

#[component]
pub(crate) fn Pair(s: S) -> impl IntoView {
    let code = RwSignal::new(String::new());
    let submit = move || pair(s, &code.get_untracked());
    let (title, what) = match mode() {
        Mode::Studio => ("STUDIO", "Pair this browser with the game: enter the code shown in the game, under Mods › Studio."),
        Mode::Control => ("CONTROL", "Pair this device to use the mods and their menus: enter the code shown in the game, under Mods › Studio, or scan its QR code."),
    };
    view! {
        <div class="pair">
            <div class="pair-card">
                <div class="brand big"><img src="/logo.png" alt=""/><b>"GLUE 5"</b><span>{title}</span></div>
                <p>{what}</p>
                <Show when=move || s.pair_failed.get()>
                    <p class="pair-error">"That code did not work: check it in the game (it changes when someone presses New)."</p>
                </Show>
                <input class="code-input" placeholder="ABCD-1234" autocomplete="off" autocapitalize="characters" spellcheck="false"
                    inputmode="text" aria-label="Pairing code"
                    prop:value=move || code.get()
                    on:input=move |ev| code.set(event_target_value(&ev))
                    on:keydown=move |ev| if ev.key() == "Enter" { submit() } />
                <button class="btn primary wide" on:click=move |_| submit()>"Pair"</button>
                <p class="pair-note">"Once paired, this browser stays paired: the game lists it under Mods › Studio, where it can be removed."</p>
            </div>
        </div>
    }
}
