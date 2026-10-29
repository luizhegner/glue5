//! What the application's performance overlay shows (glue5's ui/perf.rs):
//! the last frames' times on the renderer's thread and on the GPU, and the
//! renderer's counts, gathered only while it is on.
//!
//! GPU time is each batch's span on the GPU, summed over a frame's batches
//! (the game's rendering: what presenting adds, the upscaling and the
//! overlay, is not in it): on Apple GPUs from its command buffers'
//! completion times, elsewhere from timestamps of empty compute passes
//! (GPUs without timestamps leave it out).

use std::collections::VecDeque;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use crate::done::{Staging, StagingPool, Work};

/// Frames kept for the graph.
pub const FRAMES: usize = 120;

static ON: AtomicBool = AtomicBool::new(false);
static LIVE: Mutex<Live> = Mutex::new(Live::new());
static LAST: Mutex<Option<Snapshot>> = Mutex::new(None);

/// The overlay's figures.
#[derive(Clone, Default)]
pub struct Snapshot {
    /// Present-to-present intervals, in milliseconds, oldest first.
    pub frame_ms: Vec<f32>,
    /// The renderer thread's busy time per frame, in milliseconds.
    pub render_ms: Vec<f32>,
    /// GPU time per frame, in milliseconds (empty: not measured here).
    pub gpu_ms: Vec<f32>,
    /// The last frame's draws, passes and submits.
    pub draws: u32,
    pub passes: u32,
    pub submits: u32,
    /// What the renderer holds in textures and buffers, in bytes.
    pub gpu_bytes: u64,
}

struct Live {
    frame_ms: VecDeque<f32>,
    render_ms: VecDeque<f32>,
    gpu_ms: VecDeque<f32>,
    counts: (u32, u32, u32),
    gpu_bytes: u64,
    /// GPU seconds of the frame's batches completed so far.
    #[cfg(target_vendor = "apple")]
    gpu_pending: f64,
}

impl Live {
    const fn new() -> Live {
        Live {
            frame_ms: VecDeque::new(),
            render_ms: VecDeque::new(),
            gpu_ms: VecDeque::new(),
            counts: (0, 0, 0),
            gpu_bytes: 0,
            #[cfg(target_vendor = "apple")]
            gpu_pending: 0.0,
        }
    }
}

fn push(q: &mut VecDeque<f32>, v: f32) {
    if q.len() == FRAMES {
        q.pop_front();
    }
    q.push_back(v);
}

/// Turns the gathering on or off. The live overlay starts fresh; the last
/// session remains available to the support report after leaving the game.
pub fn set_on(on: bool) {
    let was_on = ON.swap(on, Ordering::Relaxed);
    if was_on == on {
        return;
    }
    if on {
        *LAST.lock().unwrap() = None;
        *LIVE.lock().unwrap() = Live::new();
    } else {
        let last = snapshot_live();
        *LAST.lock().unwrap() = Some(last);
        *LIVE.lock().unwrap() = Live::new();
    }
}

pub(crate) fn on() -> bool {
    ON.load(Ordering::Relaxed)
}

pub fn snapshot() -> Snapshot {
    snapshot_live()
}

/// The active metrics, or the last completed overlay session for diagnostics.
pub fn diagnostics_snapshot() -> Snapshot {
    if ON.load(Ordering::Relaxed) {
        snapshot_live()
    } else {
        LAST.lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
            .unwrap_or_default()
    }
}

fn snapshot_live() -> Snapshot {
    let live = LIVE.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    Snapshot {
        frame_ms: live.frame_ms.iter().copied().collect(),
        render_ms: live.render_ms.iter().copied().collect(),
        gpu_ms: live.gpu_ms.iter().copied().collect(),
        draws: live.counts.0,
        passes: live.counts.1,
        submits: live.counts.2,
        gpu_bytes: live.gpu_bytes,
    }
}

/// The renderer's counters at the last frame, to take the next one's from.
#[derive(Default)]
pub(crate) struct Mark {
    draws: u64,
    passes: u64,
    epoch: u64,
    busy: Duration,
    /// When a frame was last taken.
    at: Option<Instant>,
    /// When the memory was last added up.
    memory_at: Option<Instant>,
}

impl Mark {
    /// A frame was presented, `ms` after the one before: what it took from
    /// the counters (`draws`, `passes`, `busy` since the statistics were
    /// last logged, `epoch` the submits so far). True when the memory is
    /// due to be added up ([`gpu_bytes`]).
    pub(crate) fn frame(&mut self, ms: f32, draws: u64, passes: u64, busy: Duration, epoch: u64) -> bool {
        let mut live = LIVE.lock().unwrap();
        push(&mut live.frame_ms, ms);
        // The statistics' counters restart at each log, and nothing was
        // taken while the overlay was off: what they hold then is not this
        // frame's, so it only becomes the base (the last figures stay).
        let fresh = self.at.is_some_and(|t| t.elapsed() < Duration::from_millis(500));
        self.at = Some(Instant::now());
        if fresh && draws >= self.draws && passes >= self.passes && busy >= self.busy {
            push(&mut live.render_ms, (busy - self.busy).as_secs_f32() * 1000.0);
            let submits = epoch.saturating_sub(self.epoch);
            live.counts = ((draws - self.draws) as u32, (passes - self.passes) as u32, submits as u32);
        }
        (self.draws, self.passes, self.busy, self.epoch) = (draws, passes, busy, epoch);
        let due = self.memory_at.is_none_or(|t| t.elapsed() >= Duration::from_secs(1));
        if due {
            self.memory_at = Some(Instant::now());
        }
        due
    }

    /// Starts over (the statistics' counters were reset).
    pub(crate) fn reset(&mut self) {
        (self.draws, self.passes, self.busy) = (0, 0, Duration::ZERO);
        self.at = None;
    }
}

pub(crate) fn gpu_bytes(bytes: u64) {
    LIVE.lock().unwrap().gpu_bytes = bytes;
}

#[cfg(not(target_vendor = "apple"))]
fn gpu_frame(ms: f32) {
    push(&mut LIVE.lock().unwrap().gpu_ms, ms);
}

/// Times the GPU's batches, at each flush.
pub(crate) struct GpuTimer {
    #[cfg(not(target_vendor = "apple"))]
    timestamps: Option<Timestamps>,
}

/// A frame's timestamps on their way back, in `buffer`.
#[cfg(not(target_vendor = "apple"))]
pub(crate) struct Pending {
    buffer: wgpu::Buffer,
    pairs: u32,
    period: f32,
}

/// None on Apple GPUs (completion handlers time their batches).
#[cfg(target_vendor = "apple")]
pub(crate) enum Pending {}

/// A staging buffer read back with a batch, and what then becomes of it.
pub(crate) type Map = (wgpu::Buffer, Box<dyn FnOnce(Staging) -> Work + Send>);

impl Pending {
    pub(crate) fn map(self) -> Map {
        #[cfg(not(target_vendor = "apple"))]
        {
            let Pending { buffer, pairs, period } = self;
            (buffer, Box::new(move |staging| Work::GpuTime { staging, pairs, period }))
        }
        #[cfg(target_vendor = "apple")]
        match self {}
    }
}

impl GpuTimer {
    pub(crate) fn new(device: &wgpu::Device, queue: &wgpu::Queue) -> GpuTimer {
        #[cfg(not(target_vendor = "apple"))]
        return GpuTimer {
            timestamps: device
                .features()
                .contains(wgpu::Features::TIMESTAMP_QUERY)
                .then(|| Timestamps::new(device, queue)),
        };
        #[cfg(target_vendor = "apple")]
        {
            let _ = (device, queue);
            GpuTimer {}
        }
    }

    /// A batch's command encoders (`frame`: the last batch of a frame),
    /// timed while the overlay is on: on Apple GPUs between empty command
    /// buffers' completion times, elsewhere between timestamps written
    /// before the first and after the last (read back with the frame's last
    /// batch).
    pub(crate) fn time(
        &mut self,
        device: &wgpu::Device,
        encoders: &mut Vec<wgpu::CommandEncoder>,
        frame: bool,
        pool: &StagingPool,
    ) -> Option<Pending> {
        #[cfg(not(target_vendor = "apple"))]
        {
            let t = self.timestamps.as_mut()?;
            if !on() {
                t.pairs = 0;
                return None;
            }
            t.time(device, encoders, frame, pool)
        }
        #[cfg(target_vendor = "apple")]
        {
            let _ = pool;
            if !on() || (encoders.is_empty() && !frame) {
                return None;
            }
            // Empty command buffers around the batch: the span between the
            // first one's end and the last one's start (wgpu does not let
            // raw commands into an encoder that has its own).
            let ended = std::sync::Arc::new(Mutex::new(None::<f64>));
            let mut before = device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("timing") });
            {
                let ended = ended.clone();
                on_done(&mut before, move |_, end| *ended.lock().unwrap() = Some(end));
            }
            let mut after = device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("timing") });
            on_done(&mut after, move |start, _| {
                let mut live = LIVE.lock().unwrap();
                if let Some(end) = ended.lock().unwrap().take()
                    && start > end
                {
                    live.gpu_pending += start - end;
                }
                if frame {
                    let ms = std::mem::take(&mut live.gpu_pending) * 1000.0;
                    push(&mut live.gpu_ms, ms as f32);
                }
            });
            encoders.insert(0, before);
            encoders.push(after);
            None
        }
    }
}

/// Calls `done` with the GPU start and end times (in seconds) of the
/// command buffer `encoder` makes, once the GPU is past it (`encoder` must
/// have no commands of wgpu's).
#[cfg(target_vendor = "apple")]
fn on_done(encoder: &mut wgpu::CommandEncoder, done: impl Fn(f64, f64) + Send + Sync + 'static) {
    use block2::RcBlock;
    use objc2::runtime::ProtocolObject;
    use objc2_metal::MTLCommandBuffer;
    let done = RcBlock::new(move |c: std::ptr::NonNull<ProtocolObject<dyn MTLCommandBuffer>>| {
        let c = unsafe { c.as_ref() };
        done(c.GPUStartTime(), c.GPUEndTime());
    });
    unsafe {
        encoder.as_hal_mut::<wgpu::hal::api::Metal, _, _>(|e| {
            if let Some(commands) = e.and_then(|e| e.raw_command_buffer()) {
                commands.addCompletedHandler(RcBlock::as_ptr(&done));
            }
        })
    };
}

/// An empty command buffer that calls `done` with its GPU start and end
/// times (in seconds) once the GPU is past it.
#[cfg(target_vendor = "apple")]
pub(crate) fn marker(device: &wgpu::Device, done: impl Fn(f64, f64) + Send + Sync + 'static) -> wgpu::CommandBuffer {
    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("timing") });
    on_done(&mut encoder, done);
    encoder.finish()
}

/// Batches timed per frame, at most (frames have a few).
#[cfg(not(target_vendor = "apple"))]
const BATCHES: u32 = 32;

/// Query sets in turn: one per frame that may still be on the GPU.
#[cfg(not(target_vendor = "apple"))]
const SETS: usize = 4;

#[cfg(not(target_vendor = "apple"))]
struct Timestamps {
    sets: Vec<(wgpu::QuerySet, wgpu::Buffer)>,
    /// Nanoseconds per tick.
    period: f32,
    /// The set this frame's batches write, and how many have.
    set: usize,
    pairs: u32,
}

#[cfg(not(target_vendor = "apple"))]
impl Timestamps {
    fn new(device: &wgpu::Device, queue: &wgpu::Queue) -> Timestamps {
        let sets = (0..SETS)
            .map(|_| {
                let set = device.create_query_set(&wgpu::QuerySetDescriptor {
                    label: Some("frame timestamps"),
                    ty: wgpu::QueryType::Timestamp,
                    count: BATCHES * 2,
                });
                let resolve = device.create_buffer(&wgpu::BufferDescriptor {
                    label: Some("frame timestamps"),
                    size: BATCHES as u64 * 16,
                    usage: wgpu::BufferUsages::QUERY_RESOLVE | wgpu::BufferUsages::COPY_SRC,
                    mapped_at_creation: false,
                });
                (set, resolve)
            })
            .collect();
        Timestamps { sets, period: queue.get_timestamp_period(), set: 0, pairs: 0 }
    }

    fn time(
        &mut self,
        device: &wgpu::Device,
        encoders: &mut Vec<wgpu::CommandEncoder>,
        frame: bool,
        pool: &StagingPool,
    ) -> Option<Pending> {
        let (set, resolve) = &self.sets[self.set];
        // An empty compute pass writing a timestamp where it begins or ends.
        let stamp = |encoder: &mut wgpu::CommandEncoder, begin: Option<u32>, end: Option<u32>| {
            encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("timing"),
                timestamp_writes: Some(wgpu::ComputePassTimestampWrites {
                    query_set: set,
                    beginning_of_pass_write_index: begin,
                    end_of_pass_write_index: end,
                }),
            });
        };
        let timed = self.pairs < BATCHES && !encoders.is_empty();
        if timed {
            let mut first = device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("timing") });
            stamp(&mut first, Some(self.pairs * 2), None);
            encoders.insert(0, first);
            stamp(encoders.last_mut().unwrap(), None, Some(self.pairs * 2 + 1));
            self.pairs += 1;
        }
        if !frame || self.pairs == 0 {
            return None;
        }
        if encoders.is_empty() {
            encoders.push(device.create_command_encoder(&Default::default()));
        }
        let last = encoders.last_mut().unwrap();
        let bytes = self.pairs as u64 * 16;
        let staging = pool.get(device, bytes);
        last.resolve_query_set(set, 0..self.pairs * 2, resolve, 0);
        last.copy_buffer_to_buffer(resolve, 0, &staging, 0, bytes);
        let pending = Pending { buffer: staging, pairs: self.pairs, period: self.period };
        self.set = (self.set + 1) % SETS;
        self.pairs = 0;
        Some(pending)
    }
}

/// A frame's timestamps, read back: the GPU time of its batches.
#[cfg(not(target_vendor = "apple"))]
pub(crate) fn timestamps_done(data: &[u8], pairs: u32, period: f32) {
    let at = |k: usize| u64::from_le_bytes(data[k * 8..k * 8 + 8].try_into().unwrap());
    let ns: f64 = (0..pairs as usize)
        .map(|i| (at(i * 2), at(i * 2 + 1)))
        .filter(|&(begin, end)| begin != 0 && end > begin)
        .map(|(begin, end)| (end - begin) as f64 * period as f64)
        .sum();
    if on() {
        gpu_frame((ns / 1e6) as f32);
    }
}
