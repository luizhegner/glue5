//! The renderer: executes the engine's GPU command ring (D3D11-style
//! commands, see `glue5_runtime::gpu`) with wgpu — Metal on Apple platforms,
//! Vulkan elsewhere.
//!
//! Threads: one executes the ring in order (it owns every wgpu object the
//! engine refers to); compile threads build shader modules and pipelines; a
//! completion thread finishes what the engine waits on (fences, queries,
//! readbacks) once the GPU is done.
//!
//! With a window, the application opens the GPU first ([`Screen`]): its own
//! UI shows before the game starts, and the renderer takes the same device
//! and window over when it does.

mod bc;
pub mod census;
mod done;
mod draw;
mod exec;
mod format;
mod fsr;
#[cfg(target_vendor = "apple")]
mod metalfx;
mod objects;
mod ops;
mod overlay;
pub mod perf;
mod pipeline;
mod pool;
mod prepare;
mod present;
mod profile;
mod recycle;
mod scene;
mod shader;
mod slabs;
mod submit;
mod uniforms;
#[cfg(test)]
mod upscale_tests;
mod uploads;

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use glue5_runtime::gpu::{GpuBackend, GpuRing, State};

pub use overlay::Overlay;
pub use present::{Presentation, Upscaler, Window};

/// How the renderer runs.
#[derive(Clone, Debug)]
pub struct Settings {
    /// The translated shaders (`<game>/b/<build>/shaders`).
    pub shader_dir: PathBuf,
    /// Pipeline recipes and the driver's pipeline cache, kept across runs.
    pub cache_dir: Option<PathBuf>,
    /// Compile pipelines when first drawn, on the command thread (no draws
    /// skipped while compiling; hitches instead).
    pub sync_pipelines: bool,
    /// Every n-th presented frame to a PNG in `shot_dir` (0: never).
    pub shot_every: u32,
    pub shot_dir: Option<PathBuf>,
}

impl Settings {
    /// For the game in `game_dir` (`b/<build>/shaders`).
    pub fn for_game(game_dir: &std::path::Path) -> Settings {
        Settings::new(glue5_runtime::layout::shader_dir(game_dir))
    }

    pub fn new(shader_dir: PathBuf) -> Settings {
        Settings { shader_dir, cache_dir: None, sync_pipelines: false, shot_every: 0, shot_dir: None }
    }
}

/// Shows Apple's Metal performance HUD in windows opened from now on (on
/// Apple platforms; elsewhere it does nothing); [`Presentation::set_metal_hud`]
/// can then hide it and show it again.
///
/// Metal attaches the HUD to a layer a few frames after the layer's first
/// drawable is on screen, visible whatever the layer asked before, and
/// follows the layer's changes only after that. A HUD hidden from the start
/// would need frames after it attaches, which a window drawn only when
/// something changes doesn't have: enable it only to show it.
///
/// # Safety
///
/// It sets an environment variable, as [`std::env::set_var`].
pub unsafe fn enable_metal_hud() {
    // Metal's other switches don't fit: its MetalForceHudEnabled preference
    // shows the HUD whatever the layer says (and logs every second), and the
    // Info.plist key does nothing on macOS.
    #[cfg(target_vendor = "apple")]
    unsafe {
        std::env::set_var("MTL_HUD_ENABLED", "1")
    };
}

/// The wgpu instance for this platform (create surfaces with it).
pub fn instance() -> wgpu::Instance {
    let backends = if cfg!(target_vendor = "apple") { wgpu::Backends::METAL } else { wgpu::Backends::VULKAN };
    let mut desc = wgpu::InstanceDescriptor::new_without_display_handle();
    desc.backends = backends;
    desc.flags = wgpu::InstanceFlags::from_build_config();
    wgpu::Instance::new(desc.with_env())
}

/// The renderer as the runtime's GPU backend; presents on `screen` if given.
pub fn backend(settings: Settings, screen: Option<Screen>) -> GpuBackend {
    Box::new(move |ring: GpuRing| {
        let spawned = std::thread::Builder::new()
            .name("gpu".into())
            .spawn(move || match init(ring, settings, screen) {
                Ok(gpu) => {
                    glue5_runtime::qos::set(glue5_runtime::qos::Qos::Interactive);
                    gpu.run()
                }
                Err(e) => {
                    log::error!("gpu: {e:#}");
                    drain(ring);
                }
            });
        // (Called by the engine: a panic here would abort, with no message.)
        if let Err(e) = spawned {
            glue5_runtime::exit::fail(format!("no thread for the renderer: {e}"));
        }
    })
}

/// Without a device the engine still runs: commands are dropped.
fn drain(ring: GpuRing) {
    ring.set_state(State::Failed);
    let mut reader = ring.reader();
    loop {
        let head = reader.head();
        while reader.next(head).is_some() {}
        reader.wait(Duration::from_millis(100));
    }
}

pub(crate) const WANTED_FEATURES: wgpu::Features = wgpu::Features::TEXTURE_COMPRESSION_BC
    .union(wgpu::Features::TEXTURE_COMPRESSION_BC_SLICED_3D)
    .union(wgpu::Features::TEXTURE_COMPRESSION_ETC2)
    .union(wgpu::Features::DEPTH32FLOAT_STENCIL8)
    .union(wgpu::Features::FLOAT32_FILTERABLE)
    .union(wgpu::Features::INDIRECT_FIRST_INSTANCE)
    .union(wgpu::Features::RG11B10UFLOAT_RENDERABLE)
    .union(wgpu::Features::BGRA8UNORM_STORAGE)
    .union(wgpu::Features::DEPTH_CLIP_CONTROL)
    .union(wgpu::Features::CLIP_DISTANCES)
    .union(wgpu::Features::TEXTURE_FORMAT_16BIT_NORM)
    .union(wgpu::Features::ADDRESS_MODE_CLAMP_TO_BORDER)
    .union(wgpu::Features::TEXTURE_ADAPTER_SPECIFIC_FORMAT_FEATURES)
    .union(wgpu::Features::PIPELINE_CACHE)
    // FSR (sp-fidelity): its luminance pyramid's subgroup operations, the
    // reconstructed depth's texture atomics, half-float passes where there are.
    .union(wgpu::Features::SUBGROUP)
    .union(wgpu::Features::TEXTURE_ATOMIC)
    .union(wgpu::Features::SHADER_F16);

fn pipeline_cache_name(info: &wgpu::AdapterInfo) -> String {
    format!("pipelines.{:?}.{:x}.{:x}.bin", info.backend, info.vendor, info.device).to_lowercase()
}

/// Writes the driver's pipeline cache for the next run (Vulkan; Metal keeps
/// its own): the whole cache, megabytes, so only when pipelines were added.
fn save_pipeline_cache(compiler: &pipeline::Compiler, adapter: &wgpu::Adapter, dir: &std::path::Path) {
    // One at a time (they share the temporary file).
    static SAVING: std::sync::Mutex<()> = std::sync::Mutex::new(());
    let _one = SAVING.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    if let Some(cache) = &compiler.cache
        && let Some(data) = cache.get_data()
    {
        let path = dir.join(pipeline_cache_name(&adapter.get_info()));
        // Through a copy: a cache cut short is refused whole next time.
        let tmp = path.with_extension("bin.tmp");
        if let Err(e) = std::fs::write(&tmp, data).and_then(|()| std::fs::rename(&tmp, &path)) {
            log::warn!("gpu: saving {}: {e}", path.display());
        }
    }
}

/// The GPU and the window it shows frames in. The application opens it
/// before the game starts, to show its own UI ([`Screen::show_ui`]) and to
/// prepare the game's pipelines ([`Screen::prepare`]); the renderer takes it
/// over when the game starts ([`backend`]).
pub struct Screen {
    gpu: Device,
    output: present::Output,
    preparing: Option<Preparing>,
}

/// The known pipelines being compiled, for the game in `shader_dir`.
struct Preparing {
    shader_dir: PathBuf,
    progress: Arc<prepare::Progress>,
    thread: std::thread::JoinHandle<Result<prepare::Prepared>>,
}

impl Screen {
    pub fn open(window: Window) -> Result<Screen> {
        let gpu = Device::open(&window.instance, Some(&window.surface))?;
        let output = present::Output::new(gpu.adapter.clone(), &gpu.device, window);
        Ok(Screen { gpu, output, preparing: None })
    }

    /// The GPU without a window: frames are drawn, upscaled as
    /// `presentation` says, into a texture of its size, every `shots.0`-th
    /// to a PNG in `shots.1` (`outNNNNNN.png`).
    pub fn offscreen(presentation: Presentation, shots: Option<(u32, PathBuf)>) -> Result<Screen> {
        let gpu = Device::open(&instance(), None)?;
        let output = present::Output::offscreen(gpu.adapter.clone(), &gpu.device, presentation, shots);
        Ok(Screen { gpu, output, preparing: None })
    }

    /// Compiles the pipelines known for the game of `settings` before it
    /// starts (see `prepare.rs`), in the background: true once done, then
    /// start the game with this screen.
    pub fn prepare(&mut self, settings: &Settings) -> bool {
        if self.preparing.as_ref().is_none_or(|p| p.shader_dir != settings.shader_dir) {
            let (gpu, settings) = (self.gpu.clone(), settings.clone());
            let shader_dir = settings.shader_dir.clone();
            let progress = Arc::new(prepare::Progress::default());
            let shared = progress.clone();
            let thread = std::thread::Builder::new()
                .name("gpu prepare".into())
                .spawn(move || prepare::prepare(&gpu, &settings, &shared))
                .expect("spawning the prepare thread");
            self.preparing = Some(Preparing { shader_dir, progress, thread });
        }
        self.preparing.as_ref().is_some_and(|p| p.thread.is_finished())
    }

    /// How far [`Screen::prepare`] got (0 to 1), while it runs.
    pub fn progress(&self) -> Option<f32> {
        let p = self.preparing.as_ref().filter(|p| !p.thread.is_finished())?;
        let total = p.progress.total.load(Ordering::Relaxed);
        Some(if total == 0 { 0.0 } else { p.progress.done.load(Ordering::Relaxed) as f32 / total as f32 })
    }

    /// Shows the overlay alone (the game is not running). False when the
    /// window had no frame to give (hidden, minimised, being replaced).
    pub fn show_ui(&mut self) -> bool {
        let Screen { gpu, output, .. } = self;
        let Some(frame) = output.acquire(&gpu.device) else { return false };
        gpu.queue.submit(output.compose(&gpu.device, &gpu.queue, &frame, None, None));
        output.show(&gpu.device, &gpu.queue, frame, false);
        true
    }

    /// Whether this GPU has `upscaler`.
    pub fn has_upscaler(&self, upscaler: Upscaler) -> bool {
        self.output.presentation().has(upscaler)
    }

    /// Whether this GPU can generate frames (see [`Presentation::set_frame_generation`]).
    pub fn has_frame_generation(&self) -> bool {
        self.output.presentation().has_frame_generation()
    }
}

/// An open GPU.
#[derive(Clone)]
struct Device {
    adapter: wgpu::Adapter,
    device: wgpu::Device,
    queue: wgpu::Queue,
    features: wgpu::Features,
    /// GPU time per pass (`GLUE5_GPU_PROFILE`).
    profile: bool,
}

impl Device {
    fn open(instance: &wgpu::Instance, surface: Option<&wgpu::Surface>) -> Result<Device> {
        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::HighPerformance,
            force_fallback_adapter: false,
            compatible_surface: surface,
            apply_limit_buckets: false,
        }))
        .context("no GPU adapter")?;
        let info = adapter.get_info();
        let mut features = adapter.features() & WANTED_FEATURES;
        if std::env::var_os("GLUE5_NO_BC").is_some() {
            // Test the CPU decoding path of devices without BC compression.
            features.remove(wgpu::Features::TEXTURE_COMPRESSION_BC | wgpu::Features::TEXTURE_COMPRESSION_BC_SLICED_3D);
        }
        if std::env::var_os("GLUE5_NO_ETC2").is_some() {
            // BC textures, where decoded, kept as RGBA8 (not re-encoded).
            features.remove(wgpu::Features::TEXTURE_COMPRESSION_ETC2);
        }
        let profile = std::env::var_os("GLUE5_GPU_PROFILE").is_some()
            && adapter.features().contains(wgpu::Features::TIMESTAMP_QUERY);
        // The performance overlay's GPU time (Apple GPUs time it otherwise).
        if profile
            || (cfg!(not(target_vendor = "apple")) && adapter.features().contains(wgpu::Features::TIMESTAMP_QUERY))
        {
            features |= wgpu::Features::TIMESTAMP_QUERY;
        }
        let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
            label: Some("glue5"),
            required_features: features,
            required_limits: adapter.limits(),
            // Vulkan suballocates from blocks it frees only once empty (and
            // never the last of a kind): with its default 64-256 MB blocks a
            // Pixel's GPU memory grew 0.3-0.5 GB over a run while the live
            // resources stayed flat. Metal ignores it.
            memory_hints: wgpu::MemoryHints::Manual { suballocated_device_memory_block_size: (32 << 20)..(64 << 20) },
            ..Default::default()
        }))
        .context("creating the GPU device")?;
        log::info!("gpu: {} ({:?}, {}), features {:?}", info.name, info.backend, info.driver_info, features);
        device.on_uncaptured_error(error_handler(None));
        device.set_device_lost_callback(|reason, message| log::error!("gpu: device lost ({reason:?}): {message}"));
        Ok(Device { adapter, device, queue, features, profile })
    }
}

/// Logs device errors (the first of each kind), and counts them for the
/// engine once it runs.
fn error_handler(ring: Option<GpuRing>) -> Arc<dyn wgpu::UncapturedErrorHandler> {
    let errors = AtomicU32::new(0);
    let seen = std::sync::Mutex::new(std::collections::HashSet::new());
    Arc::new(move |e: wgpu::Error| {
        let n = errors.fetch_add(1, Ordering::Relaxed);
        if let Some(ring) = &ring {
            ring.add_errors(1);
        }
        let msg = match &e {
            wgpu::Error::Validation { description, .. } | wgpu::Error::Internal { description, .. } => {
                description.clone()
            }
            other => other.to_string(),
        };
        let key: String = msg.chars().filter(|c| !c.is_ascii_digit()).take(200).collect();
        let mut seen = seen.lock().unwrap();
        if seen.len() < 40 && seen.insert(key) {
            log::error!("gpu: device error ({n} so far): {}", msg.chars().take(1500).collect::<String>());
        }
    })
}

/// The pipeline compiler for `gpu`, with the driver's pipeline cache of
/// earlier runs where there is one (Vulkan; Metal keeps its own).
fn compiler(gpu: &Device, settings: &Settings) -> pipeline::Compiler {
    let Device { adapter, device, features, .. } = gpu;
    let mut caps = format::Caps::new(adapter, *features);
    if std::env::var_os("GLUE5_NO_FLOAT32_FILTER").is_some() {
        // Test the path of GPUs that cannot filter 32-bit float textures.
        caps.float32_filter = false;
    }
    if let Some(dir) = &settings.cache_dir {
        std::fs::create_dir_all(dir).ok();
    }
    let cache = features.contains(wgpu::Features::PIPELINE_CACHE).then(|| {
        let name = pipeline_cache_name(&adapter.get_info());
        let data = settings.cache_dir.as_ref().and_then(|d| std::fs::read(d.join(name)).ok());
        unsafe {
            device.create_pipeline_cache(&wgpu::PipelineCacheDescriptor {
                label: Some("pipelines"),
                data: data.as_deref(),
                fallback: true,
            })
        }
    });
    pipeline::Compiler::new(device.clone(), caps, cache)
}

fn init(ring: GpuRing, settings: Settings, screen: Option<Screen>) -> Result<exec::Gpu> {
    let t0 = Instant::now();
    let (gpu, output, preparing) = match screen {
        Some(screen) => (screen.gpu, Some(screen.output), screen.preparing),
        None => (Device::open(&instance(), None)?, None, None),
    };
    // Prepared by the application (for this game), else now.
    let prepared = match preparing.filter(|p| p.shader_dir == settings.shader_dir) {
        Some(p) => p.thread.join().map_err(|_| anyhow::anyhow!("preparing the pipelines panicked"))??,
        None => prepare::prepare(&gpu, &settings, &Default::default())?,
    };
    let prepare::Prepared { library, compiler, pipes, known } = prepared;
    let Device { adapter, device, queue, features: _, profile } = gpu;
    device.on_uncaptured_error(error_handler(Some(ring)));
    let caps = compiler.caps;
    let saved_compiled = compiler.compiled.load(Ordering::Relaxed);
    let threads = glue5_runtime::cores().saturating_sub(2).clamp(2, 6);
    let staging = Arc::new(done::StagingPool::default());
    let limits = device.limits();
    let window = output.is_some();
    let presentation = output.as_ref().map(|o| o.presentation().clone());
    log::info!("gpu: ready in {:.0} ms ({threads} compile threads)", t0.elapsed().as_secs_f64() * 1000.0);

    let profiler = profile.then(|| profile::Profiler::new(&device, &queue));
    let gpu_timer = perf::GpuTimer::new(&device, &queue);
    let uploads = uploads::Uploads::new(device.clone());
    let recycler = recycle::Recycler::default();
    let slabs = slabs::Slabs::new(device.clone(), recycler.finished.clone());
    Ok(exec::Gpu {
        submitter: submit::Submitter::start(
            device.clone(),
            queue.clone(),
            done::Done::start(device.clone(), ring.mem(), staging.clone()),
            uploads.free_chunks(),
            output,
            recycler.finished.clone(),
        ),
        uniforms: uniforms::Uniforms::new(limits.min_uniform_buffer_offset_alignment),
        uploads,
        adapter,
        device,
        caps,
        limits,
        library,
        compiler,
        pool: pool::Pool::new("gpu compile", threads),
        staging,
        ring,
        settings,
        objs: Default::default(),
        s: Default::default(),
        encoder: None,
        pass: None,
        pass_info: None,
        bound: Default::default(),
        epoch: 1,
        clears: Vec::new(),
        graveyard: Vec::new(),
        op: 0,
        works: Vec::new(),
        maps: Vec::new(),
        pdirty: true,
        last_pipe: None,
        last_programs: (0, 0, 0, 0),
        formats_seen: [0; 3],
        bound_formats: Default::default(),
        chunk_passes: 0,
        batch_since: None,
        waited_on: false,
        profiler,
        pass_profile: None,
        fast_pipes: Default::default(),
        pipes,
        known,
        pass_tails: Default::default(),
        recipes_new: Vec::new(),
        key: Vec::new(),
        binds: Default::default(),
        packs: Default::default(),
        dummies: Default::default(),
        blits: Default::default(),
        computes: Default::default(),
        next_buffer_uid: 0,
        slabs,
        recycler,
        decoder: None,
        decodes: Vec::new(),
        occlusion: Default::default(),
        counter_src: None,
        counter_slot: 0,
        window,
        presentation,
        scene: Default::default(),
        last_present: None,
        stats: Default::default(),
        total_presents: 0,
        started: Instant::now(),
        last_stats: Instant::now(),
        perf: Default::default(),
        gpu_timer,
        last_save: Instant::now(),
        saved_compiled,
    })
}

/// [`log_once`] for a kind of message about thing `id` (a program's):
/// nothing allocated for one said before (callers come at every draw).
pub(crate) fn log_once_for(kind: &'static str, id: u64, msg: impl FnOnce() -> String) {
    use std::collections::HashSet;
    use std::sync::Mutex;
    static SEEN: Mutex<Option<HashSet<(&'static str, u64)>>> = Mutex::new(None);
    if SEEN.lock().unwrap().get_or_insert_default().insert((kind, id)) {
        log::warn!("gpu: {}", msg());
    }
}

pub(crate) fn log_once(key: &str, msg: impl FnOnce() -> String) {
    use std::collections::HashSet;
    use std::sync::Mutex;
    static SEEN: Mutex<Option<HashSet<String>>> = Mutex::new(None);
    // (Some callers come at every draw: no copy of a key seen before.)
    let mut seen = SEEN.lock().unwrap();
    let seen = seen.get_or_insert_default();
    if !seen.contains(key) {
        seen.insert(key.to_owned());
        log::warn!("gpu: {}", msg());
    }
}
