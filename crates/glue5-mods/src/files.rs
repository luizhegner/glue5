//! The mods' files, for the Studio and MCP: paths are `<mod>/<file>`
//! inside the mods folder, never outside it.

use std::path::{Component, Path, PathBuf};

use crate::ModInfo;

const TEMPLATE: &str = "--!strict\n-- Runs once the player is in the game. A loop needs wait() to let the game go on.\n\nwhile true do\n    local me = player.playerPedId()\n    if pad.isControlJustPressed(0, 51) then -- E\n        print(\"position\", entity.getEntityCoords(me, true))\n    end\n    wait()\nend\n";

/// What a mod's name may be: a folder name that is the same everywhere.
pub fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 40
        && name.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-' || b == b'_')
}

/// The file at `path` (`<mod>/<file>`), checked to stay in the mods folder.
pub fn resolve(path: &str) -> Result<PathBuf, String> {
    let dir = crate::dir().ok_or("mods are off")?;
    let rel = Path::new(path);
    let mut parts = 0;
    for c in rel.components() {
        match c {
            Component::Normal(p) if !p.to_string_lossy().starts_with('.') => parts += 1,
            _ => return Err(format!("{path}: not a file of a mod")),
        }
    }
    if parts < 2 {
        return Err(format!("{path}: expected <mod>/<file>"));
    }
    let name = rel.components().next().unwrap().as_os_str().to_string_lossy();
    if !valid_name(&name) {
        return Err(format!("{name}: a mod's name is lowercase letters, digits, - and _"));
    }
    Ok(dir.join(rel))
}

/// The files of mod `name`, as `<mod>/<file>` paths, sorted.
pub fn list(name: &str) -> Vec<String> {
    let Some(dir) = crate::dir().filter(|_| valid_name(name)) else { return Vec::new() };
    let mut out = Vec::new();
    fn walk(root: &Path, at: &Path, out: &mut Vec<String>, depth: usize) {
        let Ok(entries) = std::fs::read_dir(at) else { return };
        for e in entries.flatten() {
            let p = e.path();
            if e.file_name().to_string_lossy().starts_with('.') {
                continue;
            }
            if p.is_dir() && depth < 4 {
                walk(root, &p, out, depth + 1);
            } else if p.is_file()
                && let Ok(rel) = p.strip_prefix(root)
            {
                out.push(rel.to_string_lossy().replace('\\', "/"));
            }
        }
    }
    walk(dir, &dir.join(name), &mut out, 0);
    out.sort();
    out
}

pub fn read(path: &str) -> Result<String, String> {
    let p = resolve(path)?;
    std::fs::read_to_string(&p).map_err(|e| format!("{path}: {e}"))
}

/// Writes `path` whole (a temporary file renamed over it).
pub fn write(path: &str, text: &str) -> Result<(), String> {
    let p = resolve(path)?;
    if let Some(parent) = p.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("{path}: {e}"))?;
    }
    // Hidden (not listed) and its own: two writes at once (the Studio and
    // an agent), or of `a.luau` and `a.json`, never share one.
    static NEXT: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
    let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let file = p.file_name().map_or_else(Default::default, |f| f.to_string_lossy().into_owned());
    let tmp = p.with_file_name(format!(".{file}.{n}.glue5-tmp"));
    std::fs::write(&tmp, text).and_then(|()| std::fs::rename(&tmp, &p)).map_err(|e| format!("{path}: {e}"))?;
    crate::changed(path);
    crate::refresh();
    Ok(())
}

/// Deletes a file, or a folder of a mod with what it holds (`<mod>/<dir>`);
/// folders it leaves empty go too.
pub fn delete(path: &str) -> Result<(), String> {
    let p = resolve(path)?;
    let gone = under(path);
    if p.is_dir() {
        std::fs::remove_dir_all(&p)
    } else {
        std::fs::remove_file(&p)
    }
    .map_err(|e| format!("{path}: {e}"))?;
    prune(&p);
    for f in gone.iter().map(String::as_str).chain([path]) {
        crate::checker::Checker::forget(f);
        crate::changed(f);
    }
    crate::refresh();
    Ok(())
}

/// Renames or moves a file, or a folder with what it holds, within its mod
/// (`<mod>/<dir>` to `<mod>/<other>/<dir>`): the folders it goes into are
/// made, those it leaves empty go.
pub fn rename(from: &str, to: &str) -> Result<(), String> {
    let (a, b) = (resolve(from)?, resolve(to)?);
    if from.split('/').next() != to.split('/').next() {
        return Err(format!("{to}: files move within their mod"));
    }
    if b.exists() {
        return Err(format!("{to} exists"));
    }
    if b.starts_with(&a) {
        return Err(format!("{to}: a folder does not go inside itself"));
    }
    let moved = under(from);
    if let Some(parent) = b.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("{to}: {e}"))?;
    }
    std::fs::rename(&a, &b).map_err(|e| format!("{from}: {e}"))?;
    prune(&a);
    for old in moved.iter().map(String::as_str).chain([from]) {
        crate::checker::Checker::forget(old);
        crate::changed(old);
        crate::changed(&format!("{to}{}", &old[from.len()..]));
    }
    crate::refresh();
    Ok(())
}

/// The files under folder `path` (none for a file), as `<mod>/<file>` paths.
fn under(path: &str) -> Vec<String> {
    let prefix = format!("{}/", path.trim_end_matches('/'));
    let name = path.split('/').next().unwrap_or_default();
    list(name).into_iter().filter(|f| f.starts_with(&prefix)).collect()
}

/// The empty folders from where `gone` was up to its mod's folder go (the
/// mod's own stays).
fn prune(gone: &Path) {
    let Some(root) = crate::dir() else { return };
    let mut at = gone.parent();
    while let Some(dir) = at {
        // The mods folder's child is the mod's own folder.
        if dir.parent() == Some(root) || !dir.starts_with(root) || std::fs::remove_dir(dir).is_err() {
            return;
        }
        at = dir.parent();
    }
}

/// A new mod with a `main.luau` to start from.
pub fn create_mod(name: &str) -> Result<(), String> {
    if !valid_name(name) {
        return Err(format!("{name}: a mod's name is lowercase letters, digits, - and _"));
    }
    let main = resolve(&format!("{name}/main.luau"))?;
    if main.exists() {
        return Err(format!("{name} exists"));
    }
    write(&format!("{name}/main.luau"), TEMPLATE)?;
    set_info(name, &ModInfo { title: name.to_string(), restart_on_save: true, ..ModInfo::default() })
}

/// What the mod in `dir` (named `name`) says of itself: its mod.json, what
/// it leaves out as the defaults.
pub fn info_at(dir: &Path, name: &str) -> ModInfo {
    let v: serde_json::Value = std::fs::read(dir.join("mod.json"))
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default();
    let text = |k: &str, max: usize| v[k].as_str().unwrap_or("").trim().chars().take(max).collect::<String>();
    let title = text("name", 40);
    ModInfo {
        title: if title.is_empty() { name.to_string() } else { title },
        description: text("description", 300),
        author: text("author", 60),
        version: text("version", 20),
        autostart: v["autostart"].as_bool().unwrap_or(false),
        restart_on_save: v["restartOnSave"].as_bool().unwrap_or(true),
    }
}

/// Mod `name`'s info.
pub fn info(name: &str) -> ModInfo {
    match crate::dir().filter(|_| valid_name(name)) {
        Some(dir) => info_at(&dir.join(name), name),
        None => ModInfo::default(),
    }
}

/// Writes mod `name`'s mod.json, keeping keys it does not know of.
pub fn set_info(name: &str, info: &ModInfo) -> Result<(), String> {
    let path = format!("{name}/mod.json");
    let mut v: serde_json::Value = read(&path).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default();
    if !v.is_object() {
        v = serde_json::json!({});
    }
    let title = if info.title.trim() == name { "" } else { info.title.trim() };
    for (k, text) in [("name", title), ("description", info.description.trim()), ("author", info.author.trim()), ("version", info.version.trim())] {
        if text.is_empty() {
            v.as_object_mut().unwrap().remove(k);
        } else {
            v[k] = text.into();
        }
    }
    v["autostart"] = info.autostart.into();
    v["restartOnSave"] = info.restart_on_save.into();
    write(&path, &(serde_json::to_string_pretty(&v).unwrap() + "\n"))
}

/// A new, empty file of a mod (a Luau one starts in strict mode).
pub fn create(path: &str) -> Result<(), String> {
    let p = resolve(path)?;
    if p.exists() {
        return Err(format!("{path} exists"));
    }
    write(path, if path.ends_with(".luau") { "--!strict\n" } else { "" })
}

/// Deletes mod `name`: its folder and what it stored (it must not run).
pub fn delete_mod(name: &str) -> Result<(), String> {
    if !valid_name(name) {
        return Err(format!("no mod {name}"));
    }
    let dir = crate::dir().ok_or("mods are off")?;
    std::fs::remove_dir_all(dir.join(name)).map_err(|e| format!("{name}: {e}"))?;
    let _ = std::fs::remove_file(dir.join(".glue5/storage").join(format!("{name}.json")));
    crate::changed(&format!("{name}/"));
    crate::refresh();
    Ok(())
}

/// Mod `name` as a zip: its files, under its folder's name (its stored
/// data stays: it is the player's).
pub fn export(name: &str) -> Result<Vec<u8>, String> {
    let dir = crate::dir().ok_or("mods are off")?;
    let files: Vec<(String, Vec<u8>)> = list(name)
        .into_iter()
        .filter_map(|f| std::fs::read(dir.join(&f)).ok().map(|data| (f, data)))
        .collect();
    if files.is_empty() {
        return Err(format!("no mod {name}"));
    }
    Ok(crate::zip::write(&files))
}

/// A mod from a zip (`hint`: the zip's name): its files at the zip's top
/// or in one folder, with a main.luau. A mod of the same name is not
/// replaced: the new one gets `-2`, `-3`...; it does not start with the
/// game until the player says so. Returns the mod's name.
pub fn import(zip: &[u8], hint: &str) -> Result<String, String> {
    let files = crate::zip::read(zip)?;
    // One folder holding it all, or none.
    let top = files.first().and_then(|(n, _)| n.split_once('/').map(|(t, _)| t.to_string()));
    let in_folder = top.as_ref().is_some_and(|t| files.iter().all(|(n, _)| n.starts_with(&format!("{t}/"))));
    let strip = |n: &str| if in_folder { n.split_once('/').map_or(n, |(_, r)| r).to_string() } else { n.to_string() };
    if !files.iter().any(|(n, _)| strip(n) == "main.luau") {
        return Err("not a mod: no main.luau (at the zip's top or in its folder)".into());
    }
    let base: String = (if in_folder { top.unwrap() } else { hint.trim_end_matches(".zip").to_string() })
        .to_lowercase()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '_' { c } else { '-' })
        .take(32)
        .collect();
    let base = if base.trim_matches('-').is_empty() { "mod".to_string() } else { base };
    let dir = crate::dir().ok_or("mods are off")?;
    let name = (1..)
        .map(|i| if i == 1 { base.clone() } else { format!("{base}-{i}") })
        .find(|n| !dir.join(n).exists())
        .unwrap();
    let written = (|| {
        for (n, data) in &files {
            let rel = format!("{name}/{}", strip(n));
            let path = resolve(&rel)?;
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent).map_err(|e| format!("{rel}: {e}"))?;
            }
            std::fs::write(&path, data).map_err(|e| format!("{rel}: {e}"))?;
        }
        // Code from elsewhere starts when the player says so.
        let mut info = info(&name);
        if info.autostart {
            info.autostart = false;
            set_info(&name, &info)?;
        }
        Ok(())
    })();
    if let Err(e) = written {
        // Not half a mod (which could start with the game): the folder
        // is new, made here.
        let _ = std::fs::remove_dir_all(dir.join(&name));
        return Err(e);
    }
    crate::changed(&format!("{name}/"));
    crate::refresh();
    Ok(name)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Folders: files are made in them, moved between them, deleted with
    /// them; a folder left empty goes, the mod's own stays.
    #[test]
    fn folders() {
        // The process' mods folder (the first test to open one sets it).
        crate::open(&std::env::temp_dir().join(format!("glue5-files-{}", std::process::id())));
        let dir = crate::dir().unwrap().join("files-test");
        let _ = std::fs::remove_dir_all(&dir);
        write("files-test/main.luau", "require('./core/a')").unwrap();
        write("files-test/core/a.luau", "return 1").unwrap();
        write("files-test/core/deep/b.luau", "return 2").unwrap();
        assert_eq!(list("files-test"), ["files-test/core/a.luau", "files-test/core/deep/b.luau", "files-test/main.luau"]);
        // A file into a new folder; the one it leaves still holds a file.
        rename("files-test/core/a.luau", "files-test/lib/a.luau").unwrap();
        assert!(dir.join("lib/a.luau").is_file() && dir.join("core/deep/b.luau").is_file());
        // A folder whole: into another, never into itself or another mod.
        rename("files-test/core", "files-test/lib/core").unwrap();
        assert!(dir.join("lib/core/deep/b.luau").is_file() && !dir.join("core").exists());
        assert!(rename("files-test/lib", "files-test/lib/x").unwrap_err().contains("inside itself"));
        assert!(rename("files-test/lib", "other/lib").unwrap_err().contains("within their mod"));
        // Deleting the last file of a folder deletes the folders it empties.
        delete("files-test/lib/core/deep/b.luau").unwrap();
        assert!(!dir.join("lib/core").exists() && dir.join("lib/a.luau").is_file());
        delete("files-test/lib").unwrap();
        assert_eq!(list("files-test"), ["files-test/main.luau"]);
        delete("files-test/main.luau").unwrap();
        assert!(dir.is_dir(), "the mod's folder stays");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
