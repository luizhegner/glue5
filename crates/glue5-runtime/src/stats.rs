//! GTA Online's stats that the game's data does not define. The scripts are
//! a later version than the data (and its DLC packs): they read and write
//! thousands of stats the engine has no entry for, which it drops (and
//! reads as 0). Some matter: a character whose "version" stats read 0 is an
//! old one and goes back to the creator, its hair is gone...
//!
//! The host keeps them instead, by stat hash or by packed stat (index,
//! character slot), and saves them with the character's files (cloud.rs).

use std::cell::Cell;
use std::collections::BTreeMap;
use std::sync::Mutex;

use glue5_game::{Env, orig};

use crate::memory::Mem;

thread_local! {
    /// The engine's last stat lookup found nothing.
    static MISSING: Cell<bool> = const { Cell::new(false) };
}

/// Stat values by key: `h<hash>` (int, float bits, or bool), `p<slot>:<index>`
/// (packed: a bool or a byte).
struct Store {
    values: BTreeMap<String, i64>,
    dirty: bool,
}

static STORE: Mutex<Option<Store>> = Mutex::new(None);

fn file() -> std::path::PathBuf {
    crate::rt().config.user_dir.join("cloud").join("extra-stats.txt")
}

fn with_store<R>(f: impl FnOnce(&mut Store) -> R) -> R {
    let mut store = STORE.lock().unwrap();
    let store = store.get_or_insert_with(|| {
        let values = std::fs::read_to_string(file())
            .unwrap_or_default()
            .lines()
            .filter_map(|l| {
                let (k, v) = l.split_once(' ')?;
                Some((k.to_string(), v.parse().ok()?))
            })
            .collect();
        Store { values, dirty: false }
    });
    f(store)
}

fn get(key: &str) -> i64 {
    with_store(|s| s.values.get(key).copied().unwrap_or(0))
}

fn set(key: String, value: i64) {
    with_store(|s| {
        if s.values.get(&key) != Some(&value) {
            s.values.insert(key, value);
            s.dirty = true;
        }
    })
}

/// Writes the stats down; with each save of the character's files.
pub(crate) fn save() {
    with_store(|s| {
        if !s.dirty {
            return;
        }
        let path = file();
        let saved = crate::write_file(&path, text(&s.values));
        match saved {
            Ok(()) => s.dirty = false,
            Err(e) => log::error!("stats: saving {}: {e}", path.display()),
        }
    })
}

/// The stats as saved, a line `key value` each: ~28,000 of them, written on
/// the engine's main thread at each save of the character's files, into
/// one string (a string each took 2.5 ms on an M1, this 1 ms).
fn text(values: &BTreeMap<String, i64>) -> String {
    use std::fmt::Write;
    // Most values are one digit.
    let mut text = String::with_capacity(values.keys().map(|k| k.len() + 3).sum());
    for (k, v) in values {
        let _ = writeln!(text, "{k} {v}");
    }
    text
}

/// The engine's stat operation, or the host's when the engine has no such
/// stat.
fn engine_or<R>(engine: impl FnOnce() -> R, host: impl FnOnce() -> R) -> R {
    MISSING.set(false);
    let r = engine();
    if MISSING.get() { host() } else { r }
}

/// Hooks for `CStatsDataMgr::StatIterFind(StatId const&, Iterator&)` and
/// `packed_stats::GetStatKey(StatId&, int, int)`: the engine's lookups.
pub(crate) unsafe extern "C" fn stat_find(env: *mut Env, id: u64, iter: u64) -> u32 {
    let found = unsafe { orig::stat_find(env, id, iter) };
    MISSING.set(found == 0);
    found
}

pub(crate) unsafe extern "C" fn packed_key(env: *mut Env, id: u64, index: u32, slot: u32) -> u32 {
    let m = unsafe { Mem::of(env) };
    let map = stats_map(m);
    let missing = with_misses(map, |keys| keys.get(&(index, slot)).copied());
    let found = match missing {
        Some(hash) => {
            m.set_u32(id, hash);
            0
        }
        None => {
            let found = unsafe { orig::packed_key(env, id, index, slot) };
            if found == 0 {
                with_misses(map, |keys| keys.insert((index, slot), m.u32(id)));
            }
            found
        }
    };
    MISSING.set(found == 0);
    found
}

/// The packed stats `GetStatKey` found missing: (index, slot) → the key's
/// hash. The scripts ask for the same few dozen every frame (and for every
/// packed stat once while the character loads, ~27000), and each miss
/// costs the engine a name (snprintf), its hash and an error in the log
/// (~1 GB an hour). The answer depends only on (index, slot) and on the
/// stats map, so it holds while the map is the same: no stat registered or
/// cleared since.
struct Misses {
    map: (u64, u16, u8),
    keys: BTreeMap<(u32, u32), u32>,
}

static MISSES: Mutex<Misses> = Mutex::new(Misses { map: (0, 0, 0), keys: BTreeMap::new() });

/// `CStatsDataMgr`'s stats map (an atBinaryMap: entries, count, sorted).
fn stats_map(m: Mem) -> (u64, u16, u8) {
    const ENTRIES: u64 = 25461184;
    const COUNT: u64 = 25461192;
    const SORTED: u64 = 25461176;
    (m.u64(ENTRIES), m.u16(COUNT), m.u8(SORTED))
}

fn with_misses<R>(map: (u64, u16, u8), f: impl FnOnce(&mut BTreeMap<(u32, u32), u32>) -> R) -> R {
    let mut misses = MISSES.lock().unwrap();
    if misses.map != map {
        misses.map = map;
        misses.keys.clear();
    }
    f(&mut misses.keys)
}

fn hash_key(hash: u32) -> String {
    format!("h{hash:08x}")
}

fn packed_key_name(index: u32, slot: u32) -> String {
    format!("p{}:{index}", slot as i32)
}

/// Hooks for `packed_stats::GetPackedValue(int&, int, int)` and
/// `SetPackedValue(int, int, int)`.
pub(crate) unsafe extern "C" fn packed_get(env: *mut Env, out: u64, index: u32, slot: u32) -> u32 {
    engine_or(
        || unsafe { orig::packed_get(env, out, index, slot) },
        || write(env, out, get(&packed_key_name(index, slot))),
    )
}

pub(crate) unsafe extern "C" fn packed_set(env: *mut Env, value: u32, index: u32, slot: u32) {
    engine_or(
        || unsafe { orig::packed_set(env, value, index, slot) },
        || set(packed_key_name(index, slot), i64::from(value as i32)),
    )
}

/// Hooks for `stats_commands::CommandStatGetInt(int, int&, int)`, its
/// float, bool and masked int variants, and the setters. Floats are kept as
/// their bits.
pub(crate) unsafe extern "C" fn get_int(env: *mut Env, hash: u32, out: u64, player: u32) -> u32 {
    engine_or(|| unsafe { orig::stat_get_int(env, hash, out, player) }, || write(env, out, get(&hash_key(hash))))
}

pub(crate) unsafe extern "C" fn get_float(env: *mut Env, hash: u32, out: u64, player: u32) -> u32 {
    engine_or(|| unsafe { orig::stat_get_float(env, hash, out, player) }, || write(env, out, get(&hash_key(hash))))
}

pub(crate) unsafe extern "C" fn get_bool(env: *mut Env, hash: u32, out: u64, player: u32) -> u32 {
    engine_or(
        || unsafe { orig::stat_get_bool(env, hash, out, player) },
        || write(env, out, i64::from(get(&hash_key(hash)) != 0)),
    )
}

pub(crate) unsafe extern "C" fn get_masked_int(
    env: *mut Env,
    hash: u32,
    out: u64,
    start: u32,
    bits: u32,
    player: u32,
) -> u32 {
    engine_or(
        || unsafe { orig::stat_get_masked_int(env, hash, out, start, bits, player) },
        || write(env, out, ((get(&hash_key(hash)) as u64 >> (start & 63)) & mask(bits)) as i64),
    )
}

pub(crate) unsafe extern "C" fn set_int(env: *mut Env, hash: u32, value: u32, save: u32) -> u32 {
    engine_or(|| unsafe { orig::stat_set_int(env, hash, value, save) }, || keep(hash, i64::from(value as i32)))
}

pub(crate) unsafe extern "C" fn set_float(env: *mut Env, hash: u32, value: f32, save: u32) -> u32 {
    engine_or(|| unsafe { orig::stat_set_float(env, hash, value, save) }, || keep(hash, i64::from(value.to_bits())))
}

pub(crate) unsafe extern "C" fn set_bool(env: *mut Env, hash: u32, value: u32, save: u32) -> u32 {
    engine_or(|| unsafe { orig::stat_set_bool(env, hash, value, save) }, || keep(hash, i64::from(value != 0)))
}

pub(crate) unsafe extern "C" fn set_masked_int(
    env: *mut Env,
    hash: u32,
    value: u32,
    start: u32,
    bits: u32,
    save: u32,
) -> u32 {
    engine_or(
        || unsafe { orig::stat_set_masked_int(env, hash, value, start, bits, save) },
        || {
            let field = mask(bits) << (start & 63);
            let old = get(&hash_key(hash)) as u64;
            keep(hash, ((old & !field) | ((u64::from(value) << (start & 63)) & field)) as i64)
        },
    )
}

/// The host's answer to a get: the value in `out`, success.
fn write(env: *mut Env, out: u64, value: i64) -> u32 {
    unsafe { Mem::of(env) }.set_u32(out, value as u32);
    1
}

/// The host's answer to a set: kept, success.
fn keep(hash: u32, value: i64) -> u32 {
    set(hash_key(hash), value);
    1
}

fn mask(bits: u32) -> u64 {
    if bits >= 64 { u64::MAX } else { (1 << bits) - 1 }
}

/// Hook for `stats_commands::CommandStatDeleteSlot(int)`: a deleted
/// character's packed stats go too (its stats by hash cannot be told from
/// the others').
pub(crate) unsafe extern "C" fn delete_slot(env: *mut Env, slot: u32) {
    unsafe { orig::stat_delete_slot(env, slot) };
    let prefix = format!("p{}:", slot as i32);
    with_store(|s| {
        let before = s.values.len();
        s.values.retain(|k, _| !k.starts_with(&prefix));
        s.dirty |= s.values.len() != before;
    });
}

#[cfg(test)]
mod tests {
    #[test]
    fn masks() {
        assert_eq!(super::mask(8), 0xff);
        assert_eq!(super::mask(64), u64::MAX);
    }

    /// The same file as when each line was a string of its own.
    #[test]
    fn saved_text() {
        let values: std::collections::BTreeMap<String, i64> = [
            ("h0030fc7c", 0),
            ("h00058268", -1),
            ("p-1:4095", 255),
            ("hffffffff", i64::MIN),
            ("p3:17", i64::MAX),
            ("h7f800000", 2139095040),
        ]
        .into_iter()
        .map(|(k, v)| (k.to_string(), v))
        .collect();
        let before: String = values.iter().map(|(k, v)| format!("{k} {v}\n")).collect();
        assert_eq!(super::text(&values), before);
        assert_eq!(super::text(&Default::default()), "");
    }
}

/// This module's hooks, online.
pub(crate) fn hooks(hooks: &mut glue5_game::Hooks) {
    hooks.stat_find = Some(stat_find);
    hooks.packed_key = Some(packed_key);
    hooks.packed_get = Some(packed_get);
    hooks.packed_set = Some(packed_set);
    hooks.stat_get_int = Some(get_int);
    hooks.stat_get_float = Some(get_float);
    hooks.stat_get_bool = Some(get_bool);
    hooks.stat_get_masked_int = Some(get_masked_int);
    hooks.stat_set_int = Some(set_int);
    hooks.stat_set_float = Some(set_float);
    hooks.stat_set_bool = Some(set_bool);
    hooks.stat_set_masked_int = Some(set_masked_int);
    hooks.stat_delete_slot = Some(delete_slot);
}
