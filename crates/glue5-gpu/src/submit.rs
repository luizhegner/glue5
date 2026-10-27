//! Command buffers to the GPU, off the recording thread. wgpu encodes a pass
//! for the GPU when its command encoder is finished, which costs more than
//! recording it: each batch (what the recording thread hands over at a
//! flush) is finished by one of a few encoding threads while recording goes
//! on, and submitted in order by the submit thread, which then does what
//! must follow a submit: staging memory back to the pool, readbacks mapped,
//! destroyed objects released, the overlay drawn and the frame presented.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::mpsc::{Receiver, Sender, SyncSender, channel, sync_channel};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::done::{Done, Staging, Work};
use crate::present::Output;
use crate::uploads::FreeChunks;

/// Frames submitted but not finished by the GPU, at most.
const FRAMES_IN_FLIGHT: usize = 2;
/// Batches handed over but not submitted, at most: the recording thread
/// waits beyond (about a frame's worth).
const PENDING: usize = 8;
const ENCODE_THREADS: usize = 2;

/// What a flush hands over.
pub struct Batch {
    /// Uploads first, then the commands.
    pub encoders: Vec<wgpu::CommandEncoder>,
    /// Staging chunks the uploads copy from.
    pub staging: Vec<wgpu::Buffer>,
    /// Buffers to map once submitted, and the work their contents feed.
    pub maps: Vec<(wgpu::Buffer, Box<dyn FnOnce(Staging) -> Work + Send>)>,
    /// Work for once the GPU is done with the batch.
    pub works: Vec<Work>,
    /// Objects the batch uses that the engine has destroyed.
    pub graves: Vec<Grave>,
    pub present: Option<Present>,
    /// The batch ends a frame (paces frames in flight, window or not).
    pub frame: bool,
    /// The recording thread's count of batches (its epoch).
    pub epoch: u64,
}

/// A back buffer to show after its batch.
pub struct Present {
    /// Its colour as stored (sRGB formats read raw).
    pub view: wgpu::TextureView,
    /// For MetalFX.
    #[cfg_attr(not(target_vendor = "apple"), allow(dead_code))]
    pub texture: wgpu::Texture,
    pub width: u32,
    pub height: u32,
    /// The scene without the HUD, for a temporal upscaler (see scene.rs).
    pub scene: Option<crate::scene::SceneInputs>,
}

pub enum Grave {
    Texture(wgpu::Texture),
    Buffer(wgpu::Buffer),
}

/// Times, in nanoseconds, since the statistics last took them.
#[derive(Default)]
pub struct SubmitStats {
    pub encode_ns: AtomicU64,
    pub submit_ns: AtomicU64,
    /// Waiting for the GPU to finish older frames.
    pub gpu_wait_ns: AtomicU64,
    /// Waiting for the window's next drawable.
    pub drawable_ns: AtomicU64,
    /// Presenting it.
    pub present_ns: AtomicU64,
    /// Releasing destroyed objects.
    pub release_ns: AtomicU64,
}

impl SubmitStats {
    pub fn take(&self) -> [Duration; 6] {
        let t = |a: &AtomicU64| Duration::from_nanos(a.swap(0, Ordering::Relaxed));
        [
            t(&self.encode_ns),
            t(&self.submit_ns),
            t(&self.gpu_wait_ns),
            t(&self.drawable_ns),
            t(&self.present_ns),
            t(&self.release_ns),
        ]
    }
}

type Encoded = Receiver<Vec<wgpu::CommandBuffer>>;
type Job = (Vec<wgpu::CommandEncoder>, Sender<Vec<wgpu::CommandBuffer>>);

pub struct Submitter {
    jobs: Sender<Job>,
    batches: SyncSender<(Batch, Encoded)>,
    /// Batches handed to the encoding threads and not encoded yet.
    encoding: Arc<AtomicUsize>,
    pub stats: Arc<SubmitStats>,
}

impl Submitter {
    pub fn start(
        device: wgpu::Device,
        queue: wgpu::Queue,
        done: Done,
        free: FreeChunks,
        output: Option<Output>,
        finished: Arc<AtomicU64>,
    ) -> Submitter {
        let stats = Arc::new(SubmitStats::default());
        let encoding = Arc::new(AtomicUsize::new(0));
        let (jobs, rx) = channel::<Job>();
        let rx = Arc::new(Mutex::new(rx));
        for i in 0..ENCODE_THREADS {
            let (rx, stats, encoding) = (rx.clone(), stats.clone(), encoding.clone());
            std::thread::Builder::new()
                .name(format!("gpu encode {i}"))
                .spawn(move || {
                    glue5_runtime::qos::set(glue5_runtime::qos::Qos::Interactive);
                    loop {
                        let job = rx.lock().unwrap().recv();
                        let Ok((encoders, back)) = job else { return };
                        let t = Instant::now();
                        let buffers: Vec<_> = encoders.into_iter().map(|e| e.finish()).collect();
                        encoding.fetch_sub(1, Ordering::Release);
                        stats.encode_ns.fetch_add(t.elapsed().as_nanos() as u64, Ordering::Relaxed);
                        let _ = back.send(buffers);
                    }
                })
                .expect("spawning a GPU encoding thread");
        }
        let (batches, rx) = sync_channel(PENDING);
        let (thread_stats, thread_encoding) = (stats.clone(), encoding.clone());
        std::thread::Builder::new()
            .name("gpu submit".into())
            .spawn(move || {
                glue5_runtime::qos::set(glue5_runtime::qos::Qos::Interactive);
                run(device, queue, done, free, output, rx, thread_stats, thread_encoding, finished)
            })
            .expect("spawning the GPU submit thread");
        Submitter { jobs, batches, encoding, stats }
    }

    /// Hands a batch over: encoded now on another thread, submitted after
    /// the batches before it.
    pub fn send(&self, mut batch: Batch) {
        let (back, encoded) = channel();
        let encoders = std::mem::take(&mut batch.encoders);
        if encoders.is_empty() {
            let _ = back.send(Vec::new());
        } else {
            self.encoding.fetch_add(1, Ordering::Release);
            let _ = self.jobs.send((encoders, back));
        }
        let _ = self.batches.send((batch, encoded));
    }
}

#[allow(clippy::too_many_arguments)]
fn run(
    device: wgpu::Device,
    queue: wgpu::Queue,
    done: Done,
    free: FreeChunks,
    mut output: Option<Output>,
    rx: Receiver<(Batch, Encoded)>,
    stats: Arc<SubmitStats>,
    encoding: Arc<AtomicUsize>,
    finished: Arc<AtomicU64>,
) {
    let mut in_flight: VecDeque<wgpu::SubmissionIndex> = VecDeque::new();
    // Destroyed objects whose batches are submitted, and the frames ended
    // since they began waiting.
    let mut graves: Vec<Grave> = Vec::new();
    let mut graves_frames = 0;
    let mut last_shown: Option<Instant> = None;
    // Frames' heartbeats so far (see `Done::heartbeat`).
    let mut beats = 0;
    while let Ok((batch, encoded)) = rx.recv() {
        let Ok(mut buffers) = encoded.recv() else { return };
        // The window's frames: with frame generation, one made between the
        // previous and this one first.
        let mut frames = Vec::new();
        if let (Some(p), Some(out)) = (&batch.present, output.as_mut()) {
            let (scaled, between) = match out.upscale(&device, p) {
                Some(up) => {
                    buffers.extend(up.commands);
                    (Some(up.image), up.between.map(crate::present::Scaled::Scene))
                }
                None => (None, None),
            };
            for image in between.iter().map(Some).chain([scaled.as_ref()]) {
                let t = Instant::now();
                let acquired = out.acquire(&device);
                stats.drawable_ns.fetch_add(t.elapsed().as_nanos() as u64, Ordering::Relaxed);
                if let Some(acquired) = acquired {
                    buffers.extend(out.compose(&device, &queue, &acquired, Some(p), image));
                    frames.push(acquired);
                }
            }
        }
        let t = Instant::now();
        // A frame's heartbeat, which the GPU writes before its commands.
        let beat = batch.frame.then(|| {
            beats += 1;
            done.heartbeat(&device, &queue, beats)
        });
        let submission = queue.submit(buffers);
        // Batches the GPU has finished (objects destroyed in them can be reused).
        let (finished, epoch) = (finished.clone(), batch.epoch);
        queue.on_submitted_work_done(move || {
            finished.fetch_max(epoch, Ordering::Release);
        });
        crate::uploads::recycle(batch.staging, &free);
        // Readbacks first: a fence of the batch then means the readbacks
        // asked before it are in wasm memory (the engine waits on one
        // before freeing staging memory in bulk, see glue5-runtime's
        // device_reset.rs).
        let mut works: Vec<Work> = batch.maps.into_iter().map(|(buffer, make)| make(Staging::map(buffer))).collect();
        works.extend(batch.works);
        works.extend(beat.map(crate::done::Beat::work));
        done.after(submission.clone(), works);
        stats.submit_ns.fetch_add(t.elapsed().as_nanos() as u64, Ordering::Relaxed);
        let generated = frames.len() > 1;
        let shown = !frames.is_empty();
        if let (true, Some(out)) = (shown, output.as_mut()) {
            let t = Instant::now();
            for frame in frames {
                out.show(&device, &queue, frame, generated);
            }
            stats.present_ns.fetch_add(t.elapsed().as_nanos() as u64, Ordering::Relaxed);
            // The frame threads' clocks follow how frames keep up (Android).
            let now = Instant::now();
            if let Some(last) = last_shown.replace(now) {
                glue5_runtime::qos::frame(now - last, out.frame_budget());
            }
        }
        // A frame for a window that gave no image (hidden, minimised, an
        // Android surface gone): nothing waits for the display then, and
        // frames nobody sees came as fast as the engine made them (~270 a
        // second in menus). They keep the pace shown ones would.
        if let (false, Some(_), Some(out)) = (shown, &batch.present, output.as_ref().filter(|o| o.has_display())) {
            let due = last_shown.map_or_else(Instant::now, |last| last + out.frame_budget());
            if let Some(wait) = due.checked_duration_since(Instant::now()) {
                std::thread::sleep(wait);
            }
            last_shown = Some(Instant::now());
        }
        // Destroying takes wgpu's device-wide resource lock for writing: it
        // waits for the encoding threads, which hold it through a whole
        // batch, and whatever takes it meanwhile waits behind (the command
        // thread's recording too; 7-9 ms a frame where the engine streams).
        // So objects go when no batch is being encoded, or two frames on.
        graves.extend(batch.graves);
        graves_frames += (batch.frame && !graves.is_empty()) as u32;
        if !graves.is_empty() && (encoding.load(Ordering::Acquire) == 0 || graves_frames >= 2) {
            let t = Instant::now();
            // Drop instead of calling `destroy()`: wgpu releases the
            // resources once submissions and cached bind groups no longer
            // use them. On some Mali drivers, eager destruction let the GPU
            // read freed memory and lose its context.
            graves.clear();
            graves_frames = 0;
            stats.release_ns.fetch_add(t.elapsed().as_nanos() as u64, Ordering::Relaxed);
        }
        if batch.frame {
            in_flight.push_back(submission);
            if in_flight.len() > FRAMES_IN_FLIGHT {
                let oldest = in_flight.pop_front().unwrap();
                let t = Instant::now();
                crate::done::wait(&device, oldest);
                stats.gpu_wait_ns.fetch_add(t.elapsed().as_nanos() as u64, Ordering::Relaxed);
            }
        }
    }
}
