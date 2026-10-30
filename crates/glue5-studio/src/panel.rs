//! The bottom panel: console (log and REPL) and problems.

use leptos::prelude::*;
use wasm_bindgen::prelude::*;

use crate::actions::{eval, open_file_at};
use crate::icons;
use crate::state::*;

#[component]
pub(crate) fn BottomPanel(s: S) -> impl IntoView {
    let errors = move || s.diags.with(|d| d.values().flatten().filter(|d| d.severity == "error").count());
    view! {
        <div class="panel">
            <div class="ptabs">
                <button class=move || if s.panel.get() == Panel::Console { "ptab on" } else { "ptab" } on:click=move |_| s.panel.set(Panel::Console)>"CONSOLE"</button>
                <button class=move || if s.panel.get() == Panel::Problems { "ptab on" } else { "ptab" } on:click=move |_| s.panel.set(Panel::Problems)>
                    "PROBLEMS" <Show when=move || errors() != 0><span class="cnt">{errors}</span></Show>
                </button>
                <div class="grow"></div>
                <Show when=move || s.panel.get() == Panel::Console>
                    <div class="chips">
                        <button class=move || if s.filter.get() == Filter::All { "chip on" } else { "chip" } on:click=move |_| s.filter.set(Filter::All)>"All"</button>
                        <button class=move || if s.filter.get() == Filter::Current { "chip on" } else { "chip" } on:click=move |_| s.filter.set(Filter::Current)>
                            {move || s.active.get().filter(|p| p.contains('/')).map(|p| mod_of(&p).to_string()).unwrap_or_else(|| "This mod".into())}
                        </button>
                        <button class=move || if s.filter.get() == Filter::Errors { "chip on" } else { "chip" } on:click=move |_| s.filter.set(Filter::Errors)>"Errors"</button>
                        <button class="chip" title="Clear" on:click=move |_| s.rows.set(Vec::new())>"Clear"</button>
                    </div>
                </Show>
            </div>
            {move || match s.panel.get() {
                Panel::Console => view! { <Console s=s/> }.into_any(),
                Panel::Problems => view! { <Problems s=s/> }.into_any(),
            }}
        </div>
    }
}

pub(crate) fn clock(ms: u64) -> String {
    let d = js_sys::Date::new(&JsValue::from_f64(ms as f64));
    format!("{:02}:{:02}:{:02}", d.get_hours(), d.get_minutes(), d.get_seconds())
}

/// A message with its `file.luau:line` as a link.
pub(crate) fn linked(s: S, text: String) -> impl IntoView {
    match file_location(&text) {
        Some((path, line, a, b)) => {
            let (before, link, after) = (text[..a].to_string(), text[a..b].to_string(), text[b..].to_string());
            view! {
                {before}
                <a class="link" href="#" on:click=move |ev| { ev.prevent_default(); open_file_at(s, path.clone(), Some((line, 1))); }>{link}</a>
                {after}
            }
            .into_any()
        }
        None => text.into_any(),
    }
}

#[component]
pub(crate) fn Console(s: S) -> impl IntoView {
    let log_ref = NodeRef::<leptos::html::Div>::new();
    // Follow the end, unless the person scrolled up to read; also while
    // hidden (a phone's other screens), so it opens on the latest lines.
    let follow = StoredValue::new(true);
    Effect::new(move |_| {
        s.rows.track();
        s.filter.track();
        s.screen.track();
        if let Some(el) = log_ref.get()
            && follow.get_value()
        {
            set_timeout(move || el.set_scroll_top(el.scroll_height()), std::time::Duration::from_millis(0));
        }
    });
    let scrolled = move |_| {
        if let Some(el) = log_ref.get_untracked()
            && el.client_height() > 0
        {
            follow.set_value(el.scroll_height() - el.scroll_top() - el.client_height() < 48);
        }
    };
    let shown = move || {
        let filter = s.filter.get();
        let current = s.active.get().map(|p| mod_of(&p).to_string());
        s.rows.get().into_iter().filter(move |(_, r)| match (filter, r) {
            (Filter::All, _) => true,
            (Filter::Errors, Row::Log(l)) => l.level == "error",
            (Filter::Errors, Row::Failure(_)) => true,
            (Filter::Current, Row::Log(l)) => current.as_deref() == Some(l.module.as_str()),
            _ => false,
        }).collect::<Vec<_>>()
    };
    // The input is a small editor (highlighting, completions), made once
    // and moved here whenever the console shows.
    let input_ref = NodeRef::<leptos::html::Div>::new();
    Effect::new(move |_| {
        let Some(el) = input_ref.get() else { return };
        // One callback for the page's life (the console mounts again and again).
        REPL_RUN.with(|r| {
            let mut r = r.borrow_mut();
            let on_run = r.get_or_insert_with(|| Closure::<dyn FnMut(String)>::new(move |code: String| eval(s, code)));
            with_editor(|e| e.repl(&el, on_run));
        });
    });
    view! {
        <div class="log" node_ref=log_ref on:scroll=scrolled>
            <For each=shown key=|(n, _)| *n let((_, row))>
                {match row {
                    Row::Log(l) => view! {
                        <div class=if l.level == "error" { "row err" } else { "row" }>
                            <span class="time">{clock(l.time)}</span>
                            <span class="src">{l.module.clone()}</span>
                            <span class="msg">{linked(s, l.text.clone())}</span>
                        </div>
                    }.into_any(),
                    Row::Input(code) => view! { <div class="row repl"><span class="time"></span><span class="src">"›"</span><span class="msg">{code}</span></div> }.into_any(),
                    Row::Value(v) => view! { <div class="row result"><span class="time"></span><span class="src"></span><span class="msg">{v}</span></div> }.into_any(),
                    Row::Failure(e) => view! { <div class="row err"><span class="time"></span><span class="src"></span><span class="msg">{e}</span></div> }.into_any(),
                }}
            </For>
        </div>
        // A div, not a label: a label's click goes to its first control
        // (the Run button), taking the focus from the input.
        <div class="prompt" on:mousedown=move |ev| {
            // The box's margins focus the input too.
            if ev.target() == ev.current_target() {
                ev.prevent_default();
                with_editor(|e| e.focus_repl());
            }
        }>
            <span class="gt">"›"</span>
            <div class="repl-host" node_ref=input_ref></div>
            <button class="btn primary small" on:click=move |_| with_editor(|e| e.run_repl())>"Run"</button>
        </div>
    }
}

#[component]
pub(crate) fn Problems(s: S) -> impl IntoView {
    let list = move || {
        let mut all: Vec<(String, Diag)> = s.diags.get().into_iter().flat_map(|(p, d)| d.into_iter().map(move |d| (p.clone(), d))).collect();
        all.sort_by(|a, b| (a.1.severity != "error", &a.0, a.1.line).cmp(&(b.1.severity != "error", &b.0, b.1.line)));
        all
    };
    view! {
        <div class="log problems">
            {move || {
                let l = list();
                if l.is_empty() {
                    return view! { <p class="empty">"No problems in the open files."</p> }.into_any();
                }
                l.into_iter().map(|(path, d)| {
                    let (p, line, col) = (path.clone(), d.line, d.column);
                    view! {
                        <div class=if d.severity == "error" { "problem err" } else { "problem warn" }
                            on:click=move |_| open_file_at(s, p.clone(), Some((line, col)))>
                            <span class="ico small" inner_html=if d.severity == "error" { icons::ERROR } else { icons::WARNING }></span>
                            <span class="msg">{d.message.clone()}</span>
                            <span class="where">{format!("{path}:{}:{}", d.line, d.column)}</span>
                        </div>
                    }
                }).collect_view().into_any()
            }}
        </div>
    }
}
