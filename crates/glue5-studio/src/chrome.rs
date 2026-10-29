//! Around the work: title bar, activity bar, status bar, the phone's navigation.

use leptos::prelude::*;
use serde_json::{Value, json};

use crate::actions::insert_code;
use crate::icons;
use crate::net::request;
use crate::state::*;

#[component]
pub(crate) fn TitleBar(s: S) -> impl IntoView {
    let input = NodeRef::<leptos::html::Input>::new();
    // The highlighted result (keys move it, Enter inserts it).
    let selected = RwSignal::new(0usize);
    let search = move |q: String| {
        s.query.set(q.clone());
        selected.set(0);
        if q.trim().len() < 2 {
            s.found.set(Vec::new());
            return;
        }
        let asked = q.clone();
        request("natives.search", json!({ "query": q, "limit": 12 }), move |r| {
            // Only for the query still typed (one shortened or cleared since
            // would show what it no longer asks for).
            if let Ok(Value::Array(list)) = r
                && s.query.get_untracked() == asked
            {
                s.found.set(list);
            }
        });
    };
    let close = move || {
        s.query.set(String::new());
        s.found.set(Vec::new());
        if let Some(el) = input.get_untracked() {
            let _ = el.blur();
        }
    };
    // Into the open file at the cursor, as `namespace.name(`.
    let insert = move |n: &Value| {
        let call = format!("{}(", n["name"].as_str().unwrap_or(""));
        close();
        insert_code(s, &call);
    };
    // Ctrl/Cmd+K from anywhere.
    let shortcut = window_event_listener(leptos::ev::keydown, move |ev| {
        if (ev.meta_key() || ev.ctrl_key()) && ev.key().eq_ignore_ascii_case("k") {
            ev.prevent_default();
            if let Some(el) = input.get_untracked() {
                let _ = el.focus();
                el.select();
            }
        }
    });
    on_cleanup(move || shortcut.remove());
    view! {
        <header class="titlebar">
            <div class="brand">
                <img src="/logo.png" alt=""/><b>"GLUE 5"</b><span>"STUDIO"</span>
            </div>
            <div class="palette-wrap">
                <label class="palette">
                    <span class="ico" inner_html=icons::SEARCH></span>
                    <input node_ref=input placeholder="Search natives" spellcheck="false" autocomplete="off"
                        prop:value=move || s.query.get()
                        on:focus=move |_| s.palette.set(true)
                        on:blur=move |_| set_timeout(move || s.palette.set(false), std::time::Duration::from_millis(200))
                        on:input=move |ev| search(event_target_value(&ev))
                        on:keydown=move |ev| {
                            let count = s.found.with_untracked(Vec::len);
                            match ev.key().as_str() {
                                "ArrowDown" if count > 0 => { ev.prevent_default(); selected.update(|i| *i = (*i + 1) % count); }
                                "ArrowUp" if count > 0 => { ev.prevent_default(); selected.update(|i| *i = (*i + count - 1) % count); }
                                "Enter" => {
                                    if let Some(n) = s.found.with_untracked(|f| f.get(selected.get_untracked()).cloned()) {
                                        ev.prevent_default();
                                        insert(&n);
                                    }
                                }
                                "Escape" => close(),
                                _ => {}
                            }
                        } />
                    <kbd>"⌘K"</kbd>
                </label>
                <Show when=move || s.palette.get() && s.found.with(|f| !f.is_empty())>
                    <div class="palette-results">
                        {move || s.found.get().into_iter().enumerate().map(|(i, n)| {
                            let n2 = n.clone();
                            view! {
                                <button class=move || if selected.get() == i { "presult on" } else { "presult" }
                                    on:mouseenter=move |_| selected.set(i)
                                    on:mousedown=move |ev| { ev.prevent_default(); insert(&n2); }>
                                    <span class="sig">{n["signature"].as_str().unwrap_or("").to_string()}</span>
                                    <small>{n["native"].as_str().unwrap_or("").to_string()}</small>
                                </button>
                            }
                        }).collect_view()}
                    </div>
                </Show>
            </div>
            <div class="tb-right">
                <a class="btn ghost small" href="/control" target="_blank" rel="noopener" title="The mods and their menus, on a page of their own (for a phone too)">
                    <span class="ico" inner_html=icons::SLIDERS></span>"Control page"
                </a>
                <div class=move || if s.connected.get() { "device" } else { "device off" }
                    title=move || s.hello.with(|h| format!("The game runs on {}", h["device"].as_str().unwrap_or("this device")))>
                    <span class="dot"></span>
                    <span class="name">{move || s.hello.with(|h| h["device"].as_str().unwrap_or("Game").to_string())}</span>
                </div>
            </div>
        </header>
    }
}

#[component]
pub(crate) fn Activity(s: S) -> impl IntoView {
    // What the sidebar shows (the console and problems are the panel's tabs).
    let item = move |side: Side, title: &'static str, icon: &'static str| {
        view! {
            <button class=move || if s.side.get() == side { "act on" } else { "act" } title=title
                on:click=move |_| {
                    if s.side.get_untracked() == side { s.drawer.update(|d| *d = !*d) } else { s.drawer.set(true) }
                    s.side.set(side);
                }>
                <span class="ico" inner_html=icon></span>
            </button>
        }
    };
    view! {
        <nav class="activity">
            {item(Side::Mods, "Mods", icons::FOLDER)}
            {item(Side::Natives, "Natives", icons::BOOK)}
        </nav>
    }
}

#[component]
pub(crate) fn StatusBar(s: S) -> impl IntoView {
    let errors = move || s.diags.with(|d| d.values().flatten().filter(|d| d.severity == "error").count());
    view! {
        <footer class="status">
            <span class="it"><span class=move || if s.connected.get() { "dot" } else { "dot off" }></span>
                <b>{move || if s.connected.get() { "Connected" } else { "Offline" }}</b></span>
            <span class="it hide-t">{move || s.hello.with(|h| format!("build {}", h["build"].as_str().unwrap_or("")))}</span>
            <Show when=move || errors() != 0>
                <span class="it errn" on:click=move |_| s.panel.set(Panel::Problems)>{move || format!("● {} error{}", errors(), if errors() == 1 { "" } else { "s" })}</span>
            </Show>
            <Show when=move || s.hello.with(|h| h["tainted"] == true)>
                <span class="it errn">"A native failed: restart the game soon"</span>
            </Show>
            <span class="grow"></span>
            // The editor's, while it shows a file.
            {move || s.active.get().filter(|p| mod_page(p).is_none()).map(|p| {
                let language = match p.rsplit_once('.').map(|(_, e)| e) {
                    Some("luau") => "Luau",
                    Some("json") => "JSON",
                    _ => "Plain Text",
                };
                view! {
                    <span class="it">{move || { let (l, c) = s.cursor.get(); format!("Ln {l}, Col {c}") }}</span>
                    <span class="it hide-t">{language}</span>
                }
            })}
            <span class="it">{move || {
                let mods = s.mods.get();
                let running: Vec<_> = mods.iter().filter(|m| m.state == "running").collect();
                format!("{} running · {:.1} ms/frame", running.len(), running.iter().map(|m| m.cpu_ms).fold(0.0, |a, b| a + b))
            }}</span>
        </footer>
    }
}

#[component]
pub(crate) fn PhoneNav(s: S) -> impl IntoView {
    let errors = move || s.diags.with(|d| d.values().flatten().filter(|d| d.severity == "error").count());
    let item = move |screen: Screen, label: &'static str, icon: &'static str| {
        view! {
            <button class=move || if s.screen.get() == screen { "on" } else { "" }
                on:click=move |_| {
                    s.screen.set(screen);
                    match screen {
                        Screen::Console => s.panel.set(Panel::Console),
                        Screen::Problems => s.panel.set(Panel::Problems),
                        _ => {}
                    }
                }>
                <span class="ico" inner_html=icon></span>{label}
                {(screen == Screen::Problems).then(|| view! { <Show when=move || errors() != 0><span class="badge">{errors}</span></Show> })}
            </button>
        }
    };
    view! {
        <nav class="phone-nav">
            {item(Screen::Mods, "Mods", icons::FOLDER)}
            {item(Screen::Editor, "Editor", icons::CODE)}
            {item(Screen::Console, "Console", icons::TERMINAL)}
            {item(Screen::Problems, "Problems", icons::WARNING)}
            <a class="phone-control" href="/control" target="_blank" rel="noopener">
                <span class="ico" inner_html=icons::SLIDERS></span>"Control"
            </a>
        </nav>
    }
}
