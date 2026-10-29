//! Rockstar's cloud, where GTA Online keeps the player's characters: files
//! in the user directory instead (`cloud/`).
//!
//! The engine saves and loads the online stats only with Rockstar's
//! services: with `-lan` (online.rs) it skips both. In its stats saves
//! manager `-lan` counts as unset, and the member files they post and get
//! go to and come from the disk.

use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};

use glue5_game::{Env, orig};

use crate::memory::Mem;

/// `PARAM_lan`'s value (non-null: set).
const PARAM_LAN: u64 = 13346752;
/// `fiDeviceGrowBuffer`, the device of files read into memory: its handles
/// are `datGrowBuffer`s.
const GROW_BUFFER_DEVICE: u64 = 8638336;
/// The stats' slots whose load failed, a bit each (the scripts and the
/// saves give up on such a slot).
const LOAD_FAILED: u64 = 25461316;
/// Slots without a file yet: a new player's (or a new character's),
/// loaded empty rather than failed.
static NEW_SLOTS: AtomicU32 = AtomicU32::new(0);
/// `netStatus`: its code, then a result code.
pub(crate) const NET_STATUS_FAILED: u32 = 2;
pub(crate) const NET_STATUS_SUCCEEDED: u32 = 3;

fn without_lan<R>(env: *mut Env, f: impl FnOnce() -> R) -> R {
    let m = unsafe { Mem::of(env) };
    let lan = m.u64(PARAM_LAN);
    m.set_u64(PARAM_LAN, 0);
    let r = f();
    m.set_u64(PARAM_LAN, lan);
    r
}

/// Hook for `rage::parElement::FindAttributeBoolValue(char const*, bool,
/// bool) const`: no stat's value comes from Rockstar's servers (the stats
/// files' "server authoritative" ones, the character's team among them, are
/// not in the character's files otherwise).
pub(crate) unsafe extern "C" fn xml_bool_attribute(env: *mut Env, element: u64, name: u64, default: u32) -> u32 {
    /// The literal "ServerAuthoritative", as `CStatsDataMgr::LoadDataXMLFile`
    /// asks for it.
    const SERVER_AUTHORITATIVE: u64 = 1596083;
    if name == SERVER_AUTHORITATIVE {
        return 0;
    }
    unsafe { orig::xml_bool_attribute(env, element, name, default) }
}

/// Hooks for the stats saves manager's `Init(unsigned int)`, `Save(...)`,
/// `Load(eStatsLoadType, int)`, `QueueSave(...)`, `QueueLoad(...)` and
/// `CStatsDataMgr::Update()` (which finishes the saves and loads): each
/// gives up with `-lan`.
pub(crate) unsafe extern "C" fn stats_init(env: *mut Env, mode: u32) {
    without_lan(env, || unsafe { orig::stats_init(env, mode) })
}

pub(crate) unsafe extern "C" fn stats_save(env: *mut Env, kind: u32, slot: u32, save_type: u32, reason: u32) -> u32 {
    without_lan(env, || unsafe { orig::stats_save(env, kind, slot, save_type, reason) })
}

pub(crate) unsafe extern "C" fn stats_load(env: *mut Env, kind: u32) -> u32 {
    without_lan(env, || unsafe { orig::stats_load(env, kind) })
}

pub(crate) unsafe extern "C" fn stats_queue_save(env: *mut Env) {
    without_lan(env, || unsafe { orig::stats_queue_save(env) })
}

pub(crate) unsafe extern "C" fn stats_queue_load(env: *mut Env) -> u32 {
    without_lan(env, || unsafe { orig::stats_queue_load(env) })
}

/// A slot without a file is a new one, not a failed one (`stats_load_failed`).
pub(crate) unsafe extern "C" fn stats_update(env: *mut Env) {
    without_lan(env, || unsafe { orig::stats_update(env) });
    let new = NEW_SLOTS.load(Ordering::Relaxed);
    if new != 0 {
        let m = unsafe { Mem::of(env) };
        m.set_u32(LOAD_FAILED, m.u32(LOAD_FAILED) & !new);
    }
}

/// Hook for `StatsInterface::CloudFileLoadFailed(int)`, what the scripts
/// ask: a slot without a file did not fail.
pub(crate) unsafe extern "C" fn stats_load_failed(env: *mut Env, slot: u32) -> u32 {
    let failed = unsafe { orig::stats_load_failed(env, slot) };
    let new = slot < 32 && NEW_SLOTS.load(Ordering::Relaxed) & (1 << slot) != 0;
    if new { 0 } else { failed }
}

/// Hook for `NetworkInterface::IsCloudAvailable(int)`, which the stats'
/// loads and saves ask first: the disk is, where the player is online (the
/// story's scripts would look for Rockstar's cloud files).
pub(crate) unsafe extern "C" fn cloud_available(env: *mut Env, gamer: u32) -> u32 {
    if unsafe { crate::online::online_here(env) } { 1 } else { unsafe { orig::cloud_available(env, gamer) } }
}

thread_local! {
    /// In an operation that needs Rockstar's credentials but goes to the
    /// disk or to friends: an online stats save or load, a presence event.
    static CREDENTIALS: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// Runs `f` with Rockstar's credentials counting as valid (see
/// `ros_credentials_valid`).
pub(crate) fn with_credentials<R>(f: impl FnOnce() -> R) -> R {
    let outer = CREDENTIALS.replace(true);
    let r = f();
    CREDENTIALS.set(outer);
    r
}

/// Hooks for `CSavegameQueuedOperation_MPStats_Load::Update()` and
/// `..._Save::Update()`: they need Rockstar's credentials (see
/// `ros_credentials_valid`).
pub(crate) unsafe extern "C" fn mpstats_load_update(env: *mut Env, op: u64) -> u32 {
    with_credentials(|| unsafe { orig::mpstats_load_update(env, op) })
}

pub(crate) unsafe extern "C" fn mpstats_save_update(env: *mut Env, op: u64) -> u32 {
    with_credentials(|| unsafe { orig::mpstats_save_update(env, op) })
}

/// Hook for `rage::rlRosCredentials::IsValid() const`: valid in the stats'
/// saves and loads, which go to the disk, and for the presence events
/// friends send each other (friends/game.rs); elsewhere the engine would
/// talk to Rockstar's services with them.
pub(crate) unsafe extern "C" fn ros_credentials_valid(env: *mut Env, credentials: u64) -> u32 {
    if CREDENTIALS.get() {
        return 1;
    }
    unsafe { orig::ros_credentials_valid(env, credentials) }
}

/// Hooks for `CProfileStats::SynchronizedFailed(eSyncType, int&) const`
/// and `Synchronized(eSyncType) const`: the profile stats (kept on Rockstar's servers, there are none
/// here) count as synchronized.
pub(crate) unsafe extern "C" fn profile_stats_failed(_env: *mut Env, _code: u64) -> u32 {
    0
}

pub(crate) unsafe extern "C" fn profile_stats_synced(_env: *mut Env, _kind: u32) -> u32 {
    1
}

/// A stats file's slot: `..._default0000.save` is 0, `..._char0001.save` 1.
fn slot(path: &str) -> Option<u32> {
    let stem = path.strip_suffix(".save")?;
    let digits = stem.len() - stem.bytes().rev().take_while(u8::is_ascii_digit).count();
    stem[digits..].parse().ok().filter(|&s: &u32| s < 32)
}

/// Where a member file is kept.
fn file(path: &str) -> PathBuf {
    let name: String = path
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_' | ' ') { c } else { '_' })
        .collect();
    // Not the folder itself or the one above it.
    let name = if name.bytes().all(|c| c == b'.') { format!("_{name}") } else { name };
    crate::rt().config.user_dir.join("cloud").join(name)
}

/// HTTP's "Not Found": no such file yet (a new player).
const NOT_FOUND: u32 = 404;

pub(crate) fn finish(m: Mem, status: u64, code: u32) {
    finish_with(m, status, code, 0);
}

fn finish_with(m: Mem, status: u64, code: u32, result: u32) {
    if status != 0 {
        m.set_u32(status + 4, result);
        m.set_u32(status, code);
    }
}

/// Hook for `rage::rlCloud::PostMemberFile(int, rlCloudOnlineService, char
/// const*, void const*, unsigned int, PostType, rlCloudFileInfo*,
/// sysMemAllocator*, netStatus*)` (the post type is not passed in this
/// build).
pub(crate) unsafe extern "C" fn post_member_file(
    env: *mut Env,
    _gamer: u32,
    _service: u32,
    path: u64,
    data: u64,
    size: u32,
    _info: u64,
    _allocator: u64,
    status: u64,
) -> u32 {
    let m = unsafe { Mem::of(env) };
    let name = m.str(path);
    let to = file(&name);
    let written = crate::write_file(&to, m.bytes(data, size as usize));
    match written {
        Ok(()) => {
            log::info!("cloud: saved {name} ({size} bytes)");
            crate::stats::save();
            if let Some(slot) = slot(&name) {
                NEW_SLOTS.fetch_and(!(1 << slot), Ordering::Relaxed);
            }
            finish(m, status, NET_STATUS_SUCCEEDED);
        }
        Err(e) => {
            log::error!("cloud: saving {name} to {}: {e}", to.display());
            finish(m, status, NET_STATUS_FAILED);
        }
    }
    1
}

/// Hook for `rage::rlCloud::GetMemberFile(int, rlCloudMemberId const&,
/// rlCloudOnlineService, char const*, rlRosSecurityFlags, unsigned long,
/// netHttpOptions, fiDevice const*, fiHandle*, rlCloudFileInfo*,
/// sysMemAllocator*, netStatus*)`: files read into memory only (the stats').
pub(crate) unsafe extern "C" fn get_member_file(
    env: *mut Env,
    _gamer: u32,
    _member: u64,
    _service: u32,
    path: u64,
    _security: u32,
    _since: u64,
    _options: u32,
    device: u64,
    handle: u64,
    _info: u64,
    _allocator: u64,
    status: u64,
) -> u32 {
    let m = unsafe { Mem::of(env) };
    let name = m.str(path);
    let data = match std::fs::read(file(&name)) {
        Ok(data) if device == GROW_BUFFER_DEVICE => data,
        Ok(_) => {
            log::warn!("cloud: {name} asked for into device {device:#x}");
            finish(m, status, NET_STATUS_FAILED);
            return 1;
        }
        // Only a file that is not there is a new character: one that could
        // not be read must not be replaced by one.
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => {
            log::error!("cloud: reading {name}: {e}");
            finish(m, status, NET_STATUS_FAILED);
            return 1;
        }
        Err(_) => {
            log::info!("cloud: no {name} yet");
            if let Some(slot) = slot(&name) {
                NEW_SLOTS.fetch_or(1 << slot, Ordering::Relaxed);
            }
            finish_with(m, status, NET_STATUS_FAILED, NOT_FOUND);
            return 1;
        }
    };
    let at = unsafe { orig::builtin_malloc(env, data.len() as u64) };
    if at == 0 {
        finish(m, status, NET_STATUS_FAILED);
        return 1;
    }
    m.bytes_mut(at, data.len()).copy_from_slice(&data);
    let appended = unsafe { orig::grow_buffer_append(env, handle, at, data.len() as u32) };
    unsafe { orig::builtin_free(env, at) };
    log::info!("cloud: loaded {name} ({} bytes)", data.len());
    finish(m, status, if appended != 0 { NET_STATUS_SUCCEEDED } else { NET_STATUS_FAILED });
    1
}

#[cfg(test)]
mod tests {
    #[test]
    fn slots() {
        assert_eq!(super::slot("GTA5/saves/mpstats/save_default0000.save"), Some(0));
        assert_eq!(super::slot("GTA5/saves/mpstats/save_char0002.save"), Some(2));
        assert_eq!(super::slot("GTA5/saves/mpstats/save.save"), None);
    }
}

/// This module's hooks, online.
pub(crate) fn hooks(hooks: &mut glue5_game::Hooks) {
    hooks.xml_bool_attribute = Some(xml_bool_attribute);
    hooks.stats_init = Some(stats_init);
    hooks.stats_update = Some(stats_update);
    hooks.stats_queue_load = Some(stats_queue_load);
    hooks.stats_queue_save = Some(stats_queue_save);
    hooks.stats_load_failed = Some(stats_load_failed);
    hooks.stats_save = Some(stats_save);
    hooks.stats_load = Some(stats_load);
    hooks.cloud_available = Some(cloud_available);
    hooks.profile_stats_failed = Some(profile_stats_failed);
    hooks.profile_stats_synced = Some(profile_stats_synced);
    hooks.mpstats_load_update = Some(mpstats_load_update);
    hooks.mpstats_save_update = Some(mpstats_save_update);
    hooks.ros_credentials_valid = Some(ros_credentials_valid);
    hooks.post_member_file = Some(post_member_file);
    hooks.get_member_file = Some(get_member_file);
}
