//! The sidebar: the mods and their files, the natives' search.

use leptos::prelude::*;
use wasm_bindgen::JsCast;
use serde_json::{Value, json};

use crate::actions::{delete_file, import, insert_code, move_path, new_file, new_folder, new_mod, open_file, open_mod_page, rename_file};
use crate::icons;
use crate::net::request;
use crate::state::*;

pub(crate) fn state_class(m: &ModInfo) -> &'static str {
    match (m.state.as_str(), m.waiting) {
        ("running", false) => "state run",
        ("running", true) => "state wait",
        ("error", _) => "state err",
        _ => "state stop",
    }
}

pub(crate) fn meta(m: &ModInfo) -> String {
    match m.state.as_str() {
        "running" => format!("{:.1} ms", m.cpu_ms),
        "error" => "error".into(),
        _ if m.files.is_empty() => String::new(),
        _ => "stopped".into(),
    }
}

#[component]
pub(crate) fn ModsView(s: S) -> impl IntoView {
    let new_mod = move |_| new_mod(s);
    let running = move || s.mods.get().into_iter().filter(|m| m.state == "running").collect::<Vec<_>>();
    view! {
        <div class="side-head">
            <h2>"MODS"</h2>
            <div class="tools">
                <button class="icon-btn" title="Import a mod (.zip)" on:click=move |_| import(s)><span class="ico" inner_html=icons::IMPORT></span></button>
                <button class="icon-btn" title="New mod" on:click=new_mod><span class="ico" inner_html=icons::PLUS></span></button>
                <button class="icon-btn" title="Stop all" on:click=move |_| {
                    for m in s.mods.get_untracked() { if m.state == "running" { request("mods.stop", json!({"name": m.name}), |_| {}); } }
                }><span class="ico" inner_html=icons::STOP></span></button>
            </div>
        </div>
        <div class="section">"In the game"<span class="count">{move || match s.mods.with(Vec::len) {
                _ if !s.listed.get() => String::new(),
                1 => "1 mod".to_string(),
                n => format!("{n} mods"),
            }}</span></div>
        <div class="tree">
            // Keyed by what a row shows but its time: rows stay (with their
            // hover and clicks) while the figures change.
            <For each=move || s.mods.get() key=|m| format!("{}|{}|{}|{}|{}|{}", m.name, m.state, m.waiting, m.files.join(","), m.title, m.description) let(m)>{{
                let name = m.name.clone();
                let figure = { let n = name.clone(); move || s.mods.with(|v| v.iter().find(|x| x.name == n).map(meta).unwrap_or_default()) };
                let open = { let n = name.clone(); move || s.expanded.with(|e| e.contains(&n)) };
                // The arrow opens or closes its files; the row opens its page (and its files).
                let toggle = { let n = name.clone(); move |ev: leptos::ev::MouseEvent| {
                    ev.stop_propagation();
                    s.expanded.update(|e| if let Some(i) = e.iter().position(|x| *x == n) { e.remove(i); } else { e.push(n.clone()); });
                }};
                let page = { let n = name.clone(); move |_| {
                    s.expanded.update(|e| if !e.contains(&n) { e.push(n.clone()); });
                    open_mod_page(s, &n);
                }};
                let selected = { let p = format!("{MOD_PAGE}{name}"); move || s.active.get().as_deref() == Some(p.as_str()) };
                let run = { let n = name.clone(); let running = m.state == "running"; move |ev: leptos::ev::MouseEvent| {
                    ev.stop_propagation();
                    request(if running { "mods.stop" } else { "mods.start" }, json!({ "name": n }), move |r| if let Err(e) = r { s.notice.set(Some(e)) });
                }};
                let files = m.files.clone();
                let open2 = open.clone();
                let top = name.clone();
                view! {
                    <div class=move || match (selected(), dropping(s, &top)) { (true, _) => "mod sel", (_, true) => "mod drop", _ => "mod" } role="button" tabindex="0" on:click=page
                        on:contextmenu={ let n = name.clone(); move |ev: leptos::ev::MouseEvent| {
                            ev.prevent_default();
                            s.context.set(Some((n.clone(), ev.client_x(), ev.client_y())));
                        } }
                        on:dragover=drag_over(s, name.clone()) on:dragleave=drag_leave(s, name.clone()) on:drop=dropped(s, name.clone())
                        on:keydown={ let n = name.clone(); move |ev| if ev.key() == "Enter" { open_mod_page(s, &n) } }
                        title=m.error.clone().unwrap_or_else(|| m.description.clone())>
                        <span class="chev" on:click=toggle>{move || if open() { "▾" } else { "▸" }}</span>
                        <span class=state_class(&m)></span>
                        <span class="name">{m.title.clone()}{(m.title != m.name).then(|| view! { <small>{m.name.clone()}</small> })}</span>
                        <span class=if m.state == "error" { "meta err" } else { "meta" }>{figure}</span>
                        <button class="runbtn" title=if m.state == "running" { "Stop" } else { "Start" } on:click=run>
                            <span class="ico" inner_html=if m.state == "running" { icons::STOP } else { icons::PLAY }></span>
                        </button>
                    </div>
                    <Show when=open2>
                        {file_tree(s, &name, &files, "", 0)}
                        <div class="file new" on:click={ let n = name.clone(); move |_| new_file(s, &n) }>
                            <span class="ico small" inner_html=icons::PLUS></span><span>"New file"</span>
                        </div>
                        <div class="file new" on:click={ let n = name.clone(); move |_| new_folder(s, &n) }>
                            <span class="ico small" inner_html=icons::FOLDER></span><span>"New folder"</span>
                        </div>
                    </Show>
                }
            }}</For>
            <Show when=move || s.listed.get() && s.mods.with(Vec::is_empty)>
                <p class="empty">"No mods yet. Make one with +, or have an AI agent make one (the welcome page connects it)."</p>
            </Show>
        </div>
        <Show when=move || !running().is_empty()>
            <div class="section">"Running"<span class="count">{move || {
                let r = running();
                format!("{} · {:.1} ms/frame", r.len(), r.iter().map(|m| m.cpu_ms).fold(0.0, |a, b| a + b))
            }}</span></div>
            <div class="running">
                <For each=running key=|m| format!("{}|{}", m.name, m.waiting) let(m)>{{
                    let n1 = m.name.clone();
                    let n2 = m.name.clone();
                    let n3 = m.name.clone();
                    let cpu = Memo::new(move |_| s.mods.with(|v| v.iter().find(|x| x.name == n3).map_or(0.0, |x| x.cpu_ms)));
                    view! {
                        <div class="job">
                            <div class="job-top">
                                <span class=state_class(&m)></span>
                                <span class="name">{m.name.clone()}</span>
                                <span class="pill ok">{if m.waiting { "Waiting" } else { "Running" }}</span>
                            </div>
                            <div class="job-sub">
                                <span>{move || format!("{:.1} ms", cpu.get())}</span>
                                <div class="meter"><i style=move || format!("width:{}%", (cpu.get() / 2.0 * 100.0).min(100.0))></i></div>
                                <button class="icon-btn" title="Restart" on:click=move |_| request("mods.restart", json!({"name": n1}), |_| {})><span class="ico" inner_html=icons::RESTART></span></button>
                                <button class="icon-btn" title="Stop" on:click=move |_| request("mods.stop", json!({"name": n2}), |_| {})><span class="ico" inner_html=icons::STOP></span></button>
                            </div>
                        </div>
                    }
                }}</For>
            </div>
        </Show>
        <div class="side-foot" on:click=move |_| open_file(s, "natives.d.luau".into())>
            <span class="ico small" inner_html=icons::BOOK></span>
            <span>{move || s.hello.with(|h| format!("{} natives · {} namespaces", h["natives"], h["namespaces"]))}</span>
        </div>
    }
}

/// The control page at mod `name`'s menus, in a tab of its own.
pub(crate) fn open_control(name: &str) {
    if let Some(w) = web_sys::window() {
        let _ = w.open_with_url_and_target(&format!("/control?mod={}", js_sys::encode_uri_component(name)), "_blank");
    }
}

/// A mod's context menu (a right click on its row): what can be done to it.
/// A press anywhere else, Escape or a scroll closes it.
#[component]
pub(crate) fn ContextMenu(s: S) -> impl IntoView {
    let pressed = window_event_listener(leptos::ev::pointerdown, move |ev| {
        let inside = ev
            .target()
            .and_then(|t| t.dyn_into::<web_sys::Element>().ok())
            .and_then(|e| e.closest(".ctx").ok().flatten())
            .is_some();
        if !inside {
            s.context.set(None);
        }
    });
    let keys = window_event_listener(leptos::ev::keydown, move |ev| {
        if ev.key() == "Escape" {
            s.context.set(None);
        }
    });
    let wheel = window_event_listener(leptos::ev::wheel, move |_| s.context.set(None));
    on_cleanup(move || {
        pressed.remove();
        keys.remove();
        wheel.remove();
    });
    move || {
        s.context.get().map(|(name, x, y)| {
            let running = s.mods.with(|m| m.iter().any(|m| m.name == name && m.state == "running"));
            // Inside the window.
            let (w, h) = web_sys::window()
                .map(|w| (w.inner_width().ok().and_then(|v| v.as_f64()).unwrap_or(1e4), w.inner_height().ok().and_then(|v| v.as_f64()).unwrap_or(1e4)))
                .unwrap_or((1e4, 1e4));
            let (left, top) = ((x as f64).min(w - 230.0).max(4.0), (y as f64).min(h - 300.0).max(4.0));
            let item = move |label: &'static str, f: Box<dyn Fn()>| {
                view! {
                    <button class="ctx-item" role="menuitem" on:click=move |_| { s.context.set(None); f(); }>{label}</button>
                }
            };
            let n = name.clone();
            let start = {
                let n = n.clone();
                Box::new(move || request(if running { "mods.stop" } else { "mods.start" }, json!({ "name": n }), move |r| if let Err(e) = r { s.notice.set(Some(e)) }))
            };
            let token = TOKEN.with(|t| t.borrow().clone());
            let export = format!("/export?mod={n}&token={token}");
            view! {
                <div class="ctx" role="menu" aria-label=format!("{n}") style=format!("left:{left}px;top:{top}px")>
                    {item("Open control page", { let n = n.clone(); Box::new(move || open_control(&n)) })}
                    {item("Mod page", { let n = n.clone(); Box::new(move || open_mod_page(s, &n)) })}
                    <div class="ctx-sep"></div>
                    {item(if running { "Stop" } else { "Start" }, start)}
                    {running.then(|| item("Restart", { let n = n.clone(); Box::new(move || request("mods.restart", json!({ "name": n }), move |r| if let Err(e) = r { s.notice.set(Some(e)) })) }))}
                    <div class="ctx-sep"></div>
                    {item("New file…", { let n = n.clone(); Box::new(move || new_file(s, &n)) })}
                    {item("New folder…", { let n = n.clone(); Box::new(move || new_folder(s, &n)) })}
                    {item("Export as .zip", Box::new(move || { if let Some(w) = web_sys::window() { let _ = w.location().set_href(&export); } }))}
                </div>
            }
        })
    }
}

/// Where `from` (a mod's file or folder) goes dropped into `dir` (a mod, or
/// a folder of it): None when it cannot (another mod's, where it already
/// is, into itself).
fn drop_path(from: &str, dir: &str) -> Option<String> {
    let base = from.rsplit_once('/')?.1;
    let parent = &from[..from.len() - base.len() - 1];
    let same_mod = from.split('/').next() == dir.split('/').next();
    let inside_itself = dir == from || dir.starts_with(&format!("{from}/"));
    (same_mod && dir != parent && !inside_itself).then(|| format!("{dir}/{base}"))
}

/// Whether the dragged row would drop into `dir` now (it shows so).
fn dropping(s: S, dir: &str) -> bool {
    s.drop_at.with(|d| d.as_deref() == Some(dir))
}

fn drag_start(s: S, path: String) -> impl Fn(leptos::ev::DragEvent) + 'static {
    move |ev| {
        ev.stop_propagation();
        if let Some(dt) = ev.data_transfer() {
            let _ = dt.set_data("text/plain", &path);
            dt.set_effect_allowed("move");
        }
        s.dragging.set(Some(path.clone()));
    }
}

fn drag_end(s: S) {
    s.dragging.set(None);
    s.drop_at.set(None);
}

fn drag_over(s: S, dir: String) -> impl Fn(leptos::ev::DragEvent) + 'static {
    move |ev| {
        let can = s.dragging.with_untracked(|d| d.as_deref().and_then(|f| drop_path(f, &dir)).is_some());
        if can {
            // Droppable here: the innermost row takes it.
            ev.prevent_default();
            ev.stop_propagation();
            if s.drop_at.with_untracked(|d| d.as_deref() != Some(dir.as_str())) {
                s.drop_at.set(Some(dir.clone()));
            }
        }
    }
}

fn drag_leave(s: S, dir: String) -> impl Fn(leptos::ev::DragEvent) + 'static {
    move |_| {
        if s.drop_at.with_untracked(|d| d.as_deref() == Some(dir.as_str())) {
            s.drop_at.set(None);
        }
    }
}

fn dropped(s: S, dir: String) -> impl Fn(leptos::ev::DragEvent) + 'static {
    move |ev| {
        ev.prevent_default();
        ev.stop_propagation();
        let from = s.dragging.get_untracked();
        drag_end(s);
        if let Some(from) = from
            && let Some(to) = drop_path(&from, &dir)
        {
            move_path(s, from, to);
        }
    }
}

/// The files of mod `name` in folder `dir` (`""`, `"core/"`) and those under
/// it: its folders first, each closing and opening, then its files, by name.
fn file_tree(s: S, name: &str, files: &[String], dir: &str, depth: usize) -> AnyView {
    let prefix = format!("{name}/{dir}");
    let mut folders: Vec<String> = Vec::new();
    let mut here: Vec<String> = Vec::new();
    for f in files.iter().filter(|f| shown_file(f)) {
        let Some(rest) = f.strip_prefix(&prefix) else { continue };
        match rest.split_once('/') {
            Some((folder, _)) if !folders.iter().any(|x| x == folder) => folders.push(folder.to_string()),
            Some(_) => {}
            None => here.push(f.clone()),
        }
    }
    folders.sort();
    // Deeper rows step in.
    let indent = format!("margin-left: {}px", 22 + 14 * depth);
    let folder_rows = folders.into_iter().map(|folder| {
        let path = format!("{prefix}{folder}");
        let open = { let p = path.clone(); move || !s.folded.with(|f| f.contains(&p)) };
        let toggle = { let p = path.clone(); move |_| s.folded.update(|f| if let Some(i) = f.iter().position(|x| *x == p) { f.remove(i); } else { f.push(p.clone()); }) };
        let inner = format!("{dir}{folder}/");
        let (n, files) = (name.to_string(), files.to_vec());
        view! {
            <div class={ let p = path.clone(); move || if dropping(s, &p) { "file folder drop" } else { "file folder" } } style=indent.clone() role="button" tabindex="0" on:click=toggle
                draggable="true" on:dragstart=drag_start(s, path.clone()) on:dragend=move |_| drag_end(s)
                on:dragover=drag_over(s, path.clone()) on:dragleave=drag_leave(s, path.clone()) on:drop=dropped(s, path.clone())>
                <span class="ico small" inner_html=icons::FOLDER></span>
                <span class="fname">{folder.clone()}<i class="fold">{ let o = open.clone(); move || if o() { " ▾" } else { " ▸" } }</i></span>
                <span></span>
                <span class="file-acts">
                    <button class="icon-btn" title="New file here" on:click={ let p = path.clone(); move |ev: leptos::ev::MouseEvent| { ev.stop_propagation(); new_file(s, &p) } }>
                        <span class="ico" inner_html=icons::PLUS></span>
                    </button>
                    <button class="icon-btn" title="Rename or move" on:click={ let p = path.clone(); move |ev: leptos::ev::MouseEvent| { ev.stop_propagation(); rename_file(s, &p) } }>
                        <span class="ico" inner_html=icons::PENCIL></span>
                    </button>
                    <button class="icon-btn" title="Delete the folder" on:click={ let p = path.clone(); move |ev: leptos::ev::MouseEvent| { ev.stop_propagation(); delete_file(s, &p, true) } }>
                        <span class="ico" inner_html=icons::TRASH></span>
                    </button>
                </span>
            </div>
            <Show when=open>{file_tree(s, &n, &files, &inner, depth + 1)}</Show>
        }
    }).collect_view();
    // Dropped on a file, a file goes into that file's folder.
    let folder_of = prefix.trim_end_matches('/').to_string();
    let file_rows = here.into_iter().map(|path| {
        let folder_of = folder_of.clone();
        let shown = path.rsplit('/').next().unwrap_or(&path).to_string();
        let p2 = path.clone();
        let active = move || s.active.get().as_deref() == Some(p2.as_str());
        let p3 = path.clone();
        let errs = move || s.diags.with(|d| d.get(&p3).map_or(0, |v| v.iter().filter(|d| d.severity == "error").count()));
        view! {
            <div class=move || if active() { "file on" } else { "file" } style=indent.clone() role="button" tabindex="0"
                draggable="true" on:dragstart=drag_start(s, path.clone()) on:dragend=move |_| drag_end(s)
                on:dragover=drag_over(s, folder_of.clone()) on:dragleave=drag_leave(s, folder_of.clone()) on:drop=dropped(s, folder_of.clone())
                on:click={ let p = path.clone(); move |_| open_file(s, p.clone()) }
                on:keydown={ let p = path.clone(); move |ev| if ev.key() == "Enter" { open_file(s, p.clone()) } }>
                <span class="ico small" inner_html=icons::FILE></span>
                <span class="fname">{shown}</span>
                <span class="err-count">{move || match errs() { 0 => String::new(), n => n.to_string() }}</span>
                <span class="file-acts">
                    <button class="icon-btn" title="Rename or move" on:click={ let p = path.clone(); move |ev: leptos::ev::MouseEvent| { ev.stop_propagation(); rename_file(s, &p) } }>
                        <span class="ico" inner_html=icons::PENCIL></span>
                    </button>
                    <button class="icon-btn" title="Delete" on:click={ let p = path.clone(); move |ev: leptos::ev::MouseEvent| { ev.stop_propagation(); delete_file(s, &p, false) } }>
                        <span class="ico" inner_html=icons::TRASH></span>
                    </button>
                </span>
            </div>
        }
    }).collect_view();
    view! { {folder_rows}{file_rows} }.into_any()
}

#[component]
pub(crate) fn NativesView(s: S) -> impl IntoView {
    let query = RwSignal::new(String::new());
    let results = RwSignal::new(Vec::<Value>::new());
    let run = move |q: String| {
        query.set(q.clone());
        if q.trim().len() < 2 {
            results.set(Vec::new());
            return;
        }
        let asked = q.clone();
        request("natives.search", json!({ "query": q, "limit": 60 }), move |r| {
            if let Ok(Value::Array(list)) = r
                && query.get_untracked() == asked
            {
                results.set(list);
            }
        });
    };
    view! {
        <div class="side-head"><h2>"NATIVES"</h2></div>
        <label class="side-search">
            <span class="ico" inner_html=icons::SEARCH></span>
            <input placeholder="vehicle colour, health, teleport…" spellcheck="false"
                prop:value=move || query.get() on:input=move |ev| run(event_target_value(&ev))/>
        </label>
        <div class="natives">
            {move || results.get().into_iter().map(|n| {
                let name = n["name"].as_str().unwrap_or("").to_string();
                let insert = format!("{name}(");
                let detail = RwSignal::new(None::<String>);
                let n2 = name.clone();
                view! {
                    <div class="native" on:click=move |_| {
                        if detail.get_untracked().is_some() { detail.set(None); return; }
                        request("natives.doc", json!({"name": n2}), move |r| if let Ok(v) = r {
                            detail.set(Some(v["notes"].as_str().unwrap_or("No notes.").to_string()));
                        });
                    }>
                        <div class="sig">{n["signature"].as_str().unwrap_or("").to_string()}</div>
                        <div class="native-foot">
                            <small>{n["native"].as_str().unwrap_or("").to_string()}</small>
                            <button class="chip" on:click=move |ev| { ev.stop_propagation(); insert_code(s, &insert); }>"Insert"</button>
                        </div>
                        {move || detail.get().map(|d| view! { <p class="notes">{d}</p> })}
                    </div>
                }
            }).collect_view()}
        </div>
    }
}
