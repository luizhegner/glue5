//! Completions: what the engine waits on (fences, queries, readbacks) is
//! finished by a thread of its own once the GPU is done with the submission,
//! so the command thread never blocks on the GPU for them.

use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::mpsc::{Receiver, Sender, channel, sync_channel};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use glue5_runtime::gpu::{OCCLUSION_VISIBLE_COUNT, store_query};
use glue5_runtime::memory::Mem;

pub enum Work {
    /// A fence: store `value` at `at` and wake waiters.
    Store {
        at: u64,
        value: u32,
    },
    Query {
        at: u64,
        result: u64,
        seq: u32,
    },
    /// GPU data into wasm memory, then the flag word to 0.
    Readback {
        staging: Staging,
        copy: Copy,
        flag: u64,
    },
    /// Occlusion results: per query, the sum of its slots' 8-byte counts.
    Occlusion {
        staging: Staging,
        queries: Vec<OcclusionResult>,
    },
    /// Pass timestamps (`GLUE5_GPU_PROFILE`).
    Profile {
        staging: Staging,
        resolved: crate::profile::Resolved,
    },
    /// A frame's batch timestamps, for the performance overlay.
    #[cfg(not(target_vendor = "apple"))]
    GpuTime {
        staging: Staging,
        pairs: u32,
        period: f32,
    },
    /// A frame's heartbeat: `value`, which the GPU wrote into `staging`
    /// with the frame if it ran it ([`Done::heartbeat`]); `moves`: the
    /// app's moves to the background and back when it was submitted.
    Heartbeat {
        staging: Staging,
        value: u64,
        moves: u32,
    },
    Screenshot {
        staging: Staging,
        width: u32,
        height: u32,
        bpr: u32,
        format: wgpu::TextureFormat,
        path: PathBuf,
        /// Counted until its file is written ([`screenshots_written`]).
        pending: Shot,
    },
}

/// A screenshot asked for and not written yet: counted from the frame that
/// takes it to its file, so a run can wait for its last ones before ending.
pub(crate) struct Shot(());

static SHOTS: (Mutex<usize>, Condvar) = (Mutex::new(0), Condvar::new());

impl Shot {
    pub(crate) fn new() -> Shot {
        *SHOTS.0.lock().unwrap() += 1;
        Shot(())
    }
}

impl Drop for Shot {
    fn drop(&mut self) {
        *SHOTS.0.lock().unwrap() -= 1;
        SHOTS.1.notify_all();
    }
}

/// A screenshot's encoding, for [`encode`].
type Encoding = Box<dyn FnOnce() + Send>;

/// Encodes a screenshot on a few threads of its own, about half the cores.
/// Their queue is short: runs taking a shot every few frames outpace them,
/// and then wait here rather than hold every frame's pixels (megabytes
/// each) for later.
fn encode(job: Encoding) {
    static QUEUE: std::sync::OnceLock<Option<std::sync::mpsc::SyncSender<Encoding>>> = std::sync::OnceLock::new();
    let queue = QUEUE.get_or_init(|| {
        use glue5_runtime::qos::{Qos, spawn};
        let threads = std::thread::available_parallelism().map_or(2, |n| (n.get() / 2).max(2));
        let (tx, rx) = sync_channel::<Encoding>(threads);
        let rx = Arc::new(Mutex::new(rx));
        let mut started = 0;
        for i in 0..threads {
            let rx = rx.clone();
            let thread = spawn(&format!("gpu screenshot {i}"), Qos::Utility, move || {
                // The lock only while taking a job (a `while let` would
                // keep it through the job, one encoding at a time).
                loop {
                    let job = rx.lock().unwrap().recv();
                    let Ok(job) = job else { break };
                    job();
                }
            });
            match thread {
                Ok(_) => started += 1,
                Err(e) => log::warn!("gpu: no thread for screenshots: {e}"),
            }
        }
        (started > 0).then_some(tx)
    });
    match queue {
        Some(tx) => {
            let _ = tx.send(job);
        }
        None => job(),
    }
}

/// Waits, up to `timeout`, for the screenshots asked for to be written
/// (encoding takes tens of milliseconds each, on threads of their own).
/// False if some were not.
pub(crate) fn screenshots_written(timeout: Duration) -> bool {
    let shots = SHOTS.0.lock().unwrap();
    let (shots, _) = SHOTS.1.wait_timeout_while(shots, timeout, |n| *n > 0).unwrap();
    *shots == 0
}

pub struct OcclusionResult {
    pub at: u64,
    pub seq: u32,
    /// An occlusion predicate: its result is whether any sample passed.
    pub predicate: bool,
    /// Byte offsets of the query's counts in the staging buffer.
    pub offsets: Vec<u32>,
}

pub enum Copy {
    /// `len` bytes from `skip` in the staging buffer (copies start on a
    /// 4-byte boundary).
    Bytes {
        to: u64,
        skip: usize,
        len: usize,
    },
    /// Rows of a texture copy (`bpr`-aligned) to the engine's pitches.
    Rows {
        to: u64,
        rows: u32,
        row_bytes: u32,
        bpr: u32,
        row_pitch: u32,
        depth: u32,
        slice_pitch: u32,
        /// Keep only this byte of each 4-byte texel (an A8 texture stored as
        /// RGBA8 reads back as A8).
        pick: Option<u8>,
    },
}

/// A mappable buffer being read back; returned to the pool afterwards.
pub struct Staging {
    pub buffer: wgpu::Buffer,
    /// Whether mapping succeeded, from the map's callback.
    result: Receiver<bool>,
    /// It is mapped (once waited for, see `mapped`).
    mapped: bool,
}

/// MAP_READ buffers, reused.
#[derive(Default)]
pub struct StagingPool {
    free: Mutex<Vec<wgpu::Buffer>>,
}

impl StagingPool {
    pub fn get(&self, device: &wgpu::Device, size: u64) -> wgpu::Buffer {
        let size = size.max(256).next_power_of_two();
        let mut free = self.free.lock().unwrap();
        if let Some(i) = free.iter().position(|b| b.size() == size) {
            return free.swap_remove(i);
        }
        drop(free);
        device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("readback"),
            size,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        })
    }

    /// Takes out the free buffers (memory is short).
    pub fn take_free(&self) -> Vec<wgpu::Buffer> {
        std::mem::take(&mut *self.free.lock().unwrap())
    }

    fn put(&self, buffer: wgpu::Buffer) {
        let mut free = self.free.lock().unwrap();
        if free.len() < 16 {
            free.push(buffer);
        }
    }
}

impl Staging {
    /// Starts mapping `buffer`; call right after the submit that fills it.
    pub fn map(buffer: wgpu::Buffer) -> Staging {
        let (tx, result) = sync_channel(1);
        buffer.slice(..).map_async(wgpu::MapMode::Read, move |r| {
            if let Err(e) = &r {
                log::warn!("gpu: mapping a readback: {e}");
            }
            let _ = tx.send(r.is_ok());
        });
        Staging { buffer, result, mapped: false }
    }
}

pub struct Done {
    tx: Sender<(wgpu::SubmissionIndex, Vec<Work>)>,
    pool: Arc<StagingPool>,
}

impl Done {
    pub fn start(device: wgpu::Device, mem: Mem, pool: Arc<StagingPool>) -> Done {
        let (tx, rx) = channel();
        let thread_pool = pool.clone();
        std::thread::Builder::new()
            .name("gpu done".into())
            .spawn(move || run(device, mem, thread_pool, rx))
            .expect("spawning the GPU completion thread");
        Done { tx, pool }
    }

    /// A small buffer the GPU writes `value` into first thing in the next
    /// submission; its work ([`Beat::work`]) follows that submit.
    ///
    /// A driver can lose the GPU's context without saying so: after a GPU
    /// page fault, Mali on MediaTek has the kernel terminate the context,
    /// and its submissions go on "completing" without running. The picture
    /// then stops for good while the game runs on; the heartbeats show it.
    pub fn heartbeat(&self, device: &wgpu::Device, queue: &wgpu::Queue, value: u64) -> Beat {
        let buffer = self.pool.get(device, 8);
        queue.write_buffer(&buffer, 0, &value.to_le_bytes());
        Beat { buffer, value, moves: MOVES.load(Ordering::Acquire) }
    }

    /// Runs `work` once `submission` has completed.
    pub fn after(&self, submission: wgpu::SubmissionIndex, work: Vec<Work>) {
        if !work.is_empty() {
            let _ = self.tx.send((submission, work));
        }
    }
}

/// A frame's heartbeat on its way to the GPU ([`Done::heartbeat`]).
pub struct Beat {
    buffer: wgpu::Buffer,
    value: u64,
    moves: u32,
}

impl Beat {
    /// What reads it back, once the submission that writes it is made.
    pub fn work(self) -> Work {
        Work::Heartbeat { staging: Staging::map(self.buffer), value: self.value, moves: self.moves }
    }
}

fn run(device: wgpu::Device, mem: Mem, pool: Arc<StagingPool>, rx: Receiver<(wgpu::SubmissionIndex, Vec<Work>)>) {
    // The engine paces its frames on what this thread completes.
    glue5_runtime::qos::set(glue5_runtime::qos::Qos::Interactive);
    while let Ok((submission, works)) = rx.recv() {
        let done = wait(&device, submission);
        for work in works {
            finish(mem, &pool, work, done);
        }
    }
}

/// One wait for the GPU, and how many in a row may end without the
/// submission done before the GPU counts as hung: 20 s at least. Drivers
/// end a GPU's work that runs for seconds themselves (Windows after 2 s,
/// Linux's after up to 10, Apple's and phones' likewise), reporting the
/// device lost; a submission is one frame's work or its uploads, behind a
/// couple of frames at most, and what takes long before the game (making
/// pipelines, loading) is the CPU's. Counted in waits, not in time: a
/// process stopped meanwhile (a debugger, iOS suspending the app) ends one
/// wait late, and the next finds the GPU done.
const WAIT: Duration = Duration::from_secs(1);
const HUNG_AFTER: u32 = 20;

/// Waits until the GPU has finished `submission`: false if it never will.
/// A GPU that stops working without the driver saying so (no device lost)
/// would otherwise hold the game still for good: it ends with the reason.
pub(crate) fn wait(device: &wgpu::Device, submission: wgpu::SubmissionIndex) -> bool {
    let start = Instant::now();
    for _ in 0..HUNG_AFTER {
        let poll = wgpu::PollType::Wait { submission_index: Some(submission.clone()), timeout: Some(WAIT) };
        match device.poll(poll) {
            Ok(_) => return true,
            Err(wgpu::PollError::Timeout) => {}
            Err(e) => {
                log::error!("gpu: waiting for a submission: {e}");
                return false;
            }
        }
    }
    glue5_runtime::exit::fail(format!(
        "the GPU has not finished the game's work in {} s: it no longer runs it, and the driver says nothing",
        start.elapsed().as_secs()
    ));
    false
}

/// The mapped contents of a staging buffer, or `None` if mapping failed.
///
/// Once its submission is `done`, the buffer is mapped and the map's
/// callback has run or is about to: wgpu runs it on the thread that found
/// the submission done, once that thread is through (the submit thread's
/// submits and waits find them too). Without `done` (the wait failed: the
/// device lost) it may never run.
fn mapped(s: &mut Staging, done: bool) -> Option<wgpu::BufferView> {
    s.mapped = if done { s.result.recv().unwrap_or(false) } else { s.result.try_recv().unwrap_or(false) };
    if s.mapped { s.buffer.slice(..).get_mapped_range().ok() } else { None }
}

fn release(pool: &StagingPool, s: Staging) {
    if s.mapped {
        s.buffer.unmap();
        pool.put(s.buffer);
    }
}

fn finish(mem: Mem, pool: &StagingPool, work: Work, done: bool) {
    match work {
        Work::Store { at, value } => mem.store_notify(at, value),
        Work::Query { at, result, seq } => store_query(mem, at, result, seq),
        Work::Readback { mut staging, copy, flag } => {
            if let Some(data) = mapped(&mut staging, done) {
                match copy {
                    Copy::Bytes { to, skip, len } => mem.bytes_mut(to, len).copy_from_slice(&data[skip..skip + len]),
                    Copy::Rows { to, rows, row_bytes, bpr, row_pitch, depth, slice_pitch, pick } => {
                        for z in 0..depth {
                            for row in 0..rows {
                                let from = ((z * rows + row) * bpr) as usize;
                                let src = &data[from..from + row_bytes as usize];
                                let dst = to + z as u64 * slice_pitch as u64 + row as u64 * row_pitch as u64;
                                match pick {
                                    None => mem.bytes_mut(dst, src.len()).copy_from_slice(src),
                                    Some(i) => {
                                        let out = mem.bytes_mut(dst, src.len() / 4);
                                        for (o, texel) in out.iter_mut().zip(src.as_chunks::<4>().0) {
                                            *o = texel[i as usize];
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            } else {
                log::warn!("gpu: a readback could not be mapped");
            }
            release(pool, staging);
            if flag != 0 {
                mem.store_notify(flag, 0);
            }
        }
        Work::Occlusion { mut staging, queries } => {
            let counts: Option<Vec<u64>> = mapped(&mut staging, done).map(|data| {
                queries
                    .iter()
                    .map(|q| {
                        q.offsets
                            .iter()
                            .map(|&o| u64::from_le_bytes(data[o as usize..o as usize + 8].try_into().unwrap()))
                            .sum()
                    })
                    .collect()
            });
            release(pool, staging);
            for (i, q) in queries.iter().enumerate() {
                // Samples that passed (counted by the patched wgpu-hal): the
                // engine divides them by an area (the sun's visibility) and
                // compares them with pixel thresholds. A lost readback counts
                // as visible.
                let count = counts.as_ref().map_or(OCCLUSION_VISIBLE_COUNT, |c| c[i]);
                store_query(mem, q.at, if q.predicate { (count != 0) as u64 } else { count }, q.seq);
            }
        }
        Work::Profile { mut staging, resolved } => {
            if let Some(data) = mapped(&mut staging, done) {
                resolved.add(&data);
            }
            release(pool, staging);
        }
        #[cfg(not(target_vendor = "apple"))]
        Work::GpuTime { mut staging, pairs, period } => {
            if let Some(data) = mapped(&mut staging, done) {
                crate::perf::timestamps_done(&data, pairs, period);
            }
            release(pool, staging);
        }
        Work::Heartbeat { mut staging, value, moves } => {
            let got = mapped(&mut staging, done).map(|data| u64::from_le_bytes(data[..8].try_into().unwrap()));
            release(pool, staging);
            heartbeat(got, value, moves);
        }
        Work::Screenshot { mut staging, width, height, bpr, format, path, pending } => {
            let pixels = mapped(&mut staging, done).map(|data| data.to_vec());
            release(pool, staging);
            // Encoding takes tens of milliseconds: not on this thread.
            if let Some(pixels) = pixels {
                encode(Box::new(move || {
                    match crate::present::write_png(&path, &pixels, width, height, bpr, format) {
                        Ok(()) => log::info!("gpu: screenshot {}", path.display()),
                        Err(e) => log::warn!("gpu: screenshot {}: {e:#}", path.display()),
                    }
                    drop(pending);
                }));
            }
        }
    }
}

/// Frames in a row whose heartbeat the GPU did not write.
static MISSED: AtomicU32 = AtomicU32::new(0);

/// Frames in a row without their heartbeat that make the GPU lost: more
/// than a few, which a phone may fail as it sends the app to the background
/// before the app learns of it.
const MISSED_MAX: u32 = 30;

/// The app's moves to the background and back, counted: odd while it is
/// there ([`set_background`]).
static MOVES: AtomicU32 = AtomicU32::new(0);

/// The app went to the background, or came back.
pub(crate) fn set_background(on: bool) {
    let _ = MOVES.try_update(Ordering::AcqRel, Ordering::Acquire, |n| (on != (n % 2 == 1)).then_some(n + 1));
}

/// A frame's heartbeat back: `got` read where the GPU was to write `value`
/// (None: not mapped, which a device lost reports on its own), for a frame
/// submitted after `moves` moves to the background and back.
fn heartbeat(got: Option<u64>, value: u64, moves: u32) {
    let Some(got) = got else { return };
    if got == value {
        MISSED.store(0, Ordering::Relaxed);
        return;
    }
    // iOS fails the GPU work of an app in the background, and wgpu counts it
    // finished: a frame submitted there, or still in flight as the app went
    // there or came back, says nothing of the GPU.
    if moves % 2 == 1 || moves != MOVES.load(Ordering::Acquire) {
        return;
    }
    if MISSED.fetch_add(1, Ordering::Relaxed) + 1 == MISSED_MAX {
        glue5_runtime::exit::fail(format!(
            "the GPU no longer runs the game's frames: they complete without their work done \
             ({MISSED_MAX} in a row; frame {value} read {got}), the driver has lost the GPU's context"
        ));
    }
}
