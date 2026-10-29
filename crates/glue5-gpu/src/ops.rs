//! Everything but draws: depth copies for sampling, copies between
//! resources, resolves, mipmaps, UAV clears and counters, readbacks, queries.

use std::sync::Arc;
use std::time::Instant;

use glue5_runtime::gpu::{Command, query};
use rustc_hash::FxHashMap;
use wgpu::TextureFormat;

use crate::done::{Copy, OcclusionResult, Work};
use crate::exec::Gpu;
use crate::format;
use crate::objects::{Proxy, Tex};
use crate::pipeline::Compiler;
use crate::shader::{self, Kind, Program};

const FULLSCREEN_VS: &str = "@vertex fn vs(@builtin(vertex_index) i: u32) -> @builtin(position) vec4f { var p = array<vec2f, 3>(vec2f(-1, -1), vec2f(3, -1), vec2f(-1, 3)); return vec4f(p[i], 0, 1); }\n";

pub struct ComputePipe {
    pub pipeline: wgpu::ComputePipeline,
    pub layout: wgpu::BindGroupLayout,
    pub program: Arc<Program>,
}

type ComputeKey = (u32, Vec<(u32, TextureFormat, TextureFormat)>);

/// Compute pipelines, shared with the compile threads (which build each
/// compute shader's pipeline when the engine creates it); those of the
/// recipes known are made before the game starts (see `prepare.rs`).
#[derive(Default)]
pub struct Computes {
    cells: std::sync::Mutex<FxHashMap<ComputeKey, Arc<std::sync::OnceLock<Option<Arc<ComputePipe>>>>>>,
}

impl Computes {
    /// The pipeline for `cs` with storage textures in the formats bound
    /// (`subs`: binding, declared format, bound format), and whether it was
    /// not made yet (made now, or waited for while a compile thread makes it).
    pub fn get(
        &self,
        compiler: &Compiler,
        cs: &Arc<Program>,
        subs: &[(u32, TextureFormat, TextureFormat)],
    ) -> (Option<Arc<ComputePipe>>, bool) {
        let cell = self.cells.lock().unwrap().entry((cs.id, subs.to_vec())).or_default().clone();
        let late = cell.get().is_none();
        (cell.get_or_init(|| build_compute(compiler, cs, subs)).clone(), late)
    }

    /// Takes out the idle shader modules of the programs compute pipelines
    /// were made from (see `Library::take_idle_modules`).
    pub fn take_idle_modules(&self, idle: u64, max: usize, out: &mut Vec<wgpu::ShaderModule>) {
        let pipes: Vec<_> = self.cells.lock().unwrap().values().filter_map(|c| c.get().cloned().flatten()).collect();
        for p in pipes {
            p.program.modules.take_idle(idle, max, out);
        }
    }
}

/// Builds a compute pipeline (any thread), counted with the others.
fn build_compute(
    compiler: &Compiler,
    cs: &Arc<Program>,
    subs: &[(u32, TextureFormat, TextureFormat)],
) -> Option<Arc<ComputePipe>> {
    let t0 = Instant::now();
    let pipe = make_compute(compiler, cs, subs);
    compiler.count(t0, pipe.is_some());
    pipe
}

fn make_compute(
    compiler: &Compiler,
    cs: &Arc<Program>,
    subs: &[(u32, TextureFormat, TextureFormat)],
) -> Option<Arc<ComputePipe>> {
    let device = &compiler.device;
    let program = if subs.is_empty() {
        cs.clone()
    } else {
        match cs.with_text(device, &shader::with_storage_formats(&cs.text.get(), subs), "uav formats") {
            Ok(p) => Arc::new(p),
            Err(e) => {
                log::warn!("gpu: {}: {e:#}", cs.label);
                return None;
            }
        }
    };
    let layout = device
        .create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor { label: None, entries: &program.layout_entries() });
    let module = program.module(device)?;
    // Its constants in group 2, as a vertex stage's.
    let pl = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: None,
        bind_group_layouts: &[Some(&layout), None, program.constants.as_ref()],
        immediate_size: 0,
    });
    let scope = device.push_error_scope(wgpu::ErrorFilter::Validation);
    let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
        label: Some(&program.label),
        layout: Some(&pl),
        module: &module,
        entry_point: Some("main"),
        compilation_options: wgpu::PipelineCompilationOptions {
            constants: &[],
            zero_initialize_workgroup_memory: false,
        },
        cache: compiler.cache.as_ref(),
    });
    if let Some(e) = pollster::block_on(scope.pop()) {
        log::warn!("gpu: compute pipeline {} failed: {e}", program.label);
        return None;
    }
    Some(Arc::new(ComputePipe { pipeline, layout, program }))
}

/// The renderer's own pipelines: depth copies made before the game starts,
/// mipmaps by format on first use.
pub struct Blits {
    proxies: [Option<wgpu::RenderPipeline>; 3],
    mips: FxHashMap<TextureFormat, Option<wgpu::RenderPipeline>>,
    zeros: Option<wgpu::Buffer>,
    scratch: Option<wgpu::Buffer>,
}

fn fullscreen_pipeline(
    compiler: &Compiler,
    label: &str,
    fs: &str,
    target: Option<TextureFormat>,
    depth: Option<TextureFormat>,
) -> Option<wgpu::RenderPipeline> {
    let t0 = Instant::now();
    let device = &compiler.device;
    let Ok(module) = shader::create_module(device, label, &format!("{FULLSCREEN_VS}{fs}")) else {
        compiler.count(t0, false);
        return None;
    };
    let targets = [target.map(wgpu::ColorTargetState::from)];
    let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some(label),
        layout: None,
        vertex: wgpu::VertexState {
            module: &module,
            entry_point: Some("vs"),
            compilation_options: Default::default(),
            buffers: &[],
        },
        primitive: Default::default(),
        depth_stencil: depth.map(|format| wgpu::DepthStencilState {
            format,
            depth_write_enabled: Some(true),
            depth_compare: Some(wgpu::CompareFunction::Always),
            stencil: Default::default(),
            bias: Default::default(),
        }),
        multisample: Default::default(),
        fragment: Some(wgpu::FragmentState {
            module: &module,
            entry_point: Some("fs"),
            compilation_options: Default::default(),
            targets: if target.is_some() { &targets } else { &[] },
        }),
        multiview_mask: None,
        cache: compiler.cache.as_ref(),
    });
    compiler.count(t0, true);
    Some(pipeline)
}

impl Blits {
    /// Makes the depth copies' pipelines: 0: depth -> r32float, 1: stencil
    /// -> rg8uint, 2: float -> depth32float.
    ///
    /// A D3D stencil SRV (X24_TYPELESS_G8_UINT, X32_TYPELESS_G8X24_UINT)
    /// reads the stencil in green, and the game's shaders test `.y` (the
    /// interior bit, 8, decides which local lights and which ambient light
    /// a pixel gets); WebGPU's stencil view has it in red. The copy has it
    /// in both.
    pub fn new(compiler: &Compiler) -> Blits {
        let proxies = [
            fullscreen_pipeline(
                compiler,
                "depth as float",
                "@group(0) @binding(0) var src: texture_depth_2d;\n@fragment fn fs(@builtin(position) p: vec4f) -> @location(0) vec4f { return vec4f(textureLoad(src, vec2i(p.xy), 0), 0, 0, 1); }",
                Some(TextureFormat::R32Float),
                None,
            ),
            fullscreen_pipeline(
                compiler,
                "stencil as uint",
                "@group(0) @binding(0) var src: texture_2d<u32>;\n@fragment fn fs(@builtin(position) p: vec4f) -> @location(0) vec4<u32> { let s = textureLoad(src, vec2i(p.xy), 0).x; return vec4<u32>(s, s, 0u, 1u); }",
                Some(TextureFormat::Rg8Uint),
                None,
            ),
            fullscreen_pipeline(
                compiler,
                "float as depth",
                "@group(0) @binding(0) var src: texture_2d<f32>;\n@fragment fn fs(@builtin(position) p: vec4f) -> @builtin(frag_depth) f32 { return textureLoad(src, vec2i(p.xy), 0).x; }",
                None,
                Some(TextureFormat::Depth32Float),
            ),
        ];
        Blits { proxies, mips: FxHashMap::default(), zeros: None, scratch: None }
    }

    /// A 2x2 box filter of the previous mip (textureLoad: any float format).
    fn mip(&mut self, compiler: &Compiler, format: TextureFormat) -> Option<wgpu::RenderPipeline> {
        self.mips
            .entry(format)
            .or_insert_with(|| {
                fullscreen_pipeline(
                    compiler,
                    "mip",
                    "@group(0) @binding(0) var src: texture_2d<f32>;
@fragment fn fs(@builtin(position) p: vec4f) -> @location(0) vec4f {
  let m = vec2i(textureDimensions(src)) - 1;
  let c = vec2i(p.xy) * 2;
  return (textureLoad(src, min(c, m), 0) + textureLoad(src, min(c + vec2i(1, 0), m), 0)
        + textureLoad(src, min(c + vec2i(0, 1), m), 0) + textureLoad(src, min(c + vec2i(1, 1), m), 0)) * 0.25;
}",
                    Some(format),
                    None,
                )
            })
            .clone()
    }

    fn zeros(&mut self, device: &wgpu::Device, size: u64) -> wgpu::Buffer {
        if self.zeros.as_ref().is_none_or(|b| b.size() < size) {
            self.zeros = Some(device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("zeros"),
                size: size.max(1 << 20).next_power_of_two(),
                usage: wgpu::BufferUsages::COPY_SRC,
                mapped_at_creation: false,
            }));
        }
        self.zeros.clone().unwrap()
    }

    fn scratch(&mut self, device: &wgpu::Device, size: u64) -> wgpu::Buffer {
        if self.scratch.as_ref().is_none_or(|b| b.size() < size) {
            self.scratch = Some(device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("copy scratch"),
                size: size.max(1 << 16).next_power_of_two(),
                usage: wgpu::BufferUsages::COPY_SRC | wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::QUERY_RESOLVE,
                mapped_at_creation: false,
            }));
        }
        self.scratch.clone().unwrap()
    }
}

/// Occlusion queries: one query-set slot per (query, render pass) between
/// Begin and End; the results are resolved at the next submit.
#[derive(Default)]
pub struct Occlusion {
    pub set: Option<wgpu::QuerySet>,
    next: u32,
    active: Option<u32>,
    pass_slot: Option<u32>,
    /// Ended queries to resolve: result address, sequence, predicate, slots.
    pub pending: Vec<(u64, u32, bool, Vec<u32>)>,
    resolve: Option<wgpu::Buffer>,
}

const OCCLUSION_SLOTS: u32 = 4096;

impl Occlusion {
    pub fn ensure_set(&mut self, device: &wgpu::Device) {
        if self.set.is_none() {
            self.set = Some(device.create_query_set(&wgpu::QuerySetDescriptor {
                label: Some("occlusion"),
                ty: wgpu::QueryType::Occlusion,
                count: OCCLUSION_SLOTS,
            }));
        }
    }
}

fn copy_compatible(a: TextureFormat, b: TextureFormat) -> bool {
    a == b || a.remove_srgb_suffix() == b.remove_srgb_suffix()
}

impl Gpu {
    // ---- textures read through copies ---------------------------------------------

    /// Refreshes the copies the program's textures are read through.
    /// `slots`: the texture slots that may need one (see `proxy_slots`).
    pub fn prepare_proxies(&mut self, prog: &Program, slots: u128) {
        if slots == 0 {
            return;
        }
        let st = prog.stage as usize;
        for &i in &prog.textures {
            let b = &prog.bindings[i];
            let Kind::Texture { sample, .. } = b.kind else {
                continue;
            };
            if slots & 1 << (b.binding - shader::SRVS) == 0 {
                continue;
            }
            let Some(res) = self
                .objs
                .srv(self.s.srv[st][(b.binding - shader::SRVS) as usize])
                .filter(|v| !v.is_buffer)
                .map(|v| v.res)
            else {
                continue;
            };
            let Some(t) = self.objs.tex(res) else {
                continue;
            };
            let Some(kind) = crate::draw::proxy_kind(t.format, sample) else {
                continue;
            };
            if t.samples > 1 {
                crate::log_once_for("msproxy", u64::from(prog.id), || {
                    format!("{}: multisampled depth read as float: not supported", prog.label)
                });
                continue;
            }
            if t.proxies[kind].as_ref().is_none_or(|p| p.dirty) {
                self.refresh_proxy(res, kind);
            }
        }
    }

    fn refresh_proxy(&mut self, id: u32, kind: usize) {
        if self.clears.iter().any(|c| c.tex == id) {
            self.flush_clears();
        }
        let Some(pipeline) = self.blits.proxies[kind].clone() else {
            return;
        };
        self.end_pass();
        let epoch = self.epoch;
        let device = self.device.clone();
        let Some(t) = self.objs.tex_mut(id) else {
            return;
        };
        if t.proxies[kind].is_none() {
            let format = [TextureFormat::R32Float, TextureFormat::Rg8Uint, TextureFormat::Depth32Float][kind];
            let size = wgpu::Extent3d { width: t.width, height: t.height, depth_or_array_layers: t.layers };
            let tex = device.create_texture(&wgpu::TextureDescriptor {
                label: Some("proxy"),
                size,
                mip_level_count: t.mips,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format,
                usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::RENDER_ATTACHMENT,
                view_formats: &[],
            });
            let mut passes = Vec::new();
            for mip in 0..t.mips {
                for layer in 0..t.layers {
                    let one = |aspect| wgpu::TextureViewDescriptor {
                        dimension: Some(wgpu::TextureViewDimension::D2),
                        base_mip_level: mip,
                        mip_level_count: Some(1),
                        base_array_layer: layer,
                        array_layer_count: Some(1),
                        aspect,
                        ..Default::default()
                    };
                    let src_aspect = match kind {
                        0 => wgpu::TextureAspect::DepthOnly,
                        1 => wgpu::TextureAspect::StencilOnly,
                        _ => wgpu::TextureAspect::All,
                    };
                    let src = t.tex.create_view(&one(src_aspect));
                    let dst = tex.create_view(&one(wgpu::TextureAspect::All));
                    let bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
                        label: None,
                        layout: &pipeline.get_bind_group_layout(0),
                        entries: &[wgpu::BindGroupEntry {
                            binding: 0,
                            resource: wgpu::BindingResource::TextureView(&src),
                        }],
                    });
                    passes.push((dst, bg));
                }
            }
            let proxy = Tex {
                tex,
                format,
                upload: format::Upload::Direct,
                dim: wgpu::TextureDimension::D2,
                width: t.width,
                height: t.height,
                layers: t.layers,
                mips: t.mips,
                dropped: 0,
                samples: 1,
                bytes: 0,
                recycle: None,
                views: Vec::new(),
                used: Default::default(),
                uploaded: Default::default(),
                in_bind_group: Default::default(),
                proxies: Default::default(),
                mip_chain: None,
                present: None,
            };
            t.proxies[kind] = Some(Box::new(Proxy { tex: proxy, dirty: true, passes }));
        }
        t.used.set(epoch);
        let proxy = t.proxies[kind].as_mut().unwrap();
        proxy.dirty = false;
        proxy.tex.used.set(epoch);
        let passes: Vec<_> = proxy.passes.iter().map(|(v, bg)| (v.clone(), bg.clone())).collect();
        for (dst, bg) in passes {
            let ts = self.time_pass(["proxy depth as float", "proxy stencil", "proxy float as depth"][kind]);
            self.end_pass();
            let encoder = self.encoder.get_or_insert_with(|| self.device.create_command_encoder(&Default::default()));
            let timestamp_writes = ts.and_then(|i| self.profiler.as_ref().map(|p| p.render(i)));
            let mut pass = if kind == 2 {
                encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                    depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                        view: &dst,
                        depth_ops: Some(wgpu::Operations {
                            load: wgpu::LoadOp::Clear(1.0),
                            store: wgpu::StoreOp::Store,
                        }),
                        stencil_ops: None,
                    }),
                    timestamp_writes,
                    ..Default::default()
                })
            } else {
                encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view: &dst,
                        depth_slice: None,
                        resolve_target: None,
                        ops: wgpu::Operations {
                            load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                            store: wgpu::StoreOp::Store,
                        },
                    })],
                    timestamp_writes,
                    ..Default::default()
                })
            };
            pass.set_pipeline(&pipeline);
            pass.set_bind_group(0, &bg, &[]);
            pass.draw(0..3, 0..1);
        }
    }

    // ---- copies ------------------------------------------------------------------------

    /// A buffer copy; two buffers in the same slab (see slabs.rs) go through
    /// a temporary buffer, as a copy within one buffer is not allowed.
    fn copy_buffer(&mut self, src: &wgpu::Buffer, from: u64, dst: &wgpu::Buffer, to: u64, size: u64) {
        if src != dst {
            self.encoder().copy_buffer_to_buffer(src, from, dst, to, size);
            return;
        }
        let temp = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("copy within a slab"),
            size,
            usage: wgpu::BufferUsages::COPY_SRC | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let encoder = self.encoder();
        encoder.copy_buffer_to_buffer(src, from, &temp, 0, size);
        encoder.copy_buffer_to_buffer(&temp, 0, dst, to, size);
        self.graveyard.push(crate::submit::Grave::Buffer(temp));
    }

    pub fn copy_region(&mut self, c: &Command) {
        let (dst, dst_sub, dx, dy, dz, src, src_sub, has_box) =
            (c.w(0), c.w(1), c.w(2), c.w(3), c.w(4), c.w(5), c.w(6), c.w(7) != 0);
        let bx = [c.w(8), c.w(9), c.w(10), c.w(11), c.w(12), c.w(13)]; // left top front right bottom back
        self.flush_clears_of(&[src, dst]);
        let epoch = self.epoch;
        if let (Some(s), Some(d)) = (self.objs.buf(src), self.objs.buf(dst)) {
            let (Some(sg), Some(dg)) = (s.gpu.clone(), d.gpu.clone()) else {
                return;
            };
            let from = if has_box { bx[0] as u64 } else { 0 };
            let size = if has_box { (bx[3] - bx[0]) as u64 } else { s.size.min(d.size) };
            let size = size.min(s.bytes.saturating_sub(from)).min(d.bytes.saturating_sub(dx as u64)) & !3;
            s.used.set(epoch);
            d.used.set(epoch);
            if size > 0 && from % 4 == 0 && dx % 4 == 0 {
                let (from, to) = (s.base + from, d.base + dx as u64);
                self.copy_buffer(&sg, from, &dg, to, size);
            }
            return;
        }
        let (Some(s), Some(d)) = (self.objs.tex(src), self.objs.tex(dst)) else {
            return;
        };
        let (smip, slayer) = (src_sub % s.mips, src_sub / s.mips);
        let (dmip, dlayer) = (dst_sub % d.mips, dst_sub / d.mips);
        let (mw, mh, md) = s.mip_size(smip);
        // Levels a GPU texture does without are not copied.
        let (Some(slevel), Some(dlevel)) = (s.level(smip), d.level(dmip)) else {
            return;
        };
        let (x, y, z) = if has_box { (bx[0], bx[1], bx[2]) } else { (0, 0, 0) };
        let (w, h, dep) = if has_box { (bx[3] - bx[0], bx[4] - bx[1], bx[5] - bx[2]) } else { (mw, mh, md) };
        let (sf, df) = (s.format, d.format);
        // Decoded BC data is in texels: block-padded regions are cropped to the mip.
        let (w, h) = if matches!(s.upload, format::Upload::Bc(_)) && !sf.is_compressed() {
            (w.min(mw.saturating_sub(x)), h.min(mh.saturating_sub(y)))
        } else {
            (w, h)
        };
        if s.samples > 1 || d.samples > 1 || w == 0 || h == 0 {
            return;
        }
        // D3D copies between a block-compressed texture and an uncompressed one
        // whose texel is the block size; WebGPU only through a buffer.
        let through_buffer = sf.is_compressed() != df.is_compressed() && format::block(sf).0 == format::block(df).0;
        if !through_buffer && !copy_compatible(sf, df) {
            crate::log_once(&format!("copyfmt{sf:?}{df:?}"), || {
                format!("copy {sf:?} -> {df:?}: incompatible formats, skipped")
            });
            return;
        }
        let (dw, dh, _) = d.mip_size(dmip);
        let (s_tex, d_tex, s3, d3) = (s.tex.clone(), d.tex.clone(), s.is_3d(), d.is_3d());
        let src_origin = wgpu::Origin3d { x, y, z: if s3 { z } else { slayer } };
        let dst_origin = wgpu::Origin3d { x: dx, y: dy, z: if d3 { dz } else { dlayer } };
        s.used.set(epoch);
        d.used.set(epoch);
        if through_buffer {
            let (bytes, _) = format::block(sf);
            let (cw, ch) = if sf.is_compressed() { (w.div_ceil(4), h.div_ceil(4)) } else { (w, h) };
            let bpr = (cw * bytes).next_multiple_of(256);
            let scratch = self.blits.scratch(&self.device, (bpr * ch) as u64);
            let layout = wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(bpr), rows_per_image: Some(ch) };
            let extent = |compressed: bool| wgpu::Extent3d {
                width: if compressed { cw * 4 } else { cw },
                height: if compressed { ch * 4 } else { ch },
                depth_or_array_layers: 1,
            };
            let encoder = self.encoder();
            encoder.copy_texture_to_buffer(
                wgpu::TexelCopyTextureInfo {
                    texture: &s_tex,
                    mip_level: slevel,
                    origin: src_origin,
                    aspect: wgpu::TextureAspect::All,
                },
                wgpu::TexelCopyBufferInfo { buffer: &scratch, layout },
                extent(sf.is_compressed()),
            );
            encoder.copy_buffer_to_texture(
                wgpu::TexelCopyBufferInfo { buffer: &scratch, layout },
                wgpu::TexelCopyTextureInfo {
                    texture: &d_tex,
                    mip_level: dlevel,
                    origin: dst_origin,
                    aspect: wgpu::TextureAspect::All,
                },
                extent(df.is_compressed()),
            );
        } else {
            let (_, bw) = format::block(sf);
            // Compressed copies must cover whole blocks (or reach the mip's edge).
            let w = if bw > 1 { w.next_multiple_of(bw).min(mw.next_multiple_of(bw) - x) } else { w };
            let h = if bw > 1 { h.next_multiple_of(bw).min(mh.next_multiple_of(bw) - y) } else { h };
            if dx + w > dw.next_multiple_of(bw) || dy + h > dh.next_multiple_of(bw) {
                crate::log_once("copybounds", || "texture copy out of bounds, skipped".into());
                return;
            }
            self.encoder().copy_texture_to_texture(
                wgpu::TexelCopyTextureInfo {
                    texture: &s_tex,
                    mip_level: slevel,
                    origin: src_origin,
                    aspect: wgpu::TextureAspect::All,
                },
                wgpu::TexelCopyTextureInfo {
                    texture: &d_tex,
                    mip_level: dlevel,
                    origin: dst_origin,
                    aspect: wgpu::TextureAspect::All,
                },
                wgpu::Extent3d { width: w, height: h, depth_or_array_layers: if s3 { dep } else { 1 } },
            );
        }
        if let Some(d) = self.objs.tex_mut(dst) {
            d.mark_written();
        }
    }

    pub fn copy_resource(&mut self, dst: u32, src: u32) {
        self.flush_clears_of(&[src, dst]);
        let epoch = self.epoch;
        if let (Some(s), Some(d)) = (self.objs.buf(src), self.objs.buf(dst)) {
            if let (Some(sg), Some(dg)) = (s.gpu.clone(), d.gpu.clone()) {
                let (size, from, to) = (s.bytes.min(d.bytes), s.base, d.base);
                s.used.set(epoch);
                d.used.set(epoch);
                self.copy_buffer(&sg, from, &dg, to, size);
            }
            return;
        }
        let (Some(s), Some(d)) = (self.objs.tex(src), self.objs.tex(dst)) else {
            return;
        };
        if s.samples > 1
            || d.samples > 1
            || !copy_compatible(s.format, d.format)
            || (s.width, s.height) != (d.width, d.height)
        {
            crate::log_once(&format!("copyres{:?}{:?}", s.format, d.format), || {
                format!("CopyResource {:?} -> {:?} skipped", s.format, d.format)
            });
            return;
        }
        let mips = s.mips.min(d.mips);
        // (source level, destination level, size) of the mips both GPU textures have.
        let levels: Vec<_> = (0..mips).filter_map(|m| Some((s.level(m)?, d.level(m)?, s.mip_size(m)))).collect();
        let (st, dt, layers, three) = (s.tex.clone(), d.tex.clone(), s.layers.min(d.layers), s.is_3d());
        s.used.set(epoch);
        d.used.set(epoch);
        let encoder = self.encoder();
        for (slevel, dlevel, (w, h, dep)) in levels {
            encoder.copy_texture_to_texture(
                wgpu::TexelCopyTextureInfo {
                    texture: &st,
                    mip_level: slevel,
                    origin: Default::default(),
                    aspect: wgpu::TextureAspect::All,
                },
                wgpu::TexelCopyTextureInfo {
                    texture: &dt,
                    mip_level: dlevel,
                    origin: Default::default(),
                    aspect: wgpu::TextureAspect::All,
                },
                wgpu::Extent3d { width: w, height: h, depth_or_array_layers: if three { dep } else { layers } },
            );
        }
        if let Some(d) = self.objs.tex_mut(dst) {
            d.mark_written();
        }
    }

    pub fn resolve(&mut self, dst: u32, dst_sub: u32, src: u32, src_sub: u32) {
        self.flush_clears_of(&[src, dst]);
        let epoch = self.epoch;
        let (Some(s), Some(d)) = (self.objs.tex(src), self.objs.tex(dst)) else {
            return;
        };
        if s.samples <= 1 || d.samples != 1 || !copy_compatible(s.format, d.format) || format::is_depth(s.format) {
            crate::log_once("resolve", || {
                format!("resolve {:?} x{} -> {:?} x{} skipped", s.format, s.samples, d.format, d.samples)
            });
            return;
        }
        let one = |t: &Tex, sub: u32| {
            t.tex.create_view(&wgpu::TextureViewDescriptor {
                format: Some(s.format),
                dimension: Some(wgpu::TextureViewDimension::D2),
                base_mip_level: sub % t.mips,
                mip_level_count: Some(1),
                base_array_layer: sub / t.mips,
                array_layer_count: Some(1),
                ..Default::default()
            })
        };
        let (sv, dv) = (one(s, src_sub), one(d, dst_sub));
        s.used.set(epoch);
        d.used.set(epoch);
        let ts = self.time_pass("resolve");
        self.end_pass();
        let encoder = self.encoder.get_or_insert_with(|| self.device.create_command_encoder(&Default::default()));
        encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &sv,
                depth_slice: None,
                resolve_target: Some(&dv),
                ops: wgpu::Operations { load: wgpu::LoadOp::Load, store: wgpu::StoreOp::Store },
            })],
            timestamp_writes: ts.and_then(|i| self.profiler.as_ref().map(|p| p.render(i))),
            ..Default::default()
        });
        if let Some(d) = self.objs.tex_mut(dst) {
            d.mark_written();
        }
    }

    pub fn generate_mips(&mut self, srv: u32) {
        let Some(id) = self.objs.srv(srv).map(|v| v.res) else {
            return;
        };
        self.flush_clears_of(&[id]);
        let Some(t) = self.objs.tex(id) else { return };
        if t.mips < 2 || format::is_depth(t.format) || t.format.is_compressed() || t.is_3d() || t.samples > 1 {
            return;
        }
        let Some(pipeline) = self.blits.mip(&self.compiler, t.format) else {
            return;
        };
        let device = self.device.clone();
        let epoch = self.epoch;
        let t = self.objs.tex_mut(id).unwrap();
        if t.mip_chain.is_none() {
            let mut chain = Vec::new();
            for layer in 0..t.layers {
                for mip in 1..t.mips {
                    let one = |m| wgpu::TextureViewDescriptor {
                        dimension: Some(wgpu::TextureViewDimension::D2),
                        base_mip_level: m,
                        mip_level_count: Some(1),
                        base_array_layer: layer,
                        array_layer_count: Some(1),
                        ..Default::default()
                    };
                    let src = t.tex.create_view(&one(mip - 1));
                    let dst = t.tex.create_view(&one(mip));
                    let bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
                        label: None,
                        layout: &pipeline.get_bind_group_layout(0),
                        entries: &[wgpu::BindGroupEntry {
                            binding: 0,
                            resource: wgpu::BindingResource::TextureView(&src),
                        }],
                    });
                    chain.push((dst, bg));
                }
            }
            t.mip_chain = Some(chain);
        }
        t.used.set(epoch);
        t.mark_written();
        let chain: Vec<_> = t.mip_chain.as_ref().unwrap().iter().map(|(v, b)| (v.clone(), b.clone())).collect();
        for (dst, bg) in chain {
            let ts = self.time_pass("mips");
            self.end_pass();
            let encoder = self.encoder.get_or_insert_with(|| self.device.create_command_encoder(&Default::default()));
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                timestamp_writes: ts.and_then(|i| self.profiler.as_ref().map(|p| p.render(i))),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &dst,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                ..Default::default()
            });
            pass.set_pipeline(&pipeline);
            pass.set_bind_group(0, &bg, &[]);
            pass.draw(0..3, 0..1);
        }
    }

    // ---- UAVs ------------------------------------------------------------------------------

    pub fn clear_uav(&mut self, id: u32, values: [u32; 4]) {
        let zero = values == [0; 4];
        let Some(view) = self.objs.uav(id) else {
            return;
        };
        let (res, mip, is_buffer) = (view.res, view.a, view.is_buffer);
        self.flush_clears_of(&[res]);
        let epoch = self.epoch;
        if is_buffer {
            let Some(b) = self.objs.buf(res) else { return };
            let Some(g) = b.gpu.clone() else { return };
            let (base, bytes) = (b.base, b.bytes);
            b.used.set(epoch);
            if zero {
                self.encoder().clear_buffer(&g, base, Some(bytes));
            } else {
                // Written before the commands recorded so far: submit them first.
                self.flush("uav clear");
                let words: Vec<u32> = (0..bytes / 4).map(|i| values[i as usize & 3]).collect();
                self.uploads.buffer(&g, base, bytemuck::cast_slice(&words));
            }
            return;
        }
        let Some(t) = self.objs.tex(res) else { return };
        let Some(bpt) = t.format.block_copy_size(None) else {
            return;
        };
        let (w, h, d) = t.mip_size(mip.min(t.mips - 1));
        let layers = if t.is_3d() { d } else { t.layers };
        let (tex, mip) = (t.tex.clone(), mip.min(t.mips - 1));
        t.used.set(epoch);
        if zero {
            let bpr = (w * bpt).next_multiple_of(256);
            let zeros = self.blits.zeros(&self.device, (bpr * h) as u64);
            let encoder = self.encoder();
            for layer in 0..layers {
                encoder.copy_buffer_to_texture(
                    wgpu::TexelCopyBufferInfo {
                        buffer: &zeros,
                        layout: wgpu::TexelCopyBufferLayout {
                            offset: 0,
                            bytes_per_row: Some(bpr),
                            rows_per_image: Some(h),
                        },
                    },
                    wgpu::TexelCopyTextureInfo {
                        texture: &tex,
                        mip_level: mip,
                        origin: wgpu::Origin3d { x: 0, y: 0, z: layer },
                        aspect: wgpu::TextureAspect::All,
                    },
                    wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
                );
            }
        } else {
            self.flush("uav clear");
            let texel: Vec<u8> = (0..bpt / 4).flat_map(|c| values[c as usize & 3].to_le_bytes()).collect();
            let data: Vec<u8> = texel.iter().copied().cycle().take((w * h * bpt) as usize).collect();
            for layer in 0..layers {
                self.uploads.texture(
                    wgpu::TexelCopyTextureInfo {
                        texture: &tex,
                        mip_level: mip,
                        origin: wgpu::Origin3d { x: 0, y: 0, z: layer },
                        aspect: wgpu::TextureAspect::All,
                    },
                    &data,
                    w * bpt,
                    h,
                    h,
                    wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
                    w * bpt,
                );
            }
        }
        if let Some(t) = self.objs.tex_mut(res) {
            t.mark_written();
        }
    }

    /// The counter of an append/consume buffer (a storage buffer of its own).
    pub fn counter_buffer(&mut self, buf: u32) -> wgpu::Buffer {
        let device = self.device.clone();
        let b = self.objs.buf_mut(buf).unwrap();
        b.counter
            .get_or_insert_with(|| {
                device.create_buffer(&wgpu::BufferDescriptor {
                    label: Some("uav counter"),
                    size: 16,
                    usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC | wgpu::BufferUsages::COPY_DST,
                    mapped_at_creation: false,
                })
            })
            .clone()
    }

    /// A counter reset (SetUnorderedAccessViews' initial count) at its place
    /// in the stream: the value goes into a slot of its own and is copied in.
    pub fn set_uav_counter(&mut self, uav: u32, value: u32) {
        let Some(res) = self.objs.uav(uav).filter(|v| v.is_buffer).map(|v| v.res) else {
            return;
        };
        if self.objs.buf(res).is_none() {
            return;
        }
        const SLOTS: u32 = 1024;
        if self.counter_slot >= SLOTS {
            self.flush("uav counter");
        }
        let src = self
            .counter_src
            .get_or_insert_with(|| {
                self.device.create_buffer(&wgpu::BufferDescriptor {
                    label: Some("counter values"),
                    size: SLOTS as u64 * 4,
                    usage: wgpu::BufferUsages::COPY_SRC | wgpu::BufferUsages::COPY_DST,
                    mapped_at_creation: false,
                })
            })
            .clone();
        let slot = self.counter_slot as u64;
        self.counter_slot += 1;
        self.uploads.buffer(&src, slot * 4, &value.to_le_bytes());
        let counter = self.counter_buffer(res);
        self.objs.buf(res).unwrap().used.set(self.epoch);
        self.encoder().copy_buffer_to_buffer(&src, slot * 4, &counter, 0, 4);
    }

    pub fn copy_counter(&mut self, dst: u32, offset: u32, uav: u32) {
        let Some(res) = self.objs.uav(uav).filter(|v| v.is_buffer).map(|v| v.res) else {
            return;
        };
        let Some((dg, base)) = self.objs.buf(dst).and_then(|b| b.gpu.clone().map(|g| (g, b.base))) else {
            return;
        };
        if self.objs.buf(res).is_none() || !offset.is_multiple_of(4) {
            return;
        }
        let counter = self.counter_buffer(res);
        self.objs.buf(dst).unwrap().used.set(self.epoch);
        self.encoder().copy_buffer_to_buffer(&counter, 0, &dg, base + offset as u64, 4);
    }

    // ---- readbacks ---------------------------------------------------------------------------

    /// GPU data into the CPU storage of a D3D staging resource (in wasm memory).
    pub fn readback(&mut self, c: &Command) {
        let (src, sub, has_box) = (c.w(0), c.w(1), c.w(2) != 0);
        let bx = [c.w(3), c.w(4), c.w(5), c.w(6), c.w(7), c.w(8)];
        let (to, row_pitch, slice_pitch, is_buffer, size, flag) =
            (c.addr(9), c.w(11), c.w(12), c.w(13) != 0, c.w(14), c.addr(15));
        self.flush_clears_of(&[src]);
        self.waited_on = true;
        let epoch = self.epoch;
        let done = |flag: u64| Work::Store { at: flag, value: 0 };
        if is_buffer {
            let Some((g, bytes, base)) = self.objs.buf(src).and_then(|b| b.gpu.clone().map(|g| (g, b.bytes, b.base))) else {
                if flag != 0 {
                    self.works.push(done(flag));
                }
                return;
            };
            let from = if has_box { bx[0] as u64 } else { 0 };
            let len =
                if has_box { (bx[3] - bx[0]) as u64 } else { (size as u64).min(bytes) }.min(bytes.saturating_sub(from));
            // Copies go 4 bytes at a time: from the word the box starts in.
            let skip = from & 3;
            let copied = (skip + len).next_multiple_of(4).max(4);
            let staging = self.staging.get(&self.device, copied);
            self.objs.buf(src).unwrap().used.set(epoch);
            self.encoder().copy_buffer_to_buffer(&g, base + (from - skip), &staging, 0, copied);
            let (skip, len) = (skip as usize, len as usize);
            self.maps.push((staging, Box::new(move |s| Work::Readback { staging: s, copy: Copy::Bytes { to, skip, len }, flag })));
            return;
        }
        let Some(t) = self.objs.tex(src) else {
            if flag != 0 {
                self.works.push(done(flag));
            }
            return;
        };
        // The engine's layout, not the stored one: an A8 texture is kept as
        // RGBA8 (its alpha byte reads back); a BC texture decoded for a GPU
        // without BC cannot be re-encoded.
        let pick = match t.upload {
            format::Upload::Direct => None,
            format::Upload::A8 => Some(3),
            format::Upload::Unorm16 => {
                crate::log_once("rb-unorm16", || "readback of 16-bit normalized texels kept as half floats skipped".into());
                if flag != 0 {
                    self.works.push(done(flag));
                }
                return;
            }
            format::Upload::Bc(_) => {
                crate::log_once("rb-bc", || "readback of a decoded BC texture skipped".into());
                if flag != 0 {
                    self.works.push(done(flag));
                }
                return;
            }
        };
        if t.samples > 1 || format::is_depth(t.format) {
            crate::log_once(&format!("rb{:?}", t.format), || {
                format!("readback of {:?} x{} not supported", t.format, t.samples)
            });
            if flag != 0 {
                self.works.push(done(flag));
            }
            return;
        }
        let (mip, layer) = (sub % t.mips, sub / t.mips);
        let (mw, mh, md) = t.mip_size(mip);
        let (x, y, z) = if has_box { (bx[0], bx[1], bx[2]) } else { (0, 0, 0) };
        let (w, h, d) = if has_box { (bx[3] - bx[0], bx[4] - bx[1], bx[5] - bx[2]) } else { (mw, mh, md) };
        let (bytes, bw) = format::block(t.format);
        let (cols, rows) = (w.div_ceil(bw), h.div_ceil(bw));
        let row_bytes = cols * bytes;
        let bpr = row_bytes.next_multiple_of(256);
        let depth = d.max(1);
        let staging = self.staging.get(&self.device, (bpr * rows * depth) as u64);
        let (tex, three) = (t.tex.clone(), t.is_3d());
        t.used.set(epoch);
        self.encoder().copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo {
                texture: &tex,
                mip_level: mip,
                origin: wgpu::Origin3d { x, y, z: if three { z } else { layer } },
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyBufferInfo {
                buffer: &staging,
                layout: wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(bpr), rows_per_image: Some(rows) },
            },
            wgpu::Extent3d {
                width: cols * bw,
                height: rows * bw,
                depth_or_array_layers: if three { depth } else { 1 },
            },
        );
        let copy =
            Copy::Rows { to, rows, row_bytes, bpr, row_pitch, depth: if three { depth } else { 1 }, slice_pitch, pick };
        // Submitted with the next piece of work (or as soon as the ring is idle).
        self.maps.push((staging, Box::new(move |s| Work::Readback { staging: s, copy, flag })));
    }

    // ---- queries -------------------------------------------------------------------------------

    pub fn begin_query(&mut self, id: u32) {
        let Some(q) = self.objs.query_mut(id) else {
            return;
        };
        if !matches!(q.kind, query::OCCLUSION | query::OCCLUSION_PREDICATE) {
            return;
        }
        q.slots.clear();
        self.end_occlusion_in_pass();
        self.occlusion.active = Some(id);
        self.begin_occlusion_in_pass();
    }

    pub fn end_query(&mut self, id: u32, seq: u32) {
        let Some(q) = self.objs.query(id) else { return };
        let (kind, at) = (q.kind, q.at);
        if matches!(kind, query::OCCLUSION | query::OCCLUSION_PREDICATE) {
            if self.occlusion.active == Some(id) {
                self.end_occlusion_in_pass();
                self.occlusion.active = None;
            }
            let slots = std::mem::take(&mut self.objs.query_mut(id).unwrap().slots);
            self.occlusion.pending.push((at, seq, kind == query::OCCLUSION_PREDICATE, slots));
        } else {
            // Events complete when the GPU has done everything submitted before.
            let result = if kind == query::TIMESTAMP { self.started.elapsed().as_nanos() as u64 } else { 1 };
            self.works.push(Work::Query { at, result, seq });
        }
    }

    pub fn begin_occlusion_in_pass(&mut self) {
        let Some(id) = self.occlusion.active else {
            return;
        };
        let queries = self.pass_info.as_ref().is_some_and(|p| p.queries);
        if self.pass.is_none() || self.occlusion.pass_slot.is_some() || !queries {
            return;
        }
        if self.occlusion.next >= OCCLUSION_SLOTS {
            crate::log_once("qfull", || "occlusion query slots exhausted".into());
            return;
        }
        let slot = self.occlusion.next;
        self.occlusion.next += 1;
        if let Some(q) = self.objs.query_mut(id) {
            q.slots.push(slot);
        }
        self.occlusion.pass_slot = Some(slot);
        self.pass.as_mut().unwrap().begin_occlusion_query(slot);
    }

    pub fn end_occlusion_in_pass(&mut self) {
        if let Some(pass) = self.pass.as_mut()
            && self.occlusion.pass_slot.take().is_some()
        {
            pass.end_occlusion_query();
        }
    }

    /// Resolves the occlusion queries ended since the last submit into the
    /// encoder being submitted.
    pub fn resolve_occlusion(&mut self) {
        if self.occlusion.pending.is_empty() {
            return;
        }
        let pending = std::mem::take(&mut self.occlusion.pending);
        let mut results = Vec::new();
        let mut ranges = Vec::new(); // (first slot, count, byte offset)
        let mut bytes = 0u32;
        for (at, seq, predicate, slots) in pending {
            if slots.is_empty() {
                self.works.push(Work::Query { at, result: 0, seq });
                continue;
            }
            let mut offsets = Vec::new();
            let mut i = 0;
            while i < slots.len() {
                let mut n = 1;
                while i + n < slots.len() && slots[i + n] == slots[i] + n as u32 {
                    n += 1;
                }
                ranges.push((slots[i], n as u32, bytes));
                offsets.extend((0..n as u32).map(|k| bytes + k * 8));
                bytes = (bytes + n as u32 * 8).next_multiple_of(wgpu::QUERY_RESOLVE_BUFFER_ALIGNMENT as u32);
                i += n;
            }
            results.push(OcclusionResult { at, seq, predicate, offsets });
        }
        // Slots are reusable once resolved (resolves are recorded before any later pass).
        if self.occlusion.next > OCCLUSION_SLOTS - 512 && self.occlusion.active.is_none() {
            self.occlusion.next = 0;
        }
        if results.is_empty() {
            return;
        }
        let size = (bytes as u64).max(256).next_power_of_two();
        if self.occlusion.resolve.as_ref().is_none_or(|b| b.size() < size) {
            self.occlusion.resolve = Some(self.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("occlusion resolve"),
                size,
                usage: wgpu::BufferUsages::QUERY_RESOLVE | wgpu::BufferUsages::COPY_SRC,
                mapped_at_creation: false,
            }));
        }
        let resolve = self.occlusion.resolve.clone().unwrap();
        let staging = self.staging.get(&self.device, size);
        let set = self.occlusion.set.clone().unwrap();
        let encoder = self.encoder();
        for (first, n, offset) in ranges {
            encoder.resolve_query_set(&set, first..first + n, &resolve, offset as u64);
        }
        encoder.copy_buffer_to_buffer(&resolve, 0, &staging, 0, bytes as u64);
        self.maps.push((staging, Box::new(move |s| Work::Occlusion { staging: s, queries: results })));
    }
}

impl Program {
    /// A compute program's storage textures whose format bound (`bound`, by
    /// binding) is not the one declared: (binding, declared, bound). The
    /// translation guesses formats from the component count.
    pub fn storage_formats(
        &self,
        bound: impl Fn(u32) -> Option<TextureFormat>,
    ) -> Vec<(u32, TextureFormat, TextureFormat)> {
        let declared = |b: &shader::Binding| match b.kind {
            Kind::StorageTexture { format, .. } => Some(format),
            _ => None,
        };
        self.bindings
            .iter()
            .filter_map(|b| {
                let declared = declared(b)?;
                bound(b.binding).filter(|f| *f != declared).map(|f| (b.binding, declared, f))
            })
            .collect()
    }

    /// The same program compiled from other text (same bindings layout).
    pub fn with_text(&self, device: &wgpu::Device, text: &str, what: &str) -> anyhow::Result<Program> {
        let module = shader::create_module(device, &format!("{} [{what}]", self.label), text)?;
        let mut p = self.copy();
        p.text = shader::Wgsl::new(text);
        p.modules = Arc::new(shader::Modules::with(module));
        p.bindings = shader::stage_bindings(text, self.stage);
        p.layout = None;
        Ok(p)
    }
}
