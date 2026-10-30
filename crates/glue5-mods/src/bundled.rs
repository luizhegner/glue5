//! The mods that come with GLUE 5 (the repository's `mods/`, built in by
//! build.rs): copied into the player's mods folder, where they are mods
//! like any other (the Mods page, the Studio, a player's edits), and
//! brought up to date there while the player has not edited them.
//!
//! What was installed is noted in the folder (`.glue5/bundled.json`: each
//! file's hash, the version). At each start, for every bundled mod:
//! - no folder (never installed, or deleted): it is installed as shipped;
//! - its files as installed: they become this version's (new files added,
//!   files it no longer has deleted);
//! - edited (a file changed, deleted, or a folder of that name that was
//!   never ours): it stays the player's, and a notice says once per version
//!   that a newer one is there for whoever deletes the folder.
//!
//! `mod.json` is not compared: the new one is written, keeping what the
//! player chose of it (whether it starts with the game and restarts when a
//! file is saved). What a mod stores (`settings.json`, its storage and
//! database) is never a bundled file.

use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};

use serde_json::{Map, Value as Json, json};
use sha2::{Digest, Sha256};

include!(concat!(env!("OUT_DIR"), "/bundled.rs"));

/// What is noted of the bundled mods installed, in the mods folder.
const RECORD: &str = ".glue5/bundled.json";
/// What the player chooses in a mod.json, kept through updates.
const CHOSEN: &[&str] = &["autostart", "restartOnSave"];

/// The names of the mods that come with GLUE 5.
pub fn names() -> impl Iterator<Item = &'static str> {
    BUNDLED.iter().map(|(name, _)| *name)
}

/// Installs or updates the bundled mods in mods folder `dir` (made if
/// missing), once per run of the app. Before [`crate::open`], which then
/// lists them.
pub fn install(dir: &Path) {
    static DONE: AtomicBool = AtomicBool::new(false);
    if DONE.swap(true, Ordering::AcqRel) {
        return;
    }
    let record_path = dir.join(RECORD);
    let mut record: Map<String, Json> = std::fs::read(&record_path)
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default();
    for (name, files) in BUNDLED {
        match update(dir, name, files, record.get(*name)) {
            Ok((entry, notice)) => {
                record.insert((*name).to_string(), entry);
                if let Some(text) = notice {
                    crate::log_line(name, crate::LogLevel::Info, &text);
                    crate::lock(&crate::SHARED.state).notices.push_back(((*name).to_string(), text));
                }
            }
            Err(e) => log::warn!("mods: {name}, which comes with GLUE 5: {e}"),
        }
    }
    let text = serde_json::to_string_pretty(&record).unwrap_or_default();
    if let Err(e) = std::fs::create_dir_all(dir.join(".glue5")).and_then(|()| std::fs::write(&record_path, text)) {
        log::warn!("mods: {}: {e}", record_path.display());
    }
    crate::refresh();
}

fn hash(bytes: &[u8]) -> String {
    Sha256::digest(bytes).iter().map(|b| format!("{b:02x}")).collect()
}

/// A mod's name as shown and its version, as its files say (its mod.json).
fn about(files: &[(&str, &[u8])]) -> (String, String) {
    let info = files
        .iter()
        .find(|(f, _)| *f == "mod.json")
        .and_then(|(_, b)| serde_json::from_slice::<Json>(b).ok())
        .unwrap_or_default();
    let text = |k: &str| info[k].as_str().unwrap_or_default().to_string();
    (text("name"), text("version"))
}

/// A path inside a mod's folder, as noted: plain names only.
fn plain(rel: &str) -> bool {
    Path::new(rel).components().all(|c| matches!(c, std::path::Component::Normal(p) if !p.to_string_lossy().starts_with('.')))
}

/// The bundled mod.json, with what the player chose in `installed`.
fn merged_info(bundled: &[u8], installed: Option<Vec<u8>>) -> Vec<u8> {
    let (Ok(Json::Object(mut info)), Some(Ok(Json::Object(chosen)))) =
        (serde_json::from_slice::<Json>(bundled), installed.map(|b| serde_json::from_slice::<Json>(&b)))
    else {
        return bundled.to_vec();
    };
    for key in CHOSEN {
        if let Some(v) = chosen.get(*key) {
            info.insert((*key).to_string(), v.clone());
        }
    }
    (serde_json::to_string_pretty(&Json::Object(info)).unwrap_or_default() + "\n").into_bytes()
}

/// Writes `bytes` to `path` whole (a temporary file renamed over it).
fn write(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let tmp = path.with_file_name(format!(".{}.glue5-tmp", path.file_name().unwrap_or_default().to_string_lossy()));
    std::fs::write(&tmp, bytes).and_then(|()| std::fs::rename(&tmp, path))
}

/// Mod `name` (its bundled `files`) in mods folder `dir`, given what was
/// noted of it (`before`): what to note now, and a notice for the player.
fn update(dir: &Path, name: &str, files: &[(&str, &[u8])], before: Option<&Json>) -> Result<(Json, Option<String>), String> {
    let folder = dir.join(name);
    let (title, version) = about(files);
    let title = if title.is_empty() { name } else { &title };
    let read = |f: &str| std::fs::read(folder.join(f)).ok();
    let noted: Option<Map<String, Json>> = before.and_then(|b| b["files"].as_object().cloned());
    let fresh = !folder.exists();
    let edited = !fresh
        && match &noted {
            // A file changed or gone since it was installed.
            Some(noted) => noted
                .iter()
                .filter(|(f, _)| *f != "mod.json")
                .any(|(f, h)| !plain(f) || read(f).is_none_or(|b| Some(hash(&b).as_str()) != h.as_str())),
            // Never installed here: a folder of the player's, unless it holds
            // exactly these files.
            None => files.iter().filter(|(f, _)| *f != "mod.json").any(|(f, b)| read(f).as_deref() != Some(*b)),
        };
    if edited {
        let mut entry = before.cloned().unwrap_or_else(|| json!({}));
        let notice = (entry["kept"].as_str() != Some(version.as_str()) && before.and_then(|b| b["version"].as_str()) != Some(version.as_str()))
            .then(|| format!("{title} {version} is out; your edited copy stays as it is (delete the folder {name} for the new one)"));
        entry["kept"] = json!(version);
        return Ok((entry, notice));
    }
    let mut hashes = Map::new();
    for (f, bytes) in files {
        let path = folder.join(f);
        let bytes = if *f == "mod.json" { merged_info(bytes, read(f)) } else { bytes.to_vec() };
        if read(f).as_deref() != Some(bytes.as_slice()) {
            write(&path, &bytes).map_err(|e| format!("{}: {e}", path.display()))?;
        }
        hashes.insert((*f).to_string(), json!(hash(&bytes)));
    }
    // What this version no longer has.
    for f in noted.iter().flat_map(|n| n.keys()).filter(|f| !hashes.contains_key(*f)) {
        let _ = std::fs::remove_file(folder.join(f));
    }
    let was = before.and_then(|b| b["version"].as_str()).unwrap_or_default();
    let notice = (!fresh && was != version).then(|| format!("{title} updated to {version}"));
    if fresh {
        log::info!("mods: {name} {version} installed in {}", dir.display());
    }
    Ok((json!({ "version": version, "files": hashes }), notice))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn folder(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("glue5-bundled-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    const V1: &[(&str, &[u8])] = &[
        ("main.luau", b"require('./lib/a')"),
        ("lib/a.luau", b"return 1"),
        ("old.luau", b"return 0"),
        ("mod.json", br#"{ "name": "T", "version": "1.0", "autostart": true }"#),
    ];
    const V2: &[(&str, &[u8])] = &[
        ("main.luau", b"require('./lib/a') require('./lib/b')"),
        ("lib/a.luau", b"return 1"),
        ("lib/b.luau", b"return 2"),
        ("mod.json", br#"{ "name": "T", "version": "2.0", "autostart": true }"#),
    ];

    fn info(dir: &Path) -> Json {
        serde_json::from_slice(&std::fs::read(dir.join("t/mod.json")).unwrap()).unwrap()
    }

    #[test]
    fn installed_then_updated_while_untouched() {
        let dir = folder("update");
        let (entry, notice) = update(&dir, "t", V1, None).unwrap();
        assert!(notice.is_none());
        assert_eq!(std::fs::read(dir.join("t/lib/a.luau")).unwrap(), b"return 1");
        // The player turns autostart off and plays: settings are theirs.
        std::fs::write(dir.join("t/mod.json"), r#"{ "name": "T", "version": "1.0", "autostart": false }"#).unwrap();
        std::fs::write(dir.join("t/settings.json"), "{}").unwrap();
        let (entry, notice) = update(&dir, "t", V2, Some(&entry)).unwrap();
        assert_eq!(notice.as_deref(), Some("T updated to 2.0"));
        assert_eq!(std::fs::read(dir.join("t/lib/b.luau")).unwrap(), b"return 2");
        assert!(!dir.join("t/old.luau").exists(), "a file the new version dropped goes");
        assert!(dir.join("t/settings.json").exists());
        assert_eq!((info(&dir)["version"].as_str(), info(&dir)["autostart"].as_bool()), (Some("2.0"), Some(false)));
        // Again: nothing to do, nothing said.
        let (again, notice) = update(&dir, "t", V2, Some(&entry)).unwrap();
        assert_eq!((again, notice), (entry, None));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_edited_copy_stays_and_a_deleted_one_comes_back() {
        let dir = folder("edited");
        let (entry, _) = update(&dir, "t", V1, None).unwrap();
        std::fs::write(dir.join("t/lib/a.luau"), "return 'mine'").unwrap();
        let (entry, notice) = update(&dir, "t", V2, Some(&entry)).unwrap();
        assert!(notice.unwrap().contains("2.0 is out"));
        assert_eq!(std::fs::read(dir.join("t/lib/a.luau")).unwrap(), b"return 'mine'");
        assert!(!dir.join("t/lib/b.luau").exists());
        // Said once per version.
        let (entry, notice) = update(&dir, "t", V2, Some(&entry)).unwrap();
        assert!(notice.is_none());
        // Deleted: installed as shipped.
        std::fs::remove_dir_all(dir.join("t")).unwrap();
        update(&dir, "t", V2, Some(&entry)).unwrap();
        assert_eq!(std::fs::read(dir.join("t/lib/a.luau")).unwrap(), b"return 1");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_players_folder_of_that_name_is_left_alone() {
        let dir = folder("theirs");
        std::fs::create_dir_all(dir.join("t")).unwrap();
        std::fs::write(dir.join("t/main.luau"), "print('mine')").unwrap();
        let (_, notice) = update(&dir, "t", V1, None).unwrap();
        assert!(notice.is_some());
        assert_eq!(std::fs::read(dir.join("t/main.luau")).unwrap(), b"print('mine')");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_repositorys_mods_are_built_in() {
        let trainer = BUNDLED.iter().find(|(n, _)| *n == "glue-trainer").expect("mods/glue-trainer");
        assert!(trainer.1.iter().any(|(f, _)| *f == "main.luau") && !about(trainer.1).1.is_empty());
        assert!(trainer.1.iter().all(|(f, _)| !f.starts_with('.') && *f != "settings.json"));
    }
}
