//! The game's files from a server ([`glue5_stream`]), through the engine's
//! own HTTP file device: the browser build's way of reading `/game`.
//!
//! When the game streams, `wasm_httpfs_manifest_js` hands the engine the
//! server's file list; its `fiDeviceLocal` then serves every `/game` file
//! from that list (lookups, sizes, times, directory listings) and reads
//! through `wasm_httpfs_pread`, hooked here: a read the copy holds is a
//! positioned read from it in the calling thread, as with a local folder; a
//! missing one waits for the server. The engine also announces the reads its
//! streamer has queued (and some it expects) in a ring, drained here into
//! prefetch requests. The browser build's request slots are not used.

use std::fs::File;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use glue5_game::Env;
use glue5_stream::Client;

use crate::memory::Mem;
use crate::rt;

fn client() -> Option<&'static Arc<Client>> {
    rt().config.stream.as_ref()
}

pub(crate) fn hooks(hooks: &mut glue5_game::Hooks) {
    if client().is_some() {
        hooks.httpfs_pread = Some(pread);
    }
}

/// The manifest `wasm_httpfs_manifest_js` gives the engine, when streaming.
pub(crate) fn manifest() -> Option<&'static [u8]> {
    client()?.engine_manifest()
}

/// Hook for `wasm_httpfs_pread(id, offset, dst, len)`: bytes read, or -1
/// (the engine tries again).
unsafe extern "C" fn pread(env: *mut Env, id: u32, offset: u64, dst: u64, len: u32) -> u32 {
    let Some(client) = client() else { return u32::MAX };
    let buf = unsafe { Mem::of(env) }.bytes_mut(dst, len as usize);
    if let Some(file) = overlaid(client, id) {
        return read_at(file, buf, offset).map_or(u32::MAX, |n| n as u32);
    }
    client.read(id, offset, buf).map_or(u32::MAX, |n| n as u32)
}

/// The files the runtime changes on their way to the engine
/// ([`crate::overlay`]), read from their changed copies.
fn overlaid(client: &Client, id: u32) -> Option<&'static File> {
    static FILES: OnceLock<Vec<(u32, File)>> = OnceLock::new();
    let files = FILES.get_or_init(|| {
        let scratch = rt().config.user_dir.join("fsroot");
        let mut files = Vec::new();
        for name in crate::overlay::names() {
            let Some(id) = client.find(&format!("data/{name}")) else { continue };
            let Some(real) = client.path(id) else { continue };
            // The pack brought the whole file (it is a small XML file).
            if let Some(changed) = crate::overlay::game_file(name, &real, &scratch)
                && let Ok(f) = File::open(&changed)
            {
                files.push((id, f));
            }
        }
        files
    });
    files.iter().find(|(i, _)| *i == id).map(|(_, f)| f)
}

/// Called by the engine (`wasm_httpfs_io_start_js`) with the address of its
/// I/O table: starts draining the read announcements. False if not streaming.
pub(crate) fn start_announcements(table: u64) -> bool {
    static STARTED: AtomicBool = AtomicBool::new(false);
    let Some(client) = client().cloned() else { return false };
    if STARTED.swap(true, Ordering::AcqRel) {
        return true;
    }
    log::info!("stream: the engine's read announcements at {table:#x}");
    let spawned = crate::qos::spawn("stream hints", crate::qos::Qos::Initiated, move || {
        let m = rt().memory.mem();
        let bell = m.atomic_u32(table);
        loop {
            let seen = bell.load(Ordering::SeqCst);
            drain(m, table, &client);
            glue5_game::futex::wait32(bell, seen, Some(Duration::from_secs(1)));
        }
    });
    if let Err(e) = spawned {
        log::warn!("stream: no announcement thread: {e}");
    }
    true
}

/// The announcement ring (the engine's `httpfs_wasm.cpp` layout, 32-bit
/// words from `table`): [2] the ring's word offset, [3] its entries, [4]
/// entries claimed by the engine, [5] entries taken here. An entry is a file
/// id, the offset's low and high words, and the length, written last (0:
/// claimed, not yet written; bit 30: a guess rather than a queued read).
fn drain(m: Mem, table: u64, client: &Client) {
    let word = |i: u64| m.atomic_u32(table + 4 * i);
    let (ring, cap) = (word(2).load(Ordering::Acquire) as u64, word(3).load(Ordering::Acquire));
    if ring == 0 || cap == 0 {
        return;
    }
    let head = word(4).load(Ordering::Acquire);
    let mut tail = word(5).load(Ordering::Acquire);
    if head.wrapping_sub(tail) > cap {
        // The engine went round the ring: the oldest were overwritten.
        tail = head.wrapping_sub(cap);
    }
    // Asked for together: one write for a burst (cold starts announce
    // hundreds at once).
    let mut hints = Vec::new();
    while tail != head {
        let e = ring + 4 * (tail % cap) as u64;
        let raw = word(e + 3).load(Ordering::Acquire);
        if raw == 0 {
            break;
        }
        let off = word(e + 1).load(Ordering::Relaxed) as u64 | (word(e + 2).load(Ordering::Relaxed) as u64) << 32;
        hints.push((word(e).load(Ordering::Relaxed), off, (raw & 0x3fff_ffff) as u64, raw & 0x4000_0000 != 0));
        word(e + 3).store(0, Ordering::Release);
        tail = tail.wrapping_add(1);
    }
    word(5).store(tail, Ordering::Release);
    if !hints.is_empty() {
        client.hints(&hints);
    }
}

fn read_at(f: &File, buf: &mut [u8], off: u64) -> std::io::Result<usize> {
    let mut done = 0;
    while done < buf.len() {
        #[cfg(unix)]
        let n = std::os::unix::fs::FileExt::read_at(f, &mut buf[done..], off + done as u64)?;
        #[cfg(windows)]
        let n = std::os::windows::fs::FileExt::seek_read(f, &mut buf[done..], off + done as u64)?;
        if n == 0 {
            break;
        }
        done += n;
    }
    Ok(done)
}
