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

/// Read-only view of the engine's physical-voice pressure.
///
/// These addresses are the validated 8b0b5899ed layout used by the GrandV
/// player-fixes diagnostics. Every scalar is range-checked before the list is
/// read, so another runtime layout simply reports no snapshot.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct VoicePressure {
    pub physical_active: u32,
    pub physical_limit: u32,
    pub physical_target: u32,
    pub new_voice_reserve: u32,
    pub mixing_budget: u32,
    pub cpu_limited: bool,
}

const VOICE_TARGET: u64 = 12_124_968;
const VOICE_BUDGET: u64 = 12_124_976;
const VOICE_LIMIT: u64 = 12_124_988;
const VOICE_RESERVE: u64 = 12_124_992;
const VOICE_LIST: u64 = 12_140_944;
const VOICE_CPU_LIMITED: u64 = 12_141_973;
const VOICE_LAYOUT_END: u64 = 12_141_974;

fn voice_layout_valid(limit: u32, target: u32, reserve: u32, budget: u32) -> bool {
    (1..=128).contains(&limit) && target <= limit && reserve <= limit && budget <= 20_000
}

fn voice_pressure() -> Option<VoicePressure> {
    let memory = &rt().memory;
    if memory.size() < VOICE_LAYOUT_END {
        return None;
    }
    let mem = memory.mem();
    let limit = mem.u32(VOICE_LIMIT);
    if limit == 0 {
        return None;
    }
    let target = mem.u32(VOICE_TARGET);
    let reserve = mem.u32(VOICE_RESERVE);
    let budget = mem.u32(VOICE_BUDGET);
    if !voice_layout_valid(limit, target, reserve, budget) {
        return None;
    }
    let physical_active = (0..limit).filter(|&i| mem.u64(VOICE_LIST + u64::from(i) * 8) != 0).count() as u32;
    Some(VoicePressure {
        physical_active,
        physical_limit: limit,
        physical_target: target,
        new_voice_reserve: reserve,
        mixing_budget: budget,
        cpu_limited: mem.u8(VOICE_CPU_LIMITED) != 0,
    })
}

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
    /// Engine-side voice pressure, when this runtime's layout validates.
    pub voice_pressure: Option<VoicePressure>,
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
        voice_pressure: voice_pressure(),
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

#[cfg(test)]
mod tests {
    #[test]
    fn voice_pressure_layout_rejects_implausible_values() {
        assert!(super::voice_layout_valid(64, 48, 8, 2_000));
        assert!(!super::voice_layout_valid(0, 0, 0, 0));
        assert!(!super::voice_layout_valid(129, 48, 8, 2_000));
        assert!(!super::voice_layout_valid(64, 65, 8, 2_000));
        assert!(!super::voice_layout_valid(64, 48, 65, 2_000));
        assert!(!super::voice_layout_valid(64, 48, 8, 20_001));
    }
}
