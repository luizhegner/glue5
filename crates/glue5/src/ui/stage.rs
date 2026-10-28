//! The shared stage behind Glue 5's pages: a procedural night sky, animated
//! hairline geometry, and optional transparent, parallax scene layers.
//!
//! Each mode has a curated, layered default scene; users can override individual
//! modes with `backgrounds/scenes.json` in app data. See `assets/ui/README.md`.
use std::{
    collections::{HashMap, HashSet},
    fs,
    io::Cursor,
    path::{Component, Path, PathBuf},
    sync::mpsc::{self, Receiver, Sender},
    thread,
    time::Duration,
};

use egui::{Color32, Context, LayerId, Mesh, Rect, Stroke, TextureHandle, TextureOptions, pos2};
use serde::Deserialize;

use super::{Mode, theme};

const FADE: Duration = Duration::from_millis(360);
const MAX_LAYER_BYTES: u64 = 32 * 1024 * 1024;
const MAX_LAYERS: usize = 6;
const MAX_TEXTURE_EDGE: u32 = 2048;
const MAX_SCENE_BYTES: usize = 48 * 1024 * 1024;
const MAX_SOURCE_PIXELS: usize = 16_000_000;

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct Manifest {
    scenes: HashMap<String, SceneSpec>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct SceneSpec {
    layers: Vec<LayerSpec>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
enum Fit {
    #[default]
    Cover,
    /// Preserve the full image and fit it to a fraction of screen height.
    Contain,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct LayerSpec {
    file: String,
    /// 0 is a distant skyline; 1 is the closest foreground layer.
    #[serde(default)]
    depth: f32,
    #[serde(default = "opaque")]
    opacity: f32,
    #[serde(default)]
    fit: Fit,
    /// Used by contain layers, relative to viewport height.
    #[serde(default = "unit")]
    scale: f32,
    /// Contain-layer center, as a fraction of the viewport.
    #[serde(default = "half")]
    x: f32,
    #[serde(default = "half")]
    y: f32,
}

fn unit() -> f32 {
    1.0
}

fn half() -> f32 {
    0.5
}

fn opaque() -> f32 {
    1.0
}

#[derive(Debug)]
struct DecodedLayer {
    width: usize,
    height: usize,
    rgba: Vec<u8>,
    depth: f32,
    opacity: f32,
    fit: Fit,
    scale: f32,
    x: f32,
    y: f32,
}

type LoadResult = (Mode, Result<Vec<DecodedLayer>, String>);

struct LoadedLayer {
    texture: TextureHandle,
    depth: f32,
    opacity: f32,
    fit: Fit,
    scale: f32,
    x: f32,
    y: f32,
}

struct LoadedScene {
    mode: Mode,
    layers: Vec<LoadedLayer>,
}

pub(super) struct Stage {
    manifest: Manifest,
    requests: Option<Sender<(Mode, Vec<LayerSpec>)>>,
    results: Receiver<LoadResult>,
    loading: Option<Mode>,
    active: Option<LoadedScene>,
    previous: Option<LoadedScene>,
    fade_started: Option<f64>,
    reported_bad: HashSet<Mode>,
}

impl Stage {
    pub(super) fn new(user_dir: &Path) -> Self {
        let directory = user_dir.join("backgrounds");
        let mut manifest = serde_json::from_str::<Manifest>(include_str!("../../assets/ui/scenes/scenes.json"))
            .expect("the bundled scenes manifest is valid");
        match fs::read(directory.join("scenes.json")) {
            Ok(bytes) => match serde_json::from_slice::<Manifest>(&bytes) {
                Ok(custom) => manifest.scenes.extend(custom.scenes),
                Err(error) => log::warn!("background scenes.json is invalid; retaining bundled scenes: {error}"),
            },
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => log::warn!("cannot read background scenes.json; retaining bundled scenes: {error}"),
        }
        let (result_tx, results) = mpsc::channel();
        let (request_tx, request_rx) = mpsc::channel::<(Mode, Vec<LayerSpec>)>();
        let worker_directory = directory.clone();
        let worker = thread::Builder::new().name("glue5-scenes".into()).spawn(move || {
            while let Ok((mode, layers)) = request_rx.recv() {
                let result = load_layers(&worker_directory, &layers).map_err(|error| error.to_string());
                if result_tx.send((mode, result)).is_err() {
                    break;
                }
            }
        });
        let requests = match worker {
            Ok(_) => Some(request_tx),
            Err(error) => {
                log::warn!("cannot start background-scene loader: {error}");
                None
            }
        };
        Self {
            manifest,
            requests,
            results,
            loading: None,
            active: None,
            previous: None,
            fade_started: None,
            reported_bad: HashSet::new(),
        }
    }

    /// Draw the stage over the whole viewport, independently of safe-area
    /// insets. Scene artwork is only shown on Home; utility pages stay calm.
    pub(super) fn draw(&mut self, ctx: &Context, mode: Mode, reduced_motion: bool, show_scene: bool) {
        if reduced_motion {
            self.previous = None;
            self.fade_started = None;
        }
        self.receive(ctx, mode, reduced_motion);
        if show_scene {
            self.request(mode);
        }
        let now = ctx.input(|input| input.time);
        procedural(ctx, mode, reduced_motion, now, show_scene);

        if show_scene {
            if let Some(previous) = &self.previous {
                let progress = self.fade_started.map_or(1.0, |at| ((now - at) / FADE.as_secs_f64()).clamp(0.0, 1.0)) as f32;
                draw_scene(ctx, previous, now, 1.0 - progress, reduced_motion);
            }
            if let Some(active) = self.active.as_ref().filter(|scene| scene.mode == mode) {
                let progress = self.fade_started.map_or(1.0, |at| ((now - at) / FADE.as_secs_f64()).clamp(0.0, 1.0)) as f32;
                draw_scene(ctx, active, now, progress, reduced_motion);
            }
            ctx.layer_painter(LayerId::background()).rect_filled(ctx.viewport_rect(), 0, Color32::from_black_alpha(52));
        }
        self.finish_fade(ctx, now);
        if show_scene && !reduced_motion {
            ctx.request_repaint_after(Duration::from_millis(33));
        }
    }

    fn request(&mut self, mode: Mode) {
        if self.reported_bad.contains(&mode)
            || self.active.as_ref().is_some_and(|scene| scene.mode == mode)
            || self.loading.is_some()
        {
            return;
        }
        let key = mode_key(mode);
        let Some(scene) = self.manifest.scenes.get(key) else { return };
        if scene.layers.is_empty() || scene.layers.len() > MAX_LAYERS {
            self.warn_once(mode, "scene must contain between one and six layers");
            return;
        }
        for layer in &scene.layers {
            let path = Path::new(&layer.file);
            let safe = !path.as_os_str().is_empty()
                && path.components().all(|part| matches!(part, Component::Normal(_)))
                && path.extension().and_then(|ext| ext.to_str()).is_some_and(|ext| ext.eq_ignore_ascii_case("png"));
            if !safe
                || !layer.depth.is_finite() || !(0.0..=1.0).contains(&layer.depth)
                || !layer.opacity.is_finite() || !(0.0..=1.0).contains(&layer.opacity)
                || !layer.scale.is_finite() || !(0.05..=2.0).contains(&layer.scale)
                || !layer.x.is_finite() || !(0.0..=1.0).contains(&layer.x)
                || !layer.y.is_finite() || !(0.0..=1.0).contains(&layer.y)
            {
                self.warn_once(mode, "scene contains an unsafe path or invalid depth/opacity");
                return;
            }
        }
        if let Some(tx) = &self.requests {
            match tx.send((mode, scene.layers.clone())) {
                Ok(()) => self.loading = Some(mode),
                Err(error) => self.warn_once(mode, &format!("cannot queue scene load: {error}")),
            }
        }
    }

    fn receive(&mut self, ctx: &Context, selected_mode: Mode, reduced_motion: bool) {
        while let Ok((mode, result)) = self.results.try_recv() {
            self.loading = None;
            if mode != selected_mode {
                continue;
            }
            match result {
                Ok(layers) => {
                    let loaded = LoadedScene {
                        mode,
                        layers: layers
                            .into_iter()
                            .enumerate()
                            .map(|(index, layer)| LoadedLayer {
                                texture: ctx.load_texture(
                                    format!("scene-{}-{index}", mode_key(mode)),
                                    egui::ColorImage::from_rgba_unmultiplied([layer.width, layer.height], &layer.rgba),
                                    TextureOptions::LINEAR,
                                ),
                                depth: layer.depth,
                                opacity: layer.opacity,
                                fit: layer.fit,
                                scale: layer.scale,
                                x: layer.x,
                                y: layer.y,
                            })
                            .collect(),
                    };
                    if !reduced_motion && self.active.as_ref().is_some_and(|scene| scene.mode != mode) {
                        self.previous = self.active.take();
                    } else {
                        self.previous = None;
                    }
                    self.active = Some(loaded);
                    self.fade_started = (!reduced_motion).then(|| ctx.input(|input| input.time));
                    ctx.request_repaint();
                }
                Err(error) => self.warn_once(mode, &error),
            }
        }
    }

    fn finish_fade(&mut self, ctx: &Context, now: f64) {
        if let Some(at) = self.fade_started {
            let elapsed = (now - at).max(0.0);
            if elapsed < FADE.as_secs_f64() {
                ctx.request_repaint_after(Duration::from_millis(33));
            } else {
                self.previous = None;
                self.fade_started = None;
            }
        }
    }

    fn warn_once(&mut self, mode: Mode, message: &str) {
        if self.reported_bad.insert(mode) {
            log::warn!("background scene '{}': {message}; using the procedural stage", mode_key(mode));
        }
    }

    /// A game takes over the renderer: drop scene textures so their GPU memory
    /// is promptly available to the game.
    pub(super) fn release(&mut self) {
        self.active = None;
        self.previous = None;
        self.fade_started = None;
        self.discard_pending();
    }

    /// Decoding may have finished just as the game took over; drop those CPU
    /// buffers without uploading them to the UI renderer.
    pub(super) fn discard_pending(&mut self) {
        while self.results.try_recv().is_ok() {
            self.loading = None;
        }
    }
}

fn mode_key(mode: Mode) -> &'static str {
    match mode {
        Mode::Story => "story",
        Mode::Online => "online",
        Mode::Sandbox => "sandbox",
        Mode::Benchmark => "benchmark",
    }
}

fn embedded_file(name: &str) -> Option<&'static [u8]> {
    Some(match name {
        "builtin/ls1_background.png" => include_bytes!("../../assets/ui/scenes/ls1_background.png").as_slice(),
        "builtin/ls1_foreground.png" => include_bytes!("../../assets/ui/scenes/ls1_foreground.png").as_slice(),
        "builtin/ls5_background.png" => include_bytes!("../../assets/ui/scenes/ls5_background.png").as_slice(),
        "builtin/ls5_foreground.png" => include_bytes!("../../assets/ui/scenes/ls5_foreground.png").as_slice(),
        "builtin/ls8_background.png" => include_bytes!("../../assets/ui/scenes/ls8_background.png").as_slice(),
        "builtin/ls8_foreground.png" => include_bytes!("../../assets/ui/scenes/ls8_foreground.png").as_slice(),
        "builtin/ls12_background.png" => include_bytes!("../../assets/ui/scenes/ls12_background.png").as_slice(),
        _ => return None,
    })
}

fn load_layers(directory: &Path, specs: &[LayerSpec]) -> Result<Vec<DecodedLayer>, String> {
    let root = fs::canonicalize(directory).ok();
    let mut decoded = Vec::with_capacity(specs.len());
    let mut total_bytes = 0usize;
    for spec in specs {
        let path = PathBuf::from(&spec.file);
        if path.components().any(|part| !matches!(part, Component::Normal(_))) {
            return Err(format!("unsafe layer filename {:?}", spec.file));
        }
        // Built-in scenes are embedded; custom layers are confined to the
        // user's backgrounds directory even when a filename is a symlink.
        let bytes = if let Some(bytes) = embedded_file(&spec.file) {
            bytes.to_vec()
        } else {
            let root = root.as_ref().ok_or_else(|| format!("cannot open backgrounds directory for {}", spec.file))?;
            let full = fs::canonicalize(root.join(&spec.file)).map_err(|error| format!("cannot read {}: {error}", spec.file))?;
            if !full.starts_with(root) {
                return Err(format!("{} resolves outside the backgrounds directory", spec.file));
            }
            let size = fs::metadata(&full).map_err(|error| format!("cannot inspect {}: {error}", spec.file))?.len();
            if size > MAX_LAYER_BYTES {
                return Err(format!("{} exceeds the 32 MiB layer limit", spec.file));
            }
            fs::read(&full).map_err(|error| format!("cannot read {}: {error}", spec.file))?
        };
        if bytes.len() as u64 > MAX_LAYER_BYTES {
            return Err(format!("{} exceeds the 32 MiB layer limit", spec.file));
        }
        let mut decoder = png::Decoder::new(Cursor::new(bytes));
        decoder.set_transformations(png::Transformations::EXPAND | png::Transformations::STRIP_16);
        let mut reader = decoder.read_info().map_err(|error| format!("{}: {error}", spec.file))?;
        let (width, height) = (reader.info().width as usize, reader.info().height as usize);
        if width == 0 || height == 0 || width.saturating_mul(height) > MAX_SOURCE_PIXELS {
            return Err(format!("{} has invalid or excessive dimensions", spec.file));
        }
        let mut source = vec![0; reader.output_buffer_size().ok_or_else(|| format!("{} is too large", spec.file))?];
        let info = reader.next_frame(&mut source).map_err(|error| format!("{}: {error}", spec.file))?;
        let source = &source[..info.buffer_size()];
        let rgba = match info.color_type {
            png::ColorType::Rgba => source.to_vec(),
            png::ColorType::Rgb => {
                let mut out = Vec::with_capacity(width * height * 4);
                for pixel in source.chunks_exact(3) {
                    out.extend_from_slice(&[pixel[0], pixel[1], pixel[2], 255]);
                }
                out
            }
            png::ColorType::Grayscale => {
                let mut out = Vec::with_capacity(width * height * 4);
                for &gray in source {
                    out.extend_from_slice(&[gray, gray, gray, 255]);
                }
                out
            }
            png::ColorType::GrayscaleAlpha => {
                let mut out = Vec::with_capacity(width * height * 4);
                for pixel in source.chunks_exact(2) {
                    out.extend_from_slice(&[pixel[0], pixel[0], pixel[0], pixel[1]]);
                }
                out
            }
            _ => return Err(format!("{} is not a supported PNG color type", spec.file)),
        };
        let (width, height, rgba) = resize(rgba, width, height, MAX_TEXTURE_EDGE as usize);
        total_bytes = total_bytes.saturating_add(rgba.len());
        if total_bytes > MAX_SCENE_BYTES {
            return Err("scene exceeds the 48 MiB decoded texture budget".into());
        }
        decoded.push(DecodedLayer {
            width, height, rgba,
            depth: spec.depth,
            opacity: spec.opacity,
            fit: spec.fit,
            scale: spec.scale,
            x: spec.x,
            y: spec.y,
        });
    }
    Ok(decoded)
}

/// High-quality bilinear downscale. It runs on the scene worker, never on the
/// render/UI thread, and retains alpha so layered cutouts stay clean.
fn resize(src: Vec<u8>, width: usize, height: usize, max_edge: usize) -> (usize, usize, Vec<u8>) {
    let factor = (max_edge as f64 / width.max(height) as f64).min(1.0);
    let (out_w, out_h) = ((width as f64 * factor).round().max(1.0) as usize, (height as f64 * factor).round().max(1.0) as usize);
    if out_w == width && out_h == height {
        return (width, height, src);
    }
    let mut out = vec![0; out_w * out_h * 4];
    for y in 0..out_h {
        let sy = ((y as f64 + 0.5) * height as f64 / out_h as f64 - 0.5).clamp(0.0, (height - 1) as f64);
        let y0 = sy.floor() as usize;
        let y1 = (y0 + 1).min(height - 1);
        let fy = (sy - y0 as f64) as f32;
        for x in 0..out_w {
            let sx = ((x as f64 + 0.5) * width as f64 / out_w as f64 - 0.5).clamp(0.0, (width - 1) as f64);
            let x0 = sx.floor() as usize;
            let x1 = (x0 + 1).min(width - 1);
            let fx = (sx - x0 as f64) as f32;
            for channel in 0..4 {
                let at = |px: usize, py: usize| src[(py * width + px) * 4 + channel] as f32;
                let top = at(x0, y0) * (1.0 - fx) + at(x1, y0) * fx;
                let bottom = at(x0, y1) * (1.0 - fx) + at(x1, y1) * fx;
                out[(y * out_w + x) * 4 + channel] = (top * (1.0 - fy) + bottom * fy).round() as u8;
            }
        }
    }
    (out_w, out_h, out)
}

fn draw_scene(ctx: &Context, scene: &LoadedScene, time: f64, opacity: f32, reduced_motion: bool) {
    if opacity <= 0.0 { return; }
    let rect = ctx.viewport_rect();
    let painter = ctx.layer_painter(LayerId::background());
    let aspect = rect.width() / rect.height().max(1.0);
    for layer in &scene.layers {
        if layer.fit == Fit::Contain && aspect < 1.1 {
            continue;
        }
        let size = layer.texture.size_vec2();
        let image_aspect = size.x / size.y.max(1.0);
        let (destination, uv) = match layer.fit {
            Fit::Cover => {
                let uv = if aspect > image_aspect {
                    let h = image_aspect / aspect;
                    Rect::from_min_max(pos2(0.0, (1.0 - h) * 0.5), pos2(1.0, (1.0 + h) * 0.5))
                } else {
                    let w = aspect / image_aspect;
                    Rect::from_min_max(pos2((1.0 - w) * 0.5, 0.0), pos2((1.0 + w) * 0.5, 1.0))
                };
                (rect, uv)
            }
            Fit::Contain => {
                let height = rect.height() * layer.scale;
                let width = height * image_aspect;
                let center = pos2(rect.left() + rect.width() * layer.x, rect.top() + rect.height() * layer.y);
                (Rect::from_center_size(center, egui::vec2(width, height)), Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)))
            }
        };
        let drift = if reduced_motion { 0.0 } else { ((time * 0.12 + layer.depth as f64).sin() as f32) * 14.0 * layer.depth };
        let shifted = match layer.fit {
            Fit::Cover => destination.translate(egui::vec2(drift, 0.0)).expand(18.0 * layer.depth),
            Fit::Contain => destination.translate(egui::vec2(drift, 0.0)),
        };
        painter.image(layer.texture.id(), shifted, uv, Color32::from_white_alpha((255.0 * opacity * layer.opacity).clamp(0.0, 255.0) as u8));
    }
}

fn procedural(ctx: &Context, mode: Mode, reduced_motion: bool, time: f64, home: bool) {
    use theme::palette::*;
    let rect = ctx.viewport_rect();
    let mut painter = ctx.layer_painter(LayerId::background());
    painter.set_clip_rect(rect);
    let accent = match mode {
        Mode::Story => SUN,
        Mode::Online => SEA,
        Mode::Sandbox => AMBER,
        Mode::Benchmark => RED,
    };
    let top = Color32::from_rgb(9, 12, 17);
    let bottom = Color32::from_rgb(19, 24, 31);
    let mut gradient = Mesh::default();
    gradient.colored_vertex(rect.left_top(), top);
    gradient.colored_vertex(rect.right_top(), top);
    gradient.colored_vertex(rect.right_bottom(), bottom);
    gradient.colored_vertex(rect.left_bottom(), bottom);
    gradient.add_triangle(0, 1, 2);
    gradient.add_triangle(0, 2, 3);
    painter.add(gradient);

    // Soft, low-contrast bands create depth without pretending to be game art.
    let glow_center = pos2(rect.right() * 0.78, rect.top() + rect.height() * 0.28);
    painter.circle_filled(glow_center, rect.width().min(rect.height()) * 0.48, Color32::from_rgba_unmultiplied(accent.r(), accent.g(), accent.b(), if home { 7 } else { 3 }));

    let shift = if reduced_motion { 0.0 } else { (time as f32 * 0.12).sin() * rect.width() * 0.012 };
    let line_alpha = if home { 16 } else { 8 };
    let line = Color32::from_rgba_unmultiplied(211, 218, 225, line_alpha);
    let step = (rect.width() * 0.035).clamp(28.0, 58.0);
    let mut x = rect.left() - rect.height() + shift;
    while x < rect.right() + rect.height() {
        painter.line_segment([pos2(x, rect.bottom()), pos2(x + rect.height() * 0.82, rect.top())], Stroke::new(0.7, line));
        x += step;
    }
    let arc = Color32::from_rgba_unmultiplied(accent.r(), accent.g(), accent.b(), if home { 27 } else { 12 });
    let center = pos2(rect.right() + rect.width() * 0.08, rect.top() + rect.height() * 0.08);
    painter.circle_stroke(center, rect.height() * 0.69, Stroke::new(1.0, arc));
    if !home {
        // Utility pages share the night-sky system but not the mode artwork.
        painter.rect_filled(rect, 0, Color32::from_black_alpha(44));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resizer_preserves_rgba_and_caps_dimensions() {
        let src = vec![255; 100 * 50 * 4];
        let (w, h, scaled) = resize(src, 100, 50, 40);
        assert_eq!((w, h), (40, 20));
        assert!(scaled.chunks_exact(4).all(|px| px == [255, 255, 255, 255]));
    }

    #[test]
    fn mode_keys_are_stable_manifest_names() {
        assert_eq!(mode_key(Mode::Story), "story");
        assert_eq!(mode_key(Mode::Online), "online");
        assert_eq!(mode_key(Mode::Sandbox), "sandbox");
        assert_eq!(mode_key(Mode::Benchmark), "benchmark");
    }

    #[test]
    fn bundled_manifest_has_a_layered_scene_for_each_mode() {
        let manifest: Manifest = serde_json::from_str(include_str!("../../assets/ui/scenes/scenes.json")).unwrap();
        for mode in [Mode::Story, Mode::Online, Mode::Sandbox, Mode::Benchmark] {
            let scene = manifest.scenes.get(mode_key(mode)).expect("scene for each mode");
            assert!(!scene.layers.is_empty());
            for layer in &scene.layers {
                assert!(embedded_file(&layer.file).is_some(), "missing embedded layer: {}", layer.file);
            }
        }
        assert_eq!(manifest.scenes["story"].layers[1].fit, Fit::Contain);
    }
}
