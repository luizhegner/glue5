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
    use jni::objects::{JObject, JValue};
    use jni::JNIEnv;

    unsafe extern "C" {
        fn SDL_AndroidGetJNIEnv() -> *mut jni::sys::JNIEnv;
    }

    let raw = unsafe { SDL_AndroidGetJNIEnv() };
    if raw.is_null() {
        bail!("Android did not provide a JNI environment");
    }
    let mut env = unsafe { JNIEnv::from_raw(raw) }.context("opening Android's JNI environment")?;
    let class = env.find_class("local/glue5/Glue5Activity").context("finding Glue5Activity")?;
    let value = JObject::from(env.new_string(kind.name()).context("encoding import type")?);
    env.call_static_method(class, "startImport", "(Ljava/lang/String;)V", &[JValue::Object(&value)])
        .context("asking Android to open the folder picker")?;
    Ok(())
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
        let profiles = user_dir.join("Documents/Rockstar Games/GTA V/Profiles");
        let source_name = source_name(stage);
        let direct_profile = has_save_file(stage)?;
        let destination = if direct_profile { profiles.join(&source_name) } else { profiles };
        let backup = paths::imports_dir(user_dir).join("backups").join(label).join(&source_name);
        let files = copy_tree(stage, &destination, Some(&backup))?;
        return Ok(format!("Imported {files} save file(s). Restart Story Mode before loading the profile."));
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

fn has_save_file(dir: &Path) -> Result<bool> {
    for entry in fs::read_dir(dir).with_context(|| format!("reading {}", dir.display()))? {
        let entry = entry?;
        if entry.file_type()?.is_file() && entry.file_name().to_string_lossy().starts_with("SGTA5") {
            return Ok(true);
        }
    }
    Ok(false)
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
