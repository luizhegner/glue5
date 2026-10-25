//! The command thread: executes the engine's ring in order, mirroring the
//! D3D11 device state onto wgpu.

use std::sync::Arc;
use std::sync::mpsc::TryRecvError;
use std::time::{Duration, Instant};

use glue5_runtime::gpu::{Command, GpuRing, Reader, op};
use rustc_hash::{FxHashMap, FxHashSet};
use wgpu::TextureFormat;

use crate::Settings;
use crate::done::{StagingPool, Work};
use crate::format::{self, Caps, Upload};
use crate::objects::{Buf, Layout, Obj, Objects, Query, Shader, State, Tex, View};
use crate::pipeline::{self, Compiler, Recipe, SigElement};
use crate::pool::Pool;
use crate::shader::{Library, Program, Stage};
use crate::uniforms::Uniforms;

#[derive(Clone, Copy, Default)]
pub struct VertexBinding {
    pub buf: u32,
    pub stride: u32,
    pub offset: u32,
}

#[derive(Clone, Copy, Default)]
pub struct CbBinding {
    pub buf: u32,
    /// First 16-byte constant.
    pub first: u32,
}

#[derive(Clone, Copy, Default, PartialEq, Eq)]
pub struct Target {
    pub tex: u32,
    pub mip: u32,
    pub slice: u32,
    pub format: u32,
}

/// The D3D11 state set by the engine.
pub struct D3d {
    pub shader: [u32; 3],
    pub layout: u32,
    pub topology: u32,
    pub vb: [VertexBinding; 16],
    pub ib: (u32, u32, u32),
    pub cb: [[CbBinding; 16]; 3],
    pub srv: [[u32; 128]; 3],
    pub uav: [[u32; 8]; 3],
    pub sampler: [[u32; 16]; 3],
    pub rt: [Target; 8],
    pub ds: Target,
    /// 1: read-only depth, 2: read-only stencil.
    pub ds_flags: u32,
    pub blend: u32,
    pub blend_factor: [f32; 4],
    pub depth_stencil: u32,
    pub stencil_ref: u32,
    pub raster: u32,
    pub viewport: Option<[f32; 6]>,
    pub scissor: Option<[u32; 4]>,
}

impl Default for D3d {
    fn default() -> Self {
        D3d {
            shader: [0; 3],
            layout: 0,
            topology: 4,
            vb: Default::default(),
            ib: (0, 0, 0),
            cb: Default::default(),
            srv: [[0; 128]; 3],
            uav: Default::default(),
            sampler: Default::default(),
            rt: Default::default(),
            ds: Default::default(),
            ds_flags: 0,
            blend: 0,
            blend_factor: [1.0; 4],
            depth_stencil: 0,
            stencil_ref: 0,
            raster: 0,
            viewport: None,
            scissor: None,
        }
    }
}

/// Destroyed while the commands being recorded use it.
pub use crate::submit::Grave;

/// The render pass being recorded.
pub struct PassInfo {
    pub key: [u32; 37],
    pub targets: Vec<Option<TextureFormat>>,
    pub depth: Option<TextureFormat>,
    pub samples: u32,
    pub read_only: u8,
    pub width: u32,
    pub height: u32,
    /// Interned (formats, samples, read-only flags): part of pipeline keys.
    pub tail: u32,
    /// Opened with the occlusion query set (queries can run in it).
    pub queries: bool,
}

/// What the current pass already has set: the same call twice is skipped.
#[derive(Default)]
pub struct Bound {
    pub pipeline: usize,
    pub groups: [Option<(u64, crate::draw::Offsets)>; 2],
    pub vb: [Option<(u32, u64)>; 16],
    pub ib: Option<(u32, u32, u64)>,
    pub viewport: Option<[f32; 6]>,
    pub scissor: Option<[u32; 4]>,
    pub blend: Option<[f32; 4]>,
    pub stencil_ref: Option<u32>,
    /// D3D words (bias, clamp, slope); zero: the pipelines' own (none).
    pub depth_bias: [u32; 3],
}

/// A clear not recorded yet: the next pass that targets the subresource
/// clears it as its load operation (a pass of its own otherwise).
#[derive(Clone, Copy)]
pub struct Clear {
    pub tex: u32,
    pub mip: u32,
    pub slice: u32,
    pub color: Option<wgpu::Color>,
    pub depth: Option<f32>,
    pub stencil: Option<u32>,
}

#[derive(Default)]
pub struct Stats {
    pub draws: u64,
    pub passes: u64,
    /// Render passes ended, by the command that ended them.
    pub pass_ends: FxHashMap<u32, u64>,
    pub skipped: FxHashMap<&'static str, u64>,
    pub pending_skips: u64,
    pub clamped: u64,
    pub dispatches: u64,
    pub submits: FxHashMap<&'static str, u64>,
    pub presents: u64,
    pub bind_groups: u64,
    pub busy: Duration,
    /// Present-to-present intervals, in milliseconds.
    pub frame_ms: Vec<f32>,
    pub errors: u64,
}

pub struct Gpu {
    pub adapter: wgpu::Adapter,
    pub device: wgpu::Device,
    pub caps: Caps,
    pub limits: wgpu::Limits,
    pub library: Arc<Library>,
    pub compiler: Arc<Compiler>,
    pub pool: Pool,
    pub submitter: crate::submit::Submitter,
    pub staging: Arc<StagingPool>,
    pub ring: GpuRing,
    pub settings: Settings,

    pub objs: Objects,
    pub s: D3d,

    pub encoder: Option<wgpu::CommandEncoder>,
    pub pass: Option<wgpu::RenderPass<'static>>,
    pub pass_info: Option<PassInfo>,
    pub bound: Bound,
    /// Counts submits: objects used by the commands being recorded carry it.
    pub epoch: u64,
    pub clears: Vec<Clear>,
    pub graveyard: Vec<Grave>,
    /// The command being executed.
    pub op: u32,
    /// Completions for the next submit; `maps` need their buffer mapped first.
    pub works: Vec<Work>,
    pub maps: Vec<(wgpu::Buffer, Box<dyn FnOnce(crate::done::Staging) -> Work + Send>)>,

    // pipelines
    pub pdirty: bool,
    pub last_pipe: Option<Arc<pipeline::Cell>>,
    /// Program ids (vertex, pixel) of the last draw, and the textures that
    /// make their recipes' variants (see `for_bound_formats`).
    pub last_programs: (u32, u32, u128, u128),
    /// Per stage, changes to what decides its program variant (see
    /// `for_bound_formats`): the textures bound, the depth target, objects
    /// destroyed; and the variant last picked for that count.
    pub formats_seen: [u64; 3],
    /// (program id, changes seen, variant, its 32-bit float textures, the
    /// texture slots it reads through copies).
    pub bound_formats: [Option<(u32, u64, Arc<Program>, u128, u128)>; 3],
    /// Render passes recorded since the last submit.
    pub chunk_passes: u32,
    /// When the work not yet handed over started gathering (see `idle`).
    pub batch_since: Option<Instant>,
    /// The engine may be blocked on that work (a readback): it goes to the
    /// GPU as soon as the ring is idle.
    pub waited_on: bool,
    /// GPU time per pass (`GLUE5_GPU_PROFILE`).
    pub profiler: Option<crate::profile::Profiler>,
    /// The timestamps of the open draw pass, until its first draw names it.
    pub pass_profile: Option<u32>,
    pub fast_pipes: FxHashMap<Box<[u32]>, Arc<pipeline::Cell>>,
    /// Pipelines by recipe for this device (`Recipe::for_device`).
    pub pipes: FxHashMap<Arc<Recipe>, Arc<pipeline::Cell>>,
    /// Recipes as recorded: those of the seed, of earlier runs and of this one.
    pub known: FxHashSet<Arc<Recipe>>,
    pub pass_tails: FxHashMap<(Vec<Option<TextureFormat>>, Option<TextureFormat>, u32, u8), u32>,
    pub recipes_new: Vec<Arc<Recipe>>,
    pub key: Vec<u32>,

    // resources
    pub uniforms: Uniforms,
    pub uploads: crate::uploads::Uploads,
    pub binds: crate::draw::BindCache,
    pub packs: FxHashMap<(u32, u8), crate::draw::PackCache>,
    pub dummies: crate::draw::Dummies,
    pub blits: crate::ops::Blits,
    pub computes: Arc<crate::ops::Computes>,
    pub next_buffer_uid: u32,
    pub recycler: crate::recycle::Recycler,
    pub slabs: crate::slabs::Slabs,
    /// BC textures decoded on the CPU, by threads of their own (made when
    /// the first one comes): copied in at a flush once decoded.
    pub decoder: Option<Pool>,
    pub decodes: Vec<Decode>,

    // queries
    pub occlusion: crate::ops::Occlusion,
    pub counter_src: Option<wgpu::Buffer>,
    pub counter_slot: u32,

    // presenting
    /// Frames go to a window (the submit thread presents them).
    pub window: bool,
    /// The window's presentation settings (which upscaler).
    pub presentation: Option<crate::Presentation>,
    /// The temporal upscalers' inputs (see scene.rs).
    pub scene: crate::scene::Scene,
    pub last_present: Option<Instant>,
    pub stats: Stats,
    pub total_presents: u64,
    pub started: Instant,
    pub last_stats: Instant,
    /// The counters at the last frame, for the performance overlay.
    pub perf: crate::perf::Mark,
    pub gpu_timer: crate::perf::GpuTimer,
    pub last_save: Instant,
    /// Pipelines compiled when the driver's cache was last saved.
    pub saved_compiled: u32,
}

/// A texture upload being decoded: copied in when its batch is handed over
/// (a batch's uploads go before its commands anyway).
pub struct Decode {
    data: std::sync::mpsc::Receiver<Vec<u8>>,
    /// The texture's object, and the texture it was when the data came.
    id: u32,
    texture: wgpu::Texture,
    level: u32,
    origin: wgpu::Origin3d,
    bpr: u32,
    src_rows: u32,
    rows: u32,
    extent: wgpu::Extent3d,
    row_bytes: u32,
}

/// Recorded work waits this long for more before an idle ring submits it.
const IDLE_BATCH: Duration = Duration::from_millis(4);

impl Gpu {
    pub fn run(mut self) {
        let mut reader = self.ring.reader();
        self.ring.set_state(glue5_runtime::gpu::State::Running);
        log::info!("gpu: running");
        loop {
            let head = reader.head();
            if reader.is_empty(head) {
                self.idle();
                // Until the work gathered is ripe (see `idle`).
                let timeout = match self.batch_since {
                    Some(t) => IDLE_BATCH.saturating_sub(t.elapsed()).max(Duration::from_millis(1)),
                    None => Duration::from_millis(100),
                };
                reader.wait(timeout);
                continue;
            }
            let t0 = Instant::now();
            self.execute_until(&mut reader, head);
            self.stats.busy += t0.elapsed();
            let gathering = self.encoder.is_some() || !self.works.is_empty() || !self.maps.is_empty();
            if gathering && self.batch_since.is_none() {
                self.batch_since = Some(t0);
            }
        }
    }

    fn execute_until(&mut self, reader: &mut Reader, head: u32) {
        let mut n = 0u32;
        while let Some(c) = reader.next(head) {
            self.execute(c);
            n += 1;
            if n.is_multiple_of(512) {
                // Every command copies what it needs: the space can be reused.
                reader.release();
            }
        }
        reader.release();
    }

    /// Nothing to do: complete what the engine may be waiting on.
    fn idle(&mut self) {
        // A readback the engine may be blocked on goes to the GPU now. Other
        // work once it has been gathering for a few milliseconds, else with
        // the next chunk, fence or present: the engine pauses many times a
        // frame, and every submit costs (a Metal command buffer each, about
        // thirty a frame when every pause submitted). Query results (events,
        // occlusion) are polled, frames later: they can wait as long.
        let ripe = self.batch_since.is_some_and(|t| t.elapsed() >= IDLE_BATCH);
        if self.waited_on {
            self.flush("idle (readback)");
        } else if ripe {
            self.flush("idle");
        }
        if self.last_save.elapsed() >= Duration::from_secs(20) {
            self.save_caches();
        }
    }

    fn execute(&mut self, c: Command) {
        self.op = c.op;
        match c.op {
            op::NOP | op::INIT => {}
            op::CREATE_TEXTURE => self.create_texture(&c),
            op::DESTROY_OBJECT => self.destroy(c.w(0)),
            op::CLEAR_RT => {
                let color = wgpu::Color { r: c.f(3) as f64, g: c.f(4) as f64, b: c.f(5) as f64, a: c.f(6) as f64 };
                self.clear(Clear {
                    tex: c.w(0),
                    mip: c.w(1),
                    slice: c.w(2),
                    color: Some(color),
                    depth: None,
                    stencil: None,
                });
            }
            op::CLEAR_DS => {
                let flags = c.w(3);
                self.clear(Clear {
                    tex: c.w(0),
                    mip: c.w(1),
                    slice: c.w(2),
                    color: None,
                    depth: (flags & 1 != 0).then(|| c.f(4)),
                    stencil: (flags & 2 != 0).then(|| c.w(5)),
                });
            }
            op::PRESENT => {
                let id = c.w(0);
                let size = self.objs.tex(id).map(|t| (t.width, t.height));
                crate::census::present(id, size);
                self.present(id);
            }
            op::SCENE_DONE => {
                crate::census::scene_done();
                self.scene_done();
            }
            op::FENCE => {
                self.works.push(Work::Store { at: c.addr(0), value: c.w(2) });
                self.flush("fence");
            }
            op::SCREENSHOT => {
                let len = c.w(1) as usize;
                let bytes = c.bytes_from(2);
                let name = String::from_utf8_lossy(&bytes[..len.min(bytes.len())]).into_owned();
                self.screenshot(c.w(0), &name);
            }
            op::LOG => {
                let len = c.w(0) as usize;
                let bytes = c.bytes_from(1);
                let text = String::from_utf8_lossy(&bytes[..len.min(bytes.len())]).into_owned();
                if text == "stats" {
                    self.log_stats();
                } else {
                    log::info!("gpu: {text}");
                }
            }
            op::CREATE_BUFFER => self.create_buffer(c.w(0), c.w(1), c.w(2), c.w(3), c.w(4)),
            op::UPLOAD_BUFFER | op::UPLOAD_BUFFER_NO_OVERWRITE => {
                let size = c.w(2) as usize;
                self.upload_buffer(
                    c.w(0),
                    c.w(1) as u64,
                    &c.bytes_from(3)[..size],
                    c.op == op::UPLOAD_BUFFER_NO_OVERWRITE,
                );
            }
            op::UPLOAD_TEXTURE => self.upload_texture(&c),
            op::CREATE_SHADER => self.create_shader(&c),
            op::CREATE_LAYOUT => {
                let n = c.w(1) as usize;
                let elements: Vec<_> = (0..n).map(|i| std::array::from_fn(|k| c.w(2 + i * 7 + k))).collect();
                let slots = pipeline::slots(&elements);
                self.objs.insert(c.w(0), Obj::Layout(Box::new(Layout { elements, slots })));
            }
            op::CREATE_STATE => self.create_state(&c),
            op::CREATE_SRV | op::CREATE_UAV => {
                let view = View {
                    res: c.w(2),
                    is_buffer: c.w(1) == 1,
                    format: c.w(3),
                    view_dim: c.w(4),
                    a: c.w(5),
                    b: c.w(6),
                    c: c.w(7),
                    d: c.w(8),
                    views: Vec::new(),
                };
                let obj = if c.op == op::CREATE_SRV { Obj::Srv(Box::new(view)) } else { Obj::Uav(Box::new(view)) };
                self.objs.insert(c.w(0), obj);
            }
            op::SET_SHADER => {
                if let Some(slot) = self.s.shader.get_mut(c.w(0) as usize)
                    && *slot != c.w(1)
                {
                    *slot = c.w(1);
                    self.pdirty = true;
                }
            }
            op::SET_INPUT_LAYOUT => {
                if self.s.layout != c.w(0) {
                    self.s.layout = c.w(0);
                    self.pdirty = true;
                }
            }
            op::SET_TOPOLOGY => {
                if self.s.topology != c.w(0) {
                    self.s.topology = c.w(0);
                    self.pdirty = true;
                }
            }
            op::SET_VERTEX_BUFFERS => {
                let (start, count) = (c.w(0) as usize, c.w(1) as usize);
                for i in 0..count {
                    let Some(v) = self.s.vb.get_mut(start + i) else {
                        break;
                    };
                    let (buf, stride) = (c.w(2 + i * 3), c.w(3 + i * 3));
                    if v.stride != stride || (v.buf == 0) != (buf == 0) {
                        self.pdirty = true;
                    }
                    *v = VertexBinding { buf, stride, offset: c.w(4 + i * 3) };
                }
            }
            op::SET_INDEX_BUFFER => {
                if self.s.ib.1 != c.w(1) || (self.s.ib.0 == 0) != (c.w(0) == 0) {
                    self.pdirty = true;
                }
                self.s.ib = (c.w(0), c.w(1), c.w(2));
            }
            op::SET_CBUFFERS => {
                let (stage, start, count) = (c.w(0) as usize, c.w(1) as usize, c.w(2) as usize);
                if let Some(cbs) = self.s.cb.get_mut(stage) {
                    for i in 0..count {
                        if let Some(cb) = cbs.get_mut(start + i) {
                            *cb = CbBinding { buf: c.w(3 + i * 3), first: c.w(4 + i * 3) };
                        }
                    }
                }
            }
            op::SET_SRVS | op::SET_SAMPLERS => {
                let (stage, start, count) = (c.w(0) as usize, c.w(1) as usize, c.w(2) as usize);
                let slots: &mut [u32] = match (c.op, stage) {
                    (op::SET_SRVS, 0..=2) => &mut self.s.srv[stage],
                    (_, 0..=2) => &mut self.s.sampler[stage],
                    _ => return,
                };
                let mut changed = false;
                for i in 0..count {
                    if let Some(s) = slots.get_mut(start + i) {
                        changed |= *s != c.w(3 + i);
                        *s = c.w(3 + i);
                    }
                }
                if changed && c.op == op::SET_SRVS {
                    self.formats_seen[stage] += 1;
                }
            }
            op::SET_UAVS => {
                let (stage, start, count) = (c.w(0) as usize, c.w(1) as usize, c.w(2) as usize);
                for i in 0..count {
                    let id = c.w(3 + i);
                    if let Some(s) = self.s.uav.get_mut(stage).and_then(|u| u.get_mut(start + i)) {
                        *s = id;
                    }
                    let initial = c.w(3 + count + i);
                    if initial != u32::MAX {
                        self.set_uav_counter(id, initial);
                    }
                }
            }
            op::SET_RENDER_TARGETS => {
                for i in 0..8 {
                    self.s.rt[i] =
                        Target { tex: c.w(i * 4), mip: c.w(i * 4 + 1), slice: c.w(i * 4 + 2), format: c.w(i * 4 + 3) };
                }
                let (ds_tex, ds_flags) = (self.s.ds.tex, self.s.ds_flags);
                self.s.ds = Target { tex: c.w(32), mip: c.w(33), slice: c.w(34), format: c.w(35) };
                self.s.ds_flags = c.w(36);
                if (ds_tex, ds_flags) != (self.s.ds.tex, self.s.ds_flags) {
                    self.formats_seen.iter_mut().for_each(|n| *n += 1);
                }
                self.pdirty = true;
            }
            op::SET_BLEND => {
                if self.s.blend != c.w(0) {
                    self.pdirty = true;
                }
                self.s.blend = c.w(0);
                self.s.blend_factor = [c.f(1), c.f(2), c.f(3), c.f(4)];
            }
            op::SET_DEPTH_STENCIL => {
                if self.s.depth_stencil != c.w(0) {
                    self.pdirty = true;
                }
                self.s.depth_stencil = c.w(0);
                self.s.stencil_ref = c.w(1);
            }
            op::SET_RASTER => {
                if self.s.raster != c.w(0) {
                    self.pdirty = true;
                }
                self.s.raster = c.w(0);
            }
            op::SET_VIEWPORTS => self.s.viewport = (c.w(0) > 0).then(|| std::array::from_fn(|k| c.f(1 + k))),
            op::SET_SCISSORS => self.s.scissor = (c.w(0) > 0).then(|| std::array::from_fn(|k| c.w(1 + k))),
            op::DRAW => self.draw(&c),
            op::CLEAR_STATE => {
                self.s = D3d::default();
                self.pdirty = true;
                self.formats_seen.iter_mut().for_each(|n| *n += 1);
            }
            op::COPY_REGION => self.copy_region(&c),
            op::COPY_RESOURCE => self.copy_resource(c.w(0), c.w(1)),
            op::RESOLVE => self.resolve(c.w(0), c.w(1), c.w(2), c.w(3)),
            op::DISPATCH => self.dispatch(c.w(0), c.w(1), c.w(2)),
            op::CLEAR_UAV => self.clear_uav(c.w(0), [c.w(2), c.w(3), c.w(4), c.w(5)]),
            op::GENERATE_MIPS => self.generate_mips(c.w(0)),
            op::READBACK => self.readback(&c),
            op::COPY_COUNTER => self.copy_counter(c.w(0), c.w(1), c.w(2)),
            op::CREATE_QUERY => {
                self.objs
                    .insert(c.w(0), Obj::Query(Box::new(Query { kind: c.w(1), at: c.addr(2), slots: Vec::new() })));
                self.occlusion.ensure_set(&self.device);
            }
            op::BEGIN_QUERY => self.begin_query(c.w(0)),
            op::END_QUERY => self.end_query(c.w(0), c.w(1)),
            other => crate::log_once(&format!("op{other}"), || format!("unknown opcode {other}")),
        }
    }

    // ---- encoder ---------------------------------------------------------------

    /// Timestamps for a pass about to begin, when profiling.
    pub fn time_pass(&mut self, label: &str) -> Option<u32> {
        self.profiler.as_mut()?.next(label)
    }

    pub fn encoder(&mut self) -> &mut wgpu::CommandEncoder {
        self.end_pass();
        self.encoder.get_or_insert_with(|| self.device.create_command_encoder(&Default::default()))
    }

    pub fn end_pass(&mut self) {
        if self.pass.is_some() {
            *self.stats.pass_ends.entry(self.op).or_default() += 1;
            self.end_occlusion_in_pass();
            self.pass = None;
            self.pass_info = None;
        }
    }

    /// Hands everything recorded so far (and the clears still pending) to
    /// the submit thread.
    pub fn flush(&mut self, reason: &'static str) {
        self.flush_with(reason, None, false);
    }

    /// `frame`: the batch ends a frame; `present`: what to show after it.
    pub fn flush_with(&mut self, reason: &'static str, present: Option<crate::submit::Present>, frame: bool) {
        self.flush_clears();
        self.end_pass();
        self.uniforms.flush(&mut self.uploads);
        self.resolve_occlusion();
        self.resolve_profile();
        self.finish_decodes();
        self.recycler.trim(self.epoch, &mut self.graveyard);
        let (uploads, staging) = self.uploads.finish();
        let mut encoders = uploads.into_iter().chain(self.encoder.take()).collect();
        if let Some(timing) = self.gpu_timer.time(&self.device, &mut encoders, frame, &self.staging) {
            self.maps.push(timing.map());
        }
        *self.stats.submits.entry(reason).or_default() += 1;
        self.submitter.send(crate::submit::Batch {
            encoders,
            staging,
            maps: std::mem::take(&mut self.maps),
            works: std::mem::take(&mut self.works),
            graves: std::mem::take(&mut self.graveyard),
            present,
            frame,
            epoch: self.epoch,
        });
        self.epoch += 1;
        self.counter_slot = 0;
        self.chunk_passes = 0;
        self.batch_since = None;
        self.waited_on = false;
    }

    // ---- objects -----------------------------------------------------------------

    fn create_texture(&mut self, c: &Command) {
        let (id, dim, w, h, d, mips, array, dxgi, bind, samples) =
            (c.w(0), c.w(1), c.w(2), c.w(3), c.w(4), c.w(5), c.w(6), c.w(7), c.w(8), c.w(9));
        let mut tf = format::texture(dxgi, bind & format::BIND_DS != 0, &self.caps);
        if self.objs.tex(id).is_some() {
            return;
        }
        let dimension = match dim {
            1 => wgpu::TextureDimension::D1,
            3 => wgpu::TextureDimension::D3,
            _ => wgpu::TextureDimension::D2,
        };
        let samples = samples.max(1);
        let mut usage = wgpu::TextureUsages::TEXTURE_BINDING;
        if samples == 1 {
            usage |= wgpu::TextureUsages::COPY_SRC | wgpu::TextureUsages::COPY_DST;
        }
        if bind & (format::BIND_RT | format::BIND_DS) != 0 {
            usage |= wgpu::TextureUsages::RENDER_ATTACHMENT;
        }
        if bind & format::BIND_UAV != 0 && samples == 1 {
            usage |= wgpu::TextureUsages::STORAGE_BINDING;
        }
        if let format::Upload::Bc(_) = tf.upload {
            crate::log_once("nobc", || {
                format!(
                    "the device has no BC texture compression: decoding BC textures on the CPU{}",
                    if tf.format.is_compressed() { " (colour re-encoded as ETC2)" } else { "" }
                )
            });
        }
        let layers = if dimension == wgpu::TextureDimension::D3 { d } else { array.max(1) };
        let size = wgpu::Extent3d {
            width: w.max(1),
            height: if dim == 1 { 1 } else { h.max(1) },
            depth_or_array_layers: layers.max(1),
        };
        // BC decoded on the CPU takes 4-8 times the memory (1-2 times
        // re-encoded as ETC2), which phones cannot spare (on iOS it counts
        // against the app's limit): large mipmapped textures lose their first
        // level, a quarter of the texels to decode and keep. Small and
        // unmipmapped ones (UI) stay whole.
        let dropped = match tf.upload {
            format::Upload::Bc(_)
                if mips >= 2 && dimension == wgpu::TextureDimension::D2 && size.width.max(size.height) >= 512 =>
            {
                1
            }
            _ => 0,
        };
        let gpu_size = wgpu::Extent3d {
            width: (size.width >> dropped).max(1),
            height: (size.height >> dropped).max(1),
            depth_or_array_layers: size.depth_or_array_layers,
        };
        // Compressed textures are whole blocks: a first level that is not
        // (half an odd number of blocks) stays decoded; so do 3D textures,
        // which WebGPU has in BC (TEXTURE_COMPRESSION_BC_SLICED_3D) but not ETC2.
        if matches!(tf.upload, format::Upload::Bc(_))
            && tf.format.is_compressed()
            && (gpu_size.width % 4 != 0 || gpu_size.height % 4 != 0 || dimension == wgpu::TextureDimension::D3)
        {
            tf = tf.uncompressed();
        }
        let view_formats: Vec<TextureFormat> = tf.view.into_iter().collect();
        let key = crate::recycle::TexKey {
            format: tf.format,
            view: tf.view,
            dim: dimension,
            size: gpu_size,
            mips: mips.max(1) - dropped,
            samples,
            usage,
        };
        let tex = match self.recycler.texture(&key) {
            Some(tex) => tex,
            None => {
                let scope = self.device.push_error_scope(wgpu::ErrorFilter::Validation);
                let tex = self.device.create_texture(&wgpu::TextureDescriptor {
                    label: None,
                    size: gpu_size,
                    mip_level_count: key.mips,
                    sample_count: samples,
                    dimension,
                    format: tf.format,
                    usage,
                    view_formats: &view_formats,
                });
                if let Some(e) = pollster::block_on(scope.pop()) {
                    crate::log_once(&format!("tex{:?}{dim}{samples}", tf.format), || {
                        format!(
                            "texture {w}x{h}x{layers} {:?} x{samples} (DXGI {dxgi}, bind {bind:#x}) failed: {e}",
                            tf.format
                        )
                    });
                    return;
                }
                tex
            }
        };
        let bytes = texture_bytes(tf.format, gpu_size, mips.max(1) - dropped, dimension) * samples as u64;
        self.objs.insert(
            id,
            Obj::Texture(Box::new(Tex {
                tex,
                format: tf.format,
                upload: tf.upload,
                dim: dimension,
                width: size.width,
                height: size.height,
                layers: size.depth_or_array_layers,
                mips: mips.max(1),
                dropped,
                samples,
                bytes,
                recycle: Some(key),
                views: Vec::new(),
                used: Default::default(),
                uploaded: Default::default(),
                in_bind_group: Default::default(),
                proxies: Default::default(),
                mip_chain: None,
                present: None,
            })),
        );
    }

    fn create_buffer(&mut self, id: u32, size: u32, bind: u32, misc: u32, stride: u32) {
        let mut usage = wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::COPY_SRC;
        if bind & format::BIND_VERTEX != 0 {
            usage |= wgpu::BufferUsages::VERTEX;
        }
        if bind & format::BIND_INDEX != 0 {
            usage |= wgpu::BufferUsages::INDEX;
        }
        if bind & (format::BIND_SRV | format::BIND_UAV) != 0 {
            usage |= wgpu::BufferUsages::STORAGE;
        }
        if misc & format::MISC_DRAWINDIRECT_ARGS != 0 && self.caps.indirect {
            usage |= wgpu::BufferUsages::INDIRECT;
        }
        let needs_gpu = bind & (format::BIND_VERTEX | format::BIND_INDEX | format::BIND_SRV | format::BIND_UAV) != 0
            || misc & format::MISC_DRAWINDIRECT_ARGS != 0;
        let bytes = (size as u64).next_multiple_of(4).max(4);
        let (mut gpu, mut slot) = (None, None);
        if needs_gpu && crate::slabs::Slabs::takes(bytes, usage) {
            let (s, g) = self.slabs.alloc(bytes);
            (gpu, slot) = (Some(g), Some(s));
        } else if needs_gpu {
            gpu = Some(self.new_gpu_buffer(bytes, usage));
        }
        self.next_buffer_uid += 1;
        self.objs.insert(
            id,
            Obj::Buffer(Box::new(Buf {
                size: size as u64,
                stride,
                gpu,
                base: slot.map_or(0, |s| s.offset),
                slot,
                bytes,
                uid: self.next_buffer_uid,
                usage,
                shadow: (bind & format::BIND_CONSTANT != 0).then(|| vec![0; size as usize]),
                version: 0,
                used: Default::default(),
                uploaded: Default::default(),
                in_bind_group: Default::default(),
                constants: Default::default(),
                counter: None,
            })),
        );
    }

    /// A buffer the engine destroyed (see `recycle.rs`), else a new one.
    fn new_gpu_buffer(&mut self, bytes: u64, usage: wgpu::BufferUsages) -> wgpu::Buffer {
        self.recycler.buffer(bytes, usage).unwrap_or_else(|| {
            self.device.create_buffer(&wgpu::BufferDescriptor {
                label: None,
                size: bytes,
                usage,
                mapped_at_creation: false,
            })
        })
    }

    fn upload_buffer(&mut self, id: u32, offset: u64, data: &[u8], no_overwrite: bool) {
        let epoch = self.epoch;
        let Some(b) = self.objs.buf_mut(id) else {
            return;
        };
        if let Some(shadow) = &mut b.shadow {
            let end = (offset as usize + data.len()).min(shadow.len());
            if (offset as usize) < end {
                shadow[offset as usize..end].copy_from_slice(&data[..end - offset as usize]);
            }
            b.version = b.version.wrapping_add(1);
        }
        if b.gpu.is_none() {
            return;
        }
        if !no_overwrite && b.used.get() == epoch {
            if offset == 0 && data.len() as u64 >= b.size {
                // D3D map-discard: commands already recorded keep the old
                // contents; a new allocation takes the new ones.
                let (bytes, usage) = (b.bytes, b.usage);
                let old = b.gpu.take();
                let (fresh, slot) = match b.slot.take() {
                    Some(old) => {
                        self.slabs.free(self.epoch, old);
                        let (slot, g) = self.slabs.alloc(bytes);
                        (g, Some(slot))
                    }
                    None => {
                        if let Some(old) = old {
                            self.recycler.keep_buffer(self.epoch, old);
                        }
                        (self.new_gpu_buffer(bytes, usage), None)
                    }
                };
                self.next_buffer_uid += 1;
                let b = self.objs.buf_mut(id).unwrap();
                b.gpu = Some(fresh);
                b.base = slot.map_or(0, |s| s.offset);
                b.slot = slot;
                b.uid = self.next_buffer_uid;
                b.used.set(0);
            } else {
                self.flush("buffer upload");
            }
        }
        let b = self.objs.buf(id).unwrap();
        let len = data.len().min(b.bytes.saturating_sub(offset) as usize);
        if !offset.is_multiple_of(4) || (!len.is_multiple_of(4) && offset + len as u64 != b.size) {
            // Copies move whole words: the bytes around an unaligned range would be lost.
            crate::log_once("unaligned upload", || {
                format!("unaligned buffer upload ({len} bytes at {offset} of {})", b.size)
            });
            if !offset.is_multiple_of(4) {
                return;
            }
        }
        let (gpu, base) = (b.gpu.clone().unwrap(), b.base);
        b.uploaded.set(self.epoch);
        self.uploads.buffer(&gpu, base + offset, &data[..len]);
    }

    fn upload_texture(&mut self, c: &Command) {
        let (id, mip, slice, x, y, z, w, h, d, bpr, rows, size) =
            (c.w(0), c.w(1), c.w(2), c.w(3), c.w(4), c.w(5), c.w(6), c.w(7), c.w(8), c.w(9), c.w(10), c.w(11));
        if self.objs.tex(id).is_none() {
            return;
        }
        if self.clears.iter().any(|cl| cl.tex == id) {
            self.flush_clears();
        }
        if self.objs.tex(id).unwrap().used.get() == self.epoch {
            self.flush("texture upload");
        }
        let t = self.objs.tex_mut(id).unwrap();
        t.mark_written();
        let Some(level) = t.level(mip) else {
            return; // a level the GPU texture does without
        };
        let data = &c.bytes_from(12)[..size as usize];
        let origin = wgpu::Origin3d { x, y, z: if t.is_3d() { z } else { slice } };
        let d = d.max(1);
        if let Upload::Bc(bc) = t.upload {
            // Decoded by a thread of its own while recording goes on, copied
            // in at the flush. Re-encoded as ETC2 (a compressed format) the
            // data is whole blocks, as BC data; decoded texels are cropped to
            // the mip (block-padded regions may overhang it).
            let etc2 = t.format.is_compressed();
            let (cols, block_rows) = (w.div_ceil(4), h.div_ceil(4));
            let src_rows = rows.max(block_rows) as usize;
            // Bytes per data row, data rows per layer, rows and texels to copy, bytes per copied row.
            let (data_bpr, data_rows, copy_rows, (cw, ch), row_bytes) = if etc2 {
                let row = cols * crate::bc::ETC2_RGBA_BLOCK_BYTES;
                (row, block_rows, block_rows, (cols * 4, block_rows * 4), row)
            } else {
                let texel = bc.texel_bytes() as u32;
                let (mw, mh) = ((t.width >> mip).max(1), (t.height >> mip).max(1));
                let (cw, ch) = (w.min(mw.saturating_sub(x)), h.min(mh.saturating_sub(y)));
                (cols * 4 * texel, block_rows * 4, ch, (cw, ch), cw * texel)
            };
            let (texture, src) = (t.tex.clone(), data.to_vec());
            t.uploaded.set(self.epoch);
            let (tx, rx) = std::sync::mpsc::channel();
            let bpr = bpr as usize;
            self.decoder.get_or_insert_with(|| Pool::new("gpu decode", 2)).urgent(move || {
                let texels = crate::bc::decode(bc, &src, bpr, src_rows, cols as usize, block_rows as usize, d as usize);
                let _ = tx.send(if etc2 { crate::bc::etc2_rgba(texels, cols * 4, block_rows * 4 * d) } else { texels });
            });
            self.decodes.push(Decode {
                data: rx,
                id,
                texture,
                level,
                origin,
                bpr: data_bpr,
                src_rows: data_rows,
                rows: copy_rows,
                extent: wgpu::Extent3d { width: cw, height: ch, depth_or_array_layers: d },
                row_bytes,
            });
            return;
        }
        let converted;
        // Source (data, bytes per row, rows per layer), rows to copy, copy extent (w, h), bytes per row.
        let (data, bpr, src_rows, copy_rows, (width, height), row_bytes) = match t.upload {
            Upload::Direct => {
                let (bytes, block) = format::block(t.format);
                let (cols, block_rows) = (w.div_ceil(block), h.div_ceil(block));
                // Compressed copies cover whole blocks.
                (data, bpr, rows.max(block_rows), block_rows, (cols * block, block_rows * block), cols * bytes)
            }
            Upload::A8 => {
                let mut out = vec![0u8; (w * h * d * 4) as usize];
                for layer in 0..d {
                    for r in 0..h {
                        for col in 0..w {
                            let src = (layer * rows.max(h) * bpr + r * bpr + col) as usize;
                            out[((layer * h + r) * w + col) as usize * 4 + 3] = data.get(src).copied().unwrap_or(0);
                        }
                    }
                }
                converted = out;
                (&converted[..], w * 4, h, h, (w, h), w * 4)
            }
            Upload::Unorm16 => {
                let row = w * format::block(t.format).0;
                let mut out = Vec::with_capacity((row * h * d) as usize);
                for layer in 0..d {
                    for r in 0..h {
                        let src = (layer * rows.max(h) * bpr + r * bpr) as usize;
                        let line = data.get(src..).unwrap_or_default();
                        for i in 0..row as usize / 2 {
                            let v = line.get(2 * i..2 * i + 2).map_or(0, |b| u16::from_le_bytes([b[0], b[1]]));
                            out.extend_from_slice(&half::f16::from_f32(f32::from(v) / 65535.0).to_le_bytes());
                        }
                    }
                }
                converted = out;
                (&converted[..], row, h, h, (w, h), row)
            }
            Upload::Bc(_) => unreachable!("decoded on another thread"),
        };
        let extent = wgpu::Extent3d { width, height, depth_or_array_layers: d };
        let tex = t.tex.clone();
        t.uploaded.set(self.epoch);
        self.uploads.texture(
            wgpu::TexelCopyTextureInfo { texture: &tex, mip_level: level, origin, aspect: wgpu::TextureAspect::All },
            data,
            bpr,
            src_rows,
            copy_rows,
            extent,
            row_bytes,
        );
    }

    /// Copies in the textures decoded so far. One still decoding waits for
    /// a later flush, unless the commands being handed over use its texture:
    /// the frame waits only for the textures it draws (streaming brings
    /// hundreds of decodes at once, hundreds of milliseconds on a phone).
    /// A texture's regions go in the order they came.
    fn finish_decodes(&mut self) {
        let mut later: Vec<Decode> = Vec::new();
        // The textures with a region waiting (a set: hundreds wait at times).
        let mut waiting = rustc_hash::FxHashSet::default();
        for d in std::mem::take(&mut self.decodes) {
            let Some(t) = self.objs.tex(d.id).filter(|t| t.tex == d.texture) else {
                continue; // destroyed meanwhile
            };
            let used_now = t.used.get() == self.epoch;
            let texels = if waiting.contains(&d.id) {
                None
            } else if used_now {
                let Ok(texels) = d.data.recv() else { continue };
                Some(texels)
            } else {
                match d.data.try_recv() {
                    Ok(texels) => Some(texels),
                    Err(TryRecvError::Empty) => None,
                    Err(TryRecvError::Disconnected) => continue,
                }
            };
            let Some(texels) = texels else {
                waiting.insert(d.id);
                later.push(d);
                continue;
            };
            let target = wgpu::TexelCopyTextureInfo {
                texture: &d.texture,
                mip_level: d.level,
                origin: d.origin,
                aspect: wgpu::TextureAspect::All,
            };
            self.uploads.texture(target, &texels, d.bpr, d.src_rows, d.rows, d.extent, d.row_bytes);
        }
        self.decodes = later;
    }

    fn create_shader(&mut self, c: &Command) {
        let (id, stage) = (c.w(0), c.w(1));
        let hash = c.w(2) as u64 | (c.w(3) as u64) << 32;
        let n = c.w(4) as usize;
        let sig: Arc<[SigElement]> = (0..n)
            .map(|i| SigElement { semantic: c.w(5 + i * 4), index: c.w(6 + i * 4), register: c.w(7 + i * 4) })
            .collect();
        let Some(stage) = Stage::from_d3d(stage) else {
            // Hull/domain/geometry shaders have no WebGPU counterpart: draws using them are skipped.
            return;
        };
        let slot = self.library.slot(hash);
        if slot.get().is_none() {
            let (library, device, slot) = (self.library.clone(), self.device.clone(), slot.clone());
            let computes = (stage == Stage::Compute).then(|| self.computes.clone());
            self.pool.urgent(move || {
                let program = slot.get_or_init(|| library.prepare(&device, hash, stage));
                // A dispatch would otherwise compile its pipeline on the command thread.
                if let (Some(computes), Some(program)) = (computes, program) {
                    computes.get(&device, program, &[]);
                }
            });
        }
        self.objs.insert(id, Obj::Shader(Box::new(Shader { stage, hash, sig, slot })));
    }

    fn create_state(&mut self, c: &Command) {
        let (id, kind) = (c.w(0), c.w(1));
        let words: Box<[u32]> = c.payload.get(2..).unwrap_or(&[]).into();
        let (sampler, comparison) =
            if kind == 3 { (Some(self.make_sampler(&words, false)), words[0] & 0x80 != 0) } else { (None, false) };
        self.objs.insert(id, Obj::State(Box::new(State { words, sampler, comparison, nearest: None })));
    }

    /// D3D11_SAMPLER_DESC: Filter, AddressU/V/W, MipLODBias, MaxAnisotropy,
    /// ComparisonFunc, BorderColor[4], MinLOD, MaxLOD. Filter bits: 4-5 min,
    /// 2-3 mag, 0-1 mip (set = linear), 0x80 comparison, 0x55 anisotropic.
    /// `nearest`: without filtering, whatever the engine asked.
    pub fn make_sampler(&self, w: &[u32], nearest: bool) -> wgpu::Sampler {
        let w = |i: usize| w.get(i).copied().unwrap_or(0);
        let filter = if nearest { w(0) & 0x80 } else { w(0) };
        let aniso = filter & 0x7f == 0x55;
        let linear = |bit: u32| {
            if (filter >> bit) & 1 != 0 || aniso { wgpu::FilterMode::Linear } else { wgpu::FilterMode::Nearest }
        };
        let border = self.caps.border();
        let border_color = [7, 8, 9, 10].map(|i| f32::from_bits(w(i)));
        let lod_min = f32::from_bits(w(11)).max(0.0);
        let lod_max = f32::from_bits(w(12)).clamp(0.0, 32.0).max(lod_min);
        self.device.create_sampler(&wgpu::SamplerDescriptor {
            label: None,
            address_mode_u: format::address(w(1), border),
            address_mode_v: format::address(w(2), border),
            address_mode_w: format::address(w(3), border),
            mag_filter: linear(2),
            min_filter: linear(4),
            mipmap_filter: if filter & 1 != 0 || aniso {
                wgpu::MipmapFilterMode::Linear
            } else {
                wgpu::MipmapFilterMode::Nearest
            },
            lod_min_clamp: lod_min,
            lod_max_clamp: lod_max,
            compare: (filter & 0x80 != 0).then(|| format::compare(w(6)).unwrap_or(wgpu::CompareFunction::Always)),
            anisotropy_clamp: if aniso { w(5).clamp(1, 16) as u16 } else { 1 },
            border_color: [1, 2, 3].iter().any(|&i| w(i) == 4).then_some(match border_color {
                [_, _, _, a] if a < 0.5 => wgpu::SamplerBorderColor::TransparentBlack,
                [r, _, _, _] if r < 0.5 => wgpu::SamplerBorderColor::OpaqueBlack,
                _ => wgpu::SamplerBorderColor::OpaqueWhite,
            }),
        })
    }

    fn destroy(&mut self, id: u32) {
        if self.is_bound(id) {
            self.pdirty = true;
        }
        self.clears.retain(|c| c.tex != id);
        // Memory goes right after the next submit: batches handed over
        // earlier may still be waiting for theirs. Cached bind groups must
        // not keep it.
        // What decides program variants may have gone with it.
        self.formats_seen.iter_mut().for_each(|n| *n += 1);
        match self.objs.remove(id) {
            Some(Obj::Texture(t)) => match t.recycle {
                Some(key) => self.recycler.keep_texture(self.epoch, key, t.tex, t.bytes),
                None => self.graveyard.push(Grave::Texture(t.tex)),
            },
            Some(Obj::Buffer(b)) => match (b.slot, b.gpu) {
                (Some(slot), _) => self.slabs.free(self.epoch, slot),
                (None, Some(g)) => self.recycler.keep_buffer(self.epoch, g),
                (None, None) => {}
            },
            _ => {}
        }
    }

    fn is_bound(&self, id: u32) -> bool {
        let s = &self.s;
        s.layout == id
            || s.blend == id
            || s.depth_stencil == id
            || s.raster == id
            || s.ib.0 == id
            || s.shader.contains(&id)
            || s.vb.iter().any(|v| v.buf == id)
    }

    // ---- clears ----------------------------------------------------------------

    fn clear(&mut self, clear: Clear) {
        if self.objs.tex(clear.tex).is_none() {
            return;
        }
        self.end_pass();
        if let Some(prev) =
            self.clears.iter_mut().find(|c| (c.tex, c.mip, c.slice) == (clear.tex, clear.mip, clear.slice))
        {
            prev.color = clear.color.or(prev.color);
            prev.depth = clear.depth.or(prev.depth);
            prev.stencil = clear.stencil.or(prev.stencil);
        } else {
            self.clears.push(clear);
        }
    }

    /// Records the pending clears of `ids` (a command reads or writes them)
    /// as passes of their own; the others stay pending, to become the load
    /// operation of their texture's next pass.
    pub fn flush_clears_of(&mut self, ids: &[u32]) {
        if !self.clears.iter().any(|c| ids.contains(&c.tex)) {
            return;
        }
        self.end_pass();
        let (now, later) = std::mem::take(&mut self.clears).into_iter().partition(|c| ids.contains(&c.tex));
        self.clears = later;
        for clear in now {
            self.record_clear(clear);
        }
    }

    /// Records the pending clears as passes of their own.
    pub fn flush_clears(&mut self) {
        if self.clears.is_empty() {
            return;
        }
        self.end_pass();
        for clear in std::mem::take(&mut self.clears) {
            self.record_clear(clear);
        }
    }

    fn record_clear(&mut self, c: Clear) {
        let epoch = self.epoch;
        let Some(t) = self.objs.tex_mut(c.tex) else {
            return;
        };
        t.used.set(epoch);
        t.mark_written();
        let format = t.format;
        let view = t.target_view(format, c.mip, c.slice);
        let is_3d = t.is_3d();
        let ts = self.time_pass("clear");
        let encoder = self.encoder.get_or_insert_with(|| self.device.create_command_encoder(&Default::default()));
        let timestamp_writes = ts.and_then(|i| self.profiler.as_ref().map(|p| p.render(i)));
        if let Some(color) = c.color {
            encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    depth_slice: is_3d.then_some(c.slice),
                    resolve_target: None,
                    ops: wgpu::Operations { load: wgpu::LoadOp::Clear(color), store: wgpu::StoreOp::Store },
                })],
                timestamp_writes,
                ..Default::default()
            });
        } else {
            encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                depth_stencil_attachment: Some(depth_attachment(&view, format, c.depth, c.stencil, false, false)),
                timestamp_writes,
                ..Default::default()
            });
        }
    }

    // ---- statistics ------------------------------------------------------------------

    pub fn log_stats(&mut self) {
        let secs = self.last_stats.elapsed().as_secs_f64().max(0.001);
        self.last_stats = Instant::now();
        let frames = self.stats.presents.max(1);
        let st = std::mem::take(&mut self.stats);
        let mut submits: Vec<_> = st.submits.iter().collect();
        submits.sort_by(|a, b| b.1.cmp(a.1));
        let submits: Vec<String> =
            submits.iter().map(|(k, n)| format!("{k} {:.1}", **n as f64 / frames as f64)).collect();
        let mut ends: Vec<_> = st.pass_ends.iter().collect();
        ends.sort_by(|a, b| b.1.cmp(a.1));
        let ends: Vec<String> =
            ends.iter().take(6).map(|(op, n)| format!("op {op} {:.1}", **n as f64 / frames as f64)).collect();
        self.perf.reset();
        let [textures, buffers] = self.gpu_bytes();
        let memory = format!(
            "textures {} MiB, buffers {} MiB (small ones {} MiB in {} MiB of slabs), constants {} MiB; {} reused",
            textures >> 20,
            buffers >> 20,
            self.slabs.used >> 20,
            self.slabs.bytes() >> 20,
            self.uniforms.bytes_allocated() >> 20,
            std::mem::take(&mut self.recycler.reused)
        );
        let mut frames_line = frame_times(st.frame_ms) + &on_screen(crate::present::take_on_screen());
        if let Some(ms) = crate::present::take_upscaling() {
            frames_line += &format!(", upscaling {ms:.2} ms GPU");
        }
        let [encode, submit, gpu_wait, drawable, present, release] = self.submitter.stats.take();
        let per_frame = |d: Duration| d.as_secs_f64() * 1000.0 / frames as f64;
        let mut skipped: Vec<_> = st.skipped.iter().map(|(k, n)| format!("{k} {n}")).collect();
        skipped.sort();
        let c = &self.compiler;
        log::info!(
            "gpu: {:.1} fps ({frames_line}), {} draws/frame, {} dispatches, {:.1} passes/frame (ended by {}), {:.1} submits/frame ({}), recording {:.0}% ({:.1} ms/frame), encoding {:.1} ms/frame, submitting {:.1}, presenting {:.1}, releasing {:.1}, waiting for the GPU {:.1}, for a drawable {:.1}; pipelines {} ({} failed, {:.0} ms compiling), draws waiting for one {}, skipped [{}], clamped {}, bind groups +{} ({} cached), objects {} ({memory}), errors +{}",
            st.presents as f64 / secs,
            st.draws / frames,
            st.dispatches / frames,
            st.passes as f64 / frames as f64,
            ends.join(", "),
            st.submits.values().sum::<u64>() as f64 / frames as f64,
            submits.join(", "),
            st.busy.as_secs_f64() * 100.0 / secs,
            st.busy.as_secs_f64() * 1000.0 / frames as f64,
            per_frame(encode),
            per_frame(submit),
            per_frame(present),
            per_frame(release),
            per_frame(gpu_wait),
            per_frame(drawable),
            c.compiled.load(std::sync::atomic::Ordering::Relaxed),
            c.failed.load(std::sync::atomic::Ordering::Relaxed),
            c.compile_us.swap(0, std::sync::atomic::Ordering::Relaxed) as f64 / 1000.0,
            st.pending_skips,
            skipped.join(", "),
            st.clamped,
            st.bind_groups,
            self.binds.len(),
            self.objs.len(),
            st.errors,
        );
        self.stats.errors = 0;
        if let Some(p) = &self.profiler {
            log::info!("{}", crate::profile::report(&p.totals, frames));
        }
        // Shader modules are needed only to make pipelines: idle ones go,
        // a few hundred at a time, dropped on a compile thread.
        let mut idle = Vec::new();
        self.library.take_idle_modules(30, 400, &mut idle);
        self.computes.take_idle_modules(30, 400, &mut idle);
        if !idle.is_empty() {
            log::debug!("gpu: {} idle shader modules released", idle.len());
            self.pool.background(move || drop(idle));
        }
        let (used, available) = glue5_runtime::memory::footprint();
        if let Some(used) = used {
            let more = available.map_or(String::new(), |a| format!(", {} MiB more allowed", a >> 20));
            log::info!("memory: process footprint {} MiB{more}{}", used >> 20, self.gpu_memory());
        }
    }

    /// The textures' and buffers' bytes.
    pub(crate) fn gpu_bytes(&self) -> [u64; 2] {
        let (mut textures, mut buffers) = (0u64, 0u64);
        for o in self.objs.values() {
            match o {
                Obj::Texture(t) => textures += t.bytes,
                Obj::Buffer(b) => buffers += b.gpu.as_ref().map_or(0, |_| b.bytes),
                _ => {}
            }
        }
        [textures, buffers]
    }

    /// What Metal has allocated for the device, all resources included.
    #[cfg(target_vendor = "apple")]
    fn gpu_memory(&self) -> String {
        use objc2_metal::MTLDevice;
        unsafe { self.device.as_hal::<wgpu::hal::api::Metal>() }
            .map_or(String::new(), |d| format!(", Metal {} MiB", d.raw_device().currentAllocatedSize() >> 20))
    }

    #[cfg(not(target_vendor = "apple"))]
    fn gpu_memory(&self) -> String {
        String::new()
    }

    /// The passes timed since the last submit, resolved into this one.
    fn resolve_profile(&mut self) {
        let Some(bytes) = self.profiler.as_ref().map(|p| p.pending_bytes()).filter(|&b| b > 0) else {
            return;
        };
        let staging = self.staging.get(&self.device, bytes);
        self.end_pass();
        let encoder = self.encoder.get_or_insert_with(|| self.device.create_command_encoder(&Default::default()));
        if let Some(resolved) = self.profiler.as_mut().unwrap().resolve(encoder, &staging) {
            self.maps.push((staging, Box::new(move |s| Work::Profile { staging: s, resolved })));
        }
    }
}

/// Frame-time percentiles, and the hitches: frames over 1.5 times the median.
fn frame_times(mut ms: Vec<f32>) -> String {
    if ms.is_empty() {
        return "no frames".into();
    }
    ms.sort_by(f32::total_cmp);
    let at = |q: f64| ms[((ms.len() - 1) as f64 * q).round() as usize];
    let median = at(0.5);
    let hitches = ms.iter().filter(|&&t| t > median * 1.5).count();
    format!(
        "frame ms p50 {median:.1} p95 {:.1} p99 {:.1} max {:.1}, {hitches} over 1.5x",
        at(0.95),
        at(0.99),
        ms[ms.len() - 1]
    )
}

/// How long frames stayed on screen (where the platform tells): percentiles,
/// and how often a frame stayed for another time than the one before (by
/// half a 120 Hz refresh or more): the judder an uneven pace shows as.
fn on_screen(mut ms: Vec<f32>) -> String {
    if ms.len() < 2 {
        return String::new();
    }
    let changes = ms.windows(2).filter(|w| (w[1] - w[0]).abs() >= 4.0).count();
    ms.sort_by(f32::total_cmp);
    let at = |q: f64| ms[((ms.len() - 1) as f64 * q).round() as usize];
    format!(
        "; on screen ms p5 {:.1} p50 {:.1} p95 {:.1} max {:.1}, {changes} changes",
        at(0.05),
        at(0.5),
        at(0.95),
        ms[ms.len() - 1]
    )
}

pub fn depth_attachment(
    view: &wgpu::TextureView,
    format: TextureFormat,
    depth: Option<f32>,
    stencil: Option<u32>,
    read_only_depth: bool,
    read_only_stencil: bool,
) -> wgpu::RenderPassDepthStencilAttachment<'_> {
    fn ops<V>(clear: Option<V>, read_only: bool) -> Option<wgpu::Operations<V>> {
        (!read_only).then(|| wgpu::Operations {
            load: clear.map_or(wgpu::LoadOp::Load, wgpu::LoadOp::Clear),
            store: wgpu::StoreOp::Store,
        })
    }
    wgpu::RenderPassDepthStencilAttachment {
        view,
        depth_ops: format.has_depth_aspect().then(|| ops(depth, read_only_depth)).flatten(),
        stencil_ops: format.has_stencil_aspect().then(|| ops(stencil, read_only_stencil)).flatten(),
    }
}

/// Rough GPU memory of a texture, for statistics.
fn texture_bytes(f: TextureFormat, size: wgpu::Extent3d, mips: u32, dim: wgpu::TextureDimension) -> u64 {
    let (bytes, bw) = format::block(f);
    (0..mips)
        .map(|m| {
            let w = (size.width >> m).max(1).div_ceil(bw) as u64;
            let h = (size.height >> m).max(1).div_ceil(bw) as u64;
            let d = if dim == wgpu::TextureDimension::D3 {
                (size.depth_or_array_layers >> m).max(1)
            } else {
                size.depth_or_array_layers
            };
            w * h * d as u64 * bytes as u64
        })
        .sum()
}
