//! Presenting: the submit thread scales the engine's back buffer into the
//! window's surface (aspect preserved; MetalFX where the application asks
//! for it and the GPU has it, a bicubic filter otherwise), draws the
//! overlay and presents, and keeps frames at most two ahead of the GPU,
//! window or not. Before the game runs, the application shows its UI
//! through the same [`Output`] (see [`crate::Screen`]).
//!
//! Frames are shown at an even pace: with a frame rate set, each stays on
//! screen for at least its share of a second, measured from when the one
//! before actually appeared. A display faster than the game (120 Hz
//! ProMotion, a 144 Hz monitor) would otherwise show each frame at the first
//! refresh after it is ready, for 1, 2 or 3 refreshes as it comes: judder,
//! though the frames themselves are on time. On Apple platforms the window's
//! images are taken from its Metal layer and shown by hand, as wgpu does but
//! with `presentDrawable:afterMinimumDuration:`, and the times they reached
//! the screen go into the statistics.

use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::{Result, bail};
#[cfg(target_vendor = "apple")]
use objc2::rc::{Retained, autoreleasepool};
#[cfg(target_vendor = "apple")]
use objc2::runtime::ProtocolObject;
#[cfg(target_vendor = "apple")]
use objc2_quartz_core::CAMetalDrawable;
use wgpu::TextureFormat;
#[cfg(target_vendor = "apple")]
use wgpu::hal::api::Metal;

use crate::done::Work;
use crate::exec::Gpu;
use crate::overlay::OverlayRenderer;
use crate::submit::Present;

/// Scales the back buffer into the window.
const PRESENT_WGSL: &str = "@group(0) @binding(0) var src: texture_2d<f32>;
@group(0) @binding(1) var smp: sampler;
@fragment fn fs(v: V) -> @location(0) vec4f { return vec4f(bicubic_src(v.uv), 1.0); }";

/// Puts the HUD over the scene a temporal upscaler made: what the HUD
/// changed in the back buffer (the back buffer minus the scene before it,
/// both scaled the same way) added to the upscaled scene. Where there is no
/// HUD that adds nothing, and a translucent one keeps the sharp scene
/// beneath it.
const COMPOSITE_WGSL: &str = "@group(0) @binding(0) var scene_up: texture_2d<f32>;
@group(0) @binding(1) var frame: texture_2d<f32>;
@group(0) @binding(2) var scene: texture_2d<f32>;
@group(0) @binding(3) var smp: sampler;
@fragment fn fs(v: V) -> @location(0) vec4f {
  let hud = bicubic_frame(v.uv) - bicubic_scene(v.uv);
  let up = bicubic_scene_up(v.uv);
  return vec4f(clamp(up + hud, vec3f(0.0), vec3f(1.0)), 1.0);
}";

/// What the presentation shaders share: a full-screen triangle.
const COMMON_WGSL: &str = "struct V { @builtin(position) p: vec4f, @location(0) uv: vec2f }
@vertex fn vs(@builtin(vertex_index) i: u32) -> V {
  var p = array<vec2f, 3>(vec2f(-1, -1), vec2f(3, -1), vec2f(-1, 3));
  return V(vec4f(p[i], 0, 1), p[i] * vec2f(0.5, -0.5) + 0.5);
}
";

/// Catmull-Rom bicubic in five bilinear taps. Each generated function reads
/// its texture from module scope: Mali drivers have crashed on image handles
/// passed as WGSL function parameters (wgpu issue #10186).
const BICUBIC_WGSL: &str = "
fn BICUBIC_FUNCTION(uv: vec2f) -> vec3f {
  let size = vec2f(textureDimensions(BICUBIC_TEXTURE, 0));
  let pos = uv * size;
  let center = floor(pos - 0.5) + 0.5;
  let f = pos - center;
  let f2 = f * f;
  let f3 = f2 * f;
  let w0 = -0.5 * f3 + f2 - 0.5 * f;
  let w1 = 1.5 * f3 - 2.5 * f2 + 1.0;
  let w2 = -1.5 * f3 + 2.0 * f2 + 0.5 * f;
  let w3 = 0.5 * f3 - 0.5 * f2;
  let w12 = w1 + w2;
  let t12 = (center + w2 / w12) / size;
  let t0 = (center - 1.0) / size;
  let t3 = (center + 2.0) / size;
  let c = textureSampleLevel(BICUBIC_TEXTURE, smp, vec2f(t12.x, t0.y), 0.0).rgb * (w12.x * w0.y)
    + textureSampleLevel(BICUBIC_TEXTURE, smp, vec2f(t0.x, t12.y), 0.0).rgb * (w0.x * w12.y)
    + textureSampleLevel(BICUBIC_TEXTURE, smp, t12, 0.0).rgb * (w12.x * w12.y)
    + textureSampleLevel(BICUBIC_TEXTURE, smp, vec2f(t3.x, t12.y), 0.0).rgb * (w3.x * w12.y)
    + textureSampleLevel(BICUBIC_TEXTURE, smp, vec2f(t12.x, t3.y), 0.0).rgb * (w12.x * w3.y);
  let total = w12.x * w0.y + w0.x * w12.y + w12.x * w12.y + w3.x * w12.y + w12.x * w3.y;
  return max(c / total, vec3f(0.0));
}
";

fn bicubic_functions(textures: &[(&str, &str)]) -> String {
    textures.iter().fold(String::new(), |mut source, (function, texture)| {
        source.push_str(&BICUBIC_WGSL.replace("BICUBIC_FUNCTION", function).replace("BICUBIC_TEXTURE", texture));
        source
    })
}

/// Behind the application's UI while no game frame is shown.
const BACKGROUND: wgpu::Color = wgpu::Color { r: 0.05, g: 0.055, b: 0.07, a: 1.0 };

/// How the back buffer is scaled up to the window.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Upscaler {
    /// Catmull-Rom bicubic filtering.
    Bicubic = 0,
    /// MetalFX's spatial upscaler, on Apple GPUs that have it (bicubic
    /// elsewhere).
    MetalFx = 1,
    /// MetalFX's temporal upscaler: the scene from several frames drawn at
    /// sub-pixel offsets, the HUD put back over it (Apple GPUs that have
    /// it; bicubic elsewhere).
    MetalFxTemporal = 2,
    /// AMD's FSR 2 upscaler (temporal, as MetalFX's), on GPUs with the features it needs.
    Fsr = 3,
}

impl Upscaler {
    /// It needs the scene without the HUD, jittered (see scene.rs).
    pub fn is_temporal(self) -> bool {
        matches!(self, Upscaler::MetalFxTemporal | Upscaler::Fsr)
    }
}

/// How frames are shown: set by the application whenever it likes, read by
/// the renderer at each frame.
#[derive(Clone)]
pub struct Presentation(Arc<Shared>);

struct Shared {
    /// The window's size in pixels, width << 32 | height.
    size: AtomicU64,
    vsync: AtomicBool,
    metal_hud: AtomicBool,
    upscaler: AtomicU8,
    /// Upscalers this GPU has (bit per [`Upscaler`]), once the window's
    /// output knows.
    upscalers: AtomicU8,
    /// A frame made between each two of the game's (with a temporal upscaler
    /// that can).
    frame_generation: AtomicBool,
    /// This GPU can (once the window's output knows).
    has_frame_generation: AtomicBool,
    /// Frames per second at most, shown evenly (0: as they come).
    frame_rate: AtomicU32,
    /// FSR's sharpening, 0 to 1 (f32 bits).
    sharpness: AtomicU32,
    /// A new surface for the window, to present into from the next frame.
    surface: Mutex<Option<wgpu::Surface<'static>>>,
}

impl Presentation {
    pub fn new((width, height): (u32, u32), vsync: bool, upscaler: Upscaler) -> Self {
        let p = Presentation(Arc::new(Shared {
            size: AtomicU64::new(0),
            vsync: AtomicBool::new(vsync),
            metal_hud: AtomicBool::new(false),
            upscaler: AtomicU8::new(upscaler as u8),
            upscalers: AtomicU8::new(1 << Upscaler::Bicubic as u8),
            frame_generation: AtomicBool::new(false),
            has_frame_generation: AtomicBool::new(false),
            frame_rate: AtomicU32::new(0),
            sharpness: AtomicU32::new(0.5f32.to_bits()),
            surface: Mutex::new(None),
        }));
        p.set_size(width, height);
        p
    }

    /// The window's size in pixels.
    pub fn set_size(&self, width: u32, height: u32) {
        self.0.size.store((width as u64) << 32 | height as u64, Ordering::Relaxed);
    }

    /// Presents into `surface` from the next frame: the window's native
    /// surface was replaced (Android gives it a new one each time the app
    /// comes back to the foreground).
    pub fn replace_surface(&self, surface: wgpu::Surface<'static>) {
        *self.0.surface.lock().unwrap() = Some(surface);
    }

    pub fn size(&self) -> (u32, u32) {
        let v = self.0.size.load(Ordering::Relaxed);
        ((v >> 32) as u32, v as u32)
    }

    pub fn set_vsync(&self, on: bool) {
        self.0.vsync.store(on, Ordering::Relaxed);
    }

    /// Shows or hides Apple's Metal performance HUD from the next frame, once
    /// [`crate::enable_metal_hud`] has shown it.
    pub fn set_metal_hud(&self, on: bool) {
        self.0.metal_hud.store(on, Ordering::Relaxed);
    }

    fn vsync(&self) -> bool {
        self.0.vsync.load(Ordering::Relaxed)
    }

    pub fn set_upscaler(&self, upscaler: Upscaler) {
        self.0.upscaler.store(upscaler as u8, Ordering::Relaxed);
    }

    fn upscaler(&self) -> Upscaler {
        match self.0.upscaler.load(Ordering::Relaxed) {
            1 => Upscaler::MetalFx,
            2 => Upscaler::MetalFxTemporal,
            3 => Upscaler::Fsr,
            _ => Upscaler::Bicubic,
        }
    }

    /// How much FSR sharpens its image, 0 (not at all) to 1.
    pub fn set_sharpness(&self, sharpness: f32) {
        self.0.sharpness.store(sharpness.clamp(0.0, 1.0).to_bits(), Ordering::Relaxed);
    }

    fn sharpness(&self) -> f32 {
        f32::from_bits(self.0.sharpness.load(Ordering::Relaxed))
    }

    /// Frame generation, from the next frame (with an upscaler that can:
    /// MetalFX Temporal, on macOS and iOS 26).
    pub fn set_frame_generation(&self, on: bool) {
        self.0.frame_generation.store(on, Ordering::Relaxed);
    }

    #[cfg_attr(not(target_vendor = "apple"), allow(dead_code))]
    fn frame_generation(&self) -> bool {
        self.0.frame_generation.load(Ordering::Relaxed)
    }

    /// Whether this GPU can generate frames.
    pub fn has_frame_generation(&self) -> bool {
        self.0.has_frame_generation.load(Ordering::Relaxed)
    }

    /// Whether this GPU has `upscaler`.
    pub fn has(&self, upscaler: Upscaler) -> bool {
        self.0.upscalers.load(Ordering::Relaxed) & 1 << upscaler as u8 != 0
    }

    /// The renderer is to draw the scene for a temporal upscaler (jittered,
    /// kept before the HUD).
    pub fn wants_scene(&self) -> bool {
        let upscaler = self.upscaler();
        upscaler.is_temporal() && self.has(upscaler)
    }

    /// Frames per second at most, each shown for the same time (0: each
    /// as soon as it is ready). Where the platform cannot pace frames
    /// (wgpu's surface), the engine's own limiter does it alone.
    pub fn set_frame_rate(&self, fps: u32) {
        self.0.frame_rate.store(fps, Ordering::Relaxed);
    }

    /// How long each frame stays on screen at least.
    #[cfg_attr(not(target_vendor = "apple"), allow(dead_code))]
    fn frame_time(&self) -> Option<Duration> {
        let fps = self.0.frame_rate.load(Ordering::Relaxed);
        (fps > 0).then(|| Duration::from_secs_f64(1.0 / fps as f64))
    }
}

/// When the window's frames reached the screen (host seconds), since the
/// statistics last took them.
static ON_SCREEN: Mutex<Vec<f64>> = Mutex::new(Vec::new());

/// The upscalers' GPU time (seconds) and the frames they upscaled, since the
/// statistics last took them (Apple GPUs: their command buffers' times).
static UPSCALING: Mutex<(f64, u32)> = Mutex::new((0.0, 0));

/// The upscalers' GPU time per frame they upscaled, in milliseconds, since
/// the last call (none: nothing upscaled, or not measured here).
pub(crate) fn take_upscaling() -> Option<f64> {
    let (seconds, frames) = std::mem::take(&mut *UPSCALING.lock().unwrap());
    (frames > 0 && seconds > 0.0).then(|| seconds * 1000.0 / frames as f64)
}

/// One frame's upscaling `commands` between two empty command buffers whose
/// times measure them, added to the upscalers' (on Apple GPUs; elsewhere
/// as they are).
pub(crate) fn timed(device: &wgpu::Device, commands: Vec<wgpu::CommandBuffer>) -> Vec<wgpu::CommandBuffer> {
    #[cfg(target_vendor = "apple")]
    {
        let ended = Arc::new(Mutex::new(None::<f64>));
        let before = {
            let ended = ended.clone();
            crate::perf::marker(device, move |_, end| *ended.lock().unwrap() = Some(end))
        };
        let after = crate::perf::marker(device, move |start, _| {
            if let Some(end) = ended.lock().unwrap().take()
                && start > end
            {
                UPSCALING.lock().unwrap().0 += start - end;
            }
        });
        UPSCALING.lock().unwrap().1 += 1;
        let mut all = Vec::with_capacity(commands.len() + 2);
        all.push(before);
        all.extend(commands);
        all.push(after);
        all
    }
    #[cfg(not(target_vendor = "apple"))]
    {
        let _ = device;
        commands
    }
}

/// The times frames stayed on screen, in milliseconds, since the last call.
pub(crate) fn take_on_screen() -> Vec<f32> {
    let mut times = ON_SCREEN.lock().unwrap();
    let ms = times.windows(2).map(|w| ((w[1] - w[0]) * 1000.0) as f32).collect();
    // The last one starts the next interval.
    let last = times.last().copied();
    times.clear();
    times.extend(last);
    ms
}

/// Whether the window showing `layer` is at least partly visible: the
/// drawable of a hidden window waits up to a second for the screen (wgpu's
/// own acquisition makes the same check, gfx-rs/wgpu#8309).
#[cfg(target_os = "macos")]
fn window_visible(layer: &objc2_quartz_core::CAMetalLayer) -> bool {
    use objc2::Message;
    use objc2::runtime::NSObject;
    const VISIBLE: usize = 1 << 1; // NSWindowOcclusionStateVisible
    // The first layer with a delegate is the window's view.
    let mut current = Some(layer.retain().into_super());
    while let Some(l) = current {
        if let Some(view) = l.delegate() {
            let window: Option<Retained<NSObject>> = unsafe { objc2::msg_send![&*view, window] };
            let state: usize = window.map_or(VISIBLE, |w| unsafe { objc2::msg_send![&*w, occlusionState] });
            return state & VISIBLE != 0;
        }
        current = l.superlayer();
    }
    true
}

/// The window's next image, to draw into and then show ([`Output::show`]).
pub struct Frame {
    pub texture: wgpu::Texture,
    /// The format to view it with (the surface's, without sRGB).
    pub format: TextureFormat,
    image: Image,
}

/// What a frame's texture is.
enum Image {
    #[cfg(not(target_vendor = "apple"))]
    Surface(wgpu::SurfaceTexture),
    #[cfg(target_vendor = "apple")]
    Drawable(Retained<ProtocolObject<dyn CAMetalDrawable>>),
    /// An offscreen output's (see [`Output::offscreen`]).
    Offscreen,
}

/// A window to present into.
pub struct Window {
    pub instance: wgpu::Instance,
    pub surface: wgpu::Surface<'static>,
    pub presentation: Presentation,
    /// Drawn over the frames (the application's UI).
    pub overlay: crate::Overlay,
}

/// Where a `src`-sized image goes in a `dst`-sized frame (x, y, width,
/// height in pixels): as large as fits, aspect preserved, centred. Aspects
/// within 1% fill the frame: a band one pixel wide would look like a fault,
/// a 1% stretch does not show.
fn fit((sw, sh): (u32, u32), (dw, dh): (u32, u32)) -> (u32, u32, u32, u32) {
    // A window with no area (Windows lets one shrink to no height).
    if dw == 0 || dh == 0 {
        return (0, 0, dw, dh);
    }
    let (sw, sh) = (sw.max(1) as f64, sh.max(1) as f64);
    if ((sw / sh) / (dw as f64 / dh as f64) - 1.0).abs() < 0.01 {
        return (0, 0, dw, dh);
    }
    let scale = (dw as f64 / sw).min(dh as f64 / sh);
    let w = ((sw * scale).round() as u32).clamp(1, dw);
    let h = ((sh * scale).round() as u32).clamp(1, dh);
    ((dw - w) / 2, (dh - h) / 2, w, h)
}

/// The window's surface.
struct Display {
    surface: wgpu::Surface<'static>,
    /// Size and vsync it is configured with.
    configured: Option<(u32, u32, bool)>,
    /// The surface's format, and the one its images are viewed with (no sRGB).
    surface_format: Option<TextureFormat>,
    format: Option<TextureFormat>,
    /// Whether the layer last asked for the HUD shown.
    #[cfg(target_vendor = "apple")]
    metal_hud: Option<bool>,
}

impl Display {
    fn configure(&mut self, caps: &wgpu::SurfaceCapabilities, device: &wgpu::Device, (w, h, vsync): (u32, u32, bool)) {
        // The back buffer holds display-ready (gamma-encoded) values: a non-sRGB surface.
        let format = caps
            .formats
            .iter()
            .copied()
            .find(|f| matches!(f, TextureFormat::Bgra8Unorm | TextureFormat::Rgba8Unorm))
            .unwrap_or(caps.formats[0]);
        let linear = format.remove_srgb_suffix();
        let present_mode = if vsync {
            wgpu::PresentMode::Fifo
        } else if caps.present_modes.contains(&wgpu::PresentMode::Mailbox) {
            wgpu::PresentMode::Mailbox
        } else {
            wgpu::PresentMode::AutoNoVsync
        };
        // (Waits for the GPU to finish what it has: a window being resized
        // pays it once per frame.)
        self.surface.configure(
            device,
            &wgpu::SurfaceConfiguration {
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
                format,
                color_space: Default::default(),
                width: w,
                height: h,
                present_mode,
                desired_maximum_frame_latency: 2,
                alpha_mode: wgpu::CompositeAlphaMode::Auto,
                view_formats: if linear != format { vec![linear] } else { vec![] },
            },
        );
        self.surface_format = Some(format);
        self.format = Some(linear);
        self.configured = Some((w, h, vsync));
        log::debug!("gpu: surface {w}x{h} {format:?} {present_mode:?}");
    }

    fn acquire(
        &mut self,
        adapter: &wgpu::Adapter,
        device: &wgpu::Device,
        wanted: (u32, u32, bool),
        metal_hud: bool,
    ) -> Option<Frame> {
        if wanted.0 == 0 || wanted.1 == 0 {
            return None;
        }
        if self.configured != Some(wanted) {
            let caps = self.surface.get_capabilities(adapter);
            if caps.formats.is_empty() {
                return None;
            }
            self.configure(&caps, device, wanted);
        }
        self.next_image(device, metal_hud)
    }

    #[cfg(not(target_vendor = "apple"))]
    fn next_image(&mut self, _: &wgpu::Device, _: bool) -> Option<Frame> {
        let format = self.format?;
        let surface = match self.surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(t) => t,
            wgpu::CurrentSurfaceTexture::Suboptimal(t) => {
                self.configured = None;
                t
            }
            wgpu::CurrentSurfaceTexture::Outdated | wgpu::CurrentSurfaceTexture::Lost => {
                self.configured = None;
                return None;
            }
            _ => return None,
        };
        Some(Frame { texture: surface.texture.clone(), format, image: Image::Surface(surface) })
    }

    /// The layer's next drawable (waiting for one, as wgpu does), as a
    /// texture to draw into.
    #[cfg(target_vendor = "apple")]
    fn next_image(&mut self, device: &wgpu::Device, metal_hud: bool) -> Option<Frame> {
        use objc2_metal::{MTLTexture, MTLTextureType};
        let (surface_format, format) = (self.surface_format?, self.format?);
        let layer = unsafe { self.surface.as_hal::<Metal>() }.map(|s| s.render_layer().lock().clone())?;
        // Followed once the HUD `crate::enable_metal_hud` showed is attached.
        if self.metal_hud != Some(metal_hud) {
            use objc2_foundation::{NSDictionary, ns_string};
            let mode = if metal_hud { ns_string!("default") } else { ns_string!("disabled") };
            let properties = NSDictionary::from_slices(&[ns_string!("mode")], &[mode]);
            // SAFETY: HUD properties are string keys and string values; the cast
            // only erases NSDictionary's generic types for the Objective-C API.
            let properties = unsafe { Retained::cast_unchecked(properties) };
            autoreleasepool(|_| unsafe { layer.setDeveloperHUDProperties(Some(&properties)) });
            self.metal_hud = Some(metal_hud);
        }
        #[cfg(target_os = "macos")]
        if !window_visible(&layer) {
            return None;
        }
        let drawable = autoreleasepool(|_| layer.nextDrawable())?;
        let raw = drawable.texture();
        let size = wgpu::Extent3d { width: raw.width() as u32, height: raw.height() as u32, depth_or_array_layers: 1 };
        let extent = wgpu::hal::CopyExtent { width: size.width, height: size.height, depth: 1 };
        let hal = unsafe {
            wgpu::hal::metal::Device::texture_from_raw(raw, surface_format, MTLTextureType::Type2D, 1, 1, extent, None)
        };
        let view_formats = if format != surface_format { vec![format] } else { vec![] };
        let texture = unsafe {
            device.create_texture_from_hal::<Metal>(
                hal,
                &wgpu::TextureDescriptor {
                    label: Some("window"),
                    size,
                    mip_level_count: 1,
                    sample_count: 1,
                    dimension: wgpu::TextureDimension::D2,
                    format: surface_format,
                    usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
                    view_formats: &view_formats,
                },
                wgpu::wgt::TextureUses::UNINITIALIZED,
            )
        };
        Some(Frame { texture, format, image: Image::Drawable(drawable) })
    }
}

/// The present pass's pipelines, for the format they draw into.
#[derive(Default)]
struct Passes {
    format: Option<TextureFormat>,
    blit: Option<(wgpu::RenderPipeline, wgpu::Sampler)>,
    composite: Option<(wgpu::RenderPipeline, wgpu::Sampler)>,
}

impl Passes {
    fn for_format(&mut self, format: TextureFormat) {
        if self.format != Some(format) {
            *self = Passes { format: Some(format), ..Default::default() };
        }
    }

    /// `src` scaled into `rect` (x, y, width, height) of the pass's target.
    fn blit(
        &mut self,
        device: &wgpu::Device,
        pass: &mut wgpu::RenderPass<'_>,
        src: &wgpu::TextureView,
        format: TextureFormat,
        rect: (u32, u32, u32, u32),
    ) {
        self.for_format(format);
        let (pipeline, sampler) = match &self.blit {
            Some(b) => b.clone(),
            None => {
                let bicubic = bicubic_functions(&[("bicubic_src", "src")]);
                let Some(pipeline) = present_pipeline(device, "present", PRESENT_WGSL, &bicubic, format) else { return };
                self.blit.insert((pipeline, linear_sampler(device))).clone()
            }
        };
        let bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: None,
            layout: &pipeline.get_bind_group_layout(0),
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(src) },
                wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::Sampler(&sampler) },
            ],
        });
        draw_in(pass, rect, &pipeline, &bg);
    }

    /// The upscaled scene `scene_up` with the HUD of `frame` over it, into
    /// `rect` of the pass's target (see `COMPOSITE_WGSL`).
    #[cfg_attr(not(target_vendor = "apple"), allow(dead_code))]
    #[allow(clippy::too_many_arguments)]
    fn composite(
        &mut self,
        device: &wgpu::Device,
        pass: &mut wgpu::RenderPass<'_>,
        scene_up: &wgpu::TextureView,
        frame: &Present,
        scene: &wgpu::TextureView,
        format: TextureFormat,
        rect: (u32, u32, u32, u32),
    ) {
        self.for_format(format);
        let (pipeline, sampler) = match &self.composite {
            Some(c) => c.clone(),
            None => {
                let bicubic = bicubic_functions(&[
                    ("bicubic_scene_up", "scene_up"),
                    ("bicubic_frame", "frame"),
                    ("bicubic_scene", "scene"),
                ]);
                let Some(pipeline) = present_pipeline(device, "composite", COMPOSITE_WGSL, &bicubic, format) else {
                    return;
                };
                self.composite.insert((pipeline, linear_sampler(device))).clone()
            }
        };
        let bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: None,
            layout: &pipeline.get_bind_group_layout(0),
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(scene_up) },
                wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::TextureView(&frame.view) },
                wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::TextureView(scene) },
                wgpu::BindGroupEntry { binding: 3, resource: wgpu::BindingResource::Sampler(&sampler) },
            ],
        });
        draw_in(pass, rect, &pipeline, &bg);
    }
}

/// A pipeline drawing a full-screen triangle with `fs` (and the bicubic
/// filter, see `COMMON_WGSL`) into `format`.
fn present_pipeline(
    device: &wgpu::Device,
    label: &str,
    fs: &str,
    bicubic: &str,
    format: TextureFormat,
) -> Option<wgpu::RenderPipeline> {
    let module = crate::shader::create_module(device, label, &format!("{COMMON_WGSL}{bicubic}{fs}")).ok()?;
    Some(device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some(label),
        layout: None,
        vertex: wgpu::VertexState {
            module: &module,
            entry_point: Some("vs"),
            compilation_options: Default::default(),
            buffers: &[],
        },
        primitive: Default::default(),
        depth_stencil: None,
        multisample: Default::default(),
        fragment: Some(wgpu::FragmentState {
            module: &module,
            entry_point: Some("fs"),
            compilation_options: Default::default(),
            targets: &[Some(format.into())],
        }),
        multiview_mask: None,
        cache: None,
    }))
}

fn linear_sampler(device: &wgpu::Device) -> wgpu::Sampler {
    device.create_sampler(&wgpu::SamplerDescriptor {
        mag_filter: wgpu::FilterMode::Linear,
        min_filter: wgpu::FilterMode::Linear,
        ..Default::default()
    })
}

/// One full-screen triangle into `rect` of the pass's target.
fn draw_in(
    pass: &mut wgpu::RenderPass<'_>,
    (x, y, w, h): (u32, u32, u32, u32),
    pipeline: &wgpu::RenderPipeline,
    bg: &wgpu::BindGroup,
) {
    pass.set_viewport(x as f32, y as f32, w as f32, h as f32, 0.0, 1.0);
    pass.set_pipeline(pipeline);
    pass.set_bind_group(0, bg, &[]);
    pass.draw(0..3, 0..1);
}

pub(crate) fn begin_pass<'e>(
    encoder: &'e mut wgpu::CommandEncoder,
    target: &wgpu::TextureView,
    clear: wgpu::Color,
) -> wgpu::RenderPass<'e> {
    encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
        color_attachments: &[Some(wgpu::RenderPassColorAttachment {
            view: target,
            depth_slice: None,
            resolve_target: None,
            ops: wgpu::Operations { load: wgpu::LoadOp::Clear(clear), store: wgpu::StoreOp::Store },
        })],
        ..Default::default()
    })
}

/// The window's side of presenting: its surface, the overlay drawn over the
/// frames, the upscaler. The application has it before the game runs
/// ([`crate::Screen`]), the submit thread after.
pub struct Output {
    adapter: wgpu::Adapter,
    presentation: Presentation,
    /// The window's surface; none: frames go to an offscreen texture.
    display: Option<Display>,
    offscreen: Option<Offscreen>,
    passes: Passes,
    overlay: OverlayRenderer,
    fsr: crate::fsr::Fsr,
    /// When the last game frame came, and the time between them (smoothed).
    last_frame: Option<Instant>,
    frame_interval: Duration,
    #[cfg(target_vendor = "apple")]
    metalfx: crate::metalfx::MetalFx,
}

/// The most a temporal upscaler scales by (MetalFX's temporal scaler and
/// FSR are made for 3x at most: past it MetalFX's crashes, and both lack
/// the samples to fill the pixels steadily).
pub(crate) const TEMPORAL_SCALE: f32 = 3.0;

/// The size a temporal upscaler makes of `input` for a window rect of
/// `size`: `size`, or less, `TEMPORAL_SCALE` times the input at most.
fn temporal_size(input: (u32, u32), size: (u32, u32)) -> (u32, u32) {
    let k = (TEMPORAL_SCALE * input.0 as f32 / size.0 as f32).min(TEMPORAL_SCALE * input.1 as f32 / size.1 as f32);
    if k >= 1.0 {
        return size;
    }
    ((size.0 as f32 * k).round() as u32, (size.1 as f32 * k).round() as u32)
}

/// What an upscaler made of a frame.
pub(crate) struct Upscaled {
    pub commands: Vec<wgpu::CommandBuffer>,
    pub image: Scaled,
    /// A frame between the previous one and this (the scene, at the same
    /// size), to show first (frame generation).
    pub between: Option<wgpu::TextureView>,
}

/// What an upscaler made of a frame, at the window's size.
pub(crate) enum Scaled {
    /// The whole back buffer, HUD included.
    #[cfg_attr(not(target_vendor = "apple"), allow(dead_code))]
    Frame(wgpu::TextureView),
    /// The scene alone: the HUD goes over it.
    #[cfg_attr(not(target_vendor = "apple"), allow(dead_code))]
    Scene(wgpu::TextureView),
}

/// Frames drawn into a texture instead of a window (the headless runner's
/// `--output`): the same upscalers and passes, every n-th frame to a PNG.
struct Offscreen {
    texture: Option<wgpu::Texture>,
    shots: Option<(u32, std::path::PathBuf)>,
    shown: u64,
}

impl Output {
    pub fn new(adapter: wgpu::Adapter, device: &wgpu::Device, window: Window) -> Output {
        let display = Display {
            surface: window.surface,
            configured: None,
            surface_format: None,
            format: None,
            #[cfg(target_vendor = "apple")]
            metal_hud: None,
        };
        Output::with(adapter, device, window.presentation, Some(display), None, window.overlay)
    }

    /// Frames into a texture of the presentation's size, every `shots.0`-th
    /// to a PNG in `shots.1`.
    pub fn offscreen(
        adapter: wgpu::Adapter,
        device: &wgpu::Device,
        presentation: Presentation,
        shots: Option<(u32, std::path::PathBuf)>,
    ) -> Output {
        let offscreen = Offscreen { texture: None, shots, shown: 0 };
        Output::with(adapter, device, presentation, None, Some(offscreen), Default::default())
    }

    fn with(
        adapter: wgpu::Adapter,
        device: &wgpu::Device,
        presentation: Presentation,
        display: Option<Display>,
        offscreen: Option<Offscreen>,
        overlay: crate::Overlay,
    ) -> Output {
        #[cfg(target_vendor = "apple")]
        let metalfx = crate::metalfx::MetalFx::new(device);
        #[cfg_attr(not(target_vendor = "apple"), allow(unused_mut))]
        let mut upscalers =
            1 << Upscaler::Bicubic as u8 | (crate::fsr::Fsr::supported(device) as u8) << Upscaler::Fsr as u8;
        #[cfg(target_vendor = "apple")]
        {
            use crate::metalfx::Kind;
            upscalers |= (metalfx.supports(Kind::Spatial) as u8) << Upscaler::MetalFx as u8;
            upscalers |= (metalfx.supports(Kind::Temporal) as u8) << Upscaler::MetalFxTemporal as u8;
            presentation.0.has_frame_generation.store(metalfx.supports(Kind::Interpolator), Ordering::Relaxed);
        }
        presentation.0.upscalers.store(upscalers, Ordering::Relaxed);
        Output {
            adapter,
            presentation,
            display,
            offscreen,
            passes: Passes::default(),
            overlay: OverlayRenderer::new(overlay),
            fsr: Default::default(),
            last_frame: None,
            frame_interval: Duration::ZERO,
            #[cfg(target_vendor = "apple")]
            metalfx,
        }
    }

    pub fn presentation(&self) -> &Presentation {
        &self.presentation
    }

    /// Whether it shows frames in a window (not offscreen).
    pub fn has_display(&self) -> bool {
        self.display.is_some()
    }

    /// How long a frame may take: at the frame rate set, or 60 a second.
    pub fn frame_budget(&self) -> Duration {
        let fps = self.presentation.0.frame_rate.load(Ordering::Relaxed);
        Duration::from_secs_f64(1.0 / if fps > 0 { fps } else { 60 } as f64)
    }

    /// The next frame to draw into (none while the window has no area or
    /// its surface is being replaced).
    pub fn acquire(&mut self, device: &wgpu::Device) -> Option<Frame> {
        let (w, h) = self.presentation.size();
        let Some(display) = &mut self.display else {
            return self.offscreen.as_mut().and_then(|o| o.acquire(device, (w, h)));
        };
        if let Some(surface) = self.presentation.0.surface.lock().unwrap().take() {
            display.surface = surface;
            display.configured = None;
            log::debug!("gpu: presenting into the window's new surface");
        }
        display.acquire(
            &self.adapter,
            device,
            (w, h, self.presentation.vsync()),
            self.presentation.0.metal_hud.load(Ordering::Relaxed),
        )
    }

    /// Shows `frame`, drawn by the commands submitted before.
    #[cfg(not(target_vendor = "apple"))]
    pub fn show(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, frame: Frame, _generated: bool) {
        match frame.image {
            Image::Surface(surface) => {
                drop(frame.texture);
                queue.present(surface);
            }
            Image::Offscreen => self.show_offscreen(device, queue, &frame),
        }
    }

    /// Shows `frame`, drawn by the commands submitted before: what wgpu's
    /// present does (a command buffer of its own that presents the
    /// drawable, after those on the same queue), paced by the frame rate
    /// (twice as fast with `generated`: frame generation shows two frames for
    /// each of the game's).
    #[cfg(target_vendor = "apple")]
    pub fn show(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, frame: Frame, generated: bool) {
        use objc2_metal::{MTLCommandBuffer, MTLCommandQueue, MTLDrawable};
        let drawable = match &frame.image {
            Image::Drawable(d) => d,
            Image::Offscreen => return self.show_offscreen(device, queue, &frame),
        };
        let drawable: &ProtocolObject<dyn MTLDrawable> = ProtocolObject::from_ref(&**drawable);
        // tvOS has neither presented handlers nor presents after a minimum
        // duration: its frames go out as the display takes them, unmeasured.
        #[cfg(not(target_os = "tvos"))]
        {
            let on_screen = block2::RcBlock::new(|d: std::ptr::NonNull<ProtocolObject<dyn MTLDrawable>>| {
                // 0: skipped (a newer frame replaced it).
                let at = unsafe { d.as_ref() }.presentedTime();
                if at > 0.0 {
                    ON_SCREEN.lock().unwrap().push(at);
                }
            });
            unsafe { drawable.addPresentedHandler(block2::RcBlock::as_ptr(&on_screen)) };
        }
        let Some(queue) = (unsafe { queue.as_hal::<Metal>() }) else { return };
        let frame_time = self.on_screen_for(generated);
        autoreleasepool(|_| {
            let Some(commands) = queue.as_raw().commandBuffer() else { return };
            match frame_time {
                #[cfg(not(target_os = "tvos"))]
                Some(t) => commands.presentDrawable_afterMinimumDuration(drawable, t.as_secs_f64()),
                _ => commands.presentDrawable(drawable),
            }
            commands.commit();
        });
    }

    /// An offscreen frame: every n-th to a PNG (waits for the GPU).
    fn show_offscreen(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, frame: &Frame) {
        let Some(o) = &mut self.offscreen else { return };
        o.shown += 1;
        let Some((every, dir)) = &o.shots else { return };
        if !(o.shown - 1).is_multiple_of(*every as u64) {
            return;
        }
        let path = dir.join(format!("out{:06}.png", o.shown - 1));
        if let Err(e) = save_png(device, queue, &frame.texture, &path) {
            log::warn!("gpu: saving {}: {e:#}", path.display());
        }
    }

    /// The commands that draw `frame`: the back buffer `src` scaled into it
    /// (by the bicubic filter, or as `scaled` by an upscaler, see
    /// [`Output::upscale`]; without one, the background colour), then the
    /// overlay.
    pub fn compose(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        frame: &Frame,
        src: Option<&Present>,
        scaled: Option<&Scaled>,
    ) -> Vec<wgpu::CommandBuffer> {
        let format = frame.format;
        let target =
            frame.texture.create_view(&wgpu::TextureViewDescriptor { format: Some(format), ..Default::default() });
        let size = (frame.texture.width(), frame.texture.height());
        let mut buffers = Vec::new();
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("present") });
        // The application's UI over it, in the same pass (its uploads go
        // before it; a second pass would load and store the whole window
        // again on tiling GPUs).
        let overlay = self.overlay.prepare(device, queue, &mut encoder, format, [size.0, size.1]);
        let mut pass = begin_pass(&mut encoder, &target, if src.is_some() { wgpu::Color::BLACK } else { BACKGROUND })
            .forget_lifetime();
        if let Some(p) = src {
            let rect = fit((p.width, p.height), size);
            match scaled {
                Some(Scaled::Frame(view)) => self.passes.blit(device, &mut pass, view, format, rect),
                Some(Scaled::Scene(view)) => {
                    let scene = p.scene.as_ref().unwrap().color.create_view(&wgpu::TextureViewDescriptor {
                        format: Some(p.view.texture().format().remove_srgb_suffix()),
                        ..Default::default()
                    });
                    self.passes.composite(device, &mut pass, view, p, &scene, format, rect);
                }
                None => self.passes.blit(device, &mut pass, &p.view, format, rect),
            }
        }
        if let Some(overlay) = overlay {
            self.overlay.render(&mut pass, overlay);
        }
        drop(pass);
        buffers.push(encoder.finish());
        buffers
    }

    /// The frame scaled to the window by the upscaler set, when it is not
    /// the bicubic filter and possible now: its commands, and the images to
    /// show (1:1, see [`Output::compose`]).
    pub(crate) fn upscale(&mut self, device: &wgpu::Device, src: &Present) -> Option<Upscaled> {
        let now = Instant::now();
        // The time between the engine's frames (frame generation shows one
        // between each two).
        if let Some(last) = self.last_frame.replace(now) {
            let dt = (now - last).min(Duration::from_millis(250));
            self.frame_interval = if self.frame_interval.is_zero() { dt } else { (self.frame_interval * 7 + dt) / 8 };
        }
        let rect = fit((src.width, src.height), self.presentation.size());
        if rect.2 == 0 || rect.3 == 0 {
            return None;
        }
        let mut size = (rect.2, rect.3);
        if self.presentation.upscaler().is_temporal() {
            // At most what they are made for; the present pass scales the rest.
            size = temporal_size((src.width, src.height), size);
        }
        let mut up = self.upscale_into(device, src, size)?;
        up.commands = timed(device, std::mem::take(&mut up.commands));
        Some(up)
    }

    fn upscale_into(&mut self, device: &wgpu::Device, src: &Present, size: (u32, u32)) -> Option<Upscaled> {
        if self.presentation.upscaler() != Upscaler::Fsr {
            self.fsr.release();
        }
        // Temporal upscalers only scale up (an image a little less tall
        // than the frame still fills the window: see `fit`).
        let up = size.0 >= src.width && size.1 >= src.height;
        match self.presentation.upscaler() {
            Upscaler::Fsr if up => {
                let (sharpness, dt) = (self.presentation.sharpness(), self.frame_interval.as_secs_f32());
                let up = self.fsr.upscale(device, src, size, sharpness, dt);
                // (MetalFX's effects freed.)
                drop(self.metal_upscale(device, src, size, true));
                up
            }
            _ => self.metal_upscale(device, src, size, false),
        }
    }

    #[cfg(target_vendor = "apple")]
    fn metal_upscale(
        &mut self,
        device: &wgpu::Device,
        src: &Present,
        size: (u32, u32),
        none: bool,
    ) -> Option<Upscaled> {
        use crate::metalfx::Kind;
        let kind = match self.presentation.upscaler() {
            _ if none => None,
            // (The spatial scaler only scales up.)
            Upscaler::MetalFx if size.0 > src.width && size.1 > src.height => Some(Kind::Spatial),
            Upscaler::MetalFxTemporal if size.0 >= src.width && size.1 >= src.height => Some(Kind::Temporal),
            _ => None,
        };
        let interpolate = self.presentation.frame_generation();
        let dt = self.frame_interval.as_secs_f32();
        self.metalfx.upscale(device, src, size, kind, interpolate, dt)
    }

    #[cfg(not(target_vendor = "apple"))]
    fn metal_upscale(&mut self, _: &wgpu::Device, _: &Present, _: (u32, u32), _: bool) -> Option<Upscaled> {
        None
    }

    /// How long each frame stays on screen at least: the frame rate set's
    /// share of a second, half of it with frame generation (of the measured
    /// time between frames without a frame rate).
    #[cfg_attr(not(target_vendor = "apple"), allow(dead_code))]
    fn on_screen_for(&self, generated: bool) -> Option<Duration> {
        match (self.presentation.frame_time(), generated) {
            (Some(t), false) => Some(t),
            (Some(t), true) => Some(t / 2),
            (None, false) => None,
            (None, true) => (!self.frame_interval.is_zero()).then(|| self.frame_interval / 2),
        }
    }
}

impl Offscreen {
    fn acquire(&mut self, device: &wgpu::Device, (w, h): (u32, u32)) -> Option<Frame> {
        let format = TextureFormat::Bgra8Unorm;
        if self.texture.as_ref().is_none_or(|t| (t.width(), t.height()) != (w, h)) {
            self.texture = Some(device.create_texture(&wgpu::TextureDescriptor {
                label: Some("offscreen output"),
                size: wgpu::Extent3d { width: w.max(1), height: h.max(1), depth_or_array_layers: 1 },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
                view_formats: &[],
            }));
        }
        Some(Frame { texture: self.texture.clone()?, format, image: Image::Offscreen })
    }
}

/// `texture` (8-bit RGBA or BGRA) to a PNG at `path`, once the GPU has drawn it.
fn save_png(device: &wgpu::Device, queue: &wgpu::Queue, texture: &wgpu::Texture, path: &Path) -> Result<()> {
    let (w, h) = (texture.width(), texture.height());
    let bpr = (w * 4).next_multiple_of(256);
    let buffer = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("png"),
        size: (bpr * h) as u64,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let mut encoder = device.create_command_encoder(&Default::default());
    encoder.copy_texture_to_buffer(
        texture.as_image_copy(),
        wgpu::TexelCopyBufferInfo {
            buffer: &buffer,
            layout: wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(bpr), rows_per_image: Some(h) },
        },
        texture.size(),
    );
    queue.submit([encoder.finish()]);
    buffer.slice(..).map_async(wgpu::MapMode::Read, |_| {});
    device.poll(wgpu::PollType::wait_indefinitely())?;
    let data = buffer.slice(..).get_mapped_range()?;
    write_png(path, &data, w, h, bpr, texture.format())
}

impl Gpu {
    pub fn present(&mut self, id: u32) {
        self.flush_clears();
        self.end_pass();
        let mut present = if self.window { self.present_view(id) } else { None };
        if let Some(p) = &mut present {
            p.scene = self.scene.done.take();
        }
        if self.settings.shot_every > 0
            && (self.stats.presents + self.total_presents).is_multiple_of(self.settings.shot_every as u64)
        {
            let name = format!("frame{:06}.png", self.stats.presents + self.total_presents);
            self.screenshot(id, &name);
        }
        self.flush_with("present", present, true);
        let now = Instant::now();
        if let Some(last) = self.last_present.replace(now) {
            let ms = (now - last).as_secs_f32() * 1000.0;
            self.stats.frame_ms.push(ms);
            if crate::perf::on() {
                let st = &self.stats;
                if self.perf.frame(ms, st.draws, st.passes, st.busy, self.epoch) {
                    crate::perf::gpu_bytes(self.gpu_bytes().into_iter().sum());
                }
            }
        }
        self.uniforms.next_frame();
        // The next frame: drawn jittered, its scene kept, while a temporal
        // upscaler wants them.
        let wanted = self.presentation.as_ref().is_some_and(Presentation::wants_scene);
        let size = self.objs.tex(id).map(|t| (t.width, t.height));
        // How much the upscaler scales: the window's height over the scene's.
        let scale = match (&self.presentation, size) {
            (Some(p), Some((_, h))) => (p.size().1 as f32 / h.max(1) as f32).clamp(1.0, TEMPORAL_SCALE),
            _ => 1.0,
        };
        self.scene.next_frame(wanted, id, size, scale);
        self.stats.presents += 1;
        if self.last_stats.elapsed() >= Duration::from_secs(10) {
            self.total_presents += self.stats.presents;
            self.log_stats();
            // Bind groups unused for 10-20 s, freed off the command thread.
            let stale = self.binds.age();
            self.pool.background(move || drop(stale));
        }
        if self.last_save.elapsed() >= Duration::from_secs(60) {
            self.save_caches();
        }
    }

    /// The back buffer, for the submit thread to scale into the window.
    fn present_view(&mut self, id: u32) -> Option<crate::submit::Present> {
        let epoch = self.epoch;
        let t = self.objs.tex_mut(id)?;
        let view = t.present.get_or_insert_with(|| {
            t.tex.create_view(&wgpu::TextureViewDescriptor {
                // sRGB back buffers hold encoded values: read them raw.
                format: Some(t.format.remove_srgb_suffix()),
                dimension: Some(wgpu::TextureViewDimension::D2),
                mip_level_count: Some(1),
                array_layer_count: Some(1),
                ..Default::default()
            })
        });
        let view = view.clone();
        t.used.set(epoch);
        Some(crate::submit::Present { view, texture: t.tex.clone(), width: t.width, height: t.height, scene: None })
    }

    /// Writes texture `id` to a PNG in the screenshot directory (once the GPU is done).
    pub fn screenshot(&mut self, id: u32, name: &str) {
        let Some(dir) = self.settings.shot_dir.clone() else {
            crate::log_once("noshotdir", || "screenshot requested without a screenshot directory".into());
            return;
        };
        let file = Path::new(name).file_name().map_or_else(|| "shot.png".into(), |f| f.to_owned());
        let path = dir.join(file).with_extension("png");
        let epoch = self.epoch;
        let Some(t) = self.objs.tex(id) else { return };
        if t.samples > 1 || t.format.block_copy_size(None) != Some(4) {
            crate::log_once(&format!("shot{:?}", t.format), || format!("screenshot of {:?} not supported", t.format));
            return;
        }
        let (w, h, format, tex) = (t.width, t.height, t.format, t.tex.clone());
        t.used.set(epoch);
        let bpr = (w * 4).next_multiple_of(256);
        let staging = self.staging.get(&self.device, (bpr * h) as u64);
        self.encoder().copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo {
                texture: &tex,
                mip_level: 0,
                origin: Default::default(),
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyBufferInfo {
                buffer: &staging,
                layout: wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(bpr), rows_per_image: Some(h) },
            },
            wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
        );
        self.maps.push((
            staging,
            Box::new(move |s| Work::Screenshot { staging: s, width: w, height: h, bpr, format, path }),
        ));
    }

    /// Pipeline recipes and the driver's pipeline cache, for the next run.
    pub fn save_caches(&mut self) {
        self.last_save = Instant::now();
        let Some(dir) = self.settings.cache_dir.clone() else {
            return;
        };
        if !self.recipes_new.is_empty() {
            use std::io::Write;
            let lines: String = self.recipes_new.drain(..).map(|r| r.to_json().to_string() + "\n").collect();
            let path = dir.join("pipelines.jsonl");
            let result = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(&path)
                .and_then(|mut f| f.write_all(lines.as_bytes()));
            if let Err(e) = result {
                log::warn!("gpu: saving {}: {e}", path.display());
            }
        }
        let compiled = self.compiler.compiled.load(std::sync::atomic::Ordering::Relaxed);
        if compiled != self.saved_compiled {
            // Off the command thread: the driver's cache is megabytes to get
            // and write (the ring stood still meanwhile).
            let (compiler, adapter) = (self.compiler.clone(), self.adapter.clone());
            self.pool.background(move || crate::save_pipeline_cache(&compiler, &adapter, &dir));
            self.saved_compiled = compiled;
        }
    }
}

/// An 8-bit RGBA/BGRA image to PNG.
pub fn write_png(path: &Path, data: &[u8], width: u32, height: u32, bpr: u32, format: TextureFormat) -> Result<()> {
    let bgra = match format.remove_srgb_suffix() {
        TextureFormat::Bgra8Unorm => true,
        TextureFormat::Rgba8Unorm => false,
        other => bail!("{other:?} is not an 8-bit colour format"),
    };
    let mut rgba = Vec::with_capacity((width * height * 4) as usize);
    for row in 0..height {
        let line = &data[(row * bpr) as usize..(row * bpr + width * 4) as usize];
        for px in line.as_chunks::<4>().0 {
            let (r, b) = if bgra { (px[2], px[0]) } else { (px[0], px[2]) };
            rgba.extend_from_slice(&[r, px[1], b, 255]);
        }
    }
    let file = std::io::BufWriter::new(std::fs::File::create(path)?);
    let mut encoder = png::Encoder::new(file, width, height);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    encoder.write_header()?.write_image_data(&rgba)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    #[test]
    fn temporal_sizes() {
        assert_eq!(super::temporal_size((1280, 720), (2560, 1440)), (2560, 1440));
        assert_eq!(super::temporal_size((640, 360), (2560, 1440)), (1920, 1080));
        assert_eq!(super::temporal_size((1280, 720), (1280, 720)), (1280, 720));
    }

    fn image_parameter_functions(src: &str) -> Vec<String> {
        let module = naga::front::wgsl::parse_str(src).expect("WGSL parses");
        module
            .functions
            .iter()
            .filter(|(_, function)| {
                function.arguments.iter().any(|argument| {
                    matches!(
                        &module.types[argument.ty].inner,
                        naga::TypeInner::Image { .. } | naga::TypeInner::Sampler { .. }
                    )
                })
            })
            .map(|(_, function)| function.name.clone().unwrap_or_default())
            .collect()
    }

    #[test]
    fn image_parameter_detector_is_not_vacuous() {
        let source = "@group(0) @binding(0) var t: texture_2d<f32>;
            fn bad(x: texture_2d<f32>) -> f32 { return 0.0; }
            @fragment fn main() -> @location(0) vec4f { return vec4f(0.0); }";
        assert_eq!(image_parameter_functions(source), vec!["bad".to_owned()]);
    }

    #[test]
    fn present_shaders_have_no_image_parameter_functions() {
        let shaders = [
            (
                "present",
                format!(
                    "{}{}{}",
                    super::COMMON_WGSL,
                    super::bicubic_functions(&[("bicubic_src", "src")]),
                    super::PRESENT_WGSL
                ),
                vec!["bicubic_src"],
            ),
            (
                "composite",
                format!(
                    "{}{}{}",
                    super::COMMON_WGSL,
                    super::bicubic_functions(&[
                        ("bicubic_scene_up", "scene_up"),
                        ("bicubic_frame", "frame"),
                        ("bicubic_scene", "scene"),
                    ]),
                    super::COMPOSITE_WGSL
                ),
                vec!["bicubic_scene_up", "bicubic_frame", "bicubic_scene"],
            ),
        ];
        for (name, source, bicubic_helpers) in shaders {
            let module = naga::front::wgsl::parse_str(&source).unwrap_or_else(|e| panic!("{name}: {e}"));
            assert!(image_parameter_functions(&source).is_empty(), "{name}: image/sampler parameter function");
            for helper in bicubic_helpers {
                let function = module
                    .functions
                    .iter()
                    .find(|(_, function)| function.name.as_deref() == Some(helper))
                    .map(|(_, function)| function)
                    .unwrap_or_else(|| panic!("{name}: missing bicubic helper {helper}"));
                assert_eq!(function.arguments.len(), 1, "{name}: {helper} must take only UV");
                match &module.types[function.arguments[0].ty].inner {
                    naga::TypeInner::Vector { size, scalar } => {
                        assert_eq!(*size, naga::VectorSize::Bi, "{name}: {helper} UV must be vec2f");
                        assert_eq!(scalar.kind, naga::ScalarKind::Float, "{name}: {helper} UV must be vec2f");
                        assert_eq!(scalar.width, 4, "{name}: {helper} UV must be vec2f");
                    }
                    other => panic!("{name}: {helper} UV must be vec2f, got {other:?}"),
                }
            }
            naga::valid::Validator::new(naga::valid::ValidationFlags::all(), naga::valid::Capabilities::all())
                .validate(&module)
                .unwrap_or_else(|e| panic!("{name}: {e}"));
        }
    }
}
