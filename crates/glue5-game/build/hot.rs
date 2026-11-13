//! The game's hot functions together: wasm2c spreads the game's functions
//! over its 64 files by name, so the ones a frame runs lie scattered over
//! ~57 MB of code (instruction cache and TLB misses at every call). With a
//! profile (`GLUE5_GAME_PGO=<file.profdata>`, see optimise.rs) its list of
//! functions run (`hot.txt` beside it, the most called first, as `mise run
//! pgo` writes it) moves their definitions, in that order, into files of
//! their own (`game_hot_N.c`), which the linker keeps together.
//!
//! Every game file but the first holds only definitions (all declared in
//! game-impl.h), so they can move; not from the first (the data, the tables,
//! static wrappers) nor those calling a file's static helpers (page_map.rs).

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::Result;

/// C text per hot file (as the other files' ~15 MB).
const HOT_FILE: usize = 15 << 20;

/// Moves the functions named in `hot` from the sources' texts (but the
/// first's) into new files in `gen_dir`, returned; the texts moved from
/// become `Some`.
pub fn gather(
    hot: &[String],
    sources: &[PathBuf],
    texts: &mut [Option<String>],
    gen_dir: &Path,
) -> Result<Vec<PathBuf>> {
    let rank: HashMap<&str, usize> = hot.iter().enumerate().map(|(i, name)| (name.as_str(), i)).collect();
    let mut moved: Vec<(usize, String)> = Vec::new();
    for (source, text) in sources.iter().zip(texts.iter_mut()).skip(1) {
        let current = match text {
            Some(t) => std::mem::take(t),
            None => fs::read_to_string(source)?,
        };
        let statics: Vec<&str> =
            current.lines().filter_map(|l| l.strip_prefix("static ")).filter_map(name_of).collect();
        let mut kept = String::with_capacity(current.len());
        let mut taken = false;
        for item in items(&current) {
            let (header, definition) = split_header(item);
            match name_of(definition).and_then(|name| rank.get(name)) {
                Some(&r) if !statics.iter().any(|s| definition.contains(s)) => {
                    kept.push_str(header);
                    moved.push((r, definition.to_owned()));
                    taken = true;
                }
                _ => kept.push_str(item),
            }
        }
        if taken {
            *text = Some(kept);
        } else if text.is_some() {
            *text = Some(current);
        }
    }
    moved.sort_by_key(|m| m.0);
    let mut files = Vec::new();
    let mut text = String::new();
    for (i, (_, definition)) in moved.iter().enumerate() {
        text.push_str(definition);
        if text.len() >= HOT_FILE || i + 1 == moved.len() {
            let path = gen_dir.join(format!("game_hot_{}.c", files.len()));
            let file = format!("/* The game's hot functions (hot.rs) */\n#include \"game-impl.h\"\n\n{text}");
            crate::write_if_changed(&path, &file)?;
            files.push(path);
            text.clear();
        }
    }
    eprintln!("hot functions: {} moved into {} files", moved.len(), files.len());
    Ok(files)
}

/// A file's text in pieces, each ending with a definition's closing `}`
/// line (and the blank lines after it); the last, what follows.
fn items(text: &str) -> impl Iterator<Item = &str> {
    let mut at = 0;
    std::iter::from_fn(move || {
        let rest = &text[at..];
        if rest.is_empty() {
            return None;
        }
        let end = rest.find("\n}\n").map_or(rest.len(), |e| e + 3);
        let end = end + rest[end..].bytes().take_while(|b| *b == b'\n').count();
        at += end;
        Some(&rest[..end])
    })
}

/// A piece's leading comment, preprocessor and blank lines (the file's
/// header, in its first piece), and the definition after them.
fn split_header(item: &str) -> (&str, &str) {
    let mut at = 0;
    for line in item.split_inclusive('\n') {
        let t = line.trim_start();
        if !(t.is_empty() || t.starts_with("/*") || t.starts_with('#')) {
            break;
        }
        at += line.len();
    }
    item.split_at(at)
}

/// The function a definition (or a static's line) defines: the word before
/// its first `(`, on a line taking the instance.
fn name_of(definition: &str) -> Option<&str> {
    let first = definition.lines().next()?;
    if !first.contains("(w2c_game* instance") {
        return None;
    }
    let name = first[..first.find('(')?].rsplit(' ').next()?;
    (!name.is_empty()).then_some(name)
}
