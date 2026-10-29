//! The catalog: every file a client may read, as the server has it.
//!
//! The engine's files (`data/…`, its `/game`) come first: their index is the
//! file id the engine uses (the manifest it gets lists them in this order).
//! The build's translated shaders (`b/<build>/shaders/…`) follow; the
//! client keeps them as plain files for the renderer.

use std::collections::HashMap;
use std::fs;
use std::io;
use std::path::Path;
use std::time::UNIX_EPOCH;

use anyhow::{Context, Result, bail};
use sha2::{Digest, Sha256};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    /// Relative to the game folder, `/`-separated (`data/x64/…`).
    pub path: String,
    pub size: u64,
    /// Seconds since the epoch.
    pub mtime: u64,
}

pub struct Catalog {
    pub build: String,
    pub entries: Vec<Entry>,
    /// The first `data_files` entries are the engine's.
    pub data_files: usize,
    /// Changes whenever a path, size or time does.
    pub hash: String,
    /// The dates the engine is told for its files (the first `data_files`;
    /// none: their times), see [`game_dates`]. Not in the hash: the copy's
    /// bytes are the same whatever date the engine is told.
    pub dates: Vec<u64>,
    lookup: HashMap<String, u32>,
}

/// Files of the whale5 web build's server beside the game's data, not
/// game files.
const NOT_GAME: [&str; 3] = ["manifest.json", "bootset.json", "bootset_low.json"];

/// What a catalog from elsewhere (a server, which any device on the
/// network can pretend to be; a copy's file) may hold, far above the
/// game's 12,500 files and 21 GB: the copy makes every file's entry and
/// bitmap of blocks before it reads any.
const MAX_FILES: usize = 200_000;
const MAX_BYTES: u64 = 1 << 40;
/// A path's length (the copy's index keeps it in 16 bits).
const MAX_PATH: usize = 1024;

impl Catalog {
    /// The game folder `game` (`data/` and `b/<build>/shaders/`).
    pub fn scan(game: &Path, build: &str) -> Result<Catalog> {
        let mut data = Vec::new();
        walk(&game.join("data"), "data", &mut data).with_context(|| format!("listing {}", game.join("data").display()))?;
        data.retain(|e| !NOT_GAME.iter().any(|n| e.path.strip_prefix("data/") == Some(*n)));
        data.sort_by(|a, b| a.path.cmp(&b.path));
        let shader_dir = format!("b/{build}/shaders");
        let mut shaders = Vec::new();
        walk(&game.join(&shader_dir), &shader_dir, &mut shaders)
            .with_context(|| format!("listing {}", game.join(&shader_dir).display()))?;
        // The packs repeat the shaders for the browser; the renderer reads the files.
        shaders.retain(|e| !e.path.ends_with(".bin"));
        shaders.sort_by(|a, b| a.path.cmp(&b.path));
        if !shaders.iter().any(|e| e.path.ends_with("/index.json")) {
            bail!("{}/index.json is missing", game.join(&shader_dir).display());
        }
        let data_files = data.len();
        let game = game_dates(&game.join("data"));
        let dates = data
            .iter()
            .map(|e| date(&game, e.path.strip_prefix("data/").unwrap_or(&e.path), e.size).unwrap_or(e.mtime))
            .collect();
        data.extend(shaders);
        let catalog = Catalog { dates, ..Catalog::new(build.to_string(), data, data_files) };
        if let Some((a, b)) = catalog.same_but_case() {
            bail!("{a} and {b} differ only in case: one file where the game plays (rename or remove one)");
        }
        Ok(catalog)
    }

    pub fn new(build: String, entries: Vec<Entry>, data_files: usize) -> Catalog {
        let mut sha = Sha256::new();
        sha.update(build.as_bytes());
        for e in &entries {
            sha.update(e.path.as_bytes());
            sha.update([0]);
            sha.update(e.size.to_le_bytes());
            sha.update(e.mtime.to_le_bytes());
        }
        let hash = hex(&sha.finalize()[..8]);
        let lookup = entries.iter().enumerate().map(|(i, e)| (e.path.to_ascii_lowercase(), i as u32)).collect();
        Catalog { build, entries, data_files, hash, dates: Vec::new(), lookup }
    }

    /// Two of its paths the same but for case: one file on the volumes
    /// that ignore case (Apple's, Windows'), and to the engine, which
    /// looks paths up in any case.
    fn same_but_case(&self) -> Option<(&str, &str)> {
        if self.lookup.len() == self.entries.len() {
            return None;
        }
        // (The lookup has the last of them.)
        self.entries.iter().enumerate().find_map(|(i, e)| {
            let last = self.lookup[&e.path.to_ascii_lowercase()] as usize;
            (last != i).then(|| (e.path.as_str(), self.entries[last].path.as_str()))
        })
    }

    /// The id of the file at `path` (relative to the game folder, any case).
    pub fn find(&self, path: &str) -> Option<u32> {
        self.lookup.get(&path.to_ascii_lowercase()).copied()
    }

    pub fn to_json(&self) -> Vec<u8> {
        let files: Vec<_> = self.entries.iter().map(|e| (&e.path, e.size, e.mtime)).collect();
        serde_json::to_vec(&serde_json::json!({
            "build": self.build, "data_files": self.data_files, "files": files, "dates": self.dates,
        }))
        .expect("catalog json")
    }

    /// A catalog as [`Catalog::to_json`] wrote it, from elsewhere: only
    /// what [`Catalog::scan`] could list, within [`MAX_FILES`] and
    /// [`MAX_BYTES`].
    pub fn from_json(bytes: &[u8]) -> Result<Catalog> {
        let v: serde_json::Value = serde_json::from_slice(bytes).context("parsing the catalog")?;
        let build = v["build"].as_str().context("catalog without build")?.to_string();
        let data_files = v["data_files"].as_u64().context("catalog without data_files")?;
        let files = v["files"].as_array().context("catalog without files")?;
        if files.len() > MAX_FILES || data_files > files.len() as u64 {
            bail!("catalog: {} files, {data_files} of them data", files.len());
        }
        let data_files = data_files as usize;
        let shaders = format!("b/{build}/shaders/");
        let (mut entries, mut total) = (Vec::with_capacity(files.len()), 0u64);
        for (i, f) in files.iter().enumerate() {
            let (Some(path), Some(size), Some(mtime)) = (f[0].as_str(), f[1].as_u64(), f[2].as_u64()) else {
                bail!("bad catalog entry {f}");
            };
            if !safe_path(path) || path.len() > MAX_PATH {
                bail!("catalog path {path:?} leaves the game folder");
            }
            // The engine's files, then the shaders: nothing else of the
            // copy's folder (its own files) is a catalog's.
            let listed = match path.strip_prefix("data/") {
                Some(rel) => i < data_files && !NOT_GAME.contains(&rel),
                None => i >= data_files && path.starts_with(&shaders),
            };
            if !listed {
                bail!("catalog path {path:?} is not a game file's");
            }
            total = total.checked_add(size).filter(|&t| t <= MAX_BYTES).context("catalog: too many bytes")?;
            if mtime > LAST_DATE {
                bail!("catalog: {path} dated {mtime}");
            }
            entries.push(Entry { path: path.to_string(), size, mtime });
        }
        // (A catalog of an earlier version has none.)
        let dates: Vec<u64> =
            v["dates"].as_array().map_or(Vec::new(), |d| d.iter().filter_map(|t| t.as_u64()).collect());
        if !dates.is_empty() && dates.len() != data_files {
            bail!("catalog: {} dates for {data_files} data files", dates.len());
        }
        if let Some(t) = dates.iter().find(|&&t| t > LAST_DATE) {
            bail!("catalog: a file dated {t}");
        }
        let catalog = Catalog { dates, ..Catalog::new(build, entries, data_files) };
        if let Some((a, b)) = catalog.same_but_case() {
            bail!("catalog: {a} and {b} differ only in case");
        }
        Ok(catalog)
    }

    /// The manifest the engine reads (`wasm_httpfs_manifest_js`), in the
    /// whale5 server's format: `/game/` mounted on `data/`.
    pub fn engine_manifest(&self) -> Vec<u8> {
        let files: Vec<_> = self.entries[..self.data_files]
            .iter()
            .enumerate()
            .map(|(i, e)| {
                (e.path.strip_prefix("data/").unwrap_or(&e.path), e.size, *self.dates.get(i).unwrap_or(&e.mtime))
            })
            .collect();
        serde_json::to_vec(&serde_json::json!({ "mount": "/game/", "version": self.hash, "files": files }))
            .expect("manifest json")
    }
}

/// The game's files as the whale5 server listed them, by lowercase path
/// under `data/`: (size, date in seconds), from its manifest
/// (`data/manifest.json`, `files` of [path, size, date]). The engine's map
/// caches (`common/data/gta5_cache_*.dat`) hold these dates: told others
/// (a copy's), it builds the cache again at every start (seconds) and
/// cannot save it. Empty without a manifest; a date past [`LAST_DATE`] is
/// no date.
pub fn game_dates(data: &Path) -> HashMap<String, (u64, u64)> {
    let Ok(bytes) = fs::read(data.join("manifest.json")) else { return HashMap::new() };
    let Ok(manifest) = serde_json::from_slice::<serde_json::Value>(&bytes) else {
        log::warn!("{}: not a manifest", data.join("manifest.json").display());
        return HashMap::new();
    };
    let files = manifest["files"].as_array().map_or(&[][..], Vec::as_slice);
    files
        .iter()
        .filter_map(|f| Some((f[0].as_str()?.to_ascii_lowercase(), (f[1].as_u64()?, f[2].as_u64()?))))
        .filter(|(_, (_, date))| *date <= LAST_DATE)
        .collect()
}

/// The last date a file can have: 9999-12-31 (seconds since the epoch).
/// A later one is a damaged value, which the system's times (and the
/// engine's) may not hold.
pub const LAST_DATE: u64 = 253_402_300_799;

/// The date of the file at `rel` (under `data/`) of `size` bytes, if the
/// manifest lists it so: a file changed since is not the one it lists.
pub fn date(dates: &HashMap<String, (u64, u64)>, rel: &str, size: u64) -> Option<u64> {
    dates.get(&rel.to_ascii_lowercase()).filter(|(s, _)| *s == size).map(|&(_, t)| t)
}

fn walk(dir: &Path, rel: &str, out: &mut Vec<Entry>) -> io::Result<()> {
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.starts_with('.') {
            continue;
        }
        let path = format!("{rel}/{name}");
        let meta = fs::metadata(entry.path())?;
        if meta.is_dir() {
            walk(&entry.path(), &path, out)?;
        } else if meta.is_file() {
            let mtime = meta.modified().ok().and_then(|t| t.duration_since(UNIX_EPOCH).ok()).map_or(0, |d| d.as_secs());
            out.push(Entry { path, size: meta.len(), mtime });
        }
    }
    Ok(())
}

/// Relative, `/`-separated, without `.`/`..`/empty parts or a drive.
pub fn safe_path(path: &str) -> bool {
    !path.is_empty()
        && !path.contains('\\')
        && !path.contains(':')
        && path.split('/').all(|p| !p.is_empty() && p != "." && p != "..")
}

pub fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

#[test]
fn manifest_dates() {
    let dir = std::env::temp_dir().join(format!("glue5-catalog-dates-{}", std::process::id()));
    fs::create_dir_all(&dir).unwrap();
    let manifest = br#"{"mount":"/game/","files":[["x64/A.rpf",10,1790644442],["b.xml",3,9],["c.rpf",1,18446744073709551615]]}"#;
    fs::write(dir.join("manifest.json"), manifest).unwrap();
    let dates = game_dates(&dir);
    assert_eq!(date(&dates, "x64/a.rpf", 10), Some(1790644442));
    // Past any date a file can have.
    assert_eq!(date(&dates, "c.rpf", 1), None);
    // Changed since (another size), or not listed.
    assert_eq!(date(&dates, "b.xml", 4), None);
    assert_eq!(date(&dates, "c.xml", 3), None);
    assert!(game_dates(&dir.join("none")).is_empty());
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn round_trip() {
    let entries = vec![
        Entry { path: "data/common/a.xml".into(), size: 3, mtime: 7 },
        Entry { path: "b/x/shaders/index.json".into(), size: 2, mtime: 9 },
    ];
    let c = Catalog::new("x".into(), entries.clone(), 1);
    let back = Catalog::from_json(&c.to_json()).unwrap();
    assert_eq!(back.entries, entries);
    assert_eq!(back.hash, c.hash);
    assert_eq!(back.find("DATA/Common/A.xml"), Some(0));
    let m: serde_json::Value = serde_json::from_slice(&c.engine_manifest()).unwrap();
    assert_eq!(m["files"][0][0], "common/a.xml");
    assert_eq!(m["files"][0][2], 7);
    assert_eq!(m["files"].as_array().unwrap().len(), 1);
    // The engine's dates: told it, kept, out of the hash.
    let dated = Catalog { dates: vec![5], ..Catalog::new("x".into(), entries.clone(), 1) };
    let back = Catalog::from_json(&dated.to_json()).unwrap();
    assert_eq!((back.dates, back.hash), (vec![5], c.hash.clone()));
    let m: serde_json::Value = serde_json::from_slice(&dated.engine_manifest()).unwrap();
    assert_eq!(m["files"][0][2], 5);
    assert!(!safe_path("data/../x") && !safe_path("/etc") && !safe_path("c:/x") && safe_path("data/x"));
}

/// A catalog from elsewhere holds only what a game folder's could, within
/// bounds a device can hold.
#[test]
fn untrusted_catalogs() {
    let json = |files: serde_json::Value, data_files: usize| {
        serde_json::to_vec(&serde_json::json!({ "build": "x", "data_files": data_files, "files": files })).unwrap()
    };
    let ok = serde_json::json!([["data/a.rpf", 1, 2], ["b/x/shaders/index.json", 3, 4]]);
    assert!(Catalog::from_json(&json(ok, 1)).is_ok());
    let refused = |files, data_files| Catalog::from_json(&json(files, data_files)).is_err();
    // Sizes past what any volume holds, or adding up past it.
    assert!(refused(serde_json::json!([["data/a.rpf", u64::MAX, 2]]), 1));
    assert!(refused(serde_json::json!([["data/a.rpf", 1u64 << 39, 2], ["data/b.rpf", 1u64 << 39, 2], ["data/c", 1, 2]]), 3));
    // More files than a catalog may list.
    let many: Vec<_> = (0..=MAX_FILES).map(|i| serde_json::json!([format!("data/{i}"), 0, 0])).collect();
    assert!(refused(serde_json::Value::Array(many), MAX_FILES + 1));
    // The copy's own files, files of no game folder, a server's manifest.
    assert!(refused(serde_json::json!([["index", 1, 2]]), 0));
    assert!(refused(serde_json::json!([["catalog.json", 1, 2]]), 0));
    assert!(refused(serde_json::json!([["data/manifest.json", 1, 2]]), 1));
    assert!(refused(serde_json::json!([["b/x/shaders/a", 1, 2]]), 1));
    assert!(refused(serde_json::json!([["data/a", 1, 2]]), 0));
    // A date past any a file can have.
    assert!(refused(serde_json::json!([["data/a", 1, u64::MAX]]), 1));
    // One file on many a device.
    assert!(refused(serde_json::json!([["data/A.rpf", 1, 2], ["data/a.rpf", 1, 2]]), 2));
    assert!(refused(serde_json::json!([["data/a.rpf", 1, 2], ["data/a.rpf", 1, 2]]), 2));
}

/// A game folder with two paths the same but for case (a case-sensitive
/// volume's) is not served.
#[test]
fn refuses_paths_the_same_but_for_case() {
    let dir = std::env::temp_dir().join(format!("glue5-catalog-case-{}", std::process::id()));
    fs::create_dir_all(dir.join("data/x64")).unwrap();
    fs::create_dir_all(dir.join("b/x/shaders")).unwrap();
    fs::write(dir.join("b/x/shaders/index.json"), b"{}").unwrap();
    fs::write(dir.join("data/x64/a.rpf"), b"a").unwrap();
    assert!(Catalog::scan(&dir, "x").is_ok());
    fs::create_dir_all(dir.join("data/X64")).unwrap();
    fs::write(dir.join("data/X64/a.rpf"), b"b").unwrap();
    // (Not on a volume that ignores case: one folder.)
    if fs::read_dir(dir.join("data")).unwrap().count() == 2 {
        let e = Catalog::scan(&dir, "x").err().unwrap().to_string();
        assert!(e.contains("differ only in case"), "{e}");
    }
    let _ = fs::remove_dir_all(dir);
}
