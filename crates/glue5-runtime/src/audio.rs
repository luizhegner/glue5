//! Audio, platform independent: the engine's mixer thread
//! (`audMixerDeviceWasmDiscard::MixThread`) writes 48 kHz stereo frames into a
//! ring in wasm memory; the platform's audio callback pulls them with
//! [`AudioOut::read`].
//!
//! The mixer keeps about 3072 frames (64 ms) queued while a consumer is alive,
//! that is while the heartbeat word keeps changing; after 250 ms without a
//! heartbeat it mixes on its own clock and drops the output.

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use crate::memory::Mem;
use crate::{nonzero, rt};

pub const SAMPLE_RATE: u32 = 48_000;
pub const CHANNELS: usize = 2;

// Ring header (u32 words), then interleaved f32 frames.
const WRITE: u64 = 0;
const READ: u64 = 4;
const CAPACITY: u64 = 8;
const HEARTBEAT: u64 = 12;
const DATA: u64 = 64;

/// The engine's audio output. Cheap to copy; silent until the engine has
/// published its ring.
#[derive(Clone, Copy, Default)]
pub struct AudioOut;

impl AudioOut {
    /// Fills `out` (interleaved stereo) with the next frames, silence where
    /// the mixer has none. Returns the frames that came from the mixer.
    /// Call it regularly while playing: each call is a heartbeat.
    pub fn read(&self, out: &mut [f32]) -> usize {
        let Some(at) = nonzero(rt().audio_ring.load(Ordering::Acquire)) else {
            out.fill(0.0);
            return 0;
        };
        let mem = rt().memory.mem();
        let taken = read_ring(mem, at, out);
        mem.atomic_u32(at + HEARTBEAT).fetch_add(1, Ordering::Release);
        let short = (out.len() / CHANNELS).saturating_sub(taken) as u64;
        if short > 0 {
            UNDERRUNS.fetch_add(1, Ordering::Relaxed);
            SILENT.fetch_add(short, Ordering::Relaxed);
        }
        taken
    }
}

/// Reads the mixer could not fill (since it published its ring), and the
/// frames of silence they played instead.
static UNDERRUNS: AtomicU64 = AtomicU64::new(0);
static SILENT: AtomicU64 = AtomicU64::new(0);

/// The sound's stats since the last call.
pub struct Stats {
    /// The frames' waits for the game's audio update (at the end of each):
    /// how many, how long in all, the longest.
    pub update_waits: u64,
    pub update_waited: Duration,
    pub update_longest: Duration,
    /// Reads of the device short of sound, and the silence it played.
    pub underruns: u64,
    pub silence: Duration,
}

pub fn take_stats() -> Stats {
    let (update_waits, update_waited, update_longest) = crate::audio_update::take_waits();
    let silent = SILENT.swap(0, Ordering::Relaxed);
    Stats {
        update_waits,
        update_waited,
        update_longest,
        underruns: UNDERRUNS.swap(0, Ordering::Relaxed),
        silence: Duration::from_secs_f64(silent as f64 / SAMPLE_RATE as f64),
    }
}

fn read_ring(mem: Mem, at: u64, out: &mut [f32]) -> usize {
    let capacity = mem.u32(at + CAPACITY);
    debug_assert!(capacity.is_power_of_two());
    let write = mem.atomic_u32(at + WRITE).load(Ordering::Acquire);
    let read = mem.atomic_u32(at + READ).load(Ordering::Relaxed);
    let frames = out.len() / CHANNELS;
    let take = (write.wrapping_sub(read).min(capacity) as usize).min(frames);

    let ring = unsafe { std::slice::from_raw_parts(mem.ptr(at + DATA) as *const f32, capacity as usize * CHANNELS) };
    let start = (read & (capacity - 1)) as usize;
    let first = take.min(capacity as usize - start);
    out[..first * CHANNELS].copy_from_slice(&ring[start * CHANNELS..(start + first) * CHANNELS]);
    out[first * CHANNELS..take * CHANNELS].copy_from_slice(&ring[..(take - first) * CHANNELS]);
    out[take * CHANNELS..].fill(0.0); // underrun (or an odd tail sample)

    mem.atomic_u32(at + READ).store(read.wrapping_add(take as u32), Ordering::Release);
    take
}
