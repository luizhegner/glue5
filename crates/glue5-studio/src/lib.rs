//! GLUE 5 Studio: the mods' IDE, served by the game. Leptos for the shell,
//! CodeMirror for the code (editor.rs), one WebSocket to the game for
//! files, mods, logs and eval (`/studio`) and one for the type checker
//! (`/lsp`, from the editor).
//!
//! The same page is the control page at `/control` (control.rs): the mods
//! and their menus, for a phone or another screen, without the editor.

mod actions;
mod agents;
mod chrome;
mod control;
mod editor;
mod highlight;
mod icons;
mod menus;
mod modpage;
mod net;
mod pair;
mod panel;
mod sidebar;
mod state;
mod workspace;

use std::collections::HashMap;

use leptos::prelude::*;
use serde_json::Value;
use wasm_bindgen::prelude::*;

use chrome::{Activity, PhoneNav, StatusBar, TitleBar};
use editor::StudioEditor;
use pair::Pair;
use actions::save;
use panel::BottomPanel;
use sidebar::{ModsView, NativesView};
use state::*;
use workspace::{Tabs, Toolbar, Welcome};

#[wasm_bindgen(start)]
pub fn main() {
    console_error_panic_hook::set_once();
    leptos::mount::mount_to_body(App);
}

#[component]
fn App() -> impl IntoView {
    let s = S {
        pairing: RwSignal::new(Pairing::Connecting),
        connected: RwSignal::new(false),
        hello: RwSignal::new(Value::Null),
        mods: RwSignal::new(Vec::new()),
        listed: RwSignal::new(false),
        menus: RwSignal::new(Vec::new()),
        menu_stacks: RwSignal::new(HashMap::new()),
        phone: RwSignal::new(false),
        rows: RwSignal::new(Vec::new()),
        tabs: RwSignal::new(Vec::new()),
        active: RwSignal::new(None),
        expanded: RwSignal::new(Vec::new()),
        folded: RwSignal::new(Vec::new()),
        dragging: RwSignal::new(None),
        drop_at: RwSignal::new(None),
        side: RwSignal::new(Side::Mods),
        panel: RwSignal::new(Panel::Console),
        screen: RwSignal::new(Screen::Mods),
        drawer: RwSignal::new(false),
        diags: RwSignal::new(HashMap::new()),
        cursor: RwSignal::new((1, 1)),
        filter: RwSignal::new(Filter::All),
        query: RwSignal::new(String::new()),
        found: RwSignal::new(Vec::new()),
        palette: RwSignal::new(false),
        notice: RwSignal::new(None),
        pair_failed: RwSignal::new(false),
        context: RwSignal::new(None),
    };
    let path = web_sys::window().and_then(|w| w.location().pathname().ok()).unwrap_or_default();
    MODE.with(|m| m.set(if path.starts_with("/control") { Mode::Control } else { Mode::Studio }));
    pair::start(s);
    watch_layout(s);
    // A notice shows 5 s from when it came (an older one's timer does not
    // take a newer one).
    let shown_at = StoredValue::new(0u32);
    Effect::new(move |_| {
        if s.notice.get().is_some() {
            let mine = shown_at.get_value().wrapping_add(1);
            shown_at.set_value(mine);
            set_timeout(
                move || {
                    if shown_at.get_value() == mine {
                        s.notice.set(None);
                    }
                },
                std::time::Duration::from_secs(5),
            );
        }
    });

    view! {
        <div class="backdrop"></div>
        {move || match (s.pairing.get(), mode()) {
            (Pairing::Connecting, _) => view! { <div class="splash"><img src="/logo.png" alt=""/></div> }.into_any(),
            (Pairing::Needed, _) => view! { <Pair s=s/> }.into_any(),
            (Pairing::Paired, Mode::Studio) => view! { <Shell s=s/> }.into_any(),
            (Pairing::Paired, Mode::Control) => view! { <control::Control s=s/> }.into_any(),
        }}
    }
}

/// Follows whether the page has the phone's layout (style.css's breakpoint),
/// and keeps the on-screen keyboard's height in `--kb` (js/viewport.js).
fn watch_layout(s: S) {
    let Some(list) = web_sys::window().and_then(|w| w.match_media("(max-width: 700px)").ok().flatten()) else { return };
    s.phone.set(list.matches());
    let on_change = Closure::<dyn FnMut(web_sys::MediaQueryListEvent)>::new(move |e: web_sys::MediaQueryListEvent| s.phone.set(e.matches()));
    list.set_onchange(Some(on_change.as_ref().unchecked_ref()));
    on_change.forget();
    editor::watch_keyboard();
}

#[component]
fn Shell(s: S) -> impl IntoView {
    let host_ref = NodeRef::<leptos::html::Div>::new();
    // The editor's module, loaded once (the control page never needs it).
    let loaded = RwSignal::new(EDITOR.with(|e| e.borrow().is_some()));
    if !loaded.get_untracked() {
        let done = Closure::<dyn FnMut(JsValue)>::new(move |_: JsValue| loaded.set(true));
        let failed = Closure::<dyn FnMut(JsValue)>::new(move |_: JsValue| s.notice.set(Some("The editor did not load: reload the page".into())));
        let _ = editor::load_editor().then2(&done, &failed);
        done.forget();
        failed.forget();
    }
    // The editor, made once its module and its element are there; after
    // pairing again the page is made again and the same editor moves into it.
    Effect::new(move |_| {
        let Some(el) = host_ref.get() else { return };
        if !loaded.get() {
            return;
        }
        let token = TOKEN.with(|t| t.borrow().clone());
        let lsp = format!("ws://{}/lsp?token={token}", host());
        if EDITOR.with(|e| e.borrow().is_some()) {
            with_editor(|e| e.attach(&el, &lsp));
            return;
        }
        let editor = StudioEditor::new(&el, &lsp);
        let on_change = Closure::<dyn FnMut(String, bool)>::new(move |path: String, dirty: bool| {
            // Only when it changes: not the whole tab strip at each key.
            if s.tabs.with_untracked(|t| t.iter().any(|t| t.path == path && t.dirty == dirty)) {
                return;
            }
            s.tabs.update(|t| {
                if let Some(t) = t.iter_mut().find(|t| t.path == path) {
                    t.dirty = dirty;
                }
            });
        });
        editor.on_change(&on_change);
        on_change.forget();
        let on_save = Closure::<dyn FnMut(String)>::new(move |path: String| save(s, path));
        editor.on_save(&on_save);
        on_save.forget();
        let on_diags = Closure::<dyn FnMut(String, String)>::new(move |path: String, json: String| {
            let list: Vec<Diag> = serde_json::from_str(&json).unwrap_or_default();
            s.diags.update(|d| {
                d.insert(path, list);
            });
        });
        editor.on_diagnostics(&on_diags);
        on_diags.forget();
        let on_cursor = Closure::<dyn FnMut(u32, u32)>::new(move |l, c| s.cursor.set((l, c)));
        editor.on_cursor(&on_cursor);
        on_cursor.forget();
        EDITOR.with(|e| *e.borrow_mut() = Some(editor));
        // Files opened before the editor was there: read again, into it.
        let early: Vec<String> = s.tabs.with_untracked(|t| t.iter().filter(|t| mod_page(&t.path).is_none()).map(|t| t.path.clone()).collect());
        s.tabs.update(|t| t.retain(|t| mod_page(&t.path).is_some()));
        for path in early {
            actions::open_file(s, path);
        }
    });
    let screen_class = move || {
        let screen = match s.screen.get() {
            Screen::Mods => "screen-mods",
            Screen::Editor => "screen-editor",
            Screen::Console => "screen-console",
            Screen::Problems => "screen-problems",
        };
        format!("app {screen}{}", if s.drawer.get() { " drawer" } else { "" })
    };
    view! {
        <div class=screen_class>
            <TitleBar s=s/>
            <div class="main">
                <Activity s=s/>
                <aside class="sidebar">
                    {move || match s.side.get() {
                        Side::Mods => view! { <ModsView s=s/> }.into_any(),
                        Side::Natives => view! { <NativesView s=s/> }.into_any(),
                    }}
                </aside>
                <section class=move || if s.tabs.with(|t| t.is_empty()) { "work idle" } else { "work" }>
                    <Tabs s=s/>
                    <Toolbar s=s/>
                    <div class=move || if s.active.get().is_some_and(|p| mod_page(&p).is_some()) { "code page" } else { "code" }>
                        <div class="editor-host" node_ref=host_ref></div>
                        <Show when=move || s.tabs.with(|t| t.is_empty())>
                            <Welcome s=s/>
                        </Show>
                        // A mod's page, over the editor.
                        {move || s.active.get().and_then(|p| mod_page(&p).map(str::to_string)).map(|name| view! { <modpage::ModPage s=s name=name/> })}
                    </div>
                    <BottomPanel s=s/>
                </section>
            </div>
            <StatusBar s=s/>
            <PhoneNav s=s/>
            <sidebar::ContextMenu s=s/>
            {move || s.notice.get().map(|n| view! { <div class="toast" title="Close" on:click=move |_| s.notice.set(None)>{n}</div> })}
        </div>
    }
}
