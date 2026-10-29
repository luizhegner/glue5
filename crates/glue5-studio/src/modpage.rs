//! A mod's page, in place of its mod.json: what it says of itself, how it
//! runs, what it stores, export, delete.

use leptos::prelude::*;
use serde_json::{Value, json};

use crate::actions::close_mod;
use crate::net::request;
use crate::state::*;

#[component]
pub(crate) fn ModPage(s: S, name: String) -> impl IntoView {
    let title = RwSignal::new(String::new());
    let description = RwSignal::new(String::new());
    let author = RwSignal::new(String::new());
    let version = RwSignal::new(String::new());
    let autostart = RwSignal::new(false);
    let restart = RwSignal::new(true);
    let stored = RwSignal::new((0u64, 0u64));
    let view_data = RwSignal::new(None::<String>);
    let loaded = RwSignal::new(false);
    let load = {
        let name = name.clone();
        move || {
            request("mods.info", json!({ "name": name }), move |r| {
                if let Ok(v) = r {
                    let text = |k: &str| v[k].as_str().unwrap_or("").to_string();
                    title.set(text("title"));
                    description.set(text("description"));
                    author.set(text("author"));
                    version.set(text("version"));
                    autostart.set(v["autostart"] == true);
                    restart.set(v["restartOnSave"] != false);
                    stored.set((v["stored"]["keys"].as_u64().unwrap_or(0), v["stored"]["bytes"].as_u64().unwrap_or(0)));
                    loaded.set(true);
                }
            });
        }
    };
    // Again when its mod.json changes elsewhere (Claude Code, the game's
    // Mods page, another browser): the page shows what is saved.
    {
        let name = name.clone();
        let load = load.clone();
        // A memo: the list comes again every second with the mods' CPU
        // times, which must not reload (and wipe) fields being typed in.
        let saved = Memo::new(move |_| {
            s.mods.with(|m| {
                m.iter().find(|x| x.name == name).map(|x| (x.title.clone(), x.description.clone(), x.autostart, x.restart_on_save))
            })
        });
        Effect::new(move |_| {
            saved.track();
            load();
        });
    }
    // Each change is written at once (a field when it is left).
    let save = {
        let name = name.clone();
        move || {
            let info = json!({
                "title": title.get_untracked(), "description": description.get_untracked(),
                "author": author.get_untracked(), "version": version.get_untracked(),
                "autostart": autostart.get_untracked(), "restartOnSave": restart.get_untracked(),
            });
            request("mods.setInfo", json!({ "name": name, "info": info }), move |r| {
                if let Err(e) = r {
                    s.notice.set(Some(e));
                }
            });
        }
    };
    let (save_a, save_r, save_d, save_f) = (save.clone(), save.clone(), save.clone(), save.clone());
    // At most what mod.json keeps (files::info_at).
    let field = move |label: &'static str, hint: &'static str, value: RwSignal<String>, narrow: bool, max: u32| {
        let save = save_f.clone();
        view! {
            <div class="mp-row">
                <div class="lab"><b>{label}</b>{(!hint.is_empty()).then(|| view! { <small>{hint}</small> })}</div>
                <input class=if narrow { "f narrow" } else { "f" } spellcheck="false" maxlength=max prop:value=move || value.get()
                    on:input=move |ev| value.set(event_target_value(&ev))
                    on:change=move |_| save()/>
            </div>
        }
    };
    let state = {
        let name = name.clone();
        move || s.mods.with(|m| m.iter().find(|m| m.name == name).cloned())
    };
    let failed = {
        let state = state.clone();
        move || state().and_then(|m| m.error)
    };
    let token = TOKEN.with(|t| t.borrow().clone());
    let export = format!("/export?mod={name}&token={token}");
    let (n_view, n_clear, n_delete, n_control) = (name.clone(), name.clone(), name.clone(), name.clone());
    view! {
        <div class="mp">
            // Shown once its info came.
            <div class="mp-in" style:visibility=move || if loaded.get() { "visible" } else { "hidden" }>
                <div class="mp-head">
                    <div>
                        <h1>{move || title.get().to_uppercase()}</h1>
                        <div class="sub">{
                            let name = name.clone();
                            move || {
                                // Its folder, unless the name says it already.
                                let mut parts = Vec::new();
                                if !title.get().eq_ignore_ascii_case(&name) { parts.push(name.clone()); }
                                if !version.get().is_empty() { parts.push(format!("version {}", version.get())); }
                                if !author.get().is_empty() { parts.push(format!("by {}", author.get())); }
                                parts.join(" · ")
                            }
                        }</div>
                    </div>
                    <div class="grow"></div>
                    {move || state().map(|m| {
                        let (class, text) = match m.state.as_str() {
                            "running" => ("pill ok", format!("Running · {:.1} ms", m.cpu_ms)),
                            "error" => ("pill err", "Error".to_string()),
                            _ => ("pill", "Stopped".to_string()),
                        };
                        view! { <span class=class>{text}</span> }
                    })}
                </div>
                // Why it stopped, when it failed (a phone has no hover for the list's).
                {move || failed().map(|e| view! { <div class="mp-error">{crate::panel::linked(s, e)}</div> })}
                <div class="mp-card">
                    <h2>"MOD"</h2>
                    {field("Name", "As the game's Mods page shows it", title, false, 40)}
                    <div class="mp-row">
                        <div class="lab"><b>"Description"</b><small>"One or two lines"</small></div>
                        <textarea class="f" maxlength="300" prop:value=move || description.get()
                            on:input=move |ev| description.set(event_target_value(&ev))
                            on:change=move |_| save_d()></textarea>
                    </div>
                    {field("Author", "", author, false, 60)}
                    {field("Version", "", version, true, 20)}
                </div>
                <div class="mp-card">
                    <h2>"RUNNING"</h2>
                    <div class="mp-row">
                        <div class="lab"><b>"Starts with the game"</b><small>"Otherwise from the Mods page or here"</small></div>
                        <label class="switch"><input type="checkbox" prop:checked=move || autostart.get()
                            on:change=move |ev| { autostart.set(event_target_checked(&ev)); save_a(); }/><i></i></label>
                    </div>
                    <div class="mp-row">
                        <div class="lab"><b>"Restart when a file is saved"</b><small>"While it runs: saving reloads it at once"</small></div>
                        <label class="switch"><input type="checkbox" prop:checked=move || restart.get()
                            on:change=move |ev| { restart.set(event_target_checked(&ev)); save_r(); }/><i></i></label>
                    </div>
                </div>
                <div class="mp-card">
                    <h2>"STORED DATA"</h2>
                    <div class="mp-row">
                        <div class="lab">
                            <b>{move || match stored.get() {
                                (0, _) => "Nothing stored".to_string(),
                                (1, b) => format!("1 key · {}", size(b)),
                                (k, b) => format!("{k} keys · {}", size(b)),
                            }}</b>
                            <small>"What it keeps between sessions (storage), for this player only"</small>
                        </div>
                        <button class="btn" disabled=move || stored.get().0 == 0 && view_data.get().is_none() on:click=move |_| {
                            if view_data.get_untracked().is_some() { view_data.set(None); return; }
                            request("storage.view", json!({ "mod": n_view.clone() }), move |r| if let Ok(Value::String(t)) = r {
                                let pretty = serde_json::from_str::<Value>(&t).ok().and_then(|v| serde_json::to_string_pretty(&v).ok()).unwrap_or(t);
                                view_data.set(Some(pretty));
                            });
                        }>{move || if view_data.get().is_some() { "Hide" } else { "View" }}</button>
                        <button class="btn" disabled=move || stored.get().0 == 0 on:click=move |_| {
                            let ok = web_sys::window().and_then(|w| w.confirm_with_message("Clear what this mod stored? It cannot be undone.").ok()).unwrap_or(false);
                            if !ok { return; }
                            let load = load.clone();
                            request("storage.clear", json!({ "mod": n_clear.clone() }), move |_| { view_data.set(None); load(); });
                        }>"Clear"</button>
                    </div>
                    {move || view_data.get().map(|t| view! { <pre class="mp-data">{t}</pre> })}
                </div>
                <div class="mp-card">
                    <h2>"USE"</h2>
                    <div class="mp-row">
                        <div class="lab"><b>"Control page"</b><small>"Its menus on a page of their own: on a phone, or beside the game"</small></div>
                        <button class="btn" on:click={ let n = n_control.clone(); move |_| crate::sidebar::open_control(&n) }>"Open"</button>
                    </div>
                </div>
                <div class="mp-card">
                    <h2>"SHARE"</h2>
                    <div class="mp-row">
                        <div class="lab"><b>"Export"</b><small>"A .zip with its files and this page, without the stored data"</small></div>
                        <a class="btn" href=export download="">"Download .zip"</a>
                    </div>
                </div>
                <div class="mp-actions">
                    <button class="btn danger" on:click=move |_| {
                        let ok = web_sys::window()
                            .and_then(|w| w.confirm_with_message(&format!("Delete the mod {n_delete}, its files and what it stored? It cannot be undone.")).ok())
                            .unwrap_or(false);
                        if !ok { return; }
                        let n = n_delete.clone();
                        request("mods.delete", json!({ "name": n }), move |r| match r {
                            Ok(_) => close_mod(s, &n),
                            Err(e) => s.notice.set(Some(e)),
                        });
                    }>"Delete mod…"</button>
                </div>
            </div>
        </div>
    }
}

fn size(bytes: u64) -> String {
    match bytes {
        b if b < 1024 => format!("{b} bytes"),
        b if b < 1 << 20 => format!("{:.1} KB", b as f64 / 1024.0),
        b => format!("{:.1} MB", b as f64 / (1 << 20) as f64),
    }
}
