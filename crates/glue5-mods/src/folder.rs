//! The mod's own folder: `mod.read`, `mod.write`, `mod.list` (its files,
//! nothing outside it), and `mod.settings`, the settings a player edits in
//! its `settings.json` (an INI file's place in ScriptHookV mods). Files are
//! read and written on the mod's turn: they are meant to be small (a file
//! [`FILE`] at most, the folder [`FOLDER`]). Names starting with a dot are
//! not the mod's (the app's, the system's).

use std::path::{Component, Path, PathBuf};

use mlua::{Lua, Table, Value};
use serde_json::Value as Json;

use crate::json;

/// The largest file a mod reads or writes.
const FILE: u64 = 16 << 20;
/// What a mod's folder may hold, written by it or not.
const FOLDER: u64 = 256 << 20;
/// The settings' file, in the mod's folder.
const SETTINGS: &str = "settings.json";

/// `rel` in the mod's folder `dir`: plain names, no link out of it.
fn resolve(dir: &Path, rel: &str) -> Result<PathBuf, String> {
    for c in Path::new(rel).components() {
        match c {
            Component::Normal(p) if !p.to_string_lossy().starts_with('.') => {}
            _ => return Err(format!("\"{rel}\": a path in the mod's folder, as \"data/cars.json\"")),
        }
    }
    let path = dir.join(rel);
    // What exists of it is in the folder (a link may lead elsewhere).
    let root = dir.canonicalize().map_err(|e| e.to_string())?;
    let existing = path.ancestors().find(|a| a.exists()).unwrap_or(dir);
    let real = existing.canonicalize().map_err(|e| e.to_string())?;
    if !real.starts_with(&root) {
        return Err(format!("\"{rel}\": not in the mod's folder"));
    }
    Ok(path)
}

/// The size of what is in `dir`, but `except`.
fn size(dir: &Path, except: &Path) -> u64 {
    let Ok(entries) = std::fs::read_dir(dir) else { return 0 };
    entries
        .flatten()
        .map(|e| match e.file_type() {
            Ok(t) if t.is_dir() => size(&e.path(), except),
            Ok(t) if t.is_file() && e.path() != except => e.metadata().map_or(0, |m| m.len()),
            _ => 0,
        })
        .sum()
}

fn read(dir: &Path, rel: &str) -> Result<Option<Vec<u8>>, String> {
    let path = resolve(dir, rel)?;
    match std::fs::metadata(&path) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e.to_string()),
        Ok(m) if m.is_dir() => Err("a folder".into()),
        Ok(m) if m.len() > FILE => Err(format!("{} MB: a mod reads files of {} MB at most", m.len() >> 20, FILE >> 20)),
        Ok(_) => std::fs::read(&path).map(Some).map_err(|e| e.to_string()),
    }
}

fn write(dir: &Path, rel: &str, data: &[u8]) -> Result<(), String> {
    let path = resolve(dir, rel)?;
    if data.len() as u64 > FILE {
        return Err(format!("a mod writes files of {} MB at most", FILE >> 20));
    }
    if size(dir, &path) + data.len() as u64 > FOLDER {
        return Err(format!("the mod's folder would hold more than {} MB", FOLDER >> 20));
    }
    let parent = path.parent().ok_or("no folder")?;
    std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    // Whole or not at all.
    let name = path.file_name().ok_or("not a file")?.to_string_lossy();
    let tmp = parent.join(format!(".{name}.tmp"));
    std::fs::write(&tmp, data).and_then(|()| std::fs::rename(&tmp, &path)).map_err(|e| {
        let _ = std::fs::remove_file(&tmp);
        e.to_string()
    })
}

fn list(dir: &Path, rel: &str) -> Result<Vec<String>, String> {
    let path = if rel.is_empty() { dir.to_path_buf() } else { resolve(dir, rel)? };
    let mut names: Vec<String> = std::fs::read_dir(&path)
        .map_err(|e| e.to_string())?
        .flatten()
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().into_owned();
            if name.starts_with('.') {
                return None;
            }
            Some(if e.file_type().is_ok_and(|t| t.is_dir()) { format!("{name}/") } else { name })
        })
        .collect();
    names.sort();
    Ok(names)
}

/// `defaults` with what the player set over them (objects key by key).
fn merge(defaults: &mut Json, set: Json) {
    match (defaults, set) {
        (Json::Object(d), Json::Object(s)) => {
            for (k, v) in s {
                match d.get_mut(&k) {
                    Some(dv) => merge(dv, v),
                    None => {
                        d.insert(k, v);
                    }
                }
            }
        }
        (d, s) => *d = s,
    }
}

/// Whether `defaults` has keys `set` has not (the file lacks settings a
/// newer version of the mod added).
fn missing(defaults: &Json, set: &Json) -> bool {
    match (defaults, set) {
        (Json::Object(d), Json::Object(s)) => d.iter().any(|(k, v)| s.get(k).is_none_or(|sv| missing(v, sv))),
        _ => false,
    }
}

/// `mod.settings(defaults)`: the defaults with the player's settings over
/// them; the file is written (with the defaults) when it is not there or
/// lacks some, so the player finds them all to edit.
fn settings(dir: &Path, module: &str, defaults: Json) -> Result<Json, String> {
    let set = match read(dir, SETTINGS)? {
        None => None,
        Some(bytes) => match serde_json::from_slice::<Json>(&bytes) {
            Ok(v) => Some(v),
            Err(e) => {
                // The player's file is left as it is.
                crate::log_line(module, crate::LogLevel::Error, &format!("{SETTINGS}: {e}; the defaults are used"));
                return Ok(defaults);
            }
        },
    };
    let mut merged = defaults.clone();
    let rewrite = set.as_ref().is_none_or(|s| missing(&defaults, s));
    if let Some(set) = set {
        merge(&mut merged, set);
    }
    if rewrite {
        let text = serde_json::to_string_pretty(&merged).map_err(|e| e.to_string())? + "\n";
        write(dir, SETTINGS, text.as_bytes())?;
    }
    Ok(merged)
}

/// Adds `mod.read`, `mod.write`, `mod.list` and `mod.settings` to the `mod`
/// table of mod `module`, whose folder is `dir`.
pub fn install(lua: &Lua, module: &str, dir: &Path) -> mlua::Result<()> {
    let m: Table = lua.globals().get("mod")?;
    let error = |what: &str, rel: &str, e: String| mlua::Error::runtime(format!("mod.{what}(\"{rel}\"): {e}"));
    let d = dir.to_path_buf();
    m.set(
        "read",
        lua.create_function(move |lua, rel: String| match read(&d, &rel).map_err(|e| error("read", &rel, e))? {
            Some(bytes) => Ok(Value::String(lua.create_string(bytes)?)),
            None => Ok(Value::Nil),
        })?,
    )?;
    let d = dir.to_path_buf();
    m.set(
        "write",
        lua.create_function(move |_, (rel, data): (String, Value)| {
            let bytes = match &data {
                Value::String(s) => s.as_bytes().to_vec(),
                Value::Buffer(b) => b.to_vec(),
                _ => return Err(error("write", &rel, "data is a string or a buffer".into())),
            };
            write(&d, &rel, &bytes).map_err(|e| error("write", &rel, e))
        })?,
    )?;
    let d = dir.to_path_buf();
    m.set(
        "list",
        lua.create_function(move |_, rel: Option<String>| {
            let rel = rel.unwrap_or_default();
            list(&d, rel.trim_end_matches('/')).map_err(|e| error("list", &rel, e))
        })?,
    )?;
    let (d, module) = (dir.to_path_buf(), module.to_string());
    m.set(
        "settings",
        lua.create_function(move |lua, defaults: Value| {
            let bad = |e: String| mlua::Error::runtime(format!("mod.settings: {e}"));
            let text = json::encode(&defaults, false, FILE as usize).map_err(|e| bad(e.to_string()))?;
            let defaults: Json = serde_json::from_str(&text).map_err(|e| bad(e.to_string()))?;
            if !defaults.is_object() {
                return Err(bad("the defaults are a table of names".into()));
            }
            let merged = settings(&d, &module, defaults).map_err(bad)?;
            json::from_json(lua, &merged)
        })?,
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn folder(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("glue5-folder-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn lua_in(dir: &Path) -> Lua {
        let lua = Lua::new();
        let m = lua.create_table().unwrap();
        m.set("name", "m").unwrap();
        lua.globals().set("mod", m).unwrap();
        install(&lua, "m", dir).unwrap();
        lua
    }

    #[test]
    fn files_stay_in_the_folder() {
        let dir = folder("files");
        let lua = lua_in(&dir);
        lua.load(
            r#"
            assert(mod.read("data/a.txt") == nil)
            mod.write("data/a.txt", "one")
            mod.write("b.bin", buffer.fromstring("\0\1"))
            assert(mod.read("data/a.txt") == "one" and mod.read("b.bin") == "\0\1")
            local names = mod.list()
            assert(#names == 2 and names[1] == "b.bin" and names[2] == "data/", table.concat(names, ","))
            assert(mod.list("data/")[1] == "a.txt")
            "#,
        )
        .exec()
        .unwrap();
        let err = |code: &str| lua.load(code).exec().unwrap_err().to_string();
        assert!(err("mod.read('../x')").contains("a path in the mod's folder"));
        assert!(err("mod.write('/etc/x', 'a')").contains("a path in the mod's folder"));
        assert!(err("mod.read('.git/config')").contains("a path in the mod's folder"));
        assert!(err("mod.read('data')").contains("a folder"));
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(std::env::temp_dir(), dir.join("out")).unwrap();
            assert!(err("mod.write('out/x.txt', 'a')").contains("not in the mod's folder"));
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn settings_merge_and_fill_in() {
        let dir = folder("settings");
        let lua = lua_in(&dir);
        let first: String =
            lua.load("return mod.settings({ key = 'F5', speed = 2, colors = { a = 1 } }).key").eval().unwrap();
        assert_eq!(first, "F5");
        let written: Json = serde_json::from_slice(&std::fs::read(dir.join(SETTINGS)).unwrap()).unwrap();
        assert_eq!(written["speed"], 2);
        // The player edits it; a newer version adds a setting.
        std::fs::write(dir.join(SETTINGS), r#"{ "key": "F6", "colors": { "b": 2 }, "extra": true }"#).unwrap();
        let (key, speed, a, b, new): (String, f64, f64, f64, bool) = lua
            .load("local s = mod.settings({ key = 'F5', speed = 2, colors = { a = 1 }, new = true }) return s.key, s.speed, s.colors.a, s.colors.b, s.new")
            .eval()
            .unwrap();
        assert_eq!((key.as_str(), speed, a, b, new), ("F6", 2.0, 1.0, 2.0, true));
        let written: Json = serde_json::from_slice(&std::fs::read(dir.join(SETTINGS)).unwrap()).unwrap();
        assert_eq!(
            (written["key"].as_str(), written["new"].as_bool(), written["extra"].as_bool()),
            (Some("F6"), Some(true), Some(true))
        );
        // A broken file stays; the defaults are used.
        std::fs::write(dir.join(SETTINGS), "{ broken").unwrap();
        let key: String = lua.load("return mod.settings({ key = 'F5' }).key").eval().unwrap();
        assert_eq!(key, "F5");
        assert_eq!(std::fs::read_to_string(dir.join(SETTINGS)).unwrap(), "{ broken");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
