//! The MCP endpoint (Model Context Protocol, "Streamable HTTP" without
//! streams: each POST is one JSON-RPC message, answered in JSON), so a
//! coding agent can write a mod, check it, run it, read its log and try
//! natives live:
//!
//! `claude mcp add --transport http glue5 http://<address>:47110/mcp --header "Authorization: Bearer <token>"`
//! (a pairing's token, from the Studio's welcome page, or the pairing code).

use std::time::{Duration, Instant};

use serde_json::{Value, json};

use super::{Request, api};
use crate::files;

const VERSIONS: &[&str] = &["2025-11-25", "2025-06-18", "2025-03-26", "2024-11-05"];

/// Answers one POST: status, body, extra headers.
pub fn handle(request: &Request, owner: Option<&str>) -> (u16, String, Vec<(&'static str, String)>) {
    let Ok(msg) = serde_json::from_slice::<Value>(&request.body) else {
        return (400, error(Value::Null, -32700, "parse error").to_string(), Vec::new());
    };
    // Batches are no longer in the protocol; one message per request.
    let id = msg["id"].clone();
    let method = msg["method"].as_str().unwrap_or("");
    if id.is_null() {
        // A notification (`notifications/initialized`, ...) or a response.
        return (202, String::new(), Vec::new());
    }
    let p = &msg["params"];
    let result = match method {
        "initialize" => {
            let asked = p["protocolVersion"].as_str().unwrap_or("");
            let version = VERSIONS.iter().find(|v| **v == asked).copied().unwrap_or(VERSIONS[0]);
            Ok(json!({
                "protocolVersion": version,
                "capabilities": { "tools": {}, "resources": {} },
                "serverInfo": { "name": "glue5", "title": "GLUE 5 mods", "version": env!("CARGO_PKG_VERSION") },
                "instructions": "Mods for GTA V running in GLUE 5. Read the resource glue5://guide first. \
                    Write files with write_file (it answers with type errors), run with start_mod or restart_mod \
                    (they answer with the first log lines), try natives live with eval, look them up with \
                    search_natives, and use a mod's menus with menus, menu_open and menu_input.",
            }))
        }
        "ping" => Ok(json!({})),
        "tools/list" => Ok(json!({ "tools": tools() })),
        "tools/call" => Ok(call(p["name"].as_str().unwrap_or(""), &p["arguments"], owner)),
        "resources/list" => Ok(json!({ "resources": [
            { "uri": "glue5://guide", "name": "guide", "title": "How mods work", "mimeType": "text/markdown" },
            { "uri": "glue5://natives.d.luau", "name": "natives", "title": "Every native's type (Luau definitions)", "mimeType": "text/plain" },
            { "uri": "glue5://world.d.luau", "name": "world", "title": "The installed game's vehicle, ped and weapon names (Luau types)", "mimeType": "text/plain" },
        ]})),
        "resources/read" => match p["uri"].as_str() {
            Some(uri @ "glue5://guide") => Ok(json!({ "contents": [{ "uri": uri, "mimeType": "text/markdown", "text": api::GUIDE }] })),
            Some(uri @ "glue5://natives.d.luau") => {
                let text = [crate::COMMON, crate::natives::DEFINITIONS, crate::menus::DEFINITIONS, &crate::input::definitions(), crate::draw::DEFINITIONS, crate::db::DEFINITIONS, crate::net::DEFINITIONS, crate::stdlib::DEFINITIONS].concat();
                Ok(json!({ "contents": [{ "uri": uri, "mimeType": "text/plain", "text": text }] }))
            }
            Some(uri @ "glue5://world.d.luau") => {
                Ok(json!({ "contents": [{ "uri": uri, "mimeType": "text/plain", "text": crate::world::definitions() }] }))
            }
            _ => Err((-32002, "resource not found".to_string())),
        },
        _ => Err((-32601, format!("method not found: {method}"))),
    };
    let body = match result {
        Ok(r) => json!({ "jsonrpc": "2.0", "id": id, "result": r }),
        Err((code, message)) => error(id, code, &message),
    };
    (200, body.to_string(), Vec::new())
}

fn error(id: Value, code: i32, message: &str) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } })
}

fn tools() -> Value {
    let string = |d: &str| json!({ "type": "string", "description": d });
    let tool = |name: &str, description: &str, props: Value, required: &[&str]| {
        json!({
            "name": name,
            "description": description,
            "inputSchema": { "type": "object", "properties": props, "required": required },
        })
    };
    json!([
        tool("diagnostic_control", "Consented game diagnostics. User enables Diagnóstico por IA, saves and returns to gameplay. request requires LOCAL consent for control, game image/audio and context; never approve remotely. Heartbeat every second, lease 5s. pad replaces input for 1..1000ms; sequence replaces up to 16 steps, total <=4000ms; stop cancels everything. context is read-only with freshness/unknown fields. frame/audio request a bounded game-only capture (one per second each); observe polls and consumes PNG / 1s mono WAV, retained <=3s in memory. No mic, system screen, other apps or recording to disk. Image excludes GLUE overlay and is <=640x360; raw backbuffer >3M pixels unsupported. Named Studio pairing required. Never guess button mappings or claim a successful interaction without observation.", json!({
            "op": {"type":"string","enum":["request","status","heartbeat","pad","sequence","prompt","context","frame","audio","observe","stop"]},
            "input":{"type":"integer","minimum":0,"maximum":362,"description":"For prompt: input ID returned by fresh context.prompts, NOT a pad bit. Cancels when prompt disappears."},
            "session_id": {"type":"integer","minimum":1},
            "duration_ms": {"type":"integer","minimum":1,"maximum":1000},
            "steps":{"type":"array","minItems":1,"maxItems":16,"items":{"type":"object","required":["pad","duration_ms"],"properties":{"pad":{"type":"object","description":"Same fields as pad: buttons, left/right sticks, l2/r2"},"duration_ms":{"type":"integer","minimum":1,"maximum":1000}}}},
            "pad": {"type":"object","properties":{
                "buttons":{"type":"integer","minimum":0,"maximum":65535},
                "left":{"type":"array","minItems":2,"maxItems":2,"items":{"type":"number","minimum":-1,"maximum":1}},
                "right":{"type":"array","minItems":2,"maxItems":2,"items":{"type":"number","minimum":-1,"maximum":1}},
                "l2":{"type":"number","minimum":0,"maximum":1},
                "r2":{"type":"number","minimum":0,"maximum":1}
            }}
        }), &["op"]),
        tool("list_mods", "The mods: name, state (running, stopped, error), the error, time per frame, files.", json!({}), &[]),
        tool("create_mod", "Creates a mod folder with a main.luau to start from.", json!({ "name": string("lowercase letters, digits, - and _") }), &["name"]),
        tool("read_file", "Reads a mod's file.", json!({ "path": string("<mod>/<file>, e.g. trainer/main.luau") }), &["path"]),
        tool("write_file", "Writes a mod's file whole (in folders too: <mod>/core/prefs.luau), and answers with the type checker's diagnostics for .luau files (none: it checks). A running mod restarts with it when its mod.json says restartOnSave (the default); the answer then has its state and first log lines.", json!({ "path": string("<mod>/<file>"), "content": string("the whole file") }), &["path", "content"]),
        tool("delete_file", "Deletes a mod's file, or a folder of it with what it holds (folders left empty go too).", json!({ "path": string("<mod>/<file> or <mod>/<folder>") }), &["path"]),
        tool("rename_file", "Renames or moves a mod's file or folder within the mod (the folders it goes into are made). A file in a folder is required from beside it: require(\"./name\"), require(\"../core/name\").", json!({ "from": string("<mod>/<file> or <mod>/<folder>"), "to": string("<mod>/<new path>") }), &["from", "to"]),
        tool("check", "The type checker's diagnostics for a .luau file, as saved.", json!({ "path": string("<mod>/<file>.luau") }), &["path"]),
        tool("start_mod", "Starts a mod, and answers with its state and log after about a second.", json!({ "name": string("the mod") }), &["name"]),
        tool("restart_mod", "Stops a mod if it runs and starts it again (after changing its files), and answers with its state and log after about a second.", json!({ "name": string("the mod") }), &["name"]),
        tool("stop_mod", "Stops a mod; what it created goes with it.", json!({ "name": string("the mod") }), &["name"]),
        tool("logs", "Log lines (prints, starts, errors with file:line) after a cursor; answers with the next cursor.", json!({ "since": { "type": "integer", "description": "cursor from the last call (0: all kept)" }, "mod": string("only this mod's lines") }), &[]),
        tool("eval", "Runs Luau in the game now (in a script thread, natives available) and answers with the values: e.g. entity.getEntityCoords(player.playerPedId(), true).", json!({ "code": string("an expression, or statements") }), &["code"]),
        tool("menus", "The running mods' menus (ui.menu), all flat, submenus too: {mod, id, parent, title, note, search (players filter its rows), rev, items}; \
            each item's id, kind (button, toggle, slider, number, choice, multi, text, color, label, separator, progress, submenu), \
            label, note, enabled, visible and value (indices from 1, colours #rrggbb[aa]); a submenu row's menu is the menu it opens.",
            json!({ "mod": string("only this mod's menus") }), &[]),
        tool("menu_open", "Looks at a menu as a player opening it would (its onOpen runs, which may fill it), and answers after \
            half a second with {menu, submenus, log}; then stops looking (its onClose runs).",
            json!({ "mod": string("the mod"), "menu": { "type": "integer", "description": "the menu's id (from menus)" } }), &["mod", "menu"]),
        tool("menu_input", "Does what a player would to a mod's menu item, and answers with the mod's log after half a second. \
            value: a toggle true or false (none flips it); a slider or number a number (clamped, on the step); a choice the option's \
            number from 1 or its text; a multi a list of those (the whole selection); a text a string; a color \"#rrggbb\", \
            \"#rrggbbaa\" or {r, g, b, a} (0-255); a button none (it is pressed). Disabled or hidden items refuse input.",
            json!({ "mod": string("the mod"), "item": { "type": "integer", "description": "the item's id (from menus)" }, "value": { "description": "see above; leave out for buttons" } }), &["mod", "item"]),
        tool("search_natives", "Finds natives by words of their name or notes; answers with signatures.", json!({ "query": string("e.g. vehicle colour"), "limit": { "type": "integer", "description": "at most this many (20)" } }), &["query"]),
        tool("native_doc", "A native's signature and notes.", json!({ "name": string("namespace.name (vehicle.createVehicle) or the game's name (CREATE_VEHICLE)") }), &["name"]),
    ])
}

/// A tool's result: text content, or an error the agent sees.
fn call(name: &str, a: &Value, owner: Option<&str>) -> Value {
    let s = |k: &str| a[k].as_str().ok_or_else(|| format!("missing argument {k}"));
    let result: Result<Value, String> = (|| {
        Ok(match name {
            "diagnostic_control" => diagnostic_call(owner, a)?,
            "list_mods" => api::mods(),
            "create_mod" => {
                files::create_mod(s("name")?)?;
                json!({ "created": files::list(s("name")?) })
            }
            "read_file" => Value::String(files::read(s("path")?)?),
            "write_file" => {
                let path = s("path")?;
                let (_, cursor) = crate::logs(u64::MAX);
                files::write(path, s("content")?)?;
                let mut out = json!({ "written": path });
                if path.ends_with(".luau") || path.ends_with(".json") {
                    out["diagnostics"] = api::diagnostics(path, None);
                    super::checker_used_by_agent();
                }
                // A running mod restarts with it, as from the Studio.
                if crate::saved(path) {
                    out["restarted"] = settle(path.split('/').next().unwrap_or(""), cursor);
                }
                out
            }
            "delete_file" => {
                files::delete(s("path")?)?;
                json!({ "deleted": s("path")? })
            }
            "rename_file" => {
                files::rename(s("from")?, s("to")?)?;
                json!({ "renamed": s("from")?, "to": s("to")? })
            }
            "check" => {
                super::checker_used_by_agent();
                api::diagnostics(s("path")?, None)
            }
            "start_mod" | "restart_mod" => {
                let mod_name = s("name")?;
                if !crate::mods().iter().any(|m| m.name == mod_name) {
                    return Err(format!("no mod {mod_name}"));
                }
                if let Some(why) = crate::cannot_start() {
                    return Err(why.into());
                }
                let (_, cursor) = crate::logs(u64::MAX);
                if name == "start_mod" {
                    crate::start(mod_name);
                } else {
                    crate::restart(mod_name);
                }
                settle(mod_name, cursor)
            }
            "stop_mod" => {
                crate::stop(s("name")?);
                json!({ "stopping": s("name")? })
            }
            "logs" => {
                let (lines, next) = api::logs(a["since"].as_u64().unwrap_or(0), a["mod"].as_str());
                json!({ "lines": lines, "next": next })
            }
            "eval" => match api::eval(s("code")?, Duration::from_secs(10)) {
                Ok(v) => Value::String(v),
                Err(e) => return Err(e),
            },
            "menus" => match a["mod"].as_str() {
                Some(only) => Value::Array(api::menus().as_array().into_iter().flatten().filter(|m| m["mod"] == only).cloned().collect()),
                None => api::menus(),
            },
            "menu_open" => {
                let module = s("mod")?;
                let menu = a["menu"].as_u64().ok_or("missing argument menu")? as u32;
                // There (an error otherwise) before it is looked at.
                api::menu_open(module, menu)?;
                let (_, cursor) = crate::logs(u64::MAX);
                crate::menu_viewer(module, menu, true);
                std::thread::sleep(Duration::from_millis(500));
                let now = api::menu_open(module, menu);
                crate::menu_viewer(module, menu, false);
                let mut out = now?;
                let (lines, next) = api::logs(cursor, Some(module));
                out["log"] = json!(lines.iter().map(line).collect::<Vec<_>>());
                out["next"] = json!(next);
                out
            }
            "menu_input" => {
                let module = s("mod")?;
                let item = a["item"].as_u64().ok_or("missing argument item")? as u32;
                let (_, cursor) = crate::logs(u64::MAX);
                api::menu_input(module, item, &a["value"])?;
                std::thread::sleep(Duration::from_millis(500));
                let (lines, next) = api::logs(cursor, Some(module));
                json!({ "log": lines.iter().map(line).collect::<Vec<_>>(), "next": next })
            }
            "search_natives" => Value::Array(api::search(s("query")?, a["limit"].as_u64().unwrap_or(20).min(100) as usize)),
            "native_doc" => api::find(s("name")?).map(|n| api::describe(n, true)).ok_or("no such native")?,
            _ => return Err(format!("unknown tool {name}")),
        })
    })();
    match result {
        Ok(mut v) if name=="diagnostic_control"&&a["op"]=="observe"=>{
            let mut content=vec![json!({"type":"text","text":v["metadata"].to_string()})];
            for (key,kind) in [("image","image"),("audio","audio")]{
                if let Some(media)=v.get_mut(key).filter(|m|m.is_object()){
                    let mut media=media.take();media["type"]=json!(kind);content.push(media);
                }
            }
            json!({"content":content})
        },
        Ok(Value::String(text)) => json!({ "content": [{ "type": "text", "text": text }] }),
        Ok(v) => json!({ "content": [{ "type": "text", "text": serde_json::to_string_pretty(&v).unwrap() }] }),
        Err(e) => json!({ "content": [{ "type": "text", "text": e }], "isError": true }),
    }
}

fn diagnostic_call(owner: Option<&str>, args: &Value) -> Result<Value, String> {
    let owner = owner.ok_or("a named Studio pairing is required; the pairing code alone cannot control the game")?;
    let body = serde_json::to_vec(args).map_err(|e| e.to_string())?;
    if body.len() > 4096 { return Err("diagnostic request exceeds 4096 bytes".into()); }
    let (status, body) = super::diagnostics::handle(owner, &body);
    let value: Value = serde_json::from_str(&body).map_err(|e| e.to_string())?;
    if status != 200 {
        return Err(value["error"].as_str().unwrap_or("diagnostic request failed").to_owned());
    }
    Ok(value)
}

#[cfg(test)]
mod diagnostic_tests {
    use super::*;

    #[test]
    fn diagnostic_tool_is_discoverable_and_cannot_use_raw_pairing_code() {
        assert!(tools().as_array().unwrap().iter().any(|t| t["name"] == "diagnostic_control"));
        assert!(diagnostic_call(None, &json!({"op":"request"})).is_err());
        let response = call("diagnostic_control", &json!({"op":"request"}), None);
        assert_eq!(response["isError"], true);
    }

    #[test]
    fn malformed_and_oversized_diagnostic_calls_are_refused() {
        assert!(diagnostic_call(Some("test-owner"), &json!({"op":"unknown"})).is_err());
        assert!(diagnostic_call(Some("test-owner"), &json!({"op":"status","extra":"x".repeat(4096)})).is_err());
    }
}

/// A mod's state and log about a second after it was started (sooner if
/// it fails or ends).
fn settle(name: &str, cursor: u64) -> Value {
    let started = Instant::now();
    let mut seen_running = false;
    loop {
        std::thread::sleep(Duration::from_millis(100));
        let m = crate::mods().into_iter().find(|m| m.name == name);
        let state = m.as_ref().map_or("gone", |m| m.state.name());
        seen_running |= state == "running";
        let done = (seen_running && state != "running") || state == "error";
        if done || started.elapsed() > Duration::from_millis(1200) {
            let (lines, next) = api::logs(cursor, Some(name));
            return json!({
                "state": state,
                "error": m.and_then(|m| m.error),
                "log": lines.iter().map(line).collect::<Vec<_>>(),
                "next": next,
                "note": if state == "running" && lines.is_empty() { "running; a mod starts once the player is in the game" } else { "" },
            });
        }
    }
}

fn line(l: &Value) -> String {
    format!("[{}] {}", l["level"].as_str().unwrap_or(""), l["text"].as_str().unwrap_or(""))
}
