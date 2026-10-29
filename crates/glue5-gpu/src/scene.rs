//! What the temporal upscalers need of a frame (MetalFX's temporal scaler
//! and frame interpolator, FSR): the scene without the HUD, its depth, how
//! it moved since the frame before, and the sub-pixel offset it was drawn at.
//!
//! The engine has none of it: its DirectX 10 path has no temporal
//! antialiasing, so no motion vectors and no jitter. Both come from its
//! camera, read from the constants its draws use. Every vertex program reads
//! RAGE's `rage_matrices` in slot 1 (gWorld, gWorldView, gWorldViewProj,
//! gViewInverse; row vectors): the first perspective draw of a frame into a
//! scene-sized target gives the view-projection, gWorld⁻¹ · gWorldViewProj.
//! Motion is then the camera's, reprojected from the depth: right for all
//! that does not move by itself; vehicles and people moving on their own get
//! the camera's motion, and the upscaler's own checks catch what that gets
//! wrong.
//!
//! The jitter moves every perspective draw into a scene-sized target by a
//! fraction of a pixel (a Halton sequence), added to its gWorldViewProj as
//! the draw's constants are copied (`jitter_constants`). Full-screen passes
//! are orthographic and stay put, so lighting and post-processing, which
//! read the scene pixel for pixel, keep its offset instead of undoing it.
//!
//! The host's hook on `PostFX::ProcessNonDepthFX` marks where
//! post-processing has finished the scene in the back buffer
//! (`op::SCENE_DONE`); everything drawn after it is HUD. There the scene is
//! copied, its depth and motion drawn, and the next present takes them to
//! the submit thread ([`SceneInputs`]).

use crate::exec::Gpu;

/// The frame's scene, for the upscalers on the submit thread.
#[derive(Clone)]
pub struct SceneInputs {
    /// The back buffer as post-processing left it (before the HUD).
    pub color: wgpu::Texture,
    /// Depth (`Depth32Float`, as the engine's: 1 near, 0 far).
    pub depth: wgpu::Texture,
    /// In pixels, where each pixel's surface was in the frame before
    /// (`Rg16Float`, previous minus current; the jitter left out).
    pub motion: wgpu::Texture,
    /// The pixels the scene was moved by (right, down).
    pub jitter: [f32; 2],
    /// Nothing to accumulate with: the first frame, a cut, a new size.
    pub reset: bool,
    pub camera: Camera,
}

/// The camera's projection, for frame interpolation.
#[derive(Clone, Copy, Debug, Default)]
pub struct Camera {
    pub near: f32,
    pub far: f32,
    /// Vertical field of view, in degrees.
    pub fov_y: f32,
    /// (Width over height: for MetalFX's frame interpolation.)
    #[cfg_attr(not(target_vendor = "apple"), allow(dead_code))]
    pub aspect: f32,
    /// Depth 1 at the near plane, 0 at the far one.
    pub reversed: bool,
}

/// A 4x4 matrix as the engine stores it: rows, for row vectors (v · M).
type Mat = [[f64; 4]; 4];

/// Jitter phases before the sequence repeats, at 1x: eight per output
/// pixel's share of an input pixel (8 * scale², as FSR does), so each one
/// gets samples all over it before the history fades.
pub(crate) const PHASES: u32 = 8;

/// The jitter phases for an upscaler scaling by `scale`.
pub(crate) fn phases(scale: f32) -> u32 {
    ((PHASES as f32 * scale * scale).ceil() as u32).clamp(PHASES, 8 * PHASES)
}

/// The camera moved this far (m) or turned this much (cosine) between two
/// frames: a cut, nothing to accumulate.
const CUT_DISTANCE: f64 = 8.0;
const CUT_COS: f64 = 0.7;

/// Depth and motion from the scene's depth.
const MOTION_WGSL: &str = "
struct U { reproject: mat4x4f, size: vec2f, pad: vec2f }
@group(0) @binding(0) var depth: texture_depth_2d;
@group(0) @binding(1) var<uniform> u: U;
@vertex fn vs(@builtin(vertex_index) i: u32) -> @builtin(position) vec4f {
  var p = array<vec2f, 3>(vec2f(-1, -1), vec2f(3, -1), vec2f(-1, 3));
  return vec4f(p[i], 0, 1);
}
struct O { @builtin(frag_depth) depth: f32, @location(0) motion: vec4f }
@fragment fn fs(@builtin(position) p: vec4f) -> O {
  let d = textureLoad(depth, vec2i(p.xy), 0);
  let uv = p.xy / u.size;
  // Where this pixel's surface was: its clip position this frame through
  // the inverse view-projection, then the previous one's.
  let prev = u.reproject * vec4f(uv.x * 2.0 - 1.0, 1.0 - uv.y * 2.0, d, 1.0);
  var motion = vec2f(0.0);
  if prev.w > 0.0 {
    let prev_uv = vec2f(prev.x / prev.w * 0.5 + 0.5, 0.5 - prev.y / prev.w * 0.5);
    motion = (prev_uv - uv) * u.size;
  }
  return O(d, vec4f(motion, 0.0, 1.0));
}";

/// The camera of one frame.
#[derive(Clone, Copy)]
struct View {
    /// View-projection, unjittered.
    view_proj: Mat,
    /// Where the camera is and where it looks (world).
    position: [f64; 3],
    forward: [f64; 3],
    camera: Camera,
}

/// Textures the inputs are drawn into, for one scene size and format.
struct Targets {
    size: (u32, u32),
    format: wgpu::TextureFormat,
    color: wgpu::Texture,
    depth: wgpu::Texture,
    motion: wgpu::Texture,
    depth_view: wgpu::TextureView,
    motion_view: wgpu::TextureView,
}

/// The command thread's side: the camera and jitter of the frame being
/// recorded, and the inputs drawn at its scene's end.
#[derive(Default)]
pub struct Scene {
    /// The presenting side wants the inputs (a temporal upscaler is on):
    /// checked at each present.
    pub wanted: bool,
    /// Sprites in the world want the camera (glue5-runtime's sprites).
    pub sprites: bool,
    /// The back buffer's size, the scene's (from the last present).
    pub size: Option<(u32, u32)>,
    /// The back buffer last presented.
    pub back_buffer: u32,
    /// This frame's camera, once a draw showed it, and the depth target
    /// that draw wrote.
    view: Option<View>,
    depth_target: u32,
    /// The previous frame's, when that frame's inputs were drawn.
    prev: Option<View>,
    /// This frame's offset, in pixels, and its index in the sequence.
    pub jitter: [f32; 2],
    phase: u32,
    targets: Option<Targets>,
    pipeline: Option<(wgpu::RenderPipeline, wgpu::Buffer)>,
    /// Drawn this frame, for its present.
    pub done: Option<SceneInputs>,
    /// This frame had its scene drawn (`done` is taken by its present).
    drawn: bool,
}

impl Scene {
    /// A frame starts (after a present): its jitter, if wanted, for an
    /// upscaler scaling by `scale`.
    pub fn next_frame(&mut self, wanted: bool, back_buffer: u32, size: Option<(u32, u32)>, scale: f32) {
        if !wanted {
            self.prev = None;
            self.targets = None;
        }
        self.wanted = wanted;
        self.back_buffer = back_buffer;
        // A frame without a scene (a menu over everything) or of another
        // size breaks the history.
        if self.size != size || !self.drawn {
            self.prev = None;
        }
        self.drawn = false;
        self.size = size;
        self.view = None;
        self.done = None;
        self.phase = (self.phase + 1) % phases(scale);
        self.jitter =
            if wanted { [halton(self.phase + 1, 2) - 0.5, halton(self.phase + 1, 3) - 0.5] } else { [0.0; 2] };
    }

    /// This frame's camera is still to be found.
    pub fn wants_camera(&self) -> bool {
        (self.wanted || self.sprites) && self.view.is_none()
    }

    /// This frame's camera, once a draw showed it: view-projection, rows.
    pub fn view_proj(&self) -> Option<[[f32; 4]; 4]> {
        self.view.map(|v| v.view_proj.map(|row| row.map(|x| x as f32)))
    }

    /// The jitter in clip space for a pass of `size`, if the scene is drawn
    /// jittered and the pass is the scene's.
    pub fn clip_jitter(&self, size: (u32, u32)) -> Option<[f32; 2]> {
        let scene = self.size.filter(|&s| self.wanted && s == size)?;
        Some([2.0 * self.jitter[0] / scene.0 as f32, -2.0 * self.jitter[1] / scene.1 as f32])
    }

    /// A draw into a scene-sized target with depth: its constants show the
    /// camera if it is the frame's first perspective one. `matrices` are
    /// slot 1's first 64 floats (or fewer).
    pub fn observe(&mut self, matrices: &[f32], depth_target: u32) {
        if self.view.is_some() || matrices.len() < 64 {
            return;
        }
        let m =
            |i: usize| -> Mat { std::array::from_fn(|r| std::array::from_fn(|c| matrices[i * 16 + r * 4 + c] as f64)) };
        let (world, world_view, wvp, view_inverse) = (m(0), m(1), m(2), m(3));
        if !perspective(&wvp) {
            return;
        }
        let (Some(world_inv), Some(world_view_inv)) = (invert(&world), invert(&world_view)) else {
            return;
        };
        let proj = mul(&world_view_inv, &wvp);
        let p = |z: f64| (z * proj[2][2] + proj[3][2]) / (z * proj[2][3] + proj[3][3]);
        // Planes where depth is 0 and 1 (view space looks down -z).
        let (z0, z1) = (-proj[3][2] / proj[2][2], (proj[3][3] - proj[3][2]) / (proj[2][2] - proj[2][3]));
        let reversed = p(-1.0) > p(-100.0);
        let (near, far) = if reversed { (-z1, -z0) } else { (-z0, -z1) };
        let camera = Camera {
            near: near as f32,
            far: far as f32,
            fov_y: (2.0 * (1.0 / proj[1][1]).atan()).to_degrees() as f32,
            aspect: (proj[1][1] / proj[0][0]) as f32,
            reversed,
        };
        self.view = Some(View {
            view_proj: mul(&world_inv, &wvp),
            position: [view_inverse[3][0], view_inverse[3][1], view_inverse[3][2]],
            forward: [-view_inverse[2][0], -view_inverse[2][1], -view_inverse[2][2]],
            camera,
        });
        self.depth_target = depth_target;
    }
}

impl Gpu {
    /// The scene is finished in the back buffer: its inputs, for the
    /// present that follows.
    pub fn scene_done(&mut self) {
        let sc = &self.scene;
        if !sc.wanted || sc.done.is_some() {
            return;
        }
        let (Some(size), Some(view)) = (sc.size, sc.view) else { return };
        let (bb, depth_id) = (sc.back_buffer, sc.depth_target);
        let Some(color_src) =
            self.objs.tex(bb).filter(|t| (t.width, t.height) == size).map(|t| (t.tex.clone(), t.format))
        else {
            return;
        };
        let Some(depth_src) = self
            .objs
            .tex(depth_id)
            .filter(|t| (t.width, t.height) == size && t.samples == 1 && crate::format::is_depth(t.format))
            .map(|t| t.tex.clone())
        else {
            return;
        };
        self.flush_clears();
        self.end_pass();
        let epoch = self.epoch;
        for id in [bb, depth_id] {
            if let Some(t) = self.objs.tex(id) {
                t.used.set(epoch);
            }
        }
        let device = self.device.clone();
        let sc = &mut self.scene;
        if sc.targets.as_ref().is_none_or(|t| t.size != size || t.format != color_src.1) {
            sc.targets = Some(Targets::new(&device, size, color_src.1));
            sc.prev = None;
        }
        let (pipeline, uniforms) = sc.pipeline.get_or_insert_with(|| motion_pipeline(&device)).clone();
        // Where this frame's clip positions were in the previous frame.
        let prev = sc.prev.filter(|p| !is_cut(p, &view));
        let reproject = match prev.and_then(|p| invert(&view.view_proj).map(|inv| mul(&inv, &p.view_proj))) {
            Some(r) => r,
            None => std::array::from_fn(|r| std::array::from_fn(|c| (r == c) as u32 as f64)),
        };
        let mut data = [0f32; 20];
        for r in 0..4 {
            for c in 0..4 {
                data[r * 4 + c] = reproject[r][c] as f32;
            }
        }
        data[16] = size.0 as f32;
        data[17] = size.1 as f32;
        let t = sc.targets.as_ref().unwrap();
        let inputs = SceneInputs {
            color: t.color.clone(),
            depth: t.depth.clone(),
            motion: t.motion.clone(),
            jitter: sc.jitter,
            reset: prev.is_none(),
            camera: view.camera,
        };
        let (depth_view, motion_view, color) = (t.depth_view.clone(), t.motion_view.clone(), t.color.clone());
        sc.prev = Some(view);
        sc.done = Some(inputs);
        sc.drawn = true;
        self.uploads.buffer(&uniforms, 0, bytemuck::cast_slice(&data));
        let src = depth_src.create_view(&wgpu::TextureViewDescriptor {
            dimension: Some(wgpu::TextureViewDimension::D2),
            aspect: wgpu::TextureAspect::DepthOnly,
            mip_level_count: Some(1),
            array_layer_count: Some(1),
            ..Default::default()
        });
        let bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("scene motion"),
            layout: &pipeline.get_bind_group_layout(0),
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(&src) },
                wgpu::BindGroupEntry { binding: 1, resource: uniforms.as_entire_binding() },
            ],
        });
        let ts = self.time_pass("scene inputs");
        let encoder = self.encoder.get_or_insert_with(|| self.device.create_command_encoder(&Default::default()));
        let timestamp_writes = ts.and_then(|i| self.profiler.as_ref().map(|p| p.render(i)));
        let extent = wgpu::Extent3d { width: size.0, height: size.1, depth_or_array_layers: 1 };
        encoder.copy_texture_to_texture(color_src.0.as_image_copy(), color.as_image_copy(), extent);
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("scene motion"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &motion_view,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                view: &depth_view,
                depth_ops: Some(wgpu::Operations { load: wgpu::LoadOp::Clear(0.0), store: wgpu::StoreOp::Store }),
                stencil_ops: None,
            }),
            timestamp_writes,
            ..Default::default()
        });
        pass.set_pipeline(&pipeline);
        pass.set_bind_group(0, &bind, &[]);
        pass.draw(0..3, 0..1);
    }
}

impl Targets {
    fn new(device: &wgpu::Device, size: (u32, u32), format: wgpu::TextureFormat) -> Targets {
        let make = |label, format, usage| {
            device.create_texture(&wgpu::TextureDescriptor {
                label: Some(label),
                size: wgpu::Extent3d { width: size.0, height: size.1, depth_or_array_layers: 1 },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format,
                usage: usage | wgpu::TextureUsages::TEXTURE_BINDING,
                view_formats: &[],
            })
        };
        // The colour as stored (an sRGB back buffer holds it encoded, as
        // the upscalers and the present pass want it).
        let color = make("scene colour", format.remove_srgb_suffix(), wgpu::TextureUsages::COPY_DST);
        let motion = make("scene motion", wgpu::TextureFormat::Rg16Float, wgpu::TextureUsages::RENDER_ATTACHMENT);
        let depth = make("scene depth", wgpu::TextureFormat::Depth32Float, wgpu::TextureUsages::RENDER_ATTACHMENT);
        let (depth_view, motion_view) =
            (depth.create_view(&Default::default()), motion.create_view(&Default::default()));
        Targets { size, format, color, depth, motion, depth_view, motion_view }
    }
}

fn motion_pipeline(device: &wgpu::Device) -> (wgpu::RenderPipeline, wgpu::Buffer) {
    let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("scene motion"),
        source: wgpu::ShaderSource::Wgsl(MOTION_WGSL.into()),
    });
    let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("scene motion"),
        layout: None,
        vertex: wgpu::VertexState {
            module: &module,
            entry_point: Some("vs"),
            compilation_options: Default::default(),
            buffers: &[],
        },
        primitive: Default::default(),
        depth_stencil: Some(wgpu::DepthStencilState {
            format: wgpu::TextureFormat::Depth32Float,
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
            targets: &[Some(wgpu::TextureFormat::Rg16Float.into())],
        }),
        multiview_mask: None,
        cache: None,
    });
    let uniforms = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("scene motion"),
        size: 80,
        usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    (pipeline, uniforms)
}

/// Adds the scene's jitter to a copy of `rage_matrices` (slot 1, `bytes` its
/// first 192 or more): clip x and y move by `clip` times w. Orthographic
/// matrices (w = 1 everywhere: full-screen passes, the HUD) are left as
/// they are.
pub fn jitter_constants(bytes: &mut [u8], clip: [f32; 2]) {
    const WVP: usize = 32 * 4;
    let Some(m) = bytes.get_mut(WVP..WVP + 64) else { return };
    // (Unaligned: the copy sits anywhere in the uniform ring's mirror.)
    let mut f: [f32; 16] = std::array::from_fn(|i| f32::from_le_bytes(m[i * 4..i * 4 + 4].try_into().unwrap()));
    if f[3].abs() + f[7].abs() + f[11].abs() < 1e-6 {
        return;
    }
    for row in 0..4 {
        let w = f[row * 4 + 3];
        f[row * 4] += clip[0] * w;
        f[row * 4 + 1] += clip[1] * w;
    }
    for (i, v) in f.iter().enumerate() {
        m[i * 4..i * 4 + 4].copy_from_slice(&v.to_le_bytes());
    }
}

/// The engine's matrix projects with perspective (its w column is not
/// (0, 0, 0, 1)).
fn perspective(m: &Mat) -> bool {
    m[0][3].abs() + m[1][3].abs() + m[2][3].abs() > 1e-6
}

/// A cut: the camera jumped or turned too far for the frames to blend.
fn is_cut(a: &View, b: &View) -> bool {
    let d = (0..3).map(|i| (a.position[i] - b.position[i]).powi(2)).sum::<f64>().sqrt();
    let len = |v: &[f64; 3]| v.iter().map(|x| x * x).sum::<f64>().sqrt().max(1e-9);
    let cos = (0..3).map(|i| a.forward[i] * b.forward[i]).sum::<f64>() / (len(&a.forward) * len(&b.forward));
    d > CUT_DISTANCE || cos < CUT_COS
}

/// Element `index` (from 1) of the Halton sequence in `base`, in [0, 1).
pub(crate) fn halton(mut index: u32, base: u32) -> f32 {
    let (mut f, mut r) = (1.0f32, 0.0f32);
    while index > 0 {
        f /= base as f32;
        r += f * (index % base) as f32;
        index /= base;
    }
    r
}

fn mul(a: &Mat, b: &Mat) -> Mat {
    std::array::from_fn(|r| std::array::from_fn(|c| (0..4).map(|k| a[r][k] * b[k][c]).sum()))
}

/// Gauss-Jordan with partial pivoting.
fn invert(m: &Mat) -> Option<Mat> {
    let mut a = *m;
    let mut inv: Mat = std::array::from_fn(|r| std::array::from_fn(|c| (r == c) as u32 as f64));
    for c in 0..4 {
        let p = (c..4).max_by(|&x, &y| a[x][c].abs().total_cmp(&a[y][c].abs()))?;
        if a[p][c].abs() < 1e-12 {
            return None;
        }
        a.swap(c, p);
        inv.swap(c, p);
        let d = a[c][c];
        for k in 0..4 {
            a[c][k] /= d;
            inv[c][k] /= d;
        }
        for r in 0..4 {
            if r != c {
                let f = a[r][c];
                for k in 0..4 {
                    a[r][k] -= f * a[c][k];
                    inv[r][k] -= f * inv[c][k];
                }
            }
        }
    }
    Some(inv)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn motion_shader_validates() {
        let module = naga::front::wgsl::parse_str(MOTION_WGSL).unwrap();
        naga::valid::Validator::new(naga::valid::ValidationFlags::all(), naga::valid::Capabilities::all())
            .validate(&module)
            .unwrap();
    }

    #[test]
    fn halton_in_unit_interval() {
        assert_eq!(halton(1, 2), 0.5);
        assert!((halton(2, 3) - 2.0 / 3.0).abs() < 1e-6);
        assert!((1..=phases(3.0)).all(|i| (0.0..1.0).contains(&halton(i, 3))));
        assert_eq!((phases(1.0), phases(2.0), phases(3.0)), (8, 32, 64));
    }

    #[test]
    #[allow(clippy::excessive_precision)]
    fn camera_from_the_engine_constants() {
        // gWorld, gWorldView, gWorldViewProj, gViewInverse of a draw (logged in the sandbox).
        #[rustfmt::skip]
        let m: [f32; 64] = [
            -0.86997, 0.49310, 0.0, 0.0, -0.49310, -0.86997, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 4.93608, -1453.50830, 29.49050, 1.0,
            -0.49449, -0.00387, 0.86918, 0.0, 0.86918, 0.00077, 0.49449, 0.0, -0.00258, 0.99999, 0.00298, 0.0, 0.39234, -1.61482, -21.71248, 1.0,
            -3.17925, -0.04418, 0.00001, -0.86918, 5.58832, 0.00879, 0.00001, -0.49449, -0.01659, 11.42996, 0.0, -0.00298, 2.52254, -18.45745, 0.14968, 21.71248,
            0.00159, -1.0, -0.00258, 0.0, 0.00298, -0.00257, 0.99999, 0.0, -0.99999, -0.00160, 0.00298, 0.0, -16.77208, -1453.15490, 31.17100, 1.0,
        ];
        let mut s = Scene::default();
        s.observe(&m, 7);
        let v = s.view.unwrap();
        assert!(v.camera.reversed);
        assert!((v.camera.aspect - 16.0 / 9.0).abs() < 0.01, "{:?}", v.camera);
        assert!((v.camera.near - 0.15).abs() < 0.01, "{:?}", v.camera);
        assert!((v.camera.fov_y - 10.0).abs() < 0.1, "{:?}", v.camera);
        assert_eq!(s.depth_target, 7);
    }

    /// The history goes on from frame to frame while each has its scene
    /// (its present takes `done`), and breaks after one without.
    #[test]
    fn history_across_frames() {
        let mut s = Scene::default();
        s.next_frame(true, 1, Some((64, 36)), 2.0);
        s.prev = Some(View {
            view_proj: [[0.0; 4]; 4],
            position: [0.0; 3],
            forward: [0.0, 0.0, -1.0],
            camera: Camera::default(),
        });
        s.drawn = true;
        s.done.take();
        s.next_frame(true, 1, Some((64, 36)), 2.0);
        assert!(s.prev.is_some(), "kept after a frame with its scene");
        s.next_frame(true, 1, Some((64, 36)), 2.0);
        assert!(s.prev.is_none(), "broken after a frame without");
    }

    #[test]
    fn jitter_moves_perspective_only() {
        let mut bytes = vec![0u8; 256];
        let f: &mut [f32] = bytemuck::cast_slice_mut(&mut bytes);
        // An orthographic gWorldViewProj (the HUD's).
        f[32..48].copy_from_slice(&[1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, -1.0, 1.0, 0.0, 1.0]);
        let before = bytes.clone();
        jitter_constants(&mut bytes, [0.1, 0.2]);
        assert_eq!(bytes, before);
        let f: &mut [f32] = bytemuck::cast_slice_mut(&mut bytes);
        // A perspective one: w = -z.
        f[32..48].copy_from_slice(&[1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 0.0, -1.0, 0.0, 0.0, 0.15, 0.0]);
        jitter_constants(&mut bytes, [0.1, 0.2]);
        let f: &[f32] = bytemuck::cast_slice(&bytes);
        assert_eq!(&f[40..44], &[-0.1, -0.2, 0.0, -1.0]);
    }
}
