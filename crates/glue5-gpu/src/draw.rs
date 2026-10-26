//! Draws and dispatches: render passes, pipelines, bind groups.

use std::sync::{Arc, OnceLock};

use arrayvec::ArrayVec;
use glue5_runtime::gpu::Command;
use glue5_runtime::qos::{self, Qos};
use rustc_hash::FxHashMap;
use wgpu::{TextureFormat, TextureViewDimension};

use crate::exec::{Bound, Clear, Gpu, PassInfo, depth_attachment};
use crate::format;
use crate::objects::{NO_LAYOUT, Objects, View};
use crate::pipeline::{self, Recipe};
use crate::shader::{self, Kind, Program, Sample, Stage};
use crate::uniforms::{Alloc, Uniforms};

/// Render passes per submit, at most (see `Gpu::ensure_pass`).
const CHUNK_PASSES: u32 = 16;

const ZERO_VB: u64 = 4096;

/// Bind groups by the ids of what they hold (ids are never reused). Two
/// generations: the oldest is dropped when the newest is full, or when it
/// ages ([`BindCache::age`]).
#[derive(Default)]
pub struct BindCache {
    current: BindGroups,
    old: BindGroups,
    serial: u64,
}

type BindGroups = FxHashMap<Box<[u32]>, (wgpu::BindGroup, u64)>;

const BIND_GROUPS: usize = 16384;

impl BindCache {
    fn get(&mut self, key: &[u32]) -> Option<(wgpu::BindGroup, u64)> {
        if let Some(v) = self.current.get(key) {
            return Some(v.clone());
        }
        let (k, v) = self.old.remove_entry(key)?;
        self.current.insert(k, v.clone());
        Some(v)
    }

    /// Returns its serial, and the generation dropped to make room, if one
    /// was (thousands of bind groups: the caller drops them elsewhere).
    fn insert(&mut self, key: &[u32], bg: wgpu::BindGroup) -> (u64, Option<BindGroups>) {
        let dropped = (self.current.len() >= BIND_GROUPS)
            .then(|| std::mem::replace(&mut self.old, std::mem::take(&mut self.current)));
        self.serial += 1;
        self.current.insert(key.into(), (bg, self.serial));
        (self.serial, dropped)
    }

    pub fn len(&self) -> usize {
        self.current.len() + self.old.len()
    }

    /// Starts a new generation: the groups no draw takes until the next one
    /// go then. Most are of textures streamed out or areas left (~2 KB of
    /// the driver's and wgpu's each; an iPhone kept ~27,000 for minutes).
    /// Returns the generation dropped, to free elsewhere.
    pub fn age(&mut self) -> BindGroups {
        std::mem::replace(&mut self.old, std::mem::take(&mut self.current))
    }
}

/// Identity of the constant buffers a pack reads: (buffer id, version, first
/// constant) per member.
type PackIds = [(u32, u32, u32); 16];

/// Where a stage's constants went: its pack, and its own buffers (in the
/// order of `Pack::own`).
#[derive(Clone, Copy)]
struct Constants {
    pack: Alloc,
    own: [Option<Alloc>; shader::OWN_MAX],
}

/// A bind group's dynamic offsets, in binding order: its pack's, then its
/// own constant buffers'.
#[derive(Clone, Copy, Default, PartialEq, Eq)]
pub struct Offsets {
    values: [u32; 1 + shader::OWN_MAX],
    len: u8,
}

impl Offsets {
    fn push(&mut self, offset: u32) {
        self.values[self.len as usize] = offset;
        self.len += 1;
    }

    pub fn as_slice(&self) -> &[u32] {
        &self.values[..self.len as usize]
    }
}

/// The last packs of a (program, stage): the same constants in the same
/// frame reuse their copy.
#[derive(Default)]
pub struct PackCache {
    /// (frame, constants, jitter added to them, copy).
    entries: Vec<(u64, PackIds, Option<[u32; 2]>, Alloc)>,
}

/// Bound in place of what the engine left unbound.
#[derive(Default)]
pub struct Dummies {
    textures: FxHashMap<(TextureViewDimension, u8, bool), wgpu::TextureView>,
    samplers: [Option<wgpu::Sampler>; 2],
    buffers: FxHashMap<u32, wgpu::Buffer>,
    vertices: Option<wgpu::Buffer>,
}

impl Dummies {
    fn texture(
        &mut self,
        device: &wgpu::Device,
        dim: TextureViewDimension,
        sample: Sample,
        ms: bool,
    ) -> wgpu::TextureView {
        self.textures
            .entry((dim, sample as u8, ms))
            .or_insert_with(|| {
                let format = match sample {
                    Sample::Uint => TextureFormat::Rgba8Uint,
                    Sample::Sint => TextureFormat::Rgba8Sint,
                    Sample::Depth => TextureFormat::Depth32Float,
                    Sample::Float => TextureFormat::Rgba8Unorm,
                };
                let layers =
                    if matches!(dim, TextureViewDimension::Cube | TextureViewDimension::CubeArray) { 6 } else { 1 };
                let dimension = match dim {
                    TextureViewDimension::D1 => wgpu::TextureDimension::D1,
                    TextureViewDimension::D3 => wgpu::TextureDimension::D3,
                    _ => wgpu::TextureDimension::D2,
                };
                let tex = device.create_texture(&wgpu::TextureDescriptor {
                    label: Some("dummy"),
                    size: wgpu::Extent3d { width: 1, height: 1, depth_or_array_layers: layers },
                    mip_level_count: 1,
                    sample_count: if ms { 4 } else { 1 },
                    dimension,
                    format,
                    usage: wgpu::TextureUsages::TEXTURE_BINDING
                        | if ms { wgpu::TextureUsages::RENDER_ATTACHMENT } else { wgpu::TextureUsages::empty() },
                    view_formats: &[],
                });
                tex.create_view(&wgpu::TextureViewDescriptor { dimension: Some(dim), ..Default::default() })
            })
            .clone()
    }

    fn sampler(&mut self, device: &wgpu::Device, comparison: bool) -> wgpu::Sampler {
        self.samplers[comparison as usize]
            .get_or_insert_with(|| {
                device.create_sampler(&wgpu::SamplerDescriptor {
                    compare: comparison.then_some(wgpu::CompareFunction::Always),
                    ..Default::default()
                })
            })
            .clone()
    }

    /// One per binding: a writable storage buffer bound twice is an aliasing error.
    fn buffer(&mut self, device: &wgpu::Device, binding: u32) -> wgpu::Buffer {
        self.buffers
            .entry(binding)
            .or_insert_with(|| {
                device.create_buffer(&wgpu::BufferDescriptor {
                    label: Some("dummy"),
                    size: 65536,
                    usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
                    mapped_at_creation: false,
                })
            })
            .clone()
    }

    fn vertices(&mut self, device: &wgpu::Device) -> wgpu::Buffer {
        self.vertices
            .get_or_insert_with(|| {
                device.create_buffer(&wgpu::BufferDescriptor {
                    label: Some("zero vertices"),
                    size: ZERO_VB,
                    usage: wgpu::BufferUsages::VERTEX,
                    mapped_at_creation: false,
                })
            })
            .clone()
    }
}

/// What a bind group entry is built from.
enum Res {
    Buffer(wgpu::Buffer, u64, Option<u64>),
    Sampler(wgpu::Sampler),
    View(wgpu::TextureView),
}

impl Gpu {
    pub fn program(&self, id: u32) -> Option<Arc<Program>> {
        let sh = self.objs.shader(id)?;
        sh.slot.get_or_init(|| self.library.prepare(&self.device, sh.hash, sh.stage)).clone()
    }

    fn skip(&mut self, why: &'static str) {
        *self.stats.skipped.entry(why).or_default() += 1;
    }

    // ---- render passes -----------------------------------------------------------

    fn target_key(&self) -> [u32; 37] {
        let mut k = [0u32; 37];
        for (i, t) in self.s.rt.iter().enumerate() {
            k[i * 4..i * 4 + 4].copy_from_slice(&[t.tex, t.mip, t.slice, t.format]);
        }
        let d = self.s.ds;
        k[32..37].copy_from_slice(&[d.tex, d.mip, d.slice, d.format, self.s.ds_flags]);
        k
    }

    /// Opens the render pass for the bound targets (false: nothing bound).
    pub fn ensure_pass(&mut self) -> bool {
        let key = self.target_key();
        if self.pass.is_some() && self.pass_info.as_ref().is_some_and(|p| p.key == key) {
            return true;
        }
        self.end_pass();
        // A long frame goes to the GPU in pieces: wgpu encodes passes when
        // their encoder is finished, and the GPU starts on the first piece
        // while the engine is still sending the rest.
        if self.chunk_passes >= CHUNK_PASSES && self.clears.is_empty() {
            self.flush("chunk");
        }
        let epoch = self.epoch;
        let mut colors: Vec<Option<(wgpu::TextureView, Option<u32>, wgpu::LoadOp<wgpu::Color>)>> = Vec::new();
        let mut targets = Vec::new();
        let (mut samples, mut width, mut height) = (1, 1, 1);
        let mut clears = std::mem::take(&mut self.clears);
        let take_clear = |clears: &mut Vec<Clear>, tex, mip, slice| {
            let i = clears.iter().position(|c| (c.tex, c.mip, c.slice) == (tex, mip, slice))?;
            Some(clears.swap_remove(i))
        };
        for i in 0..8 {
            let r = self.s.rt[i];
            let caps = self.caps;
            let Some(t) = self.objs.tex_mut(r.tex) else {
                colors.push(None);
                targets.push(None);
                continue;
            };
            let format = format::view(r.format, t.format, &caps);
            let view = t.target_view(format, r.mip, r.slice);
            t.used.set(epoch);
            t.mark_written();
            let load = match take_clear(&mut clears, r.tex, r.mip, r.slice) {
                Some(Clear { color: Some(c), .. }) => wgpu::LoadOp::Clear(c),
                _ => wgpu::LoadOp::Load,
            };
            colors.push(Some((view, t.is_3d().then_some(r.slice), load)));
            targets.push(Some(format));
            samples = t.samples;
            (width, height) = ((t.width >> r.mip).max(1), (t.height >> r.mip).max(1));
        }
        while colors.last().is_some_and(Option::is_none) {
            colors.pop();
            targets.pop();
        }
        let ro = (self.s.ds_flags & 3) as u8;
        let mut depth = None;
        if let Some(t) = self.objs.tex_mut(self.s.ds.tex) {
            let view = t.target_view(t.format, self.s.ds.mip, self.s.ds.slice);
            t.used.set(epoch);
            t.mark_aspects_written(ro & 1 == 0, ro & 2 == 0);
            let mut clear = take_clear(&mut clears, self.s.ds.tex, self.s.ds.mip, self.s.ds.slice);
            // A read-only aspect cannot be cleared as the pass loads it: the
            // clear goes first, as a pass of its own (D3D clears it anyway).
            if let Some(c) = clear.take_if(|c| (c.depth.is_some() && ro & 1 != 0) || (c.stencil.is_some() && ro & 2 != 0)) {
                clears.push(c);
            }
            if targets.is_empty() {
                (width, height) = ((t.width >> self.s.ds.mip).max(1), (t.height >> self.s.ds.mip).max(1));
            }
            samples = t.samples;
            depth = Some((view, t.format, clear));
        }
        if colors.is_empty() && depth.is_none() {
            self.clears = clears;
            return false;
        }
        // Clears of other subresources come first, as passes of their own.
        self.clears = clears;
        self.flush_clears();

        self.pass_profile = self.time_pass("draw");
        let encoder = self.encoder.get_or_insert_with(|| self.device.create_command_encoder(&Default::default()));
        let attachments: Vec<_> = colors
            .iter()
            .map(|c| {
                c.as_ref().map(|(view, depth_slice, load)| wgpu::RenderPassColorAttachment {
                    view,
                    depth_slice: *depth_slice,
                    resolve_target: None,
                    ops: wgpu::Operations { load: *load, store: wgpu::StoreOp::Store },
                })
            })
            .collect();
        let depth_format = depth.as_ref().map(|d| d.1);
        let pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: None,
            color_attachments: &attachments,
            depth_stencil_attachment: depth.as_ref().map(|(view, format, clear)| {
                let (d, s) = clear.map_or((None, None), |c| (c.depth, c.stencil));
                depth_attachment(view, *format, d, s, ro & 1 != 0, ro & 2 != 0)
            }),
            timestamp_writes: self.pass_profile.and_then(|i| self.profiler.as_ref().map(|p| p.render(i))),
            occlusion_query_set: self.occlusion.set.as_ref(),
            multiview_mask: None,
        });
        self.pass = Some(pass.forget_lifetime());
        self.stats.passes += 1;
        self.chunk_passes += 1;
        let tail_key = (targets.clone(), depth_format, samples, ro);
        let next = self.pass_tails.len() as u32 + 1;
        let tail = *self.pass_tails.entry(tail_key).or_insert(next);
        self.pass_info = Some(PassInfo {
            key,
            targets,
            depth: depth_format,
            samples,
            read_only: ro,
            width,
            height,
            tail,
            queries: self.occlusion.set.is_some(),
        });
        self.bound = Bound::default();
        self.last_pipe = None;
        self.pdirty = true;
        self.begin_occlusion_in_pass();
        true
    }

    // ---- pipelines -------------------------------------------------------------------

    /// The pipeline for the bound state (`None`: not drawable; a pending cell
    /// is still compiling). `float32`: per stage, the textures bound that
    /// GPUs without 32-bit float filtering read unfiltered (for the recipe).
    fn pipeline(
        &mut self,
        vs: &Arc<Program>,
        ps: Option<&Arc<Program>>,
        float32: [u128; 2],
    ) -> Option<Arc<pipeline::Cell>> {
        if !self.pdirty
            && let Some(p) = &self.last_pipe
        {
            return Some(p.clone());
        }
        let info = self.pass_info.as_ref()?;
        let layout = self.objs.layout(self.s.layout).unwrap_or(&NO_LAYOUT);
        let (_, strip) = format::topology(self.s.topology);
        let strip = if strip && self.s.ib.0 != 0 { if self.s.ib.1 == format::INDEX_U16 { 1 } else { 2 } } else { 0 };
        let stride = |slot: u32| {
            let v = self.s.vb[slot as usize];
            if self.objs.buf(v.buf).is_some_and(|b| b.gpu.is_some()) { v.stride } else { 0 }
        };
        let mut key = std::mem::take(&mut self.key);
        key.clear();
        key.extend([vs.id, ps.map_or(0, |p| p.id), self.s.layout, self.s.topology, strip as u32]);
        key.extend([self.s.blend, self.s.depth_stencil, self.s.raster, info.tail]);
        key.extend(layout.slots.iter().map(|&(slot, _)| stride(slot)));
        key.extend(float32.iter().flat_map(|m| (0..4).map(move |i| (m >> (32 * i)) as u32)));
        let cell = match self.fast_pipes.get(&key[..]) {
            Some(cell) => cell.clone(),
            None => {
                let words = |id: u32| self.objs.state(id).map(|s| s.words.clone());
                let recipe = Recipe {
                    vs: vs.hash,
                    ps: ps.map(|p| p.hash),
                    sig: self.objs.shader(self.s.shader[0]).map(|s| s.sig.clone()).unwrap_or_default(),
                    elements: layout.elements.clone(),
                    strides: layout.slots.iter().map(|&(slot, _)| stride(slot)).collect(),
                    topology: self.s.topology,
                    strip,
                    blend: words(self.s.blend),
                    depth: words(self.s.depth_stencil),
                    raster: words(self.s.raster).map(|w| pipeline::without_depth_bias(&w)),
                    targets: info.targets.clone(),
                    samples: info.samples,
                    depth_format: info.depth,
                    read_only: info.read_only,
                    variant: [
                        float32[0] | vs.depth,
                        vs.depth,
                        ps.map_or(0, |p| float32[1] | p.depth),
                        ps.map_or(0, |p| p.depth),
                    ],
                    hud_mask: ps.is_some_and(|p| p.hud_mask),
                };
                let cell = self.pipeline_for(Arc::new(recipe), vs.clone(), ps.cloned());
                self.fast_pipes.insert(key.as_slice().into(), cell.clone());
                cell
            }
        };
        self.key = key;
        Some(cell)
    }

    /// The pipeline of a recipe: prepared before the game started (see
    /// `prepare.rs`), else compiled now, in the background. Recipes not seen
    /// before are kept for the next start.
    fn pipeline_for(&mut self, recipe: Arc<Recipe>, vs: Arc<Program>, ps: Option<Arc<Program>>) -> Arc<pipeline::Cell> {
        if self.known.insert(recipe.clone()) {
            self.recipes_new.push(recipe.clone());
        }
        let recipe = Arc::new(recipe.for_device(&self.caps));
        if let Some(cell) = self.pipes.get(&recipe) {
            return cell.clone();
        }
        let cell: Arc<pipeline::Cell> = Arc::new(OnceLock::new());
        self.pipes.insert(recipe.clone(), cell.clone());
        let (compiler, c) = (self.compiler.clone(), cell.clone());
        let job = move || {
            c.get_or_init(|| compiler.compile(&vs, ps.as_deref(), &recipe));
        };
        if self.settings.sync_pipelines {
            job();
        } else {
            // Its draws are skipped meanwhile, nothing waits: in a lower
            // class (efficiency cores, where there are some) it leaves the
            // engine's threads their cores. A new pipeline takes ~200 ms
            // of CPU on an A15, which has two performance cores.
            self.pool.urgent(move || {
                qos::set(Qos::Utility);
                job();
                qos::set(Qos::Initiated);
            });
        }
        cell
    }

    // ---- bind groups -----------------------------------------------------------------

    /// The bind group of a program's stage and its dynamic offsets.
    fn bind_group(
        &mut self,
        prog: &Program,
        stage: Stage,
        layout: &wgpu::BindGroupLayout,
    ) -> Option<(wgpu::BindGroup, u64, Offsets)> {
        // The scene's draws move by its jitter (temporal upscaling, see scene.rs).
        let jitter = match (stage, &self.pass_info) {
            (Stage::Vertex, Some(p)) => self.scene.clip_jitter((p.width, p.height)),
            _ => None,
        };
        let pack = match &prog.pack {
            Some(pack) => Some(pack_constants(
                &self.objs,
                &self.s.cb,
                &mut self.uniforms,
                &mut self.packs,
                &self.device,
                prog,
                stage,
                pack,
                jitter,
            )),
            None => None,
        };
        let st = stage as usize;
        let epoch = self.epoch;
        let mut key = std::mem::take(&mut self.key);
        key.clear();
        key.extend([prog.id, stage as u32, pack.map_or(0, |c| c.pack.chunk_id)]);
        if let Some(c) = &pack {
            key.extend(c.own.iter().flatten().map(|a| a.chunk_id));
        }
        for b in &prog.bindings {
            match &b.kind {
                Kind::Uniform => {}
                Kind::Sampler { comparison } => {
                    let id = self.s.sampler[st][(b.binding - shader::SAMPLERS) as usize];
                    let ok = self.objs.state(id).is_some_and(|s| s.sampler.is_some() && s.comparison == *comparison);
                    key.push(if ok { id } else { 0 });
                }
                Kind::Texture { dim, sample, .. } => {
                    let slot = b.binding - shader::SRVS;
                    let id = self.s.srv[st][slot as usize];
                    let res = self.objs.srv(id).filter(|v| {
                        !v.is_buffer && !self.reads_target(prog, stage, slot, v, *dim) && !self.compares_depth_target(v, *sample, stage)
                    });
                    match res.and_then(|v| self.objs.tex(v.res)) {
                        Some(t) => {
                            t.used.set(epoch);
                            t.in_bind_group.set(true);
                            key.push(id);
                        }
                        None => key.push(0),
                    }
                }
                Kind::Storage { .. } => {
                    let (id, view) = self.storage_view(st, b.binding);
                    match view.and_then(|v| self.objs.buf(v.res).map(|buf| (v, buf))) {
                        Some((v, buf)) => {
                            buf.used.set(epoch);
                            buf.in_bind_group.set(true);
                            key.extend([id, buf.uid, v.a, v.b]);
                        }
                        None => key.extend([0, 0, 0, 0]),
                    }
                }
                Kind::StorageTexture { .. } => {
                    let id = self.s.uav[st][(b.binding - shader::UAVS) as usize];
                    match self.objs.uav(id).filter(|v| !v.is_buffer).and_then(|v| self.objs.tex(v.res)) {
                        Some(t) => {
                            t.used.set(epoch);
                            t.in_bind_group.set(true);
                            key.push(id);
                        }
                        None => key.push(0),
                    }
                }
            }
        }
        // Storage textures are written: their depth copies go stale.
        for b in &prog.bindings {
            if matches!(b.kind, Kind::StorageTexture { .. }) {
                let id = self.s.uav[st][(b.binding - shader::UAVS) as usize];
                if let Some(res) = self.objs.uav(id).map(|v| v.res)
                    && let Some(t) = self.objs.tex_mut(res)
                {
                    t.mark_written();
                }
            }
        }
        let found = self.binds.get(&key);
        let result = match found {
            Some((bg, serial)) => Some((bg, serial)),
            None => self.create_bind_group(prog, stage, layout, pack).map(|bg| {
                self.stats.bind_groups += 1;
                let (serial, dropped) = self.binds.insert(&key, bg.clone());
                // Freed off the command thread (a hitch of tens of ms there).
                if let Some(dropped) = dropped {
                    self.pool.background(move || drop(dropped));
                }
                (bg, serial)
            }),
        };
        self.key = key;
        let mut offsets = Offsets::default();
        if let Some(c) = &pack {
            offsets.push(c.pack.offset);
            c.own.iter().flatten().for_each(|a| offsets.push(a.offset));
        }
        result.map(|(bg, serial)| (bg, serial, offsets))
    }

    fn storage_view(&self, st: usize, binding: u32) -> (u32, Option<&View>) {
        let id = if binding >= shader::UAV_COUNTERS {
            self.s.uav[st][(binding - shader::UAV_COUNTERS) as usize]
        } else if binding >= shader::UAVS {
            self.s.uav[st][(binding - shader::UAVS) as usize]
        } else {
            self.s.srv[st][(binding - shader::SRVS) as usize]
        };
        let view = if binding >= shader::UAVS { self.objs.uav(id) } else { self.objs.srv(id) };
        (id, view.filter(|v| v.is_buffer))
    }

    fn create_bind_group(
        &mut self,
        prog: &Program,
        stage: Stage,
        layout: &wgpu::BindGroupLayout,
        constants: Option<Constants>,
    ) -> Option<wgpu::BindGroup> {
        let st = stage as usize;
        let mut resources: Vec<(u32, Res)> = Vec::with_capacity(prog.bindings.len());
        for b in &prog.bindings {
            let res = match &b.kind {
                Kind::Uniform if b.binding == shader::CONSTANT_TABLE => {
                    let Some(consts) = &prog.consts else {
                        crate::log_once(&format!("c15{}", prog.label), || {
                            format!("{} reads the constant table but has none", prog.label)
                        });
                        return None;
                    };
                    Res::Buffer(consts.clone(), 0, None)
                }
                Kind::Uniform => {
                    let (pack, constants) = (prog.pack.as_ref()?, constants?);
                    let (alloc, size) = if b.binding == shader::PACKED_CBUFFERS {
                        (constants.pack, pack.size)
                    } else {
                        let i = pack.own.iter().position(|m| m.slot == b.binding)?;
                        (constants.own[i]?, pack.own[i].size)
                    };
                    Res::Buffer(self.uniforms.buffer(alloc.chunk).clone(), 0, Some(size as u64))
                }
                Kind::Sampler { comparison } => {
                    let id = self.s.sampler[st][(b.binding - shader::SAMPLERS) as usize];
                    let nearest = !*comparison && prog.nonfiltering(b.binding);
                    match self.sampler(id, *comparison, nearest) {
                        Some(s) => Res::Sampler(s),
                        // Nearest: fits filtering and non-filtering bindings.
                        None => Res::Sampler(self.dummies.sampler(&self.device, *comparison)),
                    }
                }
                Kind::Texture { sample, dim, multisampled } => {
                    let id = self.s.srv[st][(b.binding - shader::SRVS) as usize];
                    let target =
                        self.objs.srv(id).is_some_and(|v| self.is_target(v, *dim) || self.compares_depth_target(v, *sample, stage));
                    match if target { None } else { self.srv_view(id, *dim, *sample) } {
                        Some(v) => Res::View(v),
                        None => Res::View(self.dummies.texture(&self.device, *dim, *sample, *multisampled)),
                    }
                }
                Kind::Storage { .. } => {
                    let counter = b.binding >= shader::UAV_COUNTERS;
                    let (_, view) = self.storage_view(st, b.binding);
                    let found = view.map(|v| (v.res, v.a, v.b));
                    match found {
                        Some((res, first, count)) if self.objs.buf(res).is_some_and(|b| b.gpu.is_some()) => {
                            if counter {
                                Res::Buffer(self.counter_buffer(res), 0, None)
                            } else {
                                let buf = self.objs.buf(res).unwrap();
                                let stride = buf.stride.max(4) as u64;
                                let offset = first as u64 * stride;
                                let align = self.limits.min_storage_buffer_offset_alignment as u64;
                                if offset != 0 && offset.is_multiple_of(align) && offset < buf.bytes {
                                    let size = (count != 0 && count != u32::MAX)
                                        .then(|| (count as u64 * stride).min(buf.bytes - offset));
                                    Res::Buffer(buf.gpu.clone().unwrap(), buf.base + offset, size.or(buf.slot.map(|_| buf.bytes - offset)))
                                } else {
                                    if offset != 0 {
                                        crate::log_once(&format!("bufoff{}", prog.label), || {
                                            format!(
                                                "{}: buffer view at {offset} is not aligned: bound whole",
                                                prog.label
                                            )
                                        });
                                    }
                                    Res::Buffer(buf.gpu.clone().unwrap(), buf.base, buf.slot.map(|_| buf.bytes))
                                }
                            }
                        }
                        _ => Res::Buffer(self.dummies.buffer(&self.device, b.binding), 0, None),
                    }
                }
                Kind::StorageTexture { dim, .. } => {
                    let id = self.s.uav[st][(b.binding - shader::UAVS) as usize];
                    match self.uav_view(id, *dim) {
                        Some(v) => Res::View(v),
                        None => {
                            crate::log_once(&format!("nouav{}", prog.label), || {
                                format!("{}: storage texture u{} unbound", prog.label, b.binding - shader::UAVS)
                            });
                            return None;
                        }
                    }
                }
            };
            resources.push((b.binding, res));
        }
        let entries: Vec<wgpu::BindGroupEntry> = resources
            .iter()
            .map(|(binding, r)| wgpu::BindGroupEntry {
                binding: *binding,
                resource: match r {
                    Res::Buffer(buffer, offset, size) => wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                        buffer,
                        offset: *offset,
                        size: size.and_then(wgpu::BufferSize::new),
                    }),
                    Res::Sampler(s) => wgpu::BindingResource::Sampler(s),
                    Res::View(v) => wgpu::BindingResource::TextureView(v),
                },
            })
            .collect();
        let scope = self.device.push_error_scope(wgpu::ErrorFilter::Validation);
        let bg = self.device.create_bind_group(&wgpu::BindGroupDescriptor { label: None, layout, entries: &entries });
        match pollster::block_on(scope.pop()) {
            None => Some(bg),
            Some(e) => {
                crate::log_once(&format!("bg{}", prog.label), || format!("{}: bind group failed: {e}", prog.label));
                None
            }
        }
    }

    /// A sampler state's sampler (`nearest`: its non-filtering twin).
    fn sampler(&mut self, id: u32, comparison: bool, nearest: bool) -> Option<wgpu::Sampler> {
        let state = self.objs.state(id).filter(|s| s.comparison == comparison && s.sampler.is_some())?;
        if !nearest {
            return state.sampler.clone();
        }
        if let Some(s) = &state.nearest {
            return Some(s.clone());
        }
        let sampler = self.make_sampler(&state.words, true);
        self.objs.state_mut(id)?.nearest = Some(sampler.clone());
        Some(sampler)
    }

    /// `prog` for the textures bound: depth buffers it reads as floats read
    /// directly as depth textures where WebGPU allows it (else through float
    /// copies, see `prepare_proxies`); on GPUs that cannot filter 32-bit
    /// float textures, those declared unfilterable. The samplers used with
    /// either are non-filtering (nearest instead of linear). Also the
    /// textures unfilterable on such GPUs, whatever this one does: what
    /// recipes record (see `Recipe::for_device`).
    /// Also the slots of the textures it reads through copies (for
    /// `prepare_proxies`, which then looks up only those).
    fn for_bound_formats(&mut self, prog: Arc<Program>, stage: Stage) -> (Arc<Program>, u128, u128) {
        let st = stage as usize;
        // Nothing it depends on changed since the last draw with this program.
        if let Some((id, seen, variant, float32, proxies)) = &self.bound_formats[st]
            && (*id, *seen) == (prog.id, self.formats_seen[st])
        {
            return (variant.clone(), *float32, *proxies);
        }
        let (mut float32, mut depth) = (0u128, 0u128);
        for b in &prog.bindings {
            let Kind::Texture { sample: Sample::Float, multisampled: false, .. } = b.kind else {
                continue;
            };
            let bit = 1u128 << (b.binding - shader::SRVS);
            let id = self.s.srv[st][(b.binding - shader::SRVS) as usize];
            let Some(srv) = self.objs.srv(id).filter(|v| !v.is_buffer) else { continue };
            let Some(tex) = self.objs.tex(srv.res) else { continue };
            if format::is_depth(tex.format) {
                if tex.samples == 1 && self.may_sample_depth(srv.res, tex.format, stage) {
                    depth |= bit;
                } else {
                    float32 |= bit; // its R32Float copy
                }
            } else if format::is_float32(format::view(srv.format, tex.format, &self.caps)) {
                float32 |= bit;
            }
        }
        let unfiltered = if self.caps.float32_filter { 0 } else { float32 };
        let variant = prog.for_bindings(&self.device, unfiltered, depth);
        let proxies = self.proxy_slots(&variant);
        self.bound_formats[st] = Some((prog.id, self.formats_seen[st], variant.clone(), float32, proxies));
        (variant, float32, proxies)
    }

    /// Whether a depth texture can be sampled by this draw: not while it is
    /// the pass's depth attachment, unless that is read-only (both aspects).
    fn may_sample_depth(&self, tex: u32, format: TextureFormat, stage: Stage) -> bool {
        if stage == Stage::Compute || tex != self.s.ds.tex {
            return true;
        }
        let ro = self.s.ds_flags & 3;
        ro & 1 != 0 && (ro & 2 != 0 || !format.has_stencil_aspect())
    }

    /// Whether a comparison (`texture_depth`) read is of the pass's depth
    /// attachment while that is written: unbound for the draw, as D3D11
    /// does (float reads go through a copy instead, see `for_bound_formats`).
    /// With only depth read-only (stencil written) D3D would read it, but
    /// wgpu cannot sample an attachment one of whose aspects is written.
    fn compares_depth_target(&self, srv: &View, sample: Sample, stage: Stage) -> bool {
        sample == Sample::Depth
            && self.pass.is_some()
            && self.objs.tex(srv.res).is_some_and(|t| format::is_depth(t.format) && !self.may_sample_depth(srv.res, t.format, stage))
    }

    /// Whether a texture SRV reads what the open render pass draws into.
    /// D3D11 unbinds such a view (`OMSetRenderTargets` and
    /// `PSSetShaderResources` set it to NULL: it reads zeros), and the
    /// engine counts on it: it leaves a target's last SRV bound while it
    /// draws into it again. In wgpu the pass would sample and render the
    /// same subresource, and the whole submit would fail.
    fn is_target(&self, srv: &View, dim: TextureViewDimension) -> bool {
        let all = |n: u32| if n == 0 || n == u32::MAX { u32::MAX } else { n };
        let within = |i: u32, first: u32, count: u32| i >= first && i - first < count;
        self.pass.is_some()
            && self.s.rt.iter().any(|r| {
                r.tex != 0
                    && r.tex == srv.res
                    && within(r.mip, srv.a, all(srv.b))
                    && match dim {
                        // A 3D texture's slices are one subresource per mip.
                        TextureViewDimension::D3 | TextureViewDimension::D1 => true,
                        TextureViewDimension::D2 => r.slice == srv.c,
                        TextureViewDimension::Cube => within(r.slice, srv.c, 6),
                        TextureViewDimension::D2Array | TextureViewDimension::CubeArray => {
                            within(r.slice, srv.c, all(srv.d))
                        }
                    }
            })
    }

    /// [`Self::is_target`] for a draw's binding, said once per shader.
    fn reads_target(&self, prog: &Program, stage: Stage, slot: u32, srv: &View, dim: TextureViewDimension) -> bool {
        let hit = stage != Stage::Compute && self.is_target(srv, dim);
        if hit {
            crate::log_once_for("srvrt", u64::from(prog.id) << 8 | u64::from(slot), || {
                format!(
                    "{}: t{slot} reads a render target of its pass: unbound for the draw, as D3D11 does",
                    prog.label
                )
            });
        }
        hit
    }

    /// A view of an SRV's texture in the dimension the shader declares.
    fn srv_view(&mut self, id: u32, dim: TextureViewDimension, sample: Sample) -> Option<wgpu::TextureView> {
        let caps = self.caps;
        let res = self.objs.srv(id)?.res;
        if res == id {
            return None;
        }
        let (srv, tex) = match self.objs.two_mut(id, res)? {
            (crate::objects::Obj::Srv(v), crate::objects::Obj::Texture(t)) => (v, t),
            _ => return None,
        };
        // Depth read as float (or stencil as uint) and float read as depth go
        // through copies (refreshed by prepare_proxies).
        let proxy = match (format::is_depth(tex.format), sample) {
            (true, Sample::Uint) => Some(1),
            (true, Sample::Float | Sample::Sint) => Some(0),
            (false, Sample::Depth) => Some(2),
            _ => None,
        };
        let key = dim as u64 | (proxy.map_or(0, |p| p + 1) as u64) << 8;
        if let Some((_, v)) = srv.views.iter().find(|(k, _)| *k == key) {
            return Some(v.clone());
        }
        let t = match proxy {
            Some(p) => &tex.proxies[p].as_ref()?.tex,
            None => &**tex,
        };
        let layers = t.layers;
        if matches!(dim, TextureViewDimension::Cube | TextureViewDimension::CubeArray)
            && layers.saturating_sub(srv.c) < 6
        {
            crate::log_once(&format!("cube{id}"), || format!("cube SRV over a texture with {layers} layer(s)"));
            return None;
        }
        let total = t.mips;
        let first_mip = srv.a.min(total - 1);
        let mips =
            if srv.b == 0 || srv.b == u32::MAX { total - first_mip } else { srv.b.min(total - first_mip) }.max(1);
        // The GPU texture may lack the first levels: they read as the next.
        let end = (first_mip + mips).saturating_sub(t.dropped).max(1);
        let first_mip = first_mip.saturating_sub(t.dropped).min(end - 1);
        let mips = end - first_mip;
        let (first_layer, layer_count) = match dim {
            TextureViewDimension::D3 | TextureViewDimension::D1 => (0, None),
            TextureViewDimension::D2 => (srv.c.min(layers - 1), Some(1)),
            TextureViewDimension::Cube => (srv.c.min(layers - 6), Some(6)),
            _ => {
                let first = srv.c.min(layers - 1);
                let count = if srv.d == 0 || srv.d == u32::MAX { layers - first } else { srv.d.min(layers - first) };
                (first, Some(count.max(1)))
            }
        };
        let base = t.format;
        let format =
            if proxy.is_none() && !format::is_depth(base) { format::view(srv.format, base, &caps) } else { base };
        let aspect = if proxy.is_none() && format::is_depth(base) {
            if matches!(srv.format, 22 | 47) {
                wgpu::TextureAspect::StencilOnly
            } else {
                wgpu::TextureAspect::DepthOnly
            }
        } else {
            wgpu::TextureAspect::All
        };
        // A view of one aspect of a depth-stencil texture has that aspect's format.
        let format = format.aspect_specific_format(aspect).unwrap_or(format);
        let view = t.tex.create_view(&wgpu::TextureViewDescriptor {
            label: None,
            format: Some(format),
            dimension: Some(dim),
            usage: None,
            aspect,
            base_mip_level: first_mip,
            mip_level_count: Some(mips),
            base_array_layer: first_layer,
            array_layer_count: layer_count,
        });
        srv.views.push((key, view.clone()));
        Some(view)
    }

    fn uav_view(&mut self, id: u32, dim: TextureViewDimension) -> Option<wgpu::TextureView> {
        let res = self.objs.uav(id)?.res;
        if res == id {
            return None;
        }
        let (uav, tex) = match self.objs.two_mut(id, res)? {
            (crate::objects::Obj::Uav(v), crate::objects::Obj::Texture(t)) => (v, t),
            _ => return None,
        };
        let key = dim as u64;
        if let Some((_, v)) = uav.views.iter().find(|(k, _)| *k == key) {
            return Some(v.clone());
        }
        // view dimension 5: TEXTURE2DARRAY (first slice b, count c)
        let (first, count) = match dim {
            TextureViewDimension::D2Array if uav.view_dim == 5 => (uav.b, Some(uav.c.max(1))),
            TextureViewDimension::D2Array => (0, Some(tex.layers)),
            TextureViewDimension::D2 if uav.view_dim == 5 => (uav.b, Some(1)),
            TextureViewDimension::D2 => (0, Some(1)),
            _ => (0, None),
        };
        let view = tex.tex.create_view(&wgpu::TextureViewDescriptor {
            dimension: Some(dim),
            base_mip_level: uav.a.min(tex.mips - 1),
            mip_level_count: Some(1),
            base_array_layer: first,
            array_layer_count: count,
            ..Default::default()
        });
        uav.views.push((key, view.clone()));
        Some(view)
    }

    // ---- draws ---------------------------------------------------------------------------

    /// What this draw looks like to the census (see [`crate::census`]): the
    /// targets it writes and their sizes, its programs, and whether it reads
    /// the back buffer back. Only called when the census is on, so gathering
    /// it costs a finished frame nothing.
    fn census_draw(&self, vs: &Program, ps: Option<&Program>) {
        // Stable across frames (it changes at a device reset); `present` sets
        // it for the frame being drawn.
        let bb = self.scene.back_buffer;
        let target = |t: crate::exec::Target| {
            let tex = self.objs.tex(t.tex);
            let (width, height, samples) = tex.map_or((0, 0, 0), |x| {
                (
                    x.width.checked_shr(t.mip).unwrap_or(0).max(1),
                    x.height.checked_shr(t.mip).unwrap_or(0).max(1),
                    x.samples,
                )
            });
            crate::census::Target {
                id: t.tex,
                width,
                height,
                format: t.format,
                mip: t.mip,
                slice: t.slice,
                samples,
            }
        };
        let targets: Vec<_> = self.s.rt.iter().filter(|t| t.tex != 0).map(|&t| target(t)).collect();
        let ds = (self.s.ds.tex != 0).then(|| target(self.s.ds));
        // Resolve SRV ids to resources once; dedup so each source texture is
        // counted once per draw even if bound in more than one slot.
        let mut reads: Vec<crate::census::Read> = self.s.srv[..2]
            .iter()
            .flat_map(|stage| stage.iter())
            .filter_map(|&id| {
                self.objs.srv(id).filter(|v| !v.is_buffer && v.res != 0).map(|v| crate::census::Read {
                    id: v.res,
                    first_mip: v.a,
                    mip_count: v.b.max(1),
                    first_slice: v.c,
                    slice_count: v.d.max(1),
                })
            })
            .collect();
        reads.sort_unstable();
        reads.dedup();
        let reads_bb = bb != 0 && reads.iter().any(|r| r.id == bb);
        crate::census::draw_enabled(&crate::census::Draw {
            bb,
            targets: &targets,
            ds,
            viewport: self.s.viewport,
            scissor: self.s.scissor,
            vs: Some(vs.label.as_str()),
            ps: ps.map(|p| crate::census::Ps {
                id: p.id,
                label: p.label.as_str(),
                reads_position: p.frag_coord,
            }),
            reads: &reads,
            reads_bb,
            blend: self.s.blend,
        });
    }

    pub fn draw(&mut self, c: &Command) {
        // kind: 0 draw, 1 indexed, 2 instanced, 3 indexed instanced, 4/5 indirect (indexed / not)
        let (kind, count, instances, start, base_vertex, start_instance) =
            (c.w(0), c.w(1), c.w(2), c.w(3), c.w(4) as i32, c.w(5));
        let Some(vs) = self.program(self.s.shader[0]) else {
            return self.skip("no vertex shader");
        };
        let ps = match self.s.shader[1] {
            0 => None,
            id => match self.program(id) {
                Some(p) => Some(p),
                None => return self.skip("pixel shader unusable"),
            },
        };
        let (vs, ps) = if vs.label == "im::VS_Blit" && ps.as_ref().is_some_and(|p| p.label == "im::PS_BlitDepthToAlpha")
        {
            // Pause-map exploration uses world-space tiles and MIN alpha
            // blending. The HUD mask instead uses normalized vertices and
            // filtered coverage. Choose programs before reflecting bindings
            // and making pipeline keys, including the mask's distinct id.
            let pause = self.objs.state(self.s.blend).is_some_and(|s| shader::pause_map_blend(&s.words));
            if pause {
                let Some(vs) = self.library.named(&self.device, "im::VS_TransformUnlit_IM", Stage::Vertex) else {
                    return self.skip("pause-map vertex shader unusable");
                };
                (vs, ps)
            } else {
                let Some(ps) = ps.unwrap().for_hud_mask(&self.device) else {
                    return self.skip("minimap mask unusable");
                };
                (vs, Some(ps))
            }
        } else {
            (vs, ps)
        };
        // D3D draws without an input layout when the vertex shader reads no
        // vertex attributes (a full-screen triangle from SV_VertexID, as the
        // highest post-processing level draws): no vertex buffers then.
        if self.objs.layout(self.s.layout).is_none()
            && self.objs.shader(self.s.shader[0]).is_some_and(|s| !s.sig.is_empty())
        {
            return self.skip("no input layout");
        }
        let (vs, vs_float32, vs_proxies) = self.for_bound_formats(vs, Stage::Vertex);
        let (ps, ps_float32, ps_proxies) = match ps {
            Some(ps) => {
                let (ps, float32, proxies) = self.for_bound_formats(ps, Stage::Pixel);
                (Some(ps), float32, proxies)
            }
            None => (None, 0, 0),
        };
        // Same state, other textures, can mean another program variant
        // (or, here, only another recipe).
        let programs = (vs.id, ps.as_ref().map_or(0, |p| p.id), vs_float32, ps_float32);
        if programs != self.last_programs {
            self.last_programs = programs;
            self.pdirty = true;
        }
        self.prepare_proxies(&vs, vs_proxies);
        if let Some(ps) = &ps {
            self.prepare_proxies(ps, ps_proxies);
        }
        if !self.ensure_pass() {
            return self.skip("no render target");
        }
        let Some(cell) = self.pipeline(&vs, ps.as_ref(), [vs_float32, ps_float32]) else {
            return self.skip("no pipeline");
        };
        let Some(pipe_slot) = cell.get() else {
            self.stats.pending_skips += 1;
            return;
        };
        let Some(pipe) = pipe_slot else {
            return self.skip("pipeline failed");
        };
        self.last_pipe = Some(cell.clone());
        self.pdirty = false;
        if let Some(i) = self.pass_profile.take()
            && let Some(p) = self.profiler.as_mut()
        {
            p.rename(i, &ps.as_ref().unwrap_or(&vs).label);
        }

        self.observe_camera();
        let g0 = match &vs.layout {
            Some(l) => match self.bind_group(&vs, Stage::Vertex, l) {
                Some(g) => Some(g),
                None => return self.skip("vertex bind group"),
            },
            None => None,
        };
        let g1 = match ps.as_ref().and_then(|p| p.layout.as_ref().map(|l| (p, l))) {
            Some((p, l)) => match self.bind_group(p, Stage::Pixel, l) {
                Some(g) => Some(g),
                None => return self.skip("pixel bind group"),
            },
            None => None,
        };

        let indexed = kind == 4 || (kind < 4 && kind & 1 != 0);
        // SV_VertexID / SV_InstanceID do not include the draw's starts; WebGPU's
        // builtins do. Only shaders reading them get the starts folded into
        // the buffer offsets instead.
        let (fold_v, fold_i) = (vs.vertex_index, vs.instance_index);
        let first_vertex = if indexed || fold_v { 0 } else { start };
        let first_instance = if fold_i { 0 } else { start_instance };

        let epoch = self.epoch;
        let mut avail = [0u64; 16];
        // On the stack: thousands of draws a frame.
        let mut vbs: ArrayVec<(u32, Option<(wgpu::Buffer, u32)>, u64), 16> = ArrayVec::new();
        let layout = self.objs.layout(self.s.layout).unwrap_or(&NO_LAYOUT);
        for &(slot, class) in &layout.slots {
            let v = self.s.vb[slot as usize];
            match self.objs.buf(v.buf).and_then(|b| b.gpu.as_ref().map(|g| (b, g))) {
                Some((b, g)) => {
                    let shift = if class == 1 {
                        if fold_i { start_instance as i64 * v.stride as i64 } else { 0 }
                    } else if fold_v {
                        (if indexed { base_vertex as i64 } else { start as i64 }) * v.stride as i64
                    } else {
                        0
                    };
                    let offset = (v.offset as i64 + shift).max(0).min(b.bytes as i64) as u64;
                    if offset < b.bytes {
                        avail[slot as usize] = b.bytes - offset;
                        b.used.set(epoch);
                        vbs.push((slot, Some((g.clone(), b.uid)), b.base + offset));
                    } else {
                        // Bound at its end: nothing to read (a draw that
                        // reads it is clamped away below); the zeros bound
                        // instead, as an empty slice would stop the GPU thread.
                        vbs.push((slot, None, 0));
                    }
                }
                None => {
                    avail[slot as usize] = ZERO_VB;
                    vbs.push((slot, None, 0));
                }
            }
        }
        let mut index = None;
        let mut index_max = u64::MAX;
        if indexed {
            let (id, fmt, offset) = self.s.ib;
            let Some((b, g)) = self.objs.buf(id).and_then(|b| b.gpu.as_ref().map(|g| (b, g))) else {
                return self.skip("no index buffer");
            };
            if offset as u64 >= b.bytes {
                return self.skip("index buffer bound past its end");
            }
            let format = if fmt == format::INDEX_U16 { wgpu::IndexFormat::Uint16 } else { wgpu::IndexFormat::Uint32 };
            let size = if fmt == format::INDEX_U16 { 2 } else { 4 };
            index_max = (b.bytes.saturating_sub(offset as u64) / size).saturating_sub(start as u64);
            b.used.set(epoch);
            index = Some((g.clone(), b.uid, format, b.base + offset as u64));
        }
        if kind >= 4 && !self.caps.indirect {
            return self.skip("indirect draw (unsupported by the device)");
        }
        let indirect = if kind >= 4 {
            match self.objs.buf(c.w(1)).and_then(|b| b.gpu.as_ref().map(|g| (b, g))) {
                Some((b, g)) => {
                    b.used.set(epoch);
                    Some((g.clone(), b.base + c.w(2) as u64))
                }
                None => return self.skip("indirect draw without arguments"),
            }
        } else {
            None
        };

        // D3D11 reads zeros past the end of vertex/index buffers; WebGPU
        // rejects the draw (and the whole command buffer). Counts are clamped
        // to what the buffers hold: the elements dropped are those D3D fed zeros.
        let (mut n, mut inst) = (count as u64, instances as u64);
        for need in &pipe.needs {
            let a = avail[need.slot as usize];
            let fit = if a < need.last as u64 {
                0
            } else if need.stride == 0 {
                u64::MAX
            } else {
                (a - need.last as u64) / need.stride as u64 + 1
            };
            if need.instance {
                inst = inst.min(fit.saturating_sub(first_instance as u64));
            } else if !indexed {
                n = n.min(fit.saturating_sub(first_vertex as u64));
            }
        }
        if indexed {
            n = n.min(index_max);
        }
        if indirect.is_none() && (n != count as u64 || inst != instances as u64) {
            self.stats.clamped += 1;
            if n == 0 || inst == 0 {
                return self.skip("buffers smaller than the draw");
            }
        }

        let Some(info) = self.pass_info.as_ref() else {
            return;
        };
        let (width, height) = (info.width, info.height);
        let max_dim = self.limits.max_texture_dimension_2d as f32;
        let viewport = self.s.viewport;
        let scissor = match (
            self.objs.state(self.s.raster).is_some_and(|r| r.words.get(7).is_some_and(|&s| s != 0)),
            self.s.scissor,
        ) {
            (true, Some([l, t, r, b])) => {
                let (x0, y0) = (l.min(width), t.min(height));
                let (x1, y1) = (r.clamp(x0, width), b.clamp(y0, height));
                [x0, y0, x1 - x0, y1 - y0]
            }
            _ => [0, 0, width, height],
        };
        let blend = self.s.blend_factor;
        let stencil_ref = self.s.stencil_ref;
        // The rasterizer state's depth bias, set on the pass rather than
        // made part of the pipeline (see `pipeline::without_depth_bias`).
        let depth_bias = match self.objs.state(self.s.raster).and_then(|r| r.words.get(3..6)) {
            Some(&[constant, clamp, slope]) => [constant, if self.caps.depth_bias_clamp { clamp } else { 0 }, slope],
            _ => [0; 3],
        };
        let zero = if vbs.iter().any(|v| v.1.is_none()) { Some(self.dummies.vertices(&self.device)) } else { None };
        if crate::census::on() {
            self.census_draw(&vs, ps.as_deref());
        }
        let bound = &mut self.bound;
        let pass = self.pass.as_mut().unwrap();

        let pipe_id = Arc::as_ptr(&cell) as usize;
        if bound.pipeline != pipe_id {
            pass.set_pipeline(&pipe.pipeline);
            bound.pipeline = pipe_id;
        }
        for (i, g) in [g0, g1].into_iter().enumerate() {
            if let Some((bg, serial, offsets)) = g
                && bound.groups[i] != Some((serial, offsets))
            {
                pass.set_bind_group(i as u32, &bg, offsets.as_slice());
                bound.groups[i] = Some((serial, offsets));
            }
        }
        for (slot, buf, offset) in &vbs {
            let (uid, b) = match buf {
                Some((g, uid)) => (*uid, g),
                None => (0, zero.as_ref().unwrap()),
            };
            if bound.vb[*slot as usize] != Some((uid, *offset)) {
                pass.set_vertex_buffer(*slot, b.slice(*offset..));
                bound.vb[*slot as usize] = Some((uid, *offset));
            }
        }
        if let Some((g, uid, format, offset)) = &index
            && bound.ib != Some((*uid, *format as u32, *offset))
        {
            pass.set_index_buffer(g.slice(*offset..), *format);
            bound.ib = Some((*uid, *format as u32, *offset));
        }
        if let Some(v) = viewport
            && bound.viewport != Some(v)
        {
            let [x, y, w, h, min_d, max_d] = v;
            // D3D viewports may extend past the target (guard band); only the
            // API's limits apply.
            let (size, range) = (max_dim, 2.0 * max_dim - 1.0);
            let (w, h) = (w.clamp(1.0, size), h.clamp(1.0, size));
            let (x, y) = (x.clamp(-range, range - w), y.clamp(-range, range - h));
            let (min_d, max_d) = (min_d.clamp(0.0, 1.0), max_d.clamp(0.0, 1.0));
            pass.set_viewport(x, y, w, h, min_d, max_d.max(min_d));
            bound.viewport = Some(v);
        }
        if bound.scissor != Some(scissor) {
            pass.set_scissor_rect(scissor[0], scissor[1], scissor[2], scissor[3]);
            bound.scissor = Some(scissor);
        }
        if bound.blend != Some(blend) {
            pass.set_blend_constant(wgpu::Color {
                r: blend[0] as f64,
                g: blend[1] as f64,
                b: blend[2] as f64,
                a: blend[3] as f64,
            });
            bound.blend = Some(blend);
        }
        if bound.stencil_ref != Some(stencil_ref) {
            pass.set_stencil_reference(stencil_ref);
            bound.stencil_ref = Some(stencil_ref);
        }
        if bound.depth_bias != depth_bias {
            let [constant, clamp, slope] = depth_bias;
            pass.set_depth_bias(wgpu::DepthBiasState {
                constant: constant as i32,
                slope_scale: f32::from_bits(slope),
                clamp: f32::from_bits(clamp),
            });
            bound.depth_bias = depth_bias;
        }
        match indirect {
            Some((args, offset)) if kind == 4 => pass.draw_indexed_indirect(&args, offset),
            Some((args, offset)) => pass.draw_indirect(&args, offset),
            None if indexed => {
                let base = if fold_v { 0 } else { base_vertex };
                pass.draw_indexed(start..start + n as u32, base, first_instance..first_instance + inst as u32)
            }
            None => pass.draw(first_vertex..first_vertex + n as u32, first_instance..first_instance + inst as u32),
        }
        self.stats.draws += 1;
    }

    /// A draw into the scene's targets may show this frame's camera (see
    /// scene.rs): the vertex program's `rage_matrices`.
    fn observe_camera(&mut self) {
        let sc = &self.scene;
        if !sc.wants_camera() {
            return;
        }
        let scene_pass = self.pass_info.as_ref().is_some_and(|p| p.depth.is_some() && sc.size == Some((p.width, p.height)));
        if !scene_pass {
            return;
        }
        let cb = self.s.cb[0][1];
        let from = cb.first as usize * 16;
        let Some(bytes) = self.objs.buf(cb.buf).and_then(|b| b.shadow.as_deref()).and_then(|s| s.get(from..from + 256))
        else {
            return;
        };
        let m: [f32; 64] = std::array::from_fn(|i| f32::from_le_bytes(bytes[i * 4..i * 4 + 4].try_into().unwrap()));
        let depth = self.s.ds.tex;
        self.scene.observe(&m, depth);
    }

    // ---- compute ---------------------------------------------------------------------------

    pub fn dispatch(&mut self, x: u32, y: u32, z: u32) {
        self.flush_clears();
        let Some(cs) = self.program(self.s.shader[2]) else {
            return self.skip("no compute shader");
        };
        let (cs, _, proxies) = self.for_bound_formats(cs, Stage::Compute);
        self.prepare_proxies(&cs, proxies);
        self.end_pass();
        // Storage textures are declared with a guessed format; the pipeline
        // needs the bound textures' formats.
        let mut subs = Vec::new();
        for b in &cs.bindings {
            if let Kind::StorageTexture { format, .. } = &b.kind {
                let id = self.s.uav[2][(b.binding - shader::UAVS) as usize];
                if let Some(t) = self.objs.uav(id).filter(|v| !v.is_buffer).and_then(|v| self.objs.tex(v.res))
                    && t.format != *format
                {
                    subs.push((b.binding, *format, t.format));
                }
            }
        }
        let Some(compute) = self.computes.get(&self.device, &cs, &subs) else {
            return self.skip("compute pipeline failed");
        };
        let Some((bg, _, offsets)) = self.bind_group(&compute.program, Stage::Compute, &compute.layout) else {
            return self.skip("compute bind group");
        };
        self.stats.dispatches += 1;
        let ts = self.time_pass(&cs.label);
        self.end_pass();
        let encoder = self.encoder.get_or_insert_with(|| self.device.create_command_encoder(&Default::default()));
        let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
            label: None,
            timestamp_writes: ts.and_then(|i| self.profiler.as_ref().map(|p| p.compute(i))),
        });
        pass.set_pipeline(&compute.pipeline);
        pass.set_bind_group(0, &bg, offsets.as_slice());
        pass.dispatch_workgroups(x, y, z);
    }
}

/// Copies the constant buffers a program's stage reads into the uniform ring
/// (back to back, as packed), or finds the same copy made earlier this frame.
#[allow(clippy::too_many_arguments)]
fn pack_constants(
    objs: &Objects,
    cbs: &[[crate::exec::CbBinding; 16]; 3],
    uniforms: &mut Uniforms,
    packs: &mut FxHashMap<(u32, u8), PackCache>,
    device: &wgpu::Device,
    prog: &Program,
    stage: Stage,
    pack: &shader::Pack,
    jitter: Option<[f32; 2]>,
) -> Constants {
    let st = stage as usize;
    let mut own = [None; shader::OWN_MAX];
    for (i, m) in pack.own.iter().enumerate() {
        own[i] = Some(own_constants(objs, cbs[st].get(m.slot as usize).copied().unwrap_or_default(), uniforms, device, m.size));
    }
    let mut current: PackIds = [(0, 0, 0); 16];
    for (i, m) in pack.members.iter().enumerate().take(16) {
        let cb = cbs[st].get(m.slot as usize).copied().unwrap_or_default();
        let version = objs.buf(cb.buf).map_or(u32::MAX, |b| b.version);
        current[i] = (cb.buf, version, cb.first);
    }
    let jitter_key = jitter.map(|j| j.map(f32::to_bits));
    let cache = packs.entry((prog.id, st as u8)).or_default();
    if let Some((_, _, _, alloc)) = cache
        .entries
        .iter()
        .find(|(frame, ids, j, _)| *frame == uniforms.frame && *ids == current && *j == jitter_key)
    {
        return Constants { pack: *alloc, own };
    }
    let alloc = uniforms.alloc(device, pack.size);
    let dst = uniforms.bytes(alloc, pack.size);
    for (m, &(buf, _, first)) in pack.members.iter().zip(&current) {
        let out = &mut dst[m.offset as usize..(m.offset + m.size) as usize];
        copy_constants(objs.buf(buf).and_then(|b| b.shadow.as_deref()), first, out);
        // `rage_matrices`, with gWorldViewProj.
        if let Some(clip) = jitter
            && m.slot == 1
            && m.size >= 192
        {
            crate::scene::jitter_constants(out, clip);
        }
    }
    if cache.entries.len() >= 6 {
        cache.entries.remove(0);
    }
    cache.entries.push((uniforms.frame, current, jitter_key, alloc));
    Constants { pack: alloc, own }
}

/// A constant buffer bound on its own (`size` bytes from its `first`
/// constant): copied once per version and frame, for every stage binding it.
fn own_constants(
    objs: &Objects,
    cb: crate::exec::CbBinding,
    uniforms: &mut Uniforms,
    device: &wgpu::Device,
    size: u32,
) -> Alloc {
    let buf = objs.buf(cb.buf);
    let version = buf.map_or(u32::MAX, |b| b.version);
    if let Some((frame, v, first, copied, alloc)) = buf.and_then(|b| b.constants.get())
        && (frame, v, first) == (uniforms.frame, version, cb.first)
        && copied >= size
    {
        return alloc;
    }
    let alloc = uniforms.alloc(device, size);
    copy_constants(buf.and_then(|b| b.shadow.as_deref()), cb.first, uniforms.bytes(alloc, size));
    if let Some(b) = buf {
        b.constants.set(Some((uniforms.frame, version, cb.first, size, alloc)));
    }
    alloc
}

/// A constant buffer's data from its `first` constant into `out`, zeros
/// past its end (as D3D reads them).
fn copy_constants(src: Option<&[u8]>, first: u32, out: &mut [u8]) {
    let copied = match src {
        Some(s) => {
            let from = (first as usize * 16).min(s.len());
            let n = (s.len() - from).min(out.len());
            out[..n].copy_from_slice(&s[from..from + n]);
            n
        }
        None => 0,
    };
    out[copied..].fill(0);
}
