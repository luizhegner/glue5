//! The mods' named constants (`Control.JUMP`, `Bone.SKEL_R_HAND`...), from
//! what game.wasm and the natives database say, written as a Rust table
//! (`constants.rs`, the VM's) and their Luau definitions
//! (`constants.d.luau`, the checker's).
//!
//! Most come from the game's own parser enums: game.wasm keeps each as an
//! array of (name hash, value) pairs ended by (0, -1), and the names it
//! still holds as strings. A table is found by the hashes of those names;
//! an entry whose name is not in game.wasm keeps it only when one of the
//! spec's `extra` names hashes to it (checked here, never assumed). The HUD
//! components are no enum of the code: the database lists them.

use std::collections::{HashMap, HashSet};
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::{env, fs};

use anyhow::{Context, Result};
use serde_json::Value;

/// A parser enum of game.wasm: the table the mods get, the prefix its
/// names share (left out of the constants' names), names the binary does
/// not hold that the entries are checked against.
struct Spec {
    table: &'static str,
    doc: &'static str,
    prefix: &'static str,
    extra: &'static [&'static str],
}

const SPECS: &[Spec] = &[
    Spec { table: "Control", doc: "The game's controls (`INPUT_JUMP` is `Control.JUMP`), for the `pad` natives.", prefix: "INPUT_", extra: &[] },
    Spec {
        table: "Bone",
        doc: "Bone tags (`BONETAG_R_HAND` is `Bone.R_HAND`), for `ped.getPedBoneIndex`.",
        prefix: "BONETAG_",
        extra: &[],
    },
    Spec {
        table: "ExplosionType",
        doc: "Explosions (`EXP_TAG_GRENADE` is `ExplosionType.GRENADE`), for `fire.addExplosion`.",
        prefix: "EXP_TAG_",
        // The first tags' entries hash their names without the prefix.
        extra: &[
            "DONTCARE",
            "GRENADE",
            "GRENADELAUNCHER",
            "STICKYBOMB",
            "MOLOTOV",
            "ROCKET",
            "TANKSHELL",
            "HI_OCTANE",
            "CAR",
            "PLANE",
            "PETROL_PUMP",
            "BIKE",
            "DIR_STEAM",
            "DIR_FLAME",
            "DIR_WATER_HYDRANT",
            "DIR_GAS_CANISTER",
            "BOAT",
            "SHIP_DESTROY",
            "TRUCK",
            "BULLET",
            "SMOKEGRENADELAUNCHER",
            "SMOKEGRENADE",
            "BZGAS",
            "FLARE",
            "GAS_CANISTER",
            "EXTINGUISHER",
            "PROGRAMMABEAR",
            "PLANE_ROCKET",
        ],
    },
    Spec {
        table: "PedConfigFlag",
        doc: "Ped config flags (`CPED_CONFIG_FLAG_NoCriticalHits` is `PedConfigFlag.NoCriticalHits`), for `ped.setPedConfigFlag`.",
        prefix: "CPED_CONFIG_FLAG_",
        extra: &[],
    },
];

/// The game's string hash as the parser takes it (case kept).
fn hash(s: &[u8]) -> u32 {
    let mut h: u32 = 0;
    for &c in s {
        h = h.wrapping_add(u32::from(c));
        h = h.wrapping_add(h << 10);
        h ^= h >> 6;
    }
    h = h.wrapping_add(h << 3);
    h ^= h >> 11;
    h.wrapping_add(h << 15)
}

fn leb(b: &[u8], at: &mut usize) -> Option<usize> {
    let (mut v, mut shift) = (0usize, 0);
    loop {
        let byte = *b.get(*at)?;
        *at += 1;
        v |= usize::from(byte & 0x7f) << shift;
        shift += 7;
        if byte < 0x80 {
            return Some(v);
        }
    }
}

/// The data section's bytes (strings and tables: no need to look at code).
fn data_section(wasm: &[u8]) -> Option<&[u8]> {
    let mut at = 8;
    while at < wasm.len() {
        let id = wasm[at];
        at += 1;
        let size = leb(wasm, &mut at)?;
        if id == 11 {
            return wasm.get(at..at + size);
        }
        at += size;
    }
    None
}

/// The single `b/<build>/game.wasm` under the game directory.
fn game_wasm() -> Option<PathBuf> {
    let b = PathBuf::from(env::var_os("GLUE5_GAME_DIR")?).join("b");
    let mut found = fs::read_dir(b).ok()?.filter_map(|e| Some(e.ok()?.path().join("game.wasm"))).filter(|p| p.is_file());
    let wasm = found.next()?;
    found.next().is_none().then_some(wasm)
}

/// One enum's constants: name (without the prefix) and value, by value.
fn parser_enum(data: &[u8], names: &HashMap<u32, &str>, spec: &Spec) -> Vec<(String, i32)> {
    let pair = |at: usize| -> Option<(u32, i32)> {
        let b = data.get(at..at + 8)?;
        Some((u32::from_le_bytes(b[..4].try_into().unwrap()), i32::from_le_bytes(b[4..].try_into().unwrap())))
    };
    // The names an entry may have: the binary's with the prefix, the
    // spec's extra ones with it or not.
    let mut known: HashMap<u32, String> = names
        .iter()
        .filter_map(|(&h, n)| Some((h, n.strip_prefix(spec.prefix)?.to_string())))
        .collect();
    for extra in spec.extra {
        known.insert(hash(extra.as_bytes()), extra.to_string());
        known.insert(hash(format!("{}{extra}", spec.prefix).as_bytes()), extra.to_string());
    }
    // Where the most of them follow each other, 8 bytes apart: the table.
    let mut best = (0, 0);
    let mut run = (0, 0);
    for at in 0..data.len().saturating_sub(8) {
        // Most bytes are no hash of ours: a cheap look first.
        let Some((h, _)) = pair(at) else { break };
        if !known.contains_key(&h) {
            continue;
        }
        run = if run.1 > 0 && at == run.0 + 8 * run.1 { (run.0, run.1 + 1) } else { (at, 1) };
        if run.1 > best.1 {
            best = run;
        }
    }
    if best.1 < 3 {
        return Vec::new();
    }
    // Back to the first entry with a name (past one or two without: names
    // the binary lost), on to the end of the table.
    let (mut start, mut at, mut unnamed) = (best.0, best.0, 0);
    while at >= 8 && unnamed <= 2 {
        let Some((h, _)) = pair(at - 8).filter(|&(h, _)| h != 0) else { break };
        at -= 8;
        if known.contains_key(&h) {
            (start, unnamed) = (at, 0);
        } else {
            unnamed += 1;
        }
    }
    let mut entries = Vec::new();
    at = start;
    while let Some((h, value)) = pair(at) {
        if h == 0 {
            break;
        }
        if let Some(name) = known.get(&h) {
            entries.push((name.clone(), value));
        }
        at += 8;
    }
    entries.sort_by_key(|&(_, v)| v);
    entries
}

/// The HUD components the database lists for HIDE_HUD_COMPONENT_THIS_FRAME
/// (`- 1 : WANTED_STARS`).
fn hud_components(db: &Value) -> Vec<(String, i32)> {
    let Value::Object(spaces) = db else { return Vec::new() };
    let comment = spaces
        .values()
        .filter_map(Value::as_object)
        .flat_map(|e| e.values())
        .find(|e| e["name"] == "HIDE_HUD_COMPONENT_THIS_FRAME")
        .and_then(|e| e["comment"].as_str())
        .unwrap_or("");
    comment
        .lines()
        .filter_map(|l| {
            let (value, name) = l.trim().strip_prefix("- ")?.split_once(" : ")?;
            let name = name.trim();
            name.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'_').then_some(())?;
            Some((name.to_string(), value.trim().parse().ok()?))
        })
        .collect()
}

/// A Luau name for a constant: the game's, made a valid identifier.
fn identifier(name: &str) -> String {
    if name.starts_with(|c: char| c.is_ascii_digit()) { format!("_{name}") } else { name.to_string() }
}

/// Writes `constants.rs` and `constants.d.luau` into `out`.
pub fn write(out: &Path, db: Option<&Value>) -> Result<()> {
    println!("cargo:rerun-if-env-changed=GLUE5_GAME_DIR");
    let mut tables: Vec<(&str, &str, Vec<(String, i32)>)> = Vec::new();
    match game_wasm() {
        Some(path) => {
            println!("cargo:rerun-if-changed={}", path.display());
            let wasm = fs::read(&path).with_context(|| format!("reading {}", path.display()))?;
            let data = data_section(&wasm).context("game.wasm has no data section")?;
            // The binary's strings that start as an enum's names do.
            let mut names: HashMap<u32, &str> = HashMap::new();
            let mut start = 0;
            for (i, &c) in data.iter().enumerate() {
                if c.is_ascii_alphanumeric() || c == b'_' {
                    continue;
                }
                if c == 0
                    && let Ok(s) = std::str::from_utf8(&data[start..i])
                    && SPECS.iter().any(|spec| s.starts_with(spec.prefix))
                {
                    names.insert(hash(s.as_bytes()), s);
                }
                start = i + 1;
            }
            for spec in SPECS {
                let entries = parser_enum(data, &names, spec);
                if entries.is_empty() {
                    println!("cargo:warning=no {} table in game.wasm: the mods' {} constants are empty", spec.prefix, spec.table);
                }
                tables.push((spec.table, spec.doc, entries));
            }
        }
        None => {
            println!("cargo:warning=no game.wasm under GLUE5_GAME_DIR: the mods' constants are empty");
            tables.extend(SPECS.iter().map(|s| (s.table, s.doc, Vec::new())));
        }
    }
    let hud = db.map(hud_components).unwrap_or_default();
    tables.push(("HudComponent", "HUD components, for `hud.hideHudComponentThisFrame` (`ui.hideHud`).", hud));

    let mut rust = String::from("// The mods' constants, generated by glue5-mods' build script. Do not edit.\n\n");
    rust.push_str("pub const CONSTANTS: &[(&str, &[(&str, i32)])] = &[\n");
    let mut luau = String::from("-- The mods' constants, generated by glue5-mods from game.wasm and nativedb. Do not edit.\n\n");
    for (table, doc, entries) in &tables {
        // One value per name (a table type takes a name once).
        let mut seen = HashSet::new();
        let entries: Vec<&(String, i32)> = entries.iter().filter(|(n, _)| seen.insert(identifier(n))).collect();
        let _ = write!(rust, "    (\"{table}\", &[");
        for (name, value) in &entries {
            let _ = write!(rust, "(\"{}\", {value}), ", identifier(name));
        }
        rust.push_str("]),\n");
        let _ = writeln!(luau, "-- {doc}\ndeclare {table}: {{");
        for (name, value) in &entries {
            let _ = writeln!(luau, "\t{}: number, -- {value}", identifier(name));
        }
        luau.push_str("}\n\n");
    }
    rust.push_str("];\n");
    fs::write(out.join("constants.rs"), rust)?;
    fs::write(out.join("constants.d.luau"), luau)?;
    Ok(())
}
