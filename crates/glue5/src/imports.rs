//! Files brought in through Android's document picker.
//!
//! Java owns the picker and copies a selected document tree into
//! `Android/data/<app>/files/imports/ready`.  That is intentional: opening a
//! picker backgrounds the renderer, and Android may recreate the activity.
//! A completed tree survives either outcome.  This module then installs it
//! while the game is stopped, with an atomic file-by-file replacement.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};

use crate::paths;

#[derive(Clone, Copy, Debug)]
pub enum Kind {
    Save,
    Resources,
}

impl Kind {
    fn name(self) -> &'static str {
        match self {
            Self::Save => "save",
            Self::Resources => "resources",
        }
    }
}

/// Ask the Java activity to show Android's folder picker.  SDL exposes the
/// activity's JNI environment on the same thread that draws the app.
#[cfg(target_os = "android")]
pub fn pick(kind: Kind) -> Result<()> {
    ask(kind.name())
}

/// Ask for the folder the game is played from.  It is not an import: nothing
/// is copied, the engine reads the game where the player keeps it (which
/// takes the access the activity asks for first).
#[cfg(target_os = "android")]
pub fn pick_game() -> Result<()> {
    ask("game")
}

#[cfg(target_os = "android")]
fn ask(what: &str) -> Result<()> {
    use jni::objects::{JObject, JValue};
    use jni::JNIEnv;

    unsafe extern "C" {
        // SDL3's name; SDL2 called it SDL_AndroidGetJNIEnv.
        fn SDL_GetAndroidJNIEnv() -> *mut jni::sys::JNIEnv;
    }

    let raw = unsafe { SDL_GetAndroidJNIEnv() };
    if raw.is_null() {
        bail!("Android did not provide a JNI environment");
    }
    let mut env = unsafe { JNIEnv::from_raw(raw) }.context("opening Android's JNI environment")?;
    let class = env.find_class("local/glue5/Glue5Activity").context("finding Glue5Activity")?;
    let value = JObject::from(env.new_string(what).context("encoding import type")?);
    env.call_static_method(class, "startImport", "(Ljava/lang/String;)V", &[JValue::Object(&value)])
        .context("asking Android to open the folder picker")?;
    Ok(())
}

/// The folder the activity's picker chose for the game, or why it did not.
#[derive(Clone, Debug)]
pub enum GamePick {
    /// A path on the device's storage; whether it has the game is the app's
    /// call, which knows this build's layout.
    Found(PathBuf),
    /// What went wrong, for the start page.
    Refused(String),
}

/// The names the activity writes its answer under, in the imports folder.
const GAME_PATH: &str = "game-path";
const GAME_ERROR: &str = "game-error";

/// Takes the picker's answer for the game's folder, once the activity has
/// left one.  It travels as a file, so an answer already left survives the
/// activity being recreated before the app comes to read it.
pub fn take_game_pick(user_dir: &Path) -> Option<GamePick> {
    let root = paths::imports_dir(user_dir);
    if let Some(path) = take_file(&root.join(GAME_PATH)) {
        // An answer replaces whatever the last try said.
        let _ = fs::remove_file(root.join(GAME_ERROR));
        return Some(GamePick::Found(PathBuf::from(path)));
    }
    take_file(&root.join(GAME_ERROR)).map(GamePick::Refused)
}

/// A small text file's contents, read once: the file goes with them.
fn take_file(path: &Path) -> Option<String> {
    let text = fs::read_to_string(path).ok()?;
    let _ = fs::remove_file(path);
    let text = text.trim();
    (!text.is_empty()).then(|| text.to_owned())
}

/// Install every completed Android import.  A directory moves into
/// `processing` before we touch it, so it is either retried on the next app
/// launch or retained under `failed` with its error.
pub fn install_ready(user_dir: &Path, game_dir: Option<&Path>) -> Vec<Result<String>> {
    let root = paths::imports_dir(user_dir);
    let mut results = Vec::new();
    for state in ["ready", "processing"] {
        let dir = root.join(state);
        let Ok(entries) = fs::read_dir(&dir) else { continue };
        for entry in entries.flatten() {
            if !entry.file_type().is_ok_and(|t| t.is_dir()) {
                continue;
            }
            let name = entry.file_name().to_string_lossy().into_owned();
            let source = entry.path();
            let processing = root.join("processing").join(&name);
            let stage = if state == "ready" {
                match fs::create_dir_all(root.join("processing")).and_then(|()| fs::rename(&source, &processing)) {
                    Ok(()) => processing,
                    Err(error) => {
                        results.push(Err(error).context("moving the selected import into place"));
                        continue;
                    }
                }
            } else {
                source
            };
            let outcome = install_one(&stage, user_dir, game_dir);
            match outcome {
                Ok(message) => {
                    if let Err(error) = fs::remove_dir_all(&stage) {
                        results.push(Err(error).context("removing the completed import staging folder"));
                    } else {
                        results.push(Ok(message));
                    }
                }
                Err(error) => {
                    let failed = root.join("failed").join(&name);
                    let moved = fs::create_dir_all(root.join("failed")).and_then(|()| fs::rename(&stage, &failed));
                    if moved.is_ok() {
                        let _ = fs::write(failed.join("error.txt"), format!("{error:#}\n"));
                    }
                    results.push(Err(error));
                }
            }
        }
    }
    results
}

fn install_one(stage: &Path, user_dir: &Path, game_dir: Option<&Path>) -> Result<String> {
    let label = stage.file_name().and_then(|s| s.to_str()).unwrap_or_default();
    if label.starts_with("save-") {
        let source = best_profile(stage)?.ok_or_else(|| anyhow::anyhow!("the selected folder has no SGTA5 save file"))?;
        let name = profile_name(&source, stage);
        let root = user_dir.join("Documents/Rockstar Games/GTA V");
        // Next to the saves the engine wrote itself is the only destination
        // that certainly matches this build's layout (PC builds use
        // `Profiles/<id>`, console-derived ones do not).  Before the first
        // save exists, the PC layout is the best guess.
        let destination = existing_save_dir(&root).unwrap_or_else(|| root.join("Profiles").join(&name));
        let backup = paths::imports_dir(user_dir).join("backups").join(label).join(&name);
        let files = copy_tree(&source, &destination, Some(&backup))?;
        let shown = destination.strip_prefix(&root).unwrap_or(&destination).display();
        return Ok(format!(
            "Imported {files} save file(s) into Documents/Rockstar Games/GTA V/{shown}. Restart Story Mode before loading."
        ));
    }
    if label.starts_with("resources-") {
        let game = game_dir.ok_or_else(|| anyhow::anyhow!("the local game folder is not available"))?;
        let files = copy_tree(stage, game, None)?;
        return Ok(format!("Imported {files} game resource file(s). Restart the game to load them."));
    }
    bail!("unknown import kind in {label:?}")
}

fn source_name(stage: &Path) -> String {
    let raw = fs::read_to_string(stage.join(".glue5-source-name")).unwrap_or_default();
    let name = raw.trim();
    if !name.is_empty() && name.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.')) {
        name.to_owned()
    } else {
        "imported-profile".to_owned()
    }
}

/// The folder of the profile in a picked tree.  A profile can be picked as
/// is, inside a "GTA V" folder, or beside a backup copy; the folder with the
/// most save slots is the live one (a backup usually has a single slot).
fn best_profile(dir: &Path) -> Result<Option<PathBuf>> {
    use std::collections::HashMap;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn walk(dir: &Path, seen: &mut HashMap<PathBuf, (usize, SystemTime)>) -> Result<()> {
        for entry in fs::read_dir(dir).with_context(|| format!("reading {}", dir.display()))? {
            let entry = entry?;
            let ty = entry.file_type()?;
            if ty.is_dir() {
                walk(&entry.path(), seen)?;
            } else if ty.is_file() && is_save(&entry.file_name().to_string_lossy()) {
                let Some(parent) = entry.path().parent().map(Path::to_path_buf) else { continue };
                let time = entry.metadata().and_then(|m| m.modified()).unwrap_or(UNIX_EPOCH);
                let found = seen.entry(parent).or_insert((0, UNIX_EPOCH));
                found.0 += 1;
                found.1 = found.1.max(time);
            }
        }
        Ok(())
    }

    let mut seen = HashMap::new();
    walk(dir, &mut seen)?;
    Ok(seen.into_iter().max_by_key(|(_, found)| *found).map(|(path, _)| path))
}

/// `SGTA50000` … `SGTA50015`, with or without the backup suffix.
fn is_save(name: &str) -> bool {
    let Some(rest) = name.strip_prefix("SGTA5") else { return false };
    let rest = rest.strip_suffix(".bak").unwrap_or(rest);
    rest.len() == 4 && rest.chars().all(|c| c.is_ascii_digit())
}

/// The folder the engine already keeps saves in, if it has written any.
fn existing_save_dir(root: &Path) -> Option<PathBuf> {
    best_profile(root).ok().flatten()
}

fn profile_name(source: &Path, stage: &Path) -> String {
    let raw = if source == stage {
        source_name(stage)
    } else {
        source.file_name().and_then(|s| s.to_str()).unwrap_or_default().to_owned()
    };
    if !raw.is_empty() && raw.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.')) {
        raw
    } else {
        "imported-profile".to_owned()
    }
}

/// Copy only ordinary directory entries and files.  The source is already an
/// app-owned staging tree, but retaining this restriction keeps an import
/// from manufacturing links outside either destination.
fn copy_tree(from: &Path, to: &Path, backup: Option<&Path>) -> Result<usize> {
    fs::create_dir_all(to).with_context(|| format!("creating {}", to.display()))?;
    let mut copied = 0;
    for entry in fs::read_dir(from).with_context(|| format!("reading {}", from.display()))? {
        let entry = entry?;
        let name = entry.file_name();
        if name == ".glue5-source-name" || name == "error.txt" {
            continue;
        }
        let source = entry.path();
        let destination = to.join(&name);
        let ty = entry.file_type()?;
        if ty.is_dir() {
            copied += copy_tree(&source, &destination, backup.map(|b| b.join(&name)).as_deref())?;
        } else if ty.is_file() {
            if let Some(backup) = backup.filter(|_| destination.exists()) {
                if let Some(parent) = backup.parent() {
                    fs::create_dir_all(parent)?;
                }
                fs::copy(&destination, backup).with_context(|| format!("backing up {}", destination.display()))?;
            }
            copy_file(&source, &destination)?;
            copied += 1;
        }
    }
    Ok(copied)
}

fn copy_file(from: &Path, to: &Path) -> Result<()> {
    if let Some(parent) = to.parent() {
        fs::create_dir_all(parent)?;
    }
    let extension = to.extension().and_then(|e| e.to_str()).unwrap_or("file");
    let temp = to.with_extension(format!("{extension}.glue5-import"));
    fs::copy(from, &temp).with_context(|| format!("copying {}", from.display()))?;
    if let Err(error) = fs::rename(&temp, to) {
        // Android/Linux replace directly.  The fallback covers filesystems
        // that do not replace an existing file during rename.
        if to.exists() {
            fs::remove_file(to)?;
            fs::rename(&temp, to)?;
        } else {
            return Err(error).with_context(|| format!("installing {}", to.display()));
        }
    }
    Ok(())
}
