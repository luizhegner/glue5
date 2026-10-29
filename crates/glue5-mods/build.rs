//! Generates the natives' table (`natives.rs`) and their Luau definitions
//! (`natives.d.luau`) from what game.wasm says about each native (glue5-game's
//! build script, `DEP_GLUE5_GAME_NATIVES`) and, when present, the natives
//! database (`third_party/nativedb/natives.json`, from `mise run setup`):
//! names of parameters, documentation, the handle types.
//!
//! The binary decides what can be called and how (handler, slots); the
//! database adds what the binary does not say. Without the database the
//! natives with a typed command function are still there, with parameters
//! named `p0`, `p1`...

use std::collections::{BTreeMap, HashMap};
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::{env, fs};

use anyhow::{Context, Result};
use serde_json::Value;

#[path = "build/constants.rs"]
mod constants;

fn main() {
    if let Err(e) = run() {
        eprintln!("error: {e:#}");
        std::process::exit(1);
    }
}

/// What the generated code calls a value (glue5-mods' `Kind`).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Kind {
    Int,
    Float,
    Bool,
    Hash,
    Str,
    Vec3,
    Any,
    Handle(Handle),
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[allow(clippy::enum_variant_names)] // ScrHandle: the database's name
enum Handle {
    Entity,
    Ped,
    Vehicle,
    Object,
    Player,
    Cam,
    Blip,
    Pickup,
    Interior,
    ScrHandle,
    FireId,
}

const HANDLES: &[(&str, Handle, Option<&str>)] = &[
    ("Entity", Handle::Entity, None),
    ("Ped", Handle::Ped, Some("Entity")),
    ("Vehicle", Handle::Vehicle, Some("Entity")),
    ("Object", Handle::Object, Some("Entity")),
    ("Player", Handle::Player, None),
    ("Cam", Handle::Cam, None),
    ("Blip", Handle::Blip, None),
    ("Pickup", Handle::Pickup, None),
    ("Interior", Handle::Interior, None),
    ("ScrHandle", Handle::ScrHandle, None),
    ("FireId", Handle::FireId, None),
];

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Dir {
    In,
    Out,
    InOut,
}

#[derive(Clone, Debug)]
struct Param {
    name: String,
    kind: Kind,
    dir: Dir,
}

impl Param {
    fn slots(&self) -> usize {
        if self.kind == Kind::Vec3 && self.dir == Dir::In { 3 } else { 1 }
    }
}

struct Native {
    name: String,
    hash: u64,
    /// The database's hash (the one ScriptHookV's mods use), 0 if none.
    db_hash: u64,
    handler: u64,
    namespace: String,
    lua: String,
    params: Vec<Param>,
    ret: Option<Kind>,
    doc: String,
}

/// One line of glue5-game's table.
struct Binary {
    name: String,
    hash: u64,
    handler: u64,
    unused: bool,
    group: String,
    command: Option<Vec<String>>,
    /// The typed command's wasm result: "", "i32", "f32", "i64"; "sret"
    /// for a vector returned through a pointer.
    result: Option<String>,
    signature: u64,
}

/// Natives whose handle is never none: the player's ped.
const NEVER_NONE: &[&str] = &["PLAYER_PED_ID", "GET_PLAYER_PED", "GET_PLAYER_PED_SCRIPT_INDEX"];

/// Natives the mods may not call: the script thread's own control (a mod's
/// thread only waits through `wait`, and ends through `mod`).
const RESERVED: &[&str] = &["WAIT", "TERMINATE_THIS_THREAD"];

/// Natives that keep the pointer they are given past the call: a mod's
/// arguments are on the stack only while the native runs. Save blocks
/// (their ends too: nothing a mod can open), variables shared over the
/// network, variables the anti-cheat watches.
fn keeps_pointer(name: &str) -> bool {
    name.starts_with("START_SAVE_")
        || name.starts_with("STOP_SAVE_")
        || name.ends_with("_TO_SAVE")
        || (name.starts_with("NETWORK_REGISTER_") && name.ends_with("_BROADCAST_VARIABLES"))
        || name == "REGISTER_SCRIPT_VARIABLE"
        || name == "UNREGISTER_SCRIPT_VARIABLE"
}

/// Natives that read a structure through what the database calls an
/// `int*` (`CTaskScriptedAnimation::ScriptInitSlotData`): one value would
/// leave them reading past it.
const STRUCTURES: &[&str] = &["TASK_SCRIPTED_ANIMATION", "PLAY_ENTITY_SCRIPTED_ANIM"];

fn run() -> Result<()> {
    let manifest = PathBuf::from(env::var("CARGO_MANIFEST_DIR")?);
    let root = manifest.parent().and_then(Path::parent).context("workspace root")?.to_path_buf();
    let out = PathBuf::from(env::var("OUT_DIR")?);
    let table = PathBuf::from(env::var("DEP_GLUE5_GAME_NATIVES").context("glue5-game's natives table")?);
    let db_path = root.join("third_party/nativedb/natives.json");
    println!("cargo:rerun-if-changed={}", table.display());
    println!("cargo:rerun-if-changed={}", db_path.display());
    println!("cargo:rerun-if-changed=build.rs");

    let binary = parse_table(&fs::read_to_string(&table)?)?;
    let db: Option<Value> = match fs::read(&db_path) {
        Ok(bytes) => Some(serde_json::from_slice(&bytes).context("parsing natives.json")?),
        Err(_) => {
            println!(
                "cargo:warning=no {} (`mise run setup`): natives without documentation or names of parameters",
                db_path.display()
            );
            None
        }
    };
    // The database's entries by name, with their namespace and hash.
    let mut by_name: HashMap<String, (String, &Value, u64)> = HashMap::new();
    if let Some(Value::Object(spaces)) = &db {
        for (space, entries) in spaces {
            if let Value::Object(entries) = entries {
                for (hash, entry) in entries {
                    if let Some(name) = entry["name"].as_str() {
                        let hash = u64::from_str_radix(hash.trim_start_matches("0x"), 16)
                            .with_context(|| format!("natives.json: the hash of {name}"))?;
                        by_name.insert(name.to_string(), (space.clone(), entry, hash));
                    }
                }
            }
        }
    }
    // Each of the binary's groups takes the namespace most of its natives
    // have in the database (camera -> CAM), so the names match the docs.
    let mut votes: HashMap<&str, HashMap<&str, usize>> = HashMap::new();
    for b in &binary {
        if let Some((space, ..)) = by_name.get(&b.name) {
            *votes.entry(&b.group).or_default().entry(space).or_default() += 1;
        }
    }
    let namespace_of = |group: &str| -> String {
        votes
            .get(group)
            .and_then(|v| v.iter().max_by_key(|&(name, n)| (*n, std::cmp::Reverse(*name))).map(|(s, _)| s.to_ascii_lowercase()))
            .unwrap_or_else(|| group.to_string())
    };

    let mut natives = Vec::new();
    let mut skipped = BTreeMap::<&str, usize>::new();
    let mut reserved = Vec::new();
    // The natives left out, with the database's hash and why, for
    // `native.invoke`'s errors.
    let mut excluded: Vec<(String, u64, &str)> = Vec::new();
    for b in &binary {
        let db_hash = by_name.get(&b.name).map_or(0, |&(_, _, h)| h);
        let mut skip = |why: &'static str| {
            *skipped.entry(why).or_default() += 1;
            excluded.push((b.name.clone(), db_hash, why));
        };
        if RESERVED.contains(&b.name.as_str()) {
            reserved.push((b.name.clone(), b.handler));
            skip("reserved");
            continue;
        }
        if keeps_pointer(&b.name) || STRUCTURES.contains(&b.name.as_str()) {
            skip("pointers kept or structures");
            continue;
        }
        let entry = by_name.get(&b.name).map(|(_, e, _)| *e);
        let from_db = entry.map(|e| db_params(e, &b.name)).transpose()?;
        let script = script_params(b.signature);
        // The typed command may be a helper the handler calls with values
        // of its own (DRAW_SCALEFORM_MOVIE's and _FULLSCREEN's share one of
        // 11): where it takes another count, the script signature's.
        let helper = matches!((&b.command, &script), (Some(c), Some(s)) if c.len() != s.len());
        let from_binary = b.command.as_ref().filter(|_| !helper).map(|c| binary_params(c, &b.name));
        // A pointer the database cannot type (`Any*`: a structure, an array,
        // a gamer handle) where the binary takes an `int&`, which is also
        // how the commands take the scripts' structures: its size is
        // unknown, so a buffer of one value could be written past
        // (COPY_SCRIPT_STRUCT, GET_EVENT_DATA, NETWORK_HANDLE_FROM_PLAYER).
        // A `float&` or a `Vector3&` is one value; where the binary takes a
        // value, the database is wrong and the binary is right (ADD_ROPE).
        let untyped = entry.is_some_and(untyped_pointer);
        let mut params = match (from_db.clone(), from_binary) {
            (Some(Some(db)), Some(Some(bin))) => merge(db, &bin),
            (Some(Some(db)), _) => db,
            (_, Some(Some(bin))) if untyped && bin.iter().any(|p| p.dir != Dir::In && p.kind == Kind::Int) => {
                skip("untyped pointers");
                continue;
            }
            (_, Some(Some(bin))) => bin,
            _ => {
                skip(if b.unused { "unused, untyped" } else { "untyped" });
                continue;
            }
        };
        // The database may disagree with what the handler reads too
        // (NETWORK_SPENT_TAXI's five for four): the script signature's
        // values, with the database's names and finer types where they fit.
        if let Some(s) = &script
            && (helper || !fits(&params, s))
        {
            let db = from_db.flatten();
            // A pointer the database does not type: a buffer of one value
            // could be written past (native.invoke still reaches it).
            if pointers_untyped(db.as_deref(), s) {
                skip("untyped pointers");
                continue;
            }
            params = match db {
                Some(db) => vectors_out(merge(db.clone(), s), &db),
                None => s.clone(),
            };
        }
        let ret = match entry.and_then(|e| e["return_type"].as_str()) {
            Some(t) => match value_kind(t) {
                Some(k) => Some(k),
                None if t == "void" => None,
                None => {
                    skip("return type");
                    continue;
                }
            },
            None => match binary_return(b) {
                Some(r) => r,
                None => {
                    skip("return type");
                    continue;
                }
            },
        };
        let doc = entry.and_then(|e| e["comment"].as_str()).unwrap_or("").to_string();
        natives.push(Native {
            name: b.name.clone(),
            hash: b.hash,
            db_hash,
            handler: b.handler,
            namespace: namespace_of(&b.group),
            lua: camel(&b.name),
            params,
            ret,
            doc,
        });
    }
    natives.sort_by(|a, b| (&a.namespace, &a.lua).cmp(&(&b.namespace, &b.lua)));
    // The database's natives this build does not have (ScriptHookV's mods
    // may name them).
    let in_build: std::collections::HashSet<&str> = binary.iter().map(|b| b.name.as_str()).collect();
    let mut missing: Vec<(String, u64, &str)> = by_name
        .iter()
        .filter(|(name, _)| !in_build.contains(name.as_str()))
        .map(|(name, &(_, _, hash))| (name.clone(), hash, "not in this build"))
        .collect();
    excluded.append(&mut missing);
    excluded.sort();
    // The left-out natives `native.invoke` still calls, ScriptHookV's way:
    // pointers as addresses (all but the script thread's own).
    let binary_by_name: HashMap<&str, &Binary> = binary.iter().map(|b| (b.name.as_str(), b)).collect();
    let mut raw = Vec::new();
    excluded.retain(|(name, db_hash, why)| {
        let Some(b) = binary_by_name.get(name.as_str()).filter(|_| *why != "reserved") else { return true };
        let entry = by_name.get(name).map(|(_, e, _)| *e);
        let Some((params, ret)) = raw_signature(entry, b) else { return true };
        let doc = entry.and_then(|e| e["comment"].as_str()).unwrap_or("").to_string();
        let namespace = namespace_of(&b.group);
        raw.push(Native { name: name.clone(), hash: b.hash, db_hash: *db_hash, handler: b.handler, namespace, lua: camel(name), params, ret, doc });
        false
    });
    let skipped_text = skipped.iter().map(|(why, n)| format!("{n} {why}")).collect::<Vec<_>>().join(", ");
    println!("cargo:rustc-env=GLUE5_MODS_NATIVES_SUMMARY={} natives; not exposed: {skipped_text}", natives.len());

    fs::write(out.join("natives.rs"), rust_table(&natives, &raw, &reserved, &excluded))?;
    fs::write(out.join("natives.d.luau"), definitions(&natives))?;
    constants::write(&out, db.as_ref())?;
    checker(&root)?;
    let web = studio_files(&root, &out)?;
    assets(&out, &web)?;
    bundled(&root, &out)?;
    Ok(())
}

/// The mods that come with GLUE 5, the repository's `mods/` (a folder each,
/// with a main.luau): their files built in (`bundled.rs`), which
/// glue5-mods' bundled.rs copies into the player's mods folder.
fn bundled(root: &Path, out: &Path) -> Result<()> {
    fn walk(root: &Path, at: &Path, files: &mut Vec<(String, PathBuf)>) -> Result<()> {
        for entry in fs::read_dir(at)? {
            let path = entry?.path();
            let name = path.file_name().unwrap_or_default().to_string_lossy();
            if name.starts_with('.') {
                continue;
            }
            if path.is_dir() {
                walk(root, &path, files)?;
            } else {
                let rel = path.strip_prefix(root)?.to_string_lossy().replace('\\', "/");
                files.push((rel, path));
            }
        }
        Ok(())
    }
    let dir = root.join("mods");
    // A folder's whole tree, files added or removed too.
    println!("cargo:rerun-if-changed={}", dir.display());
    let mut s = String::from("pub static BUNDLED: &[(&str, &[(&str, &[u8])])] = &[\n");
    let mut names: Vec<PathBuf> = fs::read_dir(&dir)?.map(|e| e.map(|e| e.path())).collect::<Result<_, _>>()?;
    names.sort();
    for m in names.iter().filter(|m| m.join("main.luau").is_file()) {
        let name = m.file_name().unwrap_or_default().to_string_lossy();
        anyhow::ensure!(
            name.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-' || b == b'_'),
            "mods/{name}: a mod's name is lowercase letters, digits, - and _"
        );
        let mut files = Vec::new();
        walk(m, m, &mut files)?;
        files.sort();
        writeln!(s, "    ({name:?}, &[")?;
        for (rel, path) in files {
            writeln!(s, "        ({rel:?}, include_bytes!({:?})),", path.display().to_string())?;
        }
        writeln!(s, "    ]),")?;
    }
    s.push_str("];\n");
    fs::write(out.join("bundled.rs"), s)?;
    Ok(())
}

/// The Studio's files, published path and contents: crates/glue5-studio
/// built for wasm (a cargo of its own, with its own target directory, then
/// wasm-bindgen), its page and style, the editor's modules from
/// `third_party/codemirror` (an import map in the page names them), the
/// app's fonts.
fn studio_files(root: &Path, out: &Path) -> Result<Vec<(String, Vec<u8>)>> {
    let studio = root.join("crates/glue5-studio");
    let codemirror = root.join("third_party/codemirror");
    for p in ["src", "js", "web", "Cargo.toml", "Cargo.lock", "packages.txt"] {
        println!("cargo:rerun-if-changed={}", studio.join(p).display());
    }
    println!("cargo:rerun-if-changed={}", codemirror.join(".glue5-stamp").display());
    anyhow::ensure!(codemirror.join(".glue5-stamp").exists(), "{} is missing: run `mise run setup`", codemirror.display());

    // The wasm: a cargo of its own (the outer one holds this target
    // directory), without this build's flags, which are for its target.
    // (A relative target directory is cargo's, from the workspace root.)
    let target_dir = env::var_os("CARGO_TARGET_DIR")
        .map(|t| root.join(t))
        .unwrap_or_else(|| root.join("target"))
        .join("glue5-tools/studio");
    let cargo = env::var_os("CARGO").unwrap_or_else(|| "cargo".into());
    let mut command = std::process::Command::new(cargo);
    command
        .args(["build", "--release", "--target", "wasm32-unknown-unknown", "--manifest-path"])
        .arg(studio.join("Cargo.toml"))
        .arg("--target-dir")
        .arg(&target_dir);
    for (key, _) in env::vars_os() {
        let key = key.to_string_lossy();
        if key.starts_with("CARGO_") && key != "CARGO_HOME" || key == "RUSTFLAGS" || key == "RUSTC_WORKSPACE_WRAPPER" {
            command.env_remove(key.as_ref());
        }
    }
    let status = command.status().context("building glue5-studio")?;
    anyhow::ensure!(status.success(), "building glue5-studio (wasm) failed");
    let wasm = target_dir.join("wasm32-unknown-unknown/release/glue5_studio.wasm");
    let bindgen = out.join("studio");
    let _ = fs::remove_dir_all(&bindgen);
    wasm_bindgen_cli_support::Bindgen::new()
        .input_path(&wasm)
        .out_name("studio")
        .web(true)?
        .typescript(false)
        .debug(false)
        .keep_debug(false)
        .generate(&bindgen)?;

    let mut files = Vec::new();
    let add_tree = |dir: &Path, prefix: &str, files: &mut Vec<(String, Vec<u8>)>| -> Result<()> {
        let mut stack = vec![dir.to_path_buf()];
        while let Some(d) = stack.pop() {
            for e in fs::read_dir(&d)? {
                let p = e?.path();
                let name = p.file_name().unwrap().to_string_lossy().to_string();
                if name.starts_with('.') {
                    continue;
                }
                if p.is_dir() {
                    stack.push(p);
                } else {
                    let rel = p.strip_prefix(dir)?.to_string_lossy().replace('\\', "/");
                    files.push((format!("{prefix}/{rel}"), fs::read(&p)?));
                }
            }
        }
        Ok(())
    };
    add_tree(&bindgen, "", &mut files)?;
    add_tree(&codemirror, "/cm", &mut files)?;
    for f in ["style.css", "bg.jpg", "logo.png", "control.webmanifest"] {
        files.push((format!("/{f}"), fs::read(studio.join("web").join(f))?));
    }
    // The editor's module, loaded by the Studio when it needs it (the
    // control page never does).
    files.push(("/editor.js".into(), fs::read(studio.join("js/editor.js"))?));
    let fonts = root.join("crates/glue5/assets/ui");
    for f in ["Barlow-SemiBold.ttf", "BarlowCondensed-Bold.ttf"] {
        println!("cargo:rerun-if-changed={}", fonts.join(f).display());
        files.push((format!("/fonts/{f}"), fs::read(fonts.join(f))?));
    }
    // The page, with the import map of the editor's modules.
    let mut imports = serde_json::Map::new();
    for line in fs::read_to_string(studio.join("packages.txt"))?.lines() {
        let f: Vec<&str> = line.split_whitespace().collect();
        if f.len() == 4 && !line.starts_with('#') {
            imports.insert(f[0].to_string(), format!("/cm/{}/{}", f[0], f[3]).into());
        }
    }
    let map = serde_json::json!({ "imports": imports }).to_string();
    let page = fs::read_to_string(studio.join("web/index.html"))?.replace("<!--IMPORTMAP-->", &map);
    files.push(("/index.html".into(), page.into_bytes()));
    files.push(("/control.html".into(), fs::read(studio.join("web/control.html"))?));
    files.retain(|(p, _)| !p.ends_with("LICENSE") && !p.ends_with(".d.ts"));
    Ok(files)
}

/// Embeds `files` (assets.rs): each with a gzip copy when that is smaller.
fn assets(out: &Path, files: &[(String, Vec<u8>)]) -> Result<()> {
    use std::io::Write as _;
    let dir = out.join("web");
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir)?;
    let mut sorted: Vec<&(String, Vec<u8>)> = files.iter().collect();
    sorted.sort_by(|a, b| a.0.cmp(&b.0));
    let mut s = String::from("pub static ASSETS: &[(&str, Asset)] = &[\n");
    for (i, (path, body)) in sorted.iter().enumerate() {
        let raw = dir.join(format!("{i}"));
        fs::write(&raw, body)?;
        let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::best());
        gz.write_all(body)?;
        let gz = gz.finish()?;
        let gzip = if gz.len() + 64 < body.len() {
            let at = dir.join(format!("{i}.gz"));
            fs::write(&at, &gz)?;
            format!("Some(include_bytes!({:?}))", at.display().to_string())
        } else {
            "None".into()
        };
        let mime = match path.rsplit('.').next().unwrap_or("") {
            "html" => "text/html; charset=utf-8",
            "js" => "text/javascript; charset=utf-8",
            "css" => "text/css; charset=utf-8",
            "wasm" => "application/wasm",
            "json" => "application/json",
            "webmanifest" => "application/manifest+json",
            "svg" => "image/svg+xml",
            "png" => "image/png",
            "jpg" => "image/jpeg",
            "ttf" => "font/ttf",
            "woff2" => "font/woff2",
            _ => "application/octet-stream",
        };
        // Revalidated every time: an ETag (the content's hash) makes that a
        // 304 when nothing changed, and a new app never serves stale files.
        let etag = {
            use sha2::Digest as _;
            let h = sha2::Sha256::digest(body);
            format!("\"{}\"", h.iter().take(8).map(|b| format!("{b:02x}")).collect::<String>())
        };
        let _ = writeln!(
            s,
            "    ({path:?}, Asset {{ body: include_bytes!({:?}), gzip: {gzip}, mime: {mime:?}, etag: {etag:?} }}),",
            raw.display().to_string()
        );
    }
    s.push_str("];\n");
    fs::write(out.join("assets.rs"), s)?;
    Ok(())
}

/// Luau's type checker (Luau.Analysis, from `third_party/luau`, the release
/// mlua builds the rest of Luau from) and our C API over it.
fn checker(root: &Path) -> Result<()> {
    let luau = root.join("third_party/luau");
    anyhow::ensure!(luau.join("Analysis").is_dir(), "{} is missing: run `mise run setup`", luau.display());
    println!("cargo:rerun-if-changed=csrc");
    println!("cargo:rerun-if-changed={}", luau.join(".glue5-stamp").display());
    let mut sources: Vec<PathBuf> = fs::read_dir(luau.join("Analysis/src"))?
        .map(|e| e.map(|e| e.path()))
        .collect::<std::io::Result<_>>()?;
    sources.retain(|p| p.extension().is_some_and(|e| e == "cpp"));
    sources.sort();
    let mut build = cc::Build::new();
    build
        .cpp(true)
        .std("c++17")
        .warnings(false)
        .opt_level(2)
        .debug(false)
        .define("LUA_API", "extern \"C\"")
        .define("LUACODE_API", "extern \"C\"");
    for dir in ["Analysis", "Ast", "Bytecode", "Common", "Config", "Compiler", "VM"] {
        build.include(luau.join(dir).join("include"));
    }
    if env::var("CARGO_CFG_TARGET_ENV")? == "msvc" {
        build.flag("/EHsc");
    }
    build.files(&sources).file("csrc/checker.cpp").compile("glue5_luau_analysis");
    Ok(())
}

fn parse_table(text: &str) -> Result<Vec<Binary>> {
    text.lines()
        .map(|line| {
            let f: Vec<&str> = line.split('\t').collect();
            anyhow::ensure!(f.len() == 8, "bad natives line {line:?}");
            let hex = |s: &str| u64::from_str_radix(s.trim_start_matches("0x"), 16);
            let command = (f[5] != "-").then(|| {
                if f[5] == "void" { Vec::new() } else { f[5].split(", ").map(str::to_string).collect() }
            });
            let result = (f[6] != "-").then(|| {
                let (params, result) = f[6].split_once("->").unwrap_or(("", ""));
                let wasm_params = params.split(',').filter(|p| !p.is_empty()).count();
                let typed = command.as_ref().map_or(0, Vec::len);
                if result.is_empty() && wasm_params == typed + 1 { "sret".to_string() } else { result.to_string() }
            });
            Ok(Binary {
                name: f[0].to_string(),
                hash: hex(f[1])?,
                handler: f[2].parse()?,
                unused: f[3] == "1",
                group: f[4].to_string(),
                command,
                result,
                signature: hex(f[7])?,
            })
        })
        .collect()
}

fn handle(name: &str) -> Option<Handle> {
    HANDLES.iter().find(|(n, ..)| *n == name).map(|&(_, h, _)| h)
}

/// A value type of the database.
fn value_kind(t: &str) -> Option<Kind> {
    Some(match t {
        "int" => Kind::Int,
        "float" => Kind::Float,
        "BOOL" => Kind::Bool,
        "Hash" => Kind::Hash,
        "const char*" => Kind::Str,
        "Vector3" => Kind::Vec3,
        "Any" => Kind::Any,
        other => Kind::Handle(handle(other)?),
    })
}

/// Pointers to a value: outputs of getters and predicates, else the value
/// goes in and comes back (`DELETE_ENTITY(Entity*)`).
fn pointer_dir(native: &str) -> Dir {
    let getter = ["GET_", "IS_", "HAS_", "_GET_", "DOES_", "FIND_", "CAN_"].iter().any(|p| native.starts_with(p));
    if getter { Dir::Out } else { Dir::InOut }
}

/// The database's parameters, `None` when one has no type the mods can
/// pass (pointers to structures, buffers).
fn db_params(entry: &Value, native: &str) -> Result<Option<Vec<Param>>> {
    let mut params = Vec::new();
    for p in entry["params"].as_array().context("params")? {
        let t = p["type"].as_str().context("type")?;
        let name = p["name"].as_str().unwrap_or("");
        let (kind, dir) = if t == "const char*" {
            (Kind::Str, Dir::In)
        } else if let Some(inner) = t.strip_suffix('*') {
            match value_kind(inner) {
                Some(k) if !matches!(k, Kind::Any | Kind::Str) => (k, pointer_dir(native)),
                _ => return Ok(None),
            }
        } else {
            match value_kind(t) {
                Some(k) => (k, Dir::In),
                None => return Ok(None),
            }
        };
        params.push(Param { name: param_name(name), kind, dir });
    }
    Ok(Some(params))
}

/// Whether one of the database's parameters is a pointer to something
/// the mods cannot pass (`Any*`, `char*`, a structure).
fn untyped_pointer(entry: &Value) -> bool {
    entry["params"].as_array().is_some_and(|params| {
        params.iter().filter_map(|p| p["type"].as_str()).any(|t| {
            t != "const char*"
                && t.strip_suffix('*').is_some_and(|inner| !value_kind(inner).is_some_and(|k| !matches!(k, Kind::Any | Kind::Str)))
        })
    })
}

/// The typed command's C++ parameters.
fn binary_params(command: &[String], native: &str) -> Option<Vec<Param>> {
    command
        .iter()
        .enumerate()
        .map(|(i, t)| {
            let (kind, dir) = match t.as_str() {
                "int" | "unsigned int" => (Kind::Int, Dir::In),
                "bool" => (Kind::Bool, Dir::In),
                "float" => (Kind::Float, Dir::In),
                "char const*" => (Kind::Str, Dir::In),
                "rage::scrVector const&" | "rage::Vector3 const&" => (Kind::Vec3, Dir::In),
                "int&" => (Kind::Int, pointer_dir(native)),
                "float&" => (Kind::Float, pointer_dir(native)),
                "bool&" => (Kind::Bool, pointer_dir(native)),
                "rage::Vector3&" | "rage::scrVector&" => (Kind::Vec3, pointer_dir(native)),
                _ => return None,
            };
            Some(Param { name: format!("p{i}"), kind, dir })
        })
        .collect()
}

/// The database's parameters checked against the binary's, slot by slot:
/// the binary's count and kinds of values, the database's names and
/// finer types where they agree (a newer database may have parameters
/// this build lacks); the binary's vectors where the database has three
/// floats.
fn merge(db: Vec<Param>, bin: &[Param]) -> Vec<Param> {
    // The database's parameters by the slot they start at.
    let mut at = HashMap::new();
    let mut slot = 0;
    for p in &db {
        at.insert(slot, p);
        slot += p.slots();
    }
    let compatible = |b: &Param, d: &Param| {
        (b.dir == Dir::In) == (d.dir == Dir::In)
            && match (b.kind, d.kind) {
                (Kind::Int, Kind::Int | Kind::Any | Kind::Hash | Kind::Handle(_)) => true,
                (Kind::Bool, Kind::Bool | Kind::Any) => true,
                (a, b) => a == b,
            }
    };
    let mut out = Vec::new();
    let mut slot = 0;
    for b in bin {
        let p = match at.get(&slot) {
            Some(d) if b.kind == Kind::Vec3 && b.dir == Dir::In && d.kind == Kind::Float => {
                let floats = (0..3).all(|i| at.get(&(slot + i)).is_some_and(|x| x.kind == Kind::Float));
                Param { name: if floats { vector_name(&d.name) } else { format!("p{}", out.len()) }, ..b.clone() }
            }
            Some(d) if compatible(b, d) => (*d).clone(),
            Some(d) => Param { name: d.name.clone(), ..b.clone() },
            None => b.clone(),
        };
        slot += b.slots();
        out.push(p);
    }
    out
}

/// The parameters the script signature (`rage::scrSignature`) gives a
/// native, what its handler reads; none if it has none (0). A code per
/// value, the result's first, each 3 bits on from the last after a marker
/// bit: 0 none, 2 bool, 4 int, 6 float, 8 vector, 12 string, 14 a pointer
/// (`int&`, `float&`, a structure). Codes are even and those of 8 and more
/// reach into the next field: a field's low bit is the last code's high
/// one.
fn script_codes(signature: u64) -> Option<Vec<u8>> {
    if signature & 1 == 0 {
        return None;
    }
    let mut fields = Vec::new();
    let mut v = signature >> 1;
    while v != 0 {
        fields.push((v & 7) as u8);
        v >>= 3;
    }
    let mut codes: Vec<u8> = Vec::new();
    for (i, &f) in fields.iter().enumerate() {
        let carry = f & 1;
        if carry == 1
            && let Some(last) = codes.last_mut()
        {
            *last += 8;
        }
        // The last code's high bit alone.
        if i == fields.len() - 1 && f == 1 {
            break;
        }
        codes.push(f - carry);
    }
    if codes.is_empty() {
        codes.push(0);
    }
    Some(codes)
}

fn script_params(signature: u64) -> Option<Vec<Param>> {
    let codes = script_codes(signature)?;
    Some(
        codes[1..]
            .iter()
            .enumerate()
            .map(|(i, &c)| {
                let (kind, dir) = match c {
                    2 => (Kind::Bool, Dir::In),
                    6 => (Kind::Float, Dir::In),
                    8 => (Kind::Vec3, Dir::In),
                    12 => (Kind::Str, Dir::In),
                    14 => (Kind::Int, Dir::InOut),
                    _ => (Kind::Int, Dir::In),
                };
                Param { name: format!("p{i}"), kind, dir }
            })
            .collect(),
    )
}

/// Whether `params` pass what the script signature's `script` say: the same
/// values, a vector as one or as three floats, any integer kind where it
/// says int or bool, an output where it says pointer (or vector: an
/// out `Vector3&` is a vector to it).
fn fits(params: &[Param], script: &[Param]) -> bool {
    let mut j = 0;
    for s in script {
        let Some(p) = params.get(j) else { return false };
        let integer = |k: Kind| matches!(k, Kind::Int | Kind::Bool | Kind::Hash | Kind::Any | Kind::Handle(_));
        let ok = match (s.kind, s.dir) {
            (Kind::Vec3, _) if p.kind == Kind::Vec3 => true,
            (Kind::Vec3, _) => {
                let floats = params.get(j..j + 3).is_some_and(|f| f.iter().all(|f| f.kind == Kind::Float && f.dir == Dir::In));
                if floats {
                    j += 2;
                }
                floats
            }
            (_, Dir::InOut) => p.dir != Dir::In,
            (Kind::Str, _) => p.kind == Kind::Str && p.dir == Dir::In,
            (Kind::Float, _) => p.kind == Kind::Float && p.dir == Dir::In,
            _ => integer(p.kind) && p.dir == Dir::In,
        };
        if !ok {
            return false;
        }
        j += 1;
    }
    j == params.len()
}

/// A vector to the signature is one either way: an output where the
/// database, of as many parameters, has a `Vector3*` (GET_COORDS_OF_PROJECTILE_TYPE_WITHIN_DISTANCE).
fn vectors_out(mut params: Vec<Param>, db: &[Param]) -> Vec<Param> {
    if params.len() == db.len() {
        for (p, d) in params.iter_mut().zip(db) {
            if p.kind == Kind::Vec3 && d.kind == Kind::Vec3 {
                p.dir = d.dir;
            }
        }
    }
    params
}

/// Whether a pointer of the script signature's has no pointer of the
/// database at its value (by the values before it).
fn pointers_untyped(db: Option<&[Param]>, script: &[Param]) -> bool {
    let mut at = HashMap::new();
    let mut slot = 0;
    for p in db.unwrap_or_default() {
        at.insert(slot, p);
        slot += p.slots();
    }
    let mut slot = 0;
    for s in script {
        if s.dir != Dir::In && !at.get(&slot).is_some_and(|d| d.dir != Dir::In) {
            return true;
        }
        slot += s.slots();
    }
    false
}

/// What the binary says about the result, when the database has nothing.
fn binary_return(b: &Binary) -> Option<Option<Kind>> {
    match b.result.as_deref() {
        Some("") => Some(None),
        Some("sret") => Some(Some(Kind::Vec3)),
        Some("f32") => Some(Some(Kind::Float)),
        Some("i64") => Some(Some(Kind::Str)),
        Some("i32") => Some(Some(if (b.signature >> 1) & 7 == 2 { Kind::Bool } else { Kind::Int })),
        _ => None,
    }
}

/// `CREATE_VEHICLE` -> `createVehicle`.
fn camel(name: &str) -> String {
    let mut out = String::new();
    for (i, word) in name.split('_').filter(|w| !w.is_empty()).enumerate() {
        let lower = word.to_ascii_lowercase();
        if i == 0 {
            out.push_str(&lower);
        } else {
            let mut chars = lower.chars();
            if let Some(c) = chars.next() {
                out.push(c.to_ascii_uppercase());
                out.extend(chars);
            }
        }
    }
    out
}

const KEYWORDS: &[&str] = &[
    "and", "break", "do", "else", "elseif", "end", "false", "for", "function", "if", "in", "local", "nil", "not",
    "or", "repeat", "return", "then", "true", "until", "while", "continue", "type", "export",
];

/// A database parameter's name in Luau's style: `bScriptHostVeh` ->
/// `scriptHostVeh`, snake case to camel case, no keywords.
fn param_name(name: &str) -> String {
    let mut n = name.to_string();
    let bytes = n.as_bytes();
    if bytes.len() > 1 && bytes[0] == b'b' && bytes[1].is_ascii_uppercase() {
        n = format!("{}{}", n[1..2].to_ascii_lowercase(), &n[2..]);
    }
    if n.contains('_') {
        let mut c = camel(&n);
        if c.is_empty() {
            c = "value".into();
        }
        n = c;
    }
    if n.is_empty() || !n.chars().next().unwrap().is_ascii_alphabetic() {
        n = format!("p{n}");
    }
    if KEYWORDS.contains(&n.as_str()) {
        n.push('_');
    }
    n
}

/// `x` -> `coords`, `posX` -> `pos`, `x1` -> `coords1`.
fn vector_name(x: &str) -> String {
    let lower = x.to_ascii_lowercase();
    if let Some(rest) = lower.strip_prefix('x')
        && rest.chars().all(|c| c.is_ascii_digit())
    {
        return format!("coords{rest}");
    }
    let stem = x.strip_suffix('X').or_else(|| x.strip_suffix('x')).unwrap_or(x);
    match stem {
        "" => "coords".into(),
        k if KEYWORDS.contains(&k) => format!("{k}_"),
        _ => stem.to_string(),
    }
}

fn rust_kind(k: Kind) -> String {
    match k {
        Kind::Handle(h) => format!("K::Handle(H::{h:?})"),
        other => format!("K::{other:?}"),
    }
}

fn rust_table(natives: &[Native], raw: &[Native], reserved: &[(String, u64)], excluded: &[(String, u64, &str)]) -> String {
    let mut spaces: Vec<&str> = natives.iter().map(|n| n.namespace.as_str()).collect();
    spaces.dedup();
    let mut s = String::new();
    s.push_str("// Generated by glue5-mods' build script from game.wasm and nativedb.\n");
    let _ = writeln!(s, "pub const NAMESPACES: &[&str] = &{spaces:?};");
    s.push_str("pub static NATIVES: &[Native] = &[\n");
    native_rows(&mut s, natives, &spaces);
    s.push_str("];\n");
    s.push_str("pub static RAW: &[Native] = &[\n");
    native_rows(&mut s, raw, &spaces);
    s.push_str("];\n");
    let _ = writeln!(s, "pub const RESERVED: &[(&str, u64)] = &{reserved:?};");
    s.push_str("pub static EXCLUDED: &[Excluded] = &[\n");
    for (name, db_hash, why) in excluded {
        let _ = writeln!(s, "    Excluded {{ name: {name:?}, db_hash: {db_hash:#x}, why: {why:?} }},");
    }
    s.push_str("];\n");
    s
}

fn native_rows(s: &mut String, natives: &[Native], spaces: &[&str]) {
    for n in natives {
        // A left-out native of a namespace with none exposed: its group's name.
        let ns = spaces.iter().position(|&x| x == n.namespace).unwrap_or(u16::MAX as usize);
        let params: Vec<String> = n
            .params
            .iter()
            .map(|p| format!("P {{ kind: {}, dir: D::{:?} }}", rust_kind(p.kind), p.dir))
            .collect();
        let ret = n.ret.map_or("None".to_string(), |k| format!("Some({})", rust_kind(k)));
        let _ = writeln!(
            s,
            "    Native {{ ns: {ns}, lua: {:?}, name: {:?}, hash: {:#x}, db_hash: {:#x}, handler: {}, params: &[{}], ret: {ret}, doc: {:?} }},",
            n.lua,
            n.name,
            n.hash,
            n.db_hash,
            n.handler,
            params.join(", "),
            n.doc.trim()
        );
    }
}

/// The parameters and result of a native left out, as `native.invoke` passes
/// them: what the database or else the binary says, pointers as addresses
/// (an in-out int); none if neither knows.
fn raw_signature(entry: Option<&Value>, b: &Binary) -> Option<(Vec<Param>, Option<Kind>)> {
    let param = |name: String, kind, dir| Param { name, kind, dir };
    let params: Vec<Param> = match entry {
        Some(e) => e["params"]
            .as_array()?
            .iter()
            .map(|p| {
                let name = param_name(p["name"].as_str().unwrap_or(""));
                match p["type"].as_str().unwrap_or("") {
                    "float" => param(name, Kind::Float, Dir::In),
                    "BOOL" => param(name, Kind::Bool, Dir::In),
                    "Hash" => param(name, Kind::Hash, Dir::In),
                    "const char*" => param(name, Kind::Str, Dir::In),
                    "Vector3" => param(name, Kind::Vec3, Dir::In),
                    t if t.ends_with('*') => param(name, Kind::Int, Dir::InOut),
                    _ => param(name, Kind::Int, Dir::In),
                }
            })
            .collect(),
        None => b
            .command
            .as_ref()?
            .iter()
            .enumerate()
            .map(|(i, t)| {
                let name = format!("p{i}");
                match t.as_str() {
                    "float" => param(name, Kind::Float, Dir::In),
                    "bool" => param(name, Kind::Bool, Dir::In),
                    "char const*" => param(name, Kind::Str, Dir::In),
                    "rage::scrVector const&" | "rage::Vector3 const&" => param(name, Kind::Vec3, Dir::In),
                    t if t.ends_with('&') || t.ends_with('*') => param(name, Kind::Int, Dir::InOut),
                    _ => param(name, Kind::Int, Dir::In),
                }
            })
            .collect(),
    };
    let ret = match entry.and_then(|e| e["return_type"].as_str()) {
        Some("void") => None,
        Some(t) => Some(value_kind(t).unwrap_or(Kind::Any)),
        None => binary_return(b).unwrap_or(Some(Kind::Any)),
    };
    // What the handler reads, where the database or the binary say otherwise.
    let params = match script_params(b.signature) {
        Some(script) if !fits(&params, &script) => merge(params, &script),
        _ => params,
    };
    Some((params, ret))
}

fn luau_type(k: Kind, input: bool) -> String {
    match k {
        Kind::Int | Kind::Float => "number".into(),
        Kind::Bool => "boolean".into(),
        Kind::Hash if input => "Hash | string".into(),
        Kind::Hash => "Hash".into(),
        Kind::Str => "string".into(),
        Kind::Vec3 => "vector".into(),
        Kind::Any => "any".into(),
        Kind::Handle(Handle::Player) => "Player".into(),
        // None (0, nil) where a native takes one: the game's scripts pass
        // that where the native allows none.
        Kind::Handle(h) if input => format!("{h:?}?"),
        Kind::Handle(h) => format!("{h:?}"),
    }
}

/// What a `Hash` parameter takes: the names of the catalog (world.rs) when
/// the parameter is a weapon or a vehicle's or ped's model, any name for
/// other models (objects' names are not known), else any string.
fn hash_type(native: &str, ns: &str, param: &str) -> &'static str {
    let p = param.to_ascii_lowercase();
    if p.contains("weapon") {
        return "WeaponName | Hash";
    }
    let model = p == "model" || p.ends_with("model") || p.contains("modelhash");
    if !model {
        return "Hash | string";
    }
    if native.starts_with("IS_") {
        return "Model | Hash | string";
    }
    if ns == "ped" || native.starts_with("CREATE_PED") || native.contains("PED_MODEL") || native.contains("PLAYER_MODEL") {
        return "PedModel | Hash";
    }
    if ns == "vehicle" || native.contains("VEHICLE") {
        return "VehicleModel | Hash";
    }
    "Model | Hash | string"
}

/// Handles the natives return: 0 (none) becomes nil, so they are optional
/// (but for players: 0 is the local one).
fn luau_result(k: Kind) -> String {
    match k {
        Kind::Handle(Handle::Player) => "Player".into(),
        Kind::Handle(h) => format!("{h:?}?"),
        other => luau_type(other, false),
    }
}

/// `task.spawn`, `task.delay`, `task.wait` (vm.rs), beside the TASK_ natives.
const TASK_FIELDS: &str = "\
\t--- Runs `f` now, beside the mod's main code (it may wait).
\tspawn: <A...>(f: (A...) -> ...any, A...) -> (),
\t--- Runs `f` in `ms` milliseconds of game time.
\tdelay: <A...>(ms: number, f: (A...) -> ...any, A...) -> (),
\t--- Back to the game for `ms` milliseconds, as `wait`.
\twait: (ms: number?) -> (),
";

/// Natives with a structure or an array, as structs.luau gives them: a
/// table or a list (their types in structs.d.luau), beside the natives of
/// their namespace. Mods reach them raw through `native.invoke` too.
const STRUCT_FIELDS: &[(&str, &str)] = &[
    ("extrametadata", "\tgetDlcVehicleData: (dlcVehicleIndex: number) -> DlcVehicleData?,\n"),
    ("extrametadata", "\tgetDlcWeaponData: (dlcWeaponIndex: number) -> DlcWeaponData?,\n"),
    ("extrametadata", "\tgetDlcWeaponDataSp: (dlcWeaponIndex: number) -> DlcWeaponData?,\n"),
    ("extrametadata", "\tgetDlcWeaponComponentData: (dlcWeaponIndex: number, dlcWeapCompIndex: number) -> DlcWeaponComponentData?,\n"),
    ("extrametadata", "\tgetDlcWeaponComponentDataSp: (dlcWeaponIndex: number, dlcWeapCompIndex: number) -> DlcWeaponComponentData?,\n"),
    ("ped", "\t--- The vehicles near `ped`, at most `max` (16 by default, 64 at most).\n\tgetPedNearbyVehicles: (ped: Ped, max: number?) -> { Vehicle },\n"),
    ("ped", "\t--- The peds near `ped`, but those of `ignore`'s ped type (none: -1), at most `max` (16 by default, 64 at most).\n\tgetPedNearbyPeds: (ped: Ped, ignore: number?, max: number?) -> { Ped },\n"),
];

/// What the mods' VM adds besides the natives (vm.rs).
const GLOBALS: &str = "\
type Hash = number

-- Back to the game for `ms` milliseconds (0 or none: until the next frame).
declare function wait(ms: number?): ()
-- The hash the game uses for a name (GET_HASH_KEY).
declare function joaat(name: string): Hash
-- The mod: its name, its folder's files, the settings a player edits in its settings.json.
declare mod: {
\tname: string,
\t-- `defaults` with the values of the mod's settings.json over them. The file is written with the
\t-- defaults when it is not there or lacks some (a newer version's), for the player to edit.
\tsettings: <T>(defaults: T) -> T,
\t-- A file of the mod's folder (\"data/cars.json\"), or nil when there is none; 16 MB at most.
\tread: (path: string) -> string?,
\t-- Writes a file of the mod's folder, and its folders: 16 MB a file, 256 MB the folder.
\twrite: (path: string, data: string | buffer) -> (),
\t-- The names in a folder of the mod's (none: the mod's own), folders ending in /.
\tlist: (path: string?) -> { string },
}

-- Which entities `world.vehicles()`... return: at most `max` (1024 by default), only those within `radius` of
-- `near` (the closest first), only those of `model` (a name or a hash).
export type WorldFilter = { max: number?, near: vector?, radius: number?, model: (string | number)? }

declare world: {
\t-- The entities around, as handles; a number alone is `max`.
\tvehicles: (filter: (number | WorldFilter)?) -> { Vehicle },
\tpeds: (filter: (number | WorldFilter)?) -> { Ped },
\tobjects: (filter: (number | WorldFilter)?) -> { Object },
\t-- The pickups lying around (objects; a pickup placement's handle is another thing), the projectiles in flight.
\tpickups: (filter: (number | WorldFilter)?) -> { Object },
\tprojectiles: (filter: (number | WorldFilter)?) -> { Object },
\t-- The installed game's names (empty until the game has loaded once).
\tvehicleModels: () -> { VehicleModel },
\tpedModels: () -> { PedModel },
\tweapons: () -> { WeaponName },
}

declare json: {
	-- The value as JSON text: a table with keys 1..n (or none) is an array, another an object
	-- (string keys; number keys written as strings); a vector is [x, y, z]. `pretty` indents it.
	-- Text over 1 MB (what storage keeps), or tables nested over 64 deep (a table that holds itself), is an error.
	encode: (value: any, pretty: boolean?) -> string,
	-- JSON text as Luau values; null is nil.
	decode: (text: string) -> any,
}

-- What the mod keeps between sessions, by key (JSON values, 1 MB at most); written within a
-- couple of seconds of a change and when the mod stops. Kept for this player, not in the mod's folder.
declare storage: {
	-- The value kept under `key` (a copy), or nil.
	get: (key: string) -> any,
	-- Keeps a copy of `value` under `key`; nil removes it.
	set: (key: string, value: any) -> (),
	keys: () -> { string },
	clear: () -> (),
}

-- Natives the ScriptHookV way.
declare native: {
	-- Calls a native by its ScriptHookV (nativedb) hash, `0xAF35D0D2583051B0` (or that as a string), or by its
	-- name, \"CREATE_VEHICLE\", with ScriptHookV's arguments: a vector as three numbers (or a vector), a pointer
	-- as an address (memory.alloc) or a buffer (copied in, and back after the call; 16 KB at most); its result
	-- comes back. Natives the namespaces leave out too: a wrong buffer can corrupt or crash the game.
	invoke: (native: number | string, ...any) -> any,
}

-- The game's memory (addresses are the game's; a read pointer is an address too). Checked against the
-- memory's size only: a wrong write can corrupt or crash the game. Offsets are not PC GTA V's.
declare memory: {
	-- Where fields of the game's objects are (this game build's offsets): memory.offsets.vehicle.rpm.
	offsets: MemoryOffsets,
	-- The entity's address (getScriptHandleBaseAddress), nil if there is none.
	entity: (handle: Entity?) -> number?,
	readU8: (address: number) -> number,
	readU16: (address: number) -> number,
	readU32: (address: number) -> number,
	readInt: (address: number) -> number,
	-- Exact up to 2^53.
	readU64: (address: number) -> number,
	readPtr: (address: number) -> number,
	readFloat: (address: number) -> number,
	readVector3: (address: number) -> vector,
	readString: (address: number) -> string,
	writeU8: (address: number, value: number) -> (),
	writeU16: (address: number, value: number) -> (),
	writeU32: (address: number, value: number) -> (),
	writeInt: (address: number, value: number) -> (),
	writeU64: (address: number, value: number) -> (),
	writePtr: (address: number, value: number) -> (),
	writeFloat: (address: number, value: number) -> (),
	writeVector3: (address: number, value: vector) -> (),
	-- Writes the text and its end (a 0).
	writeString: (address: number, text: string) -> (),
	-- `bytes` bytes as a buffer (Luau's buffer library reads structures out of it).
	read: (address: number, bytes: number) -> buffer,
	write: (address: number, data: buffer) -> (),
	-- Zeroed scratch memory to pass to natives (16 MB a mod at most), freed when the mod stops.
	alloc: (bytes: number) -> number,
	free: (address: number) -> (),
}

-- The game scripts' globals, by ScriptHookV's index (getGlobalPtr's): an int, float or bool is one
-- value, a string spans several. Indices change with the game's scripts: check them against this game.
declare globals: {
	int: (index: number) -> number,
	float: (index: number) -> number,
	bool: (index: number) -> boolean,
	-- The text that starts at `index` (a TEXT_LABEL).
	string: (index: number) -> string,
	setInt: (index: number, value: number) -> (),
	setFloat: (index: number, value: number) -> (),
	setBool: (index: number, value: boolean) -> (),
	-- Writes `text` and its end at `index`; `size` is the label's room in bytes (16 for a TEXT_LABEL_15).
	setString: (index: number, text: string, size: number?) -> (),
	-- Whether `index` is in a block of globals the game has loaded.
	valid: (index: number) -> boolean,
}

declare game: {
	-- The game build these natives and globals are for (getGameVersion).
	version: () -> string,
}

";

fn definitions(natives: &[Native]) -> String {
    let mut s = String::new();
    s.push_str("--!strict\n-- The game's natives, generated from game.wasm and nativedb by glue5-mods. Do not edit.\n\n");
    for (name, _, base) in HANDLES {
        let base = base.map(|b| format!(" extends {b}")).unwrap_or_default();
        let _ = writeln!(s, "declare extern type {name}{base} with end");
    }
    s.push_str(GLOBALS);
    let mut i = 0;
    while i < natives.len() {
        let ns = &natives[i].namespace;
        let _ = writeln!(s, "declare {ns}: {{");
        if ns == "task" {
            // The mods' scheduler shares the table with the TASK_ natives.
            s.push_str(TASK_FIELDS);
        }
        for (_, fields) in STRUCT_FIELDS.iter().filter(|(space, _)| space == ns) {
            s.push_str(fields);
        }
        while i < natives.len() && &natives[i].namespace == ns {
            let n = &natives[i];
            let inputs: Vec<String> = n
                .params
                .iter()
                .filter(|p| p.dir != Dir::Out)
                .map(|p| match p.kind {
                    Kind::Hash => format!("{}: {}", p.name, hash_type(&n.name, &n.namespace, &p.name)),
                    k => format!("{}: {}", p.name, luau_type(k, true)),
                })
                .collect();
            let mut outputs: Vec<String> = n
                .ret
                .into_iter()
                .map(|k| if NEVER_NONE.contains(&n.name.as_str()) { luau_type(k, false) } else { luau_result(k) })
                .collect();
            outputs.extend(n.params.iter().filter(|p| p.dir != Dir::In).map(|p| luau_result(p.kind)));
            let result = match outputs.len() {
                0 => "()".to_string(),
                1 if !outputs[0].contains('|') => outputs[0].clone(),
                _ => format!("({})", outputs.join(", ")),
            };
            let _ = writeln!(s, "\t{}: ({}) -> {},", n.lua, inputs.join(", "), result);
            i += 1;
        }
        s.push_str("}\n\n");
    }
    s
}
