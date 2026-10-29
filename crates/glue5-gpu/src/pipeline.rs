//! Render pipelines, by content: a [`Recipe`] is everything a pipeline is
//! made of (shaders, input layout, strides, states, target formats). Recipes
//! seen are kept on disk and, with the app's seed, compiled before the game
//! starts on later runs (see `prepare.rs`).

use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Instant;

use rustc_hash::{FxHashMap, FxHashSet};
use serde_json::{Value, json};
use wgpu::{Features, TextureFormat};

use crate::format::{self, Caps};
use crate::shader::{self, Program, Stage};

/// One element of a vertex shader's input signature.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct SigElement {
    pub semantic: u32,
    pub index: u32,
    pub register: u32,
}

/// One element of an input layout: semantic hash, semantic index, DXGI
/// format, slot, offset, class (1 = per instance), step rate.
pub type Element = [u32; 7];

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Recipe {
    pub vs: u64,
    pub ps: Option<u64>,
    /// The vertex shader's input signature (from the engine when it creates
    /// the shader): it places the layout's elements.
    pub sig: Arc<[SigElement]>,
    pub elements: Vec<Element>,
    /// Stride of each slot the layout uses, in [`slots`] order (0: unbound).
    pub strides: Vec<u32>,
    pub topology: u32,
    /// Strip index format: 0 none, 1 uint16, 2 uint32.
    pub strip: u8,
    pub blend: Option<Box<[u32]>>,
    pub depth: Option<Box<[u32]>>,
    /// Without its depth bias (see [`without_depth_bias`]).
    pub raster: Option<Box<[u32]>>,
    pub targets: Vec<Option<TextureFormat>>,
    pub samples: u32,
    pub depth_format: Option<TextureFormat>,
    /// 1: read-only depth, 2: read-only stencil.
    pub read_only: u8,
    /// The programs' variants for what is bound (`Program::for_bindings`):
    /// vertex unfiltered and depth, pixel unfiltered and depth (bits by SRV
    /// slot; unfiltered includes the depth bits). Recorded as on a GPU that
    /// cannot filter 32-bit floats, the same on every device (see
    /// [`Recipe::for_device`]).
    pub variant: [u128; 4],
    /// Uses the HUD coverage program rather than the original depth blit.
    pub hud_mask: bool,
}

/// The slots an input layout uses, each once, in element order, with the
/// class of its first element.
pub fn slots(elements: &[Element]) -> Vec<(u32, u32)> {
    let mut out: Vec<(u32, u32)> = Vec::new();
    for e in elements {
        // (16 vertex buffer slots, as SET_VERTEX_BUFFERS takes.)
        if e[3] < 16 && !out.iter().any(|(s, _)| *s == e[3]) {
            out.push((e[3], e[5]));
        }
    }
    out
}

/// A D3D11 rasterizer state's words without its depth bias (DepthBias,
/// DepthBiasClamp, SlopeScaledDepthBias): draws set it on the render pass
/// (`RenderPass::set_depth_bias`, our wgpu patch), as Vulkan's dynamic
/// state. The engine's shadows use about two hundred biases (by cascade and
/// light): with them in the pipeline, each would be another pipeline.
pub fn without_depth_bias(words: &[u32]) -> Box<[u32]> {
    let mut words: Box<[u32]> = words.into();
    for w in words.iter_mut().skip(3).take(3) {
        *w = 0;
    }
    words
}

/// What a vertex buffer slot must hold for n elements: (n - 1) * stride + last.
#[derive(Clone, Copy, Debug)]
pub struct VertexNeed {
    pub slot: u32,
    pub instance: bool,
    pub stride: u32,
    pub last: u32,
}

pub struct Pipe {
    pub pipeline: wgpu::RenderPipeline,
    pub needs: Vec<VertexNeed>,
}

/// Set once the compile is done; `None`: the pipeline is not valid.
pub type Cell = OnceLock<Option<Pipe>>;

/// What compiling needs, shared with the compile threads.
pub struct Compiler {
    pub device: wgpu::Device,
    pub caps: Caps,
    pub max_inter_stage: u32,
    pub cache: Option<wgpu::PipelineCache>,
    null_ps: OnceLock<Option<wgpu::ShaderModule>>,
    /// See [`Compiler::clipping_ps`].
    clipping_ps: OnceLock<Option<wgpu::ShaderModule>>,
    layouts: Mutex<FxHashMap<(u32, u32), wgpu::PipelineLayout>>,
    pub compiled: AtomicU32,
    pub failed: AtomicU32,
    pub compile_us: AtomicU64,
}

impl Compiler {
    pub fn new(device: wgpu::Device, caps: Caps, cache: Option<wgpu::PipelineCache>) -> Compiler {
        let max_inter_stage = device.limits().max_inter_stage_shader_variables;
        Compiler {
            device,
            caps,
            max_inter_stage,
            cache,
            null_ps: OnceLock::new(),
            clipping_ps: OnceLock::new(),
            layouts: Mutex::default(),
            compiled: AtomicU32::new(0),
            failed: AtomicU32::new(0),
            compile_us: AtomicU64::new(0),
        }
    }

    fn layout(&self, vs: &Program, ps: Option<&Program>) -> wgpu::PipelineLayout {
        let key = (vs.id, ps.map_or(0, |p| p.id));
        self.layouts
            .lock()
            .unwrap()
            .entry(key)
            .or_insert_with(|| {
                self.device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                    label: None,
                    // Groups 0 and 1 the stages', 2 and 3 their constants'.
                    bind_group_layouts: &[
                        vs.layout.as_ref(),
                        ps.and_then(|p| p.layout.as_ref()),
                        vs.constants.as_ref(),
                        ps.and_then(|p| p.constants.as_ref()),
                    ],
                    immediate_size: 0,
                })
            })
            .clone()
    }

    /// A pixel stage that writes nothing: D3D draws without a pixel shader
    /// still have their render targets bound.
    fn null_ps(&self) -> Option<&wgpu::ShaderModule> {
        self.null_ps
            .get_or_init(|| shader::create_module(&self.device, "null ps", "@fragment fn main() {}\n").ok())
            .as_ref()
    }

    /// A pixel stage that only clips, for draws without a pixel shader on
    /// GPUs without clip distances (`at`: where the distances arrive).
    fn clipping_ps(&self, at: u32) -> Option<wgpu::ShaderModule> {
        let make = || shader::create_module(&self.device, "clipping ps", &shader::with_clipping(None, at)?).ok();
        if at == shader::CLIP_LOCATION { self.clipping_ps.get_or_init(make).clone() } else { make() }
    }

    /// Builds the pipeline (any thread). Failures are logged and give `None`.
    pub fn compile(&self, vs: &Program, ps: Option<&Program>, r: &Recipe) -> Option<Pipe> {
        let t0 = Instant::now();
        let label = format!("{} + {}", vs.label, ps.map_or("-", |p| &p.label));
        let result = self.build(vs, ps, r, &label);
        self.count(t0, result.is_ok());
        result
            .map_err(|e| {
                crate::log_once(&format!("pipe{label}{:?}", r.targets), || format!("pipeline {label} failed: {e:#}"))
            })
            .ok()
    }

    /// Counts a pipeline made (`ok`) or not, from `t0`: render, compute or
    /// the renderer's own.
    pub fn count(&self, t0: Instant, ok: bool) {
        self.compile_us.fetch_add(t0.elapsed().as_micros() as u64, Ordering::Relaxed);
        if ok { &self.compiled } else { &self.failed }.fetch_add(1, Ordering::Relaxed);
    }

    fn build(&self, vs: &Program, ps: Option<&Program>, r: &Recipe, label: &str) -> anyhow::Result<Pipe> {
        let features = self.caps.features;
        let remap = shader::remap_locations(vs, ps, self.max_inter_stage);
        let interpolation = ps.map_or(&[][..], |p| &p.interpolation);
        let hud_mask = ps.is_some_and(|p| p.hud_mask);

        // Vertex stage: outputs take the pixel shader's interpolation, and move
        // below the device's inter-stage limit if needed.
        let vs_module = if !hud_mask && interpolation.is_empty() && remap.is_none() {
            vs.module(&self.device).ok_or_else(|| anyhow::anyhow!("vertex module failed"))?
        } else {
            let key = format!("{interpolation:?}|{remap:?}|{hud_mask}");
            let hud_text = if hud_mask {
                Some(
                    shader::with_hud_coordinates(&vs.text.get())
                        .ok_or_else(|| anyhow::anyhow!("unexpected HUD mask vertices"))?,
                )
            } else {
                None
            };
            vs.variant(&self.device, &key, |text| {
                let text = shader::with_interpolation(hud_text.as_deref().unwrap_or(text), interpolation);
                match &remap {
                    Some(map) => shader::with_locations(&text, Stage::Vertex, map),
                    None => text,
                }
            })
            .ok_or_else(|| anyhow::anyhow!("vertex variant failed"))?
        };

        // Vertex buffers: layout elements matched to the shader's inputs by semantic.
        let slot_list = slots(&r.elements);
        let mut attributes: Vec<(u32, wgpu::VertexAttribute)> = Vec::new();
        for e in &r.elements {
            let Some(s) = r.sig.iter().find(|s| s.semantic == e[0] && s.index == e[1]) else {
                continue;
            };
            let Some(format) = format::vertex(e[2]) else {
                crate::log_once(&format!("vf{}", e[2]), || format!("unsupported vertex format DXGI {}", e[2]));
                continue;
            };
            attributes.push((e[3], wgpu::VertexAttribute { format, offset: e[4] as u64, shader_location: s.register }));
        }
        let mut buffers: Vec<Option<wgpu::VertexBufferLayout>> = Vec::new();
        let mut needs = Vec::new();
        let mut attrs_by_slot: Vec<Vec<wgpu::VertexAttribute>> = Vec::new();
        let mut used_slots = Vec::new();
        for (i, &(slot, class)) in slot_list.iter().enumerate() {
            let attrs: Vec<_> = attributes.iter().filter(|(s, _)| *s == slot).map(|(_, a)| *a).collect();
            if attrs.is_empty() {
                continue;
            }
            let stride = r.strides.get(i).copied().unwrap_or(0);
            let last = attrs.iter().map(|a| a.offset as u32 + a.format.size() as u32).max().unwrap_or(0);
            needs.push(VertexNeed { slot, instance: class == 1, stride, last });
            used_slots.push((slot, class, stride));
            attrs_by_slot.push(attrs);
        }
        for ((slot, class, stride), attrs) in used_slots.iter().zip(&attrs_by_slot) {
            let slot = *slot as usize;
            if buffers.len() <= slot {
                buffers.resize(slot + 1, None);
            }
            buffers[slot] = Some(wgpu::VertexBufferLayout {
                array_stride: *stride as u64,
                step_mode: if *class == 1 { wgpu::VertexStepMode::Instance } else { wgpu::VertexStepMode::Vertex },
                attributes: attrs,
            });
        }

        let States { primitive, depth_stencil, targets, alpha_to_coverage } =
            states(r, ps.map_or(0, |p| p.outputs), features);

        // Without the GPU's clip distances the pixel stage clips, from the
        // copy the vertex stage writes (see shader::without_clip_distances).
        let clip = vs.clip_distances && !features.contains(wgpu::Features::CLIP_DISTANCES);
        let ps_module = match ps {
            Some(p) => {
                let alpha_test = alpha_to_coverage && r.samples <= 1 && p.outputs & 1 != 0;
                let reads = |l: &u32| p.locations.contains(l) || (clip && *l == shader::CLIP_LOCATION);
                let ps_remap = remap.as_ref().filter(|m| m.iter().any(|(from, _)| reads(from)));
                let no_depth = p.frag_depth && r.depth_format.is_none();
                if !alpha_test && !clip && ps_remap.is_none() && !no_depth {
                    Some(p.module(&self.device).ok_or_else(|| anyhow::anyhow!("pixel module failed"))?)
                } else {
                    let key = format!("{alpha_test}|{clip}|{ps_remap:?}|{no_depth}");
                    let module = p.variant(&self.device, &key, |text| {
                        let mut text = text.to_owned();
                        if alpha_test && let Some(t) = shader::with_alpha_test(&text) {
                            text = t;
                        }
                        if no_depth && let Some(t) = shader::without_frag_depth(&text) {
                            text = t;
                        }
                        if clip && let Some(t) = shader::with_clipping(Some(&text), shader::CLIP_LOCATION) {
                            text = t;
                        }
                        if let Some(map) = ps_remap {
                            text = shader::with_locations(&text, Stage::Pixel, map);
                        }
                        text
                    });
                    Some(module.ok_or_else(|| anyhow::anyhow!("pixel variant failed"))?)
                }
            }
            None if clip => {
                let moved = remap.iter().flatten().find(|(from, _)| *from == shader::CLIP_LOCATION);
                let at = moved.map_or(shader::CLIP_LOCATION, |m| m.1);
                Some(self.clipping_ps(at).ok_or_else(|| anyhow::anyhow!("clipping module failed"))?)
            }
            None if targets.iter().any(Option::is_some) => self.null_ps().cloned(),
            None => None,
        };

        let layout = self.layout(vs, ps);
        let options = wgpu::PipelineCompilationOptions { constants: &[], zero_initialize_workgroup_memory: false };
        let desc = wgpu::RenderPipelineDescriptor {
            label: Some(label),
            layout: Some(&layout),
            vertex: wgpu::VertexState {
                module: &vs_module,
                entry_point: Some("main"),
                compilation_options: options.clone(),
                buffers: &buffers,
            },
            primitive,
            depth_stencil,
            multisample: wgpu::MultisampleState {
                count: r.samples.max(1),
                mask: !0,
                alpha_to_coverage_enabled: alpha_to_coverage && r.samples > 1,
            },
            fragment: ps_module.as_ref().map(|module| wgpu::FragmentState {
                module,
                entry_point: Some("main"),
                compilation_options: options.clone(),
                targets: &targets,
            }),
            multiview_mask: None,
            cache: self.cache.as_ref(),
        };
        let scope = self.device.push_error_scope(wgpu::ErrorFilter::Validation);
        let pipeline = self.device.create_render_pipeline(&desc);
        if let Some(e) = pollster::block_on(scope.pop()) {
            anyhow::bail!("{e}");
        }
        Ok(Pipe { pipeline, needs })
    }
}

/// The fixed-function states a recipe's pipeline is made with: what its
/// state words become (`outputs`: the colour locations the pixel shader
/// writes, none without one). Alpha-to-coverage as asked for: an alpha test
/// on a single-sample target (see [`shader::with_alpha_test`]).
#[derive(Debug, PartialEq)]
struct States {
    primitive: wgpu::PrimitiveState,
    depth_stencil: Option<wgpu::DepthStencilState>,
    targets: Vec<Option<wgpu::ColorTargetState>>,
    alpha_to_coverage: bool,
}

fn states(r: &Recipe, outputs: u8, features: Features) -> States {
    let (topology, _) = format::topology(r.topology);
    let mut primitive = wgpu::PrimitiveState {
        topology,
        strip_index_format: match r.strip {
            1 => Some(wgpu::IndexFormat::Uint16),
            2 => Some(wgpu::IndexFormat::Uint32),
            _ => None,
        },
        front_face: wgpu::FrontFace::Cw,
        cull_mode: Some(wgpu::Face::Back),
        ..Default::default()
    };
    // D3D11_RASTERIZER_DESC: FillMode, CullMode, FrontCCW, DepthBias,
    // DepthBiasClamp, SlopeScaledDepthBias, DepthClipEnable, Scissor, MS, AA lines
    if let Some(w) = &r.raster {
        primitive.cull_mode = match w[1] {
            2 => Some(wgpu::Face::Front),
            3 => Some(wgpu::Face::Back),
            _ => None,
        };
        primitive.front_face = if w[2] != 0 { wgpu::FrontFace::Ccw } else { wgpu::FrontFace::Cw };
        primitive.unclipped_depth = w[6] == 0 && features.contains(Features::DEPTH_CLIP_CONTROL);
    }

    let depth_stencil = r.depth_format.map(|format| {
        let mut d = wgpu::DepthStencilState {
            format,
            depth_write_enabled: Some(false),
            depth_compare: Some(wgpu::CompareFunction::Always),
            stencil: Default::default(),
            bias: Default::default(),
        };
        // D3D11_DEPTH_STENCIL_DESC: DepthEnable, DepthWriteMask, DepthFunc,
        // StencilEnable, masks (read | write << 8), front fail/depth fail/pass/func, back ...
        if let Some(w) = &r.depth {
            if w[0] != 0 {
                d.depth_write_enabled = Some(w[1] == 1);
                d.depth_compare = Some(format::compare(w[2]).unwrap_or(wgpu::CompareFunction::Less));
            }
            if format.has_stencil_aspect() && w[3] != 0 {
                let face = |i: usize| wgpu::StencilFaceState {
                    fail_op: format::stencil_op(w[i]),
                    depth_fail_op: format::stencil_op(w[i + 1]),
                    pass_op: format::stencil_op(w[i + 2]),
                    compare: format::compare(w[i + 3]).unwrap_or(wgpu::CompareFunction::Always),
                };
                d.stencil = wgpu::StencilState {
                    front: face(5),
                    back: face(9),
                    read_mask: w[4] & 0xff,
                    write_mask: (w[4] >> 8) & 0xff,
                };
            }
        }
        if r.read_only & 1 != 0 {
            d.depth_write_enabled = Some(false);
        }
        if r.read_only & 2 != 0 {
            d.stencil.write_mask = 0;
        }
        d
    });

    // D3D11_BLEND_DESC: AlphaToCoverage, IndependentBlend, 8 x {enable,
    // src, dst, op, srcA, dstA, opA, mask}
    let blend_words = r.blend.as_deref();
    let alpha_to_coverage = blend_words.is_some_and(|w| w[0] != 0);
    let mut targets: Vec<Option<wgpu::ColorTargetState>> = Vec::new();
    for (i, t) in r.targets.iter().enumerate() {
        let Some(format) = *t else {
            targets.push(None);
            continue;
        };
        let writes = outputs & (1 << i) != 0;
        let mut target = wgpu::ColorTargetState { format, blend: None, write_mask: wgpu::ColorWrites::empty() };
        if writes {
            target.write_mask = wgpu::ColorWrites::ALL;
            if let Some(w) = blend_words {
                let rt = if w[1] != 0 { i } else { 0 };
                let q = 2 + rt * 8;
                target.write_mask = wgpu::ColorWrites::from_bits_truncate(w[q + 7] & 0xf);
                if w[q] != 0 && format_blendable(format) {
                    let factor =
                        |v: u32, default| dual_source_fallback(format::blend_factor(v).unwrap_or(default), features);
                    target.blend = Some(wgpu::BlendState {
                        color: wgpu::BlendComponent {
                            src_factor: factor(w[q + 1], wgpu::BlendFactor::One),
                            dst_factor: factor(w[q + 2], wgpu::BlendFactor::Zero),
                            operation: format::blend_op(w[q + 3]),
                        },
                        alpha: wgpu::BlendComponent {
                            src_factor: factor(w[q + 4], wgpu::BlendFactor::One),
                            dst_factor: factor(w[q + 5], wgpu::BlendFactor::Zero),
                            operation: format::blend_op(w[q + 6]),
                        },
                    });
                }
            }
        }
        targets.push(Some(target));
    }
    States { primitive, depth_stencil, targets, alpha_to_coverage }
}

/// [`Recipe::canonical`]'s blend words (AlphaToCoverage, IndependentBlend,
/// 8 x {enable, src, dst, op, srcA, dstA, opA, mask}).
fn canonical_blend(w: &mut [u32], targets: &[Option<TextureFormat>], ps: bool, samples: u32) {
    // Alpha-to-coverage: on multisampled targets, else an alpha test of
    // the pixel shader's first output.
    w[0] = (w[0] != 0 && (ps || samples > 1)) as u32;
    // The block of each target bound; without a pixel shader nothing is written.
    let block = |i: usize| if w[1] != 0 { i } else { 0 };
    let users: Vec<(usize, TextureFormat)> =
        targets.iter().enumerate().filter_map(|(i, t)| Some((block(i), (*t)?))).filter(|_| ps).collect();
    for b in 0..8 {
        let q = 2 + b * 8;
        let mine = || users.iter().filter(|(u, _)| *u == b);
        if mine().next().is_none() {
            w[q..q + 8].fill(0);
            continue;
        }
        w[q + 7] &= 0xf;
        if w[q] != 0 && mine().any(|(_, f)| format_blendable(*f)) {
            w[q] = 1;
        } else {
            w[q..q + 7].fill(0);
        }
    }
    // Independent blocks all alike: one for all.
    if w[1] != 0 {
        let words = |b: usize| w[2 + b * 8..2 + b * 8 + 8].to_vec();
        let first = users.first().map_or(vec![0; 8], |&(b, _)| words(b));
        if users.iter().all(|&(b, _)| words(b) == first) {
            w[1] = 0;
            w[2..].fill(0);
            w[2..10].copy_from_slice(&first);
        }
    }
}

/// Blending on formats that cannot blend (32-bit float without the feature,
/// integer formats) is dropped rather than failing the pipeline.
fn format_blendable(f: TextureFormat) -> bool {
    !matches!(
        f,
        TextureFormat::R32Float
            | TextureFormat::Rg32Float
            | TextureFormat::Rgba32Float
            | TextureFormat::R32Uint
            | TextureFormat::R32Sint
    ) && f.sample_type(None, None).is_some_and(|t| matches!(t, wgpu::TextureSampleType::Float { .. }))
}

/// The translated shaders have no second blend source: dual-source factors
/// read the first one.
fn dual_source_fallback(f: wgpu::BlendFactor, _features: Features) -> wgpu::BlendFactor {
    use wgpu::BlendFactor as B;
    match f {
        B::Src1 => B::Src,
        B::OneMinusSrc1 => B::OneMinusSrc,
        B::Src1Alpha => B::SrcAlpha,
        B::OneMinusSrc1Alpha => B::OneMinusSrcAlpha,
        other => other,
    }
}

// ---- recipes on disk (the browser build's JSON format, with the signature) ----

impl Recipe {
    /// The recipe with the state words its pipeline does not depend on
    /// zeroed (see [`states`]), and those it reads only as flags as 0 or 1:
    /// recipes the engine's states make alike share one pipeline. Blend
    /// words of targets not bound or not blended, depth words of a disabled
    /// test, stencil words without stencil, the rasterizer's fill mode,
    /// scissor and multisampling flags; the read-only flags go into the
    /// depth words; states that make the defaults are none. A pure function
    /// of the recipe: the same on every device.
    pub fn canonical(mut self) -> Recipe {
        if let Some(w) = self.blend.as_deref_mut().filter(|w| w.len() == 2 + 8 * 8) {
            canonical_blend(w, &self.targets, self.ps.is_some(), self.samples);
        }
        if self.blend.as_deref().is_some_and(|w| w.iter().all(|&x| x == 0))
            && (self.ps.is_none() || self.targets.iter().all(Option::is_none))
        {
            self.blend = None;
        }
        let ro = self.read_only;
        match self.depth_format {
            None => (self.depth, self.read_only) = (None, 0),
            Some(format) => {
                if self.depth.is_none() {
                    self.read_only = 0;
                }
                if let Some(w) = self.depth.as_deref_mut().filter(|w| w.len() == 13) {
                    self.read_only = 0;
                    w[0] = (w[0] != 0) as u32;
                    w[1] = (w[0] != 0 && w[1] == 1 && ro & 1 == 0) as u32;
                    if w[0] == 0 {
                        w[2] = 0;
                    }
                    if format.has_stencil_aspect() && w[3] != 0 {
                        w[3] = 1;
                        w[4] &= if ro & 2 != 0 { 0xff } else { 0xffff };
                    } else {
                        w[3..].fill(0);
                    }
                }
                if self.depth.as_deref().is_some_and(|w| w.iter().all(|&x| x == 0)) {
                    self.depth = None;
                }
            }
        }
        if let Some(w) = self.raster.as_deref_mut().filter(|w| w.len() == 10) {
            w[0] = 0;
            if !matches!(w[1], 2 | 3) {
                w[1] = 1;
            }
            w[2] = (w[2] != 0) as u32;
            w[6] = (w[6] != 0) as u32;
            w[7..].fill(0);
        }
        // D3D's default rasterizer state: back faces culled, clockwise
        // front, depth clipped (and no bias here).
        if self.raster.as_deref() == Some(&[0, 3, 0, 0, 0, 0, 1, 0, 0, 0][..]) {
            self.raster = None;
        }
        self
    }

    /// The pipeline this device makes of a recipe recorded on any device:
    /// recipes say what the game drew, as on a GPU with every format and
    /// unable to filter 32-bit floats. A GPU that filters them needs no
    /// unfilterable bindings; one without 32-bit float depth with stencil,
    /// or without 16-bit normalized formats, has their stand-ins (see
    /// `format::texture`). Recipes recorded here already have them.
    pub fn for_device(&self, caps: &Caps) -> Recipe {
        let mut r = self.clone();
        if caps.float32_filter {
            r.variant[0] = r.variant[1];
            r.variant[2] = r.variant[3];
        }
        r.targets.iter_mut().flatten().for_each(|f| *f = device_format(*f, caps));
        r.depth_format = r.depth_format.map(|f| device_format(f, caps));
        r
    }

    pub fn to_json(&self) -> Value {
        let words = |w: &Option<Box<[u32]>>| w.as_ref().map(|w| json!(w.to_vec()));
        let strip = ["", "uint16", "uint32"][self.strip as usize];
        let mut v = json!({
            "vs": format!("{:016x}", self.vs),
            "ps": self.ps.map(|h| format!("{h:016x}")).unwrap_or_default(),
            "sig": self.sig.iter().map(|s| [s.semantic, s.index, s.register]).collect::<Vec<_>>(),
            "el": self.elements,
            "st": self.strides,
            "tp": self.topology,
            "sf": strip,
            "b": words(&self.blend),
            "d": words(&self.depth),
            "r": words(&self.raster),
            "rt": self.targets.iter().map(|t| t.map(format::name)).collect::<Vec<_>>(),
            "n": self.samples,
            "ds": self.depth_format.map(format::name).unwrap_or(""),
            "ro": self.read_only,
            "hud_mask": self.hud_mask,
        });
        if self.variant != [0; 4] {
            v["variant"] = json!(self.variant.map(|m| format!("{m:x}")));
        }
        v
    }

    pub fn from_json(v: &Value) -> Option<Recipe> {
        let hex = |k: &str| u64::from_str_radix(v.get(k)?.as_str()?, 16).ok();
        let words = |k: &str| -> Option<Option<Box<[u32]>>> {
            match v.get(k)? {
                Value::Null => Some(None),
                Value::Array(a) => {
                    Some(Some(a.iter().map(|x| x.as_u64().map(|x| x as u32)).collect::<Option<Vec<_>>>()?.into()))
                }
                _ => None,
            }
        };
        let u = |k: &str| v.get(k).and_then(Value::as_u64).map(|x| x as u32);
        let elements = v
            .get("el")?
            .as_array()?
            .iter()
            .map(|e| {
                let a = e.as_array()?;
                let mut out = [0u32; 7];
                for (i, x) in a.iter().take(7).enumerate() {
                    out[i] = x.as_u64()? as u32;
                }
                Some(out)
            })
            .collect::<Option<Vec<_>>>()?;
        let sig = v
            .get("sig")?
            .as_array()?
            .iter()
            .map(|s| {
                let w = |i: usize| s.get(i).and_then(Value::as_u64).map(|x| x as u32);
                Some(SigElement { semantic: w(0)?, index: w(1)?, register: w(2)? })
            })
            .collect::<Option<_>>()?;
        Some(Recipe {
            vs: hex("vs")?,
            ps: hex("ps"),
            sig,
            elements,
            strides: v.get("st")?.as_array()?.iter().map(|x| x.as_u64().map(|x| x as u32)).collect::<Option<_>>()?,
            topology: u("tp")?,
            strip: match v.get("sf").and_then(Value::as_str) {
                Some("uint16") => 1,
                Some("uint32") => 2,
                _ => 0,
            },
            blend: words("b")?,
            depth: words("d")?,
            raster: words("r")?.map(|w| without_depth_bias(&w)),
            targets: v.get("rt")?.as_array()?.iter().map(|t| t.as_str().and_then(format::by_name)).collect(),
            samples: u("n").unwrap_or(1),
            depth_format: v.get("ds").and_then(Value::as_str).and_then(format::by_name),
            read_only: u("ro").unwrap_or(0) as u8,
            hud_mask: v.get("hud_mask").and_then(Value::as_bool).unwrap_or(false),
            variant: match v.get("variant") {
                None => [0; 4],
                Some(a) => {
                    let mask = |i: usize| u128::from_str_radix(a.get(i)?.as_str()?, 16).ok();
                    [mask(0)?, mask(1)?, mask(2)?, mask(3)?]
                }
            },
        })
    }
}

/// A format recorded on any device as made on this one: the stand-ins of
/// formats it lacks (see `format::texture`).
fn device_format(f: TextureFormat, caps: &Caps) -> TextureFormat {
    match f {
        TextureFormat::Depth32FloatStencil8 if !caps.features.contains(Features::DEPTH32FLOAT_STENCIL8) => {
            TextureFormat::Depth24PlusStencil8
        }
        TextureFormat::Rgba16Unorm if !caps.norm16 => TextureFormat::Rgba16Float,
        TextureFormat::Rg16Unorm if !caps.norm16 => TextureFormat::Rg16Float,
        f => f,
    }
}

/// A compute pipeline's recipe: the shader, its program variant
/// (unfiltered and depth textures, as a [`Recipe`]'s pixel pair) and the
/// formats of the storage textures bound where the shader declares others
/// (binding, format).
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct ComputeRecipe {
    pub cs: u64,
    pub variant: [u128; 2],
    pub storage: Vec<(u32, TextureFormat)>,
}

impl ComputeRecipe {
    /// As [`Recipe::for_device`].
    pub fn for_device(&self, caps: &Caps) -> ComputeRecipe {
        let mut r = self.clone();
        if caps.float32_filter {
            r.variant[0] = r.variant[1];
        }
        r.storage.iter_mut().for_each(|(_, f)| *f = device_format(*f, caps));
        r
    }

    pub fn to_json(&self) -> Value {
        let mut v = json!({
            "cs": format!("{:016x}", self.cs),
            "uav": self.storage.iter().map(|&(b, f)| json!([b, format::name(f)])).collect::<Vec<_>>(),
        });
        if self.variant != [0; 2] {
            v["variant"] = json!(self.variant.map(|m| format!("{m:x}")));
        }
        v
    }

    pub fn from_json(v: &Value) -> Option<ComputeRecipe> {
        let storage = v
            .get("uav")?
            .as_array()?
            .iter()
            .map(|s| Some((s.get(0)?.as_u64()? as u32, format::by_name(s.get(1)?.as_str()?)?)))
            .collect::<Option<_>>()?;
        Some(ComputeRecipe {
            cs: u64::from_str_radix(v.get("cs")?.as_str()?, 16).ok()?,
            variant: match v.get("variant") {
                None => [0; 2],
                Some(a) => {
                    let mask = |i: usize| u128::from_str_radix(a.get(i)?.as_str()?, 16).ok();
                    [mask(0)?, mask(1)?]
                }
            },
            storage,
        })
    }
}

/// The recipes seen while playing this game build, gathered from several
/// devices and automated runs (`mise run shaders:seed`, `shaders:collect`).
const SEED: &str = include_str!("../seed/pipelines.jsonl");

/// The recipes known before the game starts (as recorded).
#[derive(Default)]
pub struct Known {
    pub render: Vec<Arc<Recipe>>,
    pub compute: Vec<ComputeRecipe>,
}

/// The recipes of the seed and of this device (`<cache>/pipelines.jsonl`),
/// each once (render recipes in their canonical form, as the renderer makes
/// them). Lines in another format (render recipes without a signature) are
/// skipped.
pub fn known_recipes(cache_dir: Option<&std::path::Path>) -> Known {
    let recorded = cache_dir.and_then(|d| std::fs::read_to_string(d.join("pipelines.jsonl")).ok()).unwrap_or_default();
    let (mut known, mut seen, mut seen_compute) = (Known::default(), FxHashSet::default(), FxHashSet::default());
    for line in SEED.lines().chain(recorded.lines()) {
        let Ok(v) = serde_json::from_str::<Value>(line) else { continue };
        if v.get("cs").is_some() {
            if let Some(r) = ComputeRecipe::from_json(&v).filter(|r| seen_compute.insert(r.clone())) {
                known.compute.push(r);
            }
        } else if let Some(r) = Recipe::from_json(&v).map(Recipe::canonical).filter(|r| seen.insert(r.clone())) {
            known.render.push(Arc::new(r));
        }
    }
    known
}

#[cfg(test)]
mod tests {
    use super::*;

    fn caps(features: Features, norm16: bool, float32_filter: bool) -> Caps {
        Caps { features, norm16, indirect: true, float32_filter, depth_bias_clamp: true }
    }

    fn recipe() -> Recipe {
        let line = SEED.lines().next().unwrap();
        let mut r = Recipe::from_json(&serde_json::from_str(line).unwrap()).unwrap();
        r.targets = vec![Some(TextureFormat::Rgba16Unorm), None, Some(TextureFormat::Rg16Unorm)];
        r.depth_format = Some(TextureFormat::Depth32FloatStencil8);
        // A 32-bit float texture in slot 0, a depth read in slot 1.
        r.variant = [0b11, 0b10, 0b01, 0];
        r
    }

    #[test]
    fn seed_parses() {
        let known = known_recipes(None);
        let render: Vec<_> = SEED.lines().filter(|l| l.contains("\"sig\":")).collect();
        assert!(render.iter().all(|l| Recipe::from_json(&serde_json::from_str(l).unwrap()).is_some()));
        // Each once, as canonical recipes (some lines make the same).
        let canonical: FxHashSet<Recipe> =
            render.iter().map(|l| Recipe::from_json(&serde_json::from_str(l).unwrap()).unwrap().canonical()).collect();
        assert_eq!(known.render.len(), canonical.len());
        assert_eq!(known.compute.len(), SEED.lines().filter(|l| l.contains("\"cs\":")).count());
    }

    /// What a pipeline is made with, from a recipe or its canonical form,
    /// for any pixel shader and device.
    fn same_states(r: &Recipe) {
        let c = r.clone().canonical();
        assert_eq!(c.clone().canonical(), c, "not idempotent");
        let outputs: &[u8] = if r.ps.is_some() { &[0, 1, 2, 3, 0xff] } else { &[0] };
        for &outputs in outputs {
            for features in [Features::empty(), Features::DEPTH_CLIP_CONTROL] {
                let (a, b) = (states(r, outputs, features), states(&c, outputs, features));
                // Alpha-to-coverage as `build` uses it.
                let used = |s: &States| {
                    (s.alpha_to_coverage && r.samples > 1, s.alpha_to_coverage && r.samples <= 1 && r.ps.is_some())
                };
                assert_eq!((&a.primitive, &a.depth_stencil, &a.targets), (&b.primitive, &b.depth_stencil, &b.targets));
                assert_eq!(used(&a), used(&b), "{r:?}");
            }
        }
    }

    #[test]
    fn canonical_recipes_make_the_same_pipelines() {
        for line in SEED.lines().filter(|l| l.contains("\"sig\":")) {
            same_states(&Recipe::from_json(&serde_json::from_str(line).unwrap()).unwrap());
        }
        // Random states, targets and formats (words in and around their ranges).
        let mut seed = 0x9e3779b97f4a7c15u64;
        let mut random = move |n: u64| {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            (seed % n) as u32
        };
        let formats = [
            None,
            Some(TextureFormat::Rgba8Unorm),
            Some(TextureFormat::R32Float),
            Some(TextureFormat::Rgba16Float),
            Some(TextureFormat::R32Uint),
        ];
        let depths = [
            None,
            Some(TextureFormat::Depth32Float),
            Some(TextureFormat::Depth24PlusStencil8),
            Some(TextureFormat::Depth32FloatStencil8),
        ];
        for _ in 0..20000 {
            let mut r = recipe();
            r.ps = (random(4) != 0).then_some(1);
            r.samples = [1, 4][random(2) as usize];
            r.targets = (0..random(5)).map(|_| formats[random(5) as usize]).collect();
            r.depth_format = depths[random(4) as usize];
            r.read_only = random(4) as u8;
            r.blend = (random(5) != 0).then(|| {
                let mut w: Vec<u32> = (0..66).map(|_| random(20)).collect();
                w[0] = random(3);
                w[1] = random(2);
                (0..8).for_each(|b| w[2 + b * 8] = random(2));
                w.into()
            });
            r.depth = (random(5) != 0).then(|| (0..13).map(|i| random(if i == 4 { 0x10000 } else { 10 })).collect());
            r.raster = (random(5) != 0)
                .then(|| without_depth_bias(&(0..10).map(|i| random(if i == 1 { 5 } else { 3 })).collect::<Vec<_>>()));
            same_states(&r);
        }
    }

    #[test]
    fn alike_states_make_one_recipe() {
        let mut a = recipe();
        a.targets = vec![Some(TextureFormat::Rgba8Unorm), Some(TextureFormat::R32Float)];
        let mut blend = vec![0u32; 66];
        // Independent, target 0 blended, target 1 (unblendable) with the
        // same words, the other blocks anything.
        blend[1] = 1;
        for b in 0..8 {
            blend[2 + b * 8..2 + b * 8 + 8].copy_from_slice(&[1, 5, 6, 1, 5, 6, 1, 15 + b as u32 * 16]);
        }
        a.blend = Some(blend.into());
        let mut b = a.clone();
        b.blend.as_mut().unwrap()[2 + 5 * 8 + 1] = 2;
        b.raster = Some([1, 3, 0, 0, 0, 0, 1, 1, 0, 0].into());
        a.raster = None;
        assert_eq!(a.clone().canonical(), b.clone().canonical());
        // Not alike: another mask for target 1.
        b.blend.as_mut().unwrap()[2 + 8 + 7] = 7;
        assert_ne!(a.canonical(), b.canonical());
    }

    #[test]
    fn compute_recipes_round_trip() {
        let r = ComputeRecipe {
            cs: 0x23a7a3c1c0d38f46,
            variant: [0b110, 0b100],
            storage: vec![(161, TextureFormat::Rgba16Unorm)],
        };
        let line = r.to_json().to_string();
        assert_eq!(ComputeRecipe::from_json(&serde_json::from_str(&line).unwrap()), Some(r.clone()));
        // Not a render recipe (which older builds read the lines as).
        assert!(Recipe::from_json(&serde_json::from_str(&line).unwrap()).is_none());
        let here = r.for_device(&caps(Features::empty(), false, true));
        assert_eq!((here.variant, here.storage), ([0b100, 0b100], vec![(161, TextureFormat::Rgba16Float)]));
    }

    #[test]
    fn a_gpu_with_everything_filters_float32() {
        let r = recipe().for_device(&caps(Features::DEPTH32FLOAT_STENCIL8, true, true));
        assert_eq!(r.variant, [0b10, 0b10, 0, 0]);
        assert_eq!(r.targets, recipe().targets);
        assert_eq!(r.depth_format, recipe().depth_format);
    }

    #[test]
    fn a_gpu_without_formats_has_their_stand_ins() {
        let r = recipe().for_device(&caps(Features::empty(), false, false));
        assert_eq!(r.variant, recipe().variant);
        assert_eq!(r.targets, [Some(TextureFormat::Rgba16Float), None, Some(TextureFormat::Rg16Float)]);
        assert_eq!(r.depth_format, Some(TextureFormat::Depth24PlusStencil8));
        assert_eq!(r.for_device(&caps(Features::empty(), false, false)), r);
    }
}
