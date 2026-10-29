//! The sprites host code draws over the game (glue5-runtime's `sprites`,
//! the mods' images, shapes and text): drawn into the window's image after
//! the game's frame is scaled to it, at its resolution, before the
//! application's UI.
//!
//! epaint (egui's painting crate, without an egui frame) turns them into
//! triangles, anti-aliased by a pixel of feathering: shapes and text go into
//! one mesh with the font's texture (its white pixel for the shapes), an
//! image into the same mesh as the images around it with its texture, so
//! draw calls are cut only where the texture changes in the drawing order.
//! The meshes, the scratch paths and the GPU buffers are kept from frame to
//! frame and grow only; a list is laid out again only when a new one comes
//! (or the window's size changes). Images are uploaded the first time they
//! are drawn and freed when nothing holds them.

use std::collections::HashMap;
use std::sync::{Arc, Weak};

use egui::epaint::text::{FontData, FontDefinitions, FontFamily, LayoutJob};
use egui::epaint::tessellator::{Path, path};
use egui::epaint::{
    ClippedPrimitive, Color32, CornerRadiusF32, FontId, Fonts, Galley, Mesh, PathStroke, Primitive,
    TessellationOptions, Tessellator, TextOptions, TextShape, TextureId, Vertex,
};
use egui::emath::{Align2, Pos2, Rect, Rot2, Vec2, pos2, vec2};
use glue5_runtime::sprites::{self, Align, Font, Image, List, NONE, Rgba, Shape, Stroke};

/// Time laying lists out (and recording their uploads), and how many,
/// for the statistics.
static LAYOUT_NS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
static LAYOUTS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// The lists laid out since the last call, and the milliseconds that took.
pub(crate) fn take_stats() -> (u64, f64) {
    use std::sync::atomic::Ordering::Relaxed;
    (LAYOUTS.swap(0, Relaxed), LAYOUT_NS.swap(0, Relaxed) as f64 / 1e6)
}

/// The font atlas's side at most (and its texture's).
const ATLAS: usize = 2048;
/// The font atlas's texture (as egui's).
const FONT_TEXTURE: TextureId = TextureId::Managed(0);

#[derive(Default)]
pub(crate) struct Sprites {
    /// The list laid out (its generation), the images dropped then, the size.
    seen: Option<(u64, u64, [u32; 2])>,
    renderer: Option<(wgpu::TextureFormat, egui_wgpu::Renderer)>,
    /// The images' textures, by image id.
    textures: HashMap<u64, Texture>,
    /// The list laid out (its images stay while it is drawn).
    list: Option<Arc<List>>,
    layout: Layout,
}

struct Texture {
    image: Weak<Image>,
    view: wgpu::TextureView,
    id: TextureId,
}

/// Where the game's image is in the window (x, y, width, height, pixels),
/// and its camera (view-projection, row vectors), if known.
pub(crate) type Game = ((u32, u32, u32, u32), Option<[[f32; 4]; 4]>);

/// Where a point of the world is on screen (pixels), and how far it is
/// from the camera (m); none when behind it.
fn project(game: &Game, p: [f32; 3]) -> Option<(Pos2, f32)> {
    let ((x, y, w, h), Some(m)) = *game else { return None };
    let clip = |j: usize| p[0] * m[0][j] + p[1] * m[1][j] + p[2] * m[2][j] + m[3][j];
    let depth = clip(3);
    // Behind, or closer than the camera's near plane.
    if depth < 0.1 {
        return None;
    }
    let (nx, ny) = (clip(0) / depth, clip(1) / depth);
    Some((pos2(x as f32 + (nx + 1.0) / 2.0 * w as f32, y as f32 + (1.0 - ny) / 2.0 * h as f32), depth))
}

/// Where a group is: what is drawn in it at `p` (pixels in its own
/// coordinates) is at `offset + rot * p * scale` on screen, cut to `clip`.
#[derive(Clone, Copy)]
struct Place {
    offset: Vec2,
    rot: Rot2,
    angle: f32,
    scale: f32,
    clip: Rect,
    hidden: bool,
}

impl Place {
    fn at(&self, p: Pos2) -> Pos2 {
        (self.offset + self.rot * (p.to_vec2() * self.scale)).to_pos2()
    }
}

/// What laying out keeps between frames.
#[derive(Default)]
struct Layout {
    fonts: Option<Fonts>,
    /// The batches drawn, and meshes kept for the next lists.
    primitives: Vec<ClippedPrimitive>,
    meshes: Vec<Mesh>,
    path: Path,
    points: Vec<Pos2>,
    /// Where each group of the list is.
    places: Vec<Place>,
    /// Laid-out texts, by their text (held, so its address is not reused)
    /// and how it is laid out; whether the last list used them.
    galleys: HashMap<TextKey, (Arc<str>, Arc<Galley>, bool)>,
    /// The mods' fonts in `fonts` (by id), and the fonts dropped then.
    registered: Vec<(u64, Weak<Font>)>,
    fonts_dropped: u64,
}

impl Sprites {
    /// Lays out the latest list when it changed, and records the uploads
    /// into `encoder`: whether there is something to [`Sprites::render`].
    pub fn prepare(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        format: wgpu::TextureFormat,
        size: [u32; 2],
        game: Game,
    ) -> bool {
        sprites::set_screen(size[0], size[1]);
        let generation = sprites::generation();
        if self.seen.is_none() && generation == 0 {
            // Nothing was ever drawn.
            return false;
        }
        if self.renderer.as_ref().is_none_or(|(f, _)| *f != format) {
            let mut renderer = egui_wgpu::Renderer::new(device, format, Default::default());
            // The images again, in this renderer; the font's atlas whole.
            for t in self.textures.values_mut() {
                t.id = register(&mut renderer, device, &t.view);
            }
            self.renderer = Some((format, renderer));
            self.layout.fonts = None;
            self.layout.galleys.clear();
            self.seen = None;
        }
        // Sprites in the world move with the camera: every frame.
        let world = game.1.is_some() && sprites::in_world();
        let relayout = world || self.seen.is_none_or(|(g, _, s)| g != generation || s != size);
        if relayout {
            let started = std::time::Instant::now();
            self.list = sprites::latest();
            let list = self.list.clone();
            let mut layout = std::mem::take(&mut self.layout);
            layout.begin(list.as_deref());
            if let Some(list) = &list {
                layout.list(list, size, game, |image| self.texture(device, queue, image));
            }
            let renderer = &mut self.renderer.as_mut().unwrap().1;
            if let Some(delta) = layout.fonts.as_mut().and_then(Fonts::font_image_delta) {
                renderer.update_texture(device, queue, FONT_TEXTURE, &delta);
            }
            if !layout.primitives.is_empty() {
                let screen = egui_wgpu::ScreenDescriptor { size_in_pixels: size, pixels_per_point: 1.0 };
                renderer.update_buffers(device, queue, encoder, &layout.primitives, &screen);
            }
            self.layout = layout;
            LAYOUT_NS.fetch_add(started.elapsed().as_nanos() as u64, std::sync::atomic::Ordering::Relaxed);
            LAYOUTS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        }
        // Images nobody holds (the list laid out holds its own): their
        // textures go.
        let dropped = sprites::dropped();
        if self.seen.is_none_or(|(_, d, _)| d != dropped) {
            let renderer = &mut self.renderer.as_mut().unwrap().1;
            self.textures.retain(|_, t| {
                let alive = t.image.strong_count() > 0;
                if !alive {
                    renderer.free_texture(&t.id);
                }
                alive
            });
        }
        self.seen = Some((generation, dropped, size));
        !self.layout.primitives.is_empty()
    }

    /// Draws what [`Sprites::prepare`] laid out into `pass`.
    pub fn render(&mut self, pass: &mut wgpu::RenderPass<'static>, size: [u32; 2]) {
        let renderer = &mut self.renderer.as_mut().unwrap().1;
        let screen = egui_wgpu::ScreenDescriptor { size_in_pixels: size, pixels_per_point: 1.0 };
        renderer.render(pass, &self.layout.primitives, &screen);
    }

    /// `image`'s texture, uploaded now if it is not yet.
    fn texture(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, image: &Arc<Image>) -> Option<TextureId> {
        if let Some(t) = self.textures.get(&image.id()) {
            return Some(t.id);
        }
        let pixels = image.take_pixels()?;
        let (width, height) = image.size();
        // What egui's renderer takes: sRGB values read as they are.
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("sprite"),
            size: wgpu::Extent3d { width, height, depth_or_array_layers: 1 },
            mip_level_count: image.levels(),
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        let (mut w, mut h, mut at) = (width, height, 0);
        for level in 0..image.levels() {
            let len = 4 * w as usize * h as usize;
            queue.write_texture(
                wgpu::TexelCopyTextureInfo { texture: &texture, mip_level: level, origin: wgpu::Origin3d::ZERO, aspect: wgpu::TextureAspect::All },
                &pixels[at..at + len],
                wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(4 * w), rows_per_image: Some(h) },
                wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
            );
            at += len;
            (w, h) = ((w / 2).max(1), (h / 2).max(1));
        }
        let view = texture.create_view(&Default::default());
        let id = register(&mut self.renderer.as_mut()?.1, device, &view);
        self.textures.insert(image.id(), Texture { image: Arc::downgrade(image), view, id });
        Some(id)
    }
}

/// An image's texture in egui's renderer, sampled from its levels.
fn register(renderer: &mut egui_wgpu::Renderer, device: &wgpu::Device, view: &wgpu::TextureView) -> TextureId {
    renderer.register_native_texture_with_sampler_options(
        device,
        view,
        wgpu::SamplerDescriptor {
            label: Some("sprite"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::MipmapFilterMode::Linear,
            ..Default::default()
        },
    )
}

/// A colour as the renderer blends it: premultiplied, and with no alpha
/// when `add`ed. The blend is "one, one minus the source's alpha", so a
/// colour without alpha adds to what is under it: additive drawings go in
/// the same batches as the others, no pipeline of their own to switch to.
fn color(c: Rgba, add: bool) -> Color32 {
    let c = Color32::from_rgba_unmultiplied(c[0], c[1], c[2], c[3]);
    if add { additive(c) } else { c }
}

fn additive(c: Color32) -> Color32 {
    Color32::from_rgba_premultiplied(c.r(), c.g(), c.b(), 0)
}

fn stroke(s: &Stroke, h: f32, add: bool) -> PathStroke {
    PathStroke::new((s.width * h).max(0.0), color(s.color, add))
}

impl Layout {
    /// A new list: the batches emptied (their memory kept), the font's
    /// texts not used since the last list dropped.
    fn begin(&mut self, list: Option<&List>) {
        for p in self.primitives.drain(..) {
            if let Primitive::Mesh(mut mesh) = p.primitive {
                mesh.clear();
                self.meshes.push(mesh);
            }
        }
        self.galleys.retain(|_, (_, _, used)| std::mem::take(used));
        // The fonts the list uses, all in one atlas: made again with the
        // ones a list brings, without those dropped.
        let mut new: Vec<Arc<Font>> = Vec::new();
        for sprite in list.map_or(&[][..], |l| &l.sprites[..]) {
            if let Shape::Text { font: Some(f), .. } = &sprite.shape
                && !self.registered.iter().any(|(id, _)| *id == f.id())
                && !new.iter().any(|n| n.id() == f.id())
            {
                new.push(f.clone());
            }
        }
        let dropped = sprites::fonts_dropped();
        let dead = dropped != self.fonts_dropped && self.registered.iter().any(|(_, f)| f.strong_count() == 0);
        self.fonts_dropped = dropped;
        if !new.is_empty() || dead {
            let mut fonts: Vec<Arc<Font>> = self.registered.iter().filter_map(|(_, f)| f.upgrade()).collect();
            fonts.extend(new);
            self.registered = fonts.iter().map(|f| (f.id(), Arc::downgrade(f))).collect();
            self.fonts = Some(Fonts::new(TextOptions { max_texture_side: ATLAS, ..Default::default() }, definitions(&fonts)));
            self.galleys.clear();
        }
        let fonts = self.fonts.get_or_insert_with(|| {
            Fonts::new(TextOptions { max_texture_side: ATLAS, ..Default::default() }, FontDefinitions::default())
        });
        let atlas = fonts.texture_atlas().fill_ratio();
        fonts.begin_pass(TextOptions { max_texture_side: ATLAS, ..Default::default() });
        // A full atlas was made again: the texts with it.
        if fonts.texture_atlas().fill_ratio() < atlas {
            self.galleys.clear();
        }
    }

    /// The mesh to add to for `texture` (the last batch, or a new one).
    fn batch(&mut self, texture: TextureId, clip: Rect) -> &mut Mesh {
        let same = self.primitives.last().is_some_and(|p| {
            p.clip_rect == clip && matches!(&p.primitive, Primitive::Mesh(m) if m.texture_id == texture)
        });
        if !same {
            let mut mesh = self.meshes.pop().unwrap_or_default();
            mesh.texture_id = texture;
            self.primitives.push(ClippedPrimitive { clip_rect: clip, primitive: Primitive::Mesh(mesh) });
        }
        match &mut self.primitives.last_mut().unwrap().primitive {
            Primitive::Mesh(mesh) => mesh,
            Primitive::Callback(_) => unreachable!("only meshes"),
        }
    }

    /// The list as batches, in pixels; `texture` gives an image's.
    fn list(&mut self, list: &List, size: [u32; 2], game: Game, mut texture: impl FnMut(&Arc<Image>) -> Option<TextureId>) {
        let (w, h) = (size[0] as f32, size[1] as f32);
        let screen = Rect::from_min_size(Pos2::ZERO, vec2(w, h));
        let px = |p: [f32; 2]| pos2(p[0] * w, p[1] * h);
        let root = Place { offset: Vec2::ZERO, rot: Rot2::IDENTITY, angle: 0.0, scale: 1.0, clip: screen, hidden: false };
        // The groups', parents before children.
        self.places.clear();
        for g in &list.groups {
            let parent = if g.parent == NONE { root } else { self.places.get(g.parent as usize).copied().unwrap_or(root) };
            let angle = parent.angle + g.rotation.to_radians();
            let mut place = Place {
                offset: parent.at(px(g.origin)).to_vec2(),
                rot: Rot2::from_angle(angle),
                angle,
                scale: parent.scale * g.scale,
                ..parent
            };
            if let Some(a) = g.anchor {
                // At the point, as this frame's camera sees it; culled
                // before anything in it is laid out.
                let seen = project(&game, a.position).filter(|(_, d)| a.max_distance <= 0.0 || *d <= a.max_distance);
                let angle = g.rotation.to_radians();
                place = match seen {
                    Some((at, distance)) => Place {
                        offset: at.to_vec2(),
                        rot: Rot2::from_angle(angle),
                        angle,
                        scale: g.scale * if a.scaled_at > 0.0 { (a.scaled_at / distance).clamp(0.05, 20.0) } else { 1.0 },
                        clip: screen,
                        hidden: false,
                    },
                    None => Place { hidden: true, ..root },
                };
            }
            if let Some(c) = g.clip {
                let half = vec2(c[2] * w, c[3] * h) / 2.0;
                let centre = px([c[0], c[1]]);
                let corners = [vec2(-1.0, -1.0), vec2(1.0, -1.0), vec2(1.0, 1.0), vec2(-1.0, 1.0)]
                    .map(|d| place.at(centre + vec2(d.x * half.x, d.y * half.y)));
                place.clip = place.clip.intersect(Rect::from_points(&corners));
                place.hidden |= !place.clip.is_positive();
            }
            self.places.push(place);
        }
        let place_of = |places: &[Place], group: u32| if group == NONE { Some(root) } else { places.get(group as usize).copied() };
        let mut fonts = self.fonts.take().expect("fonts made by begin");
        // The texts first: their glyphs may grow the atlas, whose size the
        // tessellator needs.
        for sprite in &list.sprites {
            if let Shape::Text { text, size, align, wrap, font, .. } = &sprite.shape {
                let Some(place) = place_of(&self.places, sprite.group).filter(|p| !p.hidden) else { continue };
                let (px, k) = text_size(size * h * place.scale, h, place.scale);
                let key = TextKey::new(text, px, *align, wrap * w * place.scale / k, font.as_ref());
                if let Some((_, _, used)) = self.galleys.get_mut(&key) {
                    *used = true;
                    continue;
                }
                let galley = fonts.with_pixels_per_point(1.0).layout_job(job(text, &key));
                self.galleys.insert(key, (text.clone(), galley, true));
            }
        }
        let mut tess = Tessellator::new(
            1.0,
            TessellationOptions::default(),
            fonts.font_image_size(),
            fonts.texture_atlas().prepared_discs(),
        );
        let feathering = 1.0;
        for sprite in &list.sprites {
            let add = sprite.additive;
            let Some(place) = place_of(&self.places, sprite.group).filter(|p| !p.hidden) else { continue };
            // The group's: its clip, its lengths (fractions of the screen's
            // height, scaled), positions in it, its turn.
            let screen = place.clip;
            let k = h * place.scale;
            let at = |p: [f32; 2]| place.at(px(p));
            let turn = place.angle;
            match &sprite.shape {
                Shape::Image { image, center, size, rotation, tint, uv, nine } => {
                    let (iw, ih) = image.size();
                    // What is drawn of it, in its pixels.
                    let (pw, ph) = ((uv[2] - uv[0]).abs() * iw as f32, (uv[3] - uv[1]).abs() * ih as f32);
                    let width = size[0] * w * place.scale;
                    let height = if size[1] > 0.0 { size[1] * k } else { width * ph / pw.max(1e-3) };
                    let c = at(*center);
                    let half = vec2(width, height) / 2.0;
                    if !visible(screen, c, half.length()) {
                        continue;
                    }
                    let Some(id) = texture(image) else { continue };
                    let rot = Rot2::from_angle(rotation.to_radians() + turn);
                    let mesh = self.batch(id, screen);
                    match nine {
                        None => quad(mesh, c, rot, [-half.x, -half.y, half.x, half.y], *uv, color(*tint, add)),
                        Some(b) => {
                            // Borders on screen as on a 1080-pixel-high one,
                            // smaller when the drawing is.
                            let k = (k / 1080.0).min(half.x * 2.0 / (b[0] + b[2]).max(1e-3)).min(half.y * 2.0 / (b[1] + b[3]).max(1e-3));
                            let xs = [-half.x, -half.x + b[0] * k, half.x - b[2] * k, half.x];
                            let ys = [-half.y, -half.y + b[1] * k, half.y - b[3] * k, half.y];
                            // The borders in the texture, inward from each side.
                            let (du, dv) = ((uv[2] - uv[0]).signum() / iw as f32, (uv[3] - uv[1]).signum() / ih as f32);
                            let us = [uv[0], uv[0] + b[0] * du, uv[2] - b[2] * du, uv[2]];
                            let vs = [uv[1], uv[1] + b[1] * dv, uv[3] - b[3] * dv, uv[3]];
                            for j in 0..3 {
                                for i in 0..3 {
                                    let rect = [xs[i], ys[j], xs[i + 1], ys[j + 1]];
                                    quad(mesh, c, rot, rect, [us[i], vs[j], us[i + 1], vs[j + 1]], color(*tint, add));
                                }
                            }
                        }
                    }
                }
                Shape::Rect { center, size, rotation, fill, gradient, rounding, stroke: line } => {
                    let c = at(*center);
                    let half = vec2(size[0] * w, size[1] * h) * place.scale / 2.0;
                    if !visible(screen, c, half.length() + line.map_or(0.0, |s| s.width * k)) {
                        continue;
                    }
                    let rot = Rot2::from_angle(rotation.to_radians() + turn);
                    let r = (rounding * k).min(half.x.min(half.y)).max(0.0);
                    path::rounded_rectangle(&mut self.points, Rect::from_center_size(Pos2::ZERO, half * 2.0), CornerRadiusF32::same(r));
                    for p in &mut self.points {
                        *p = c + rot * p.to_vec2();
                    }
                    let (points, path) = (std::mem::take(&mut self.points), std::mem::take(&mut self.path));
                    let (mut points, mut path) = (points, path);
                    let mesh = self.batch(FONT_TEXTURE, screen);
                    path.clear();
                    path.add_line_loop(&points);
                    let first = mesh.vertices.len();
                    match gradient {
                        None => path.fill(feathering, color(*fill, add), mesh),
                        Some((to, horizontal)) => {
                            path.fill(feathering, Color32::WHITE, mesh);
                            // Each vertex's colour by where it is along the
                            // gradient, its coverage (the feathered edge) kept.
                            let (from, to) = (egui::Rgba::from(color(*fill, false)), egui::Rgba::from(color(*to, false)));
                            let axis = if *horizontal { rot * vec2(1.0, 0.0) } else { rot * vec2(0.0, 1.0) };
                            let extent = if *horizontal { half.x } else { half.y };
                            for v in &mut mesh.vertices[first..] {
                                let t = ((v.pos - c).dot(axis) / (2.0 * extent.max(1e-3)) + 0.5).clamp(0.0, 1.0);
                                let coverage = v.color.a() as f32 / 255.0;
                                let c = Color32::from(from * (1.0 - t) + to * t).gamma_multiply(coverage);
                                v.color = if add { additive(c) } else { c };
                            }
                        }
                    }
                    if let Some(s) = line {
                        path.clear();
                        path.add_line_loop(&points);
                        path.stroke_closed(feathering, &stroke(s, k, add), mesh);
                    }
                    points.clear();
                    (self.points, self.path) = (points, path);
                }
                Shape::Text { text, at: from, size, rotation, align, color: c, wrap, font } => {
                    let (px, stretch) = text_size(size * k, h, place.scale);
                    let key = TextKey::new(text, px, *align, wrap * w * place.scale / stretch, font.as_ref());
                    let Some((_, galley, _)) = self.galleys.get(&key) else { continue };
                    let galley = galley.clone();
                    let anchor = match align {
                        Align::Left => Align2::LEFT_TOP,
                        Align::Center => Align2::CENTER_TOP,
                        Align::Right => Align2::RIGHT_TOP,
                    };
                    let p = at(*from);
                    if !visible(screen, p, galley.rect.size().length() + galley.rect.min.to_vec2().length()) {
                        continue;
                    }
                    let shape = TextShape::new(p - anchor.pos_in_rect(&galley.rect).to_vec2(), galley, color(*c, add))
                        .with_override_text_color(color(*c, add))
                        .with_angle_and_anchor(rotation.to_radians() + turn, anchor);
                    let mesh = self.batch(FONT_TEXTURE, screen);
                    let first = mesh.vertices.len();
                    tess.tessellate_text(&shape, mesh);
                    if stretch != 1.0 {
                        for v in &mut mesh.vertices[first..] {
                            v.pos = p + (v.pos - p) * stretch;
                        }
                    }
                }
                Shape::Line { from, to, stroke: s } => {
                    let (a, b) = (at(*from), at(*to));
                    if !visible(screen, a + (b - a) / 2.0, (b - a).length() / 2.0 + s.width * k) {
                        continue;
                    }
                    let mut path = std::mem::take(&mut self.path);
                    path.clear();
                    path.add_line_segment([a, b]);
                    path.stroke_open(feathering, &stroke(s, k, add), self.batch(FONT_TEXTURE, screen));
                    self.path = path;
                }
                Shape::Path { points, closed, fill, stroke: line } => {
                    let range = points.start as usize..points.end as usize;
                    let Some(src) = list.points.get(range) else { continue };
                    if src.len() < 2 {
                        continue;
                    }
                    let (mut pts, mut path) = (std::mem::take(&mut self.points), std::mem::take(&mut self.path));
                    pts.clear();
                    pts.extend(src.iter().map(|p| at(*p)));
                    let bounds = Rect::from_points(&pts);
                    let margin = line.map_or(0.0, |s| s.width * k);
                    if bounds.expand(margin).intersects(screen) {
                        let line = line.map(|s| stroke(&s, k, add)).unwrap_or(PathStroke::NONE);
                        let mesh = self.batch(FONT_TEXTURE, screen);
                        path.clear();
                        match (closed, fill) {
                            (true, Some(f)) => {
                                path.add_line_loop(&pts);
                                path.fill_and_stroke(feathering, color(*f, add), &line, mesh);
                            }
                            (true, None) => {
                                path.add_line_loop(&pts);
                                path.stroke_closed(feathering, &line, mesh);
                            }
                            (false, _) => {
                                path.add_open_points(&pts);
                                path.stroke_open(feathering, &line, mesh);
                            }
                        }
                    }
                    (self.points, self.path) = (pts, path);
                }
                Shape::Circle { center, radius, fill, stroke: line } => {
                    let (c, r) = (at(*center), radius * k);
                    if r <= 0.0 || !visible(screen, c, r + line.map_or(0.0, |s| s.width * k)) {
                        continue;
                    }
                    let line = line.map(|s| stroke(&s, k, add)).unwrap_or(PathStroke::NONE);
                    let mut path = std::mem::take(&mut self.path);
                    path.clear();
                    path.add_circle(c, r);
                    let fill = fill.map_or(Color32::TRANSPARENT, |f| color(f, add));
                    path.fill_and_stroke(feathering, fill, &line, self.batch(FONT_TEXTURE, screen));
                    self.path = path;
                }
                Shape::Arc { center, radius, from, to, stroke: s } => {
                    let (c, r) = (at(*center), radius * k);
                    if r <= 0.0 || !visible(screen, c, r + s.width * k) {
                        continue;
                    }
                    let (mut pts, mut path) = (std::mem::take(&mut self.points), std::mem::take(&mut self.path));
                    pts.clear();
                    let sweep = (to - from).clamp(-360.0, 360.0);
                    // A segment every 3 pixels or so of the arc.
                    let n = ((sweep.abs().to_radians() * r / 3.0).ceil() as usize).clamp(2, 512);
                    pts.extend((0..=n).map(|i| {
                        let a = (from + sweep * i as f32 / n as f32).to_radians() + turn;
                        c + vec2(a.sin(), -a.cos()) * r
                    }));
                    path.clear();
                    path.add_open_points(&pts);
                    path.stroke_open(feathering, &stroke(s, k, add), self.batch(FONT_TEXTURE, screen));
                    (self.points, self.path) = (pts, path);
                }
            }
        }
        self.fonts = Some(fonts);
    }
}

/// The size a text is laid out at (pixels), and how much its mesh is then
/// stretched to be `px` high. In a scaled group (one in the world, whose
/// size follows its distance) the size changes from frame to frame: it is
/// laid out at a step of a ladder (2^¼ apart) and stretched by the rest,
/// so a label coming closer is laid out and rasterized again only every
/// 19 %, not every frame. Elsewhere at its size, as crisp as it gets.
fn text_size(px: f32, h: f32, scale: f32) -> (f32, f32) {
    let px = px.clamp(1.0, h.max(1.0));
    if scale == 1.0 {
        return (px, 1.0);
    }
    let step = (4.0 * px.log2()).round() / 4.0;
    let laid = step.exp2().max(1.0);
    (laid, px / laid)
}

/// A text laid out: its string (by address), size and wrap in pixels,
/// alignment and font.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
struct TextKey {
    text: usize,
    px: u32,
    wrap: u32,
    align: u8,
    font: u64,
}

impl TextKey {
    fn new(text: &Arc<str>, px: f32, align: Align, wrap: f32, font: Option<&Arc<Font>>) -> TextKey {
        let wrap = if wrap > 0.0 { wrap.max(1.0) } else { f32::INFINITY };
        TextKey { text: text.as_ptr() as usize, px: px.to_bits(), wrap: wrap.to_bits(), align: align as u8, font: font.map_or(0, |f| f.id()) }
    }
}

/// How a text is laid out (the mods measure them the same way: draw.rs).
fn job(text: &str, key: &TextKey) -> LayoutJob {
    let family = if key.font == 0 { FontFamily::Proportional } else { FontFamily::Name(family(key.font).into()) };
    let mut job = LayoutJob::simple(text.to_string(), FontId::new(f32::from_bits(key.px), family), Color32::WHITE, f32::from_bits(key.wrap));
    job.halign = match key.align {
        0 => egui::Align::LEFT,
        1 => egui::Align::Center,
        _ => egui::Align::RIGHT,
    };
    job
}

/// A mod's font's family.
fn family(font: u64) -> String {
    format!("mod-font-{font}")
}

/// The app's fonts and `fonts`, each a family of its own with the app's as
/// fallbacks (glyphs it lacks).
fn definitions(fonts: &[Arc<Font>]) -> FontDefinitions {
    let mut defs = FontDefinitions::default();
    let fallbacks = defs.families.get(&FontFamily::Proportional).cloned().unwrap_or_default();
    for f in fonts {
        let name = family(f.id());
        defs.font_data.insert(name.clone(), Arc::new(FontData::from_owned(f.data().to_vec())));
        defs.families.insert(FontFamily::Name(name.clone().into()), std::iter::once(name).chain(fallbacks.iter().cloned()).collect());
    }
    defs
}

/// Whether something within `radius` of `center` may show on `screen`.
fn visible(screen: Rect, center: Pos2, radius: f32) -> bool {
    screen.expand(radius + 1.0).contains(center)
}

/// A textured quad: `rect` (x0, y0, x1, y1) around `center`, turned by
/// `rot` around it; `uv` its part of the texture (u0, v0, u1, v1).
fn quad(mesh: &mut Mesh, center: Pos2, rot: Rot2, rect: [f32; 4], uv: [f32; 4], color: Color32) {
    let base = mesh.vertices.len() as u32;
    for (x, y, u, v) in [(rect[0], rect[1], uv[0], uv[1]), (rect[2], rect[1], uv[2], uv[1]), (rect[2], rect[3], uv[2], uv[3]), (rect[0], rect[3], uv[0], uv[3])] {
        let pos = center + rot * vec2(x, y);
        mesh.vertices.push(Vertex { pos, uv: pos2(u, v), color });
    }
    mesh.indices.extend_from_slice(&[base, base + 1, base + 2, base, base + 2, base + 3]);
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A point ahead of the camera lands where its projection says, in the
    /// game's part of the window; one behind it is not drawn.
    /// Texts in scaled groups are laid out at a few sizes and stretched to
    /// theirs; elsewhere at theirs.
    #[test]
    fn text_sizes() {
        assert_eq!(text_size(20.0, 1080.0, 1.0), (20.0, 1.0));
        // 100 sizes from 20 to 30 pixels: four layouts.
        let sizes: std::collections::HashSet<u32> = (0..100).map(|i| text_size(20.0 + i as f32 * 0.1, 1080.0, 1.5).0.to_bits()).collect();
        assert_eq!(sizes.len(), 4, "{sizes:?}");
        for px in [3.0, 17.5, 40.0, 200.0] {
            let (laid, stretch) = text_size(px, 1080.0, 0.7);
            assert!((laid * stretch - px).abs() < 1e-3 && (0.9..1.1).contains(&stretch), "{px}: {laid} {stretch}");
        }
    }

    #[test]
    fn projection() {
        // Looking along +y from the origin: x right, z up, w the distance.
        let camera = [[1.0, 0.0, 0.0, 0.0], [0.0, 0.0, 0.0, 1.0], [0.0, 1.0, 0.0, 0.0], [0.0, 0.0, 0.0, 0.0]];
        let game: Game = ((100, 0, 800, 600), Some(camera));
        let (at, distance) = project(&game, [0.0, 10.0, 0.0]).unwrap();
        assert_eq!((at, distance), (pos2(500.0, 300.0), 10.0));
        let (at, _) = project(&game, [5.0, 10.0, 5.0]).unwrap();
        assert_eq!(at, pos2(700.0, 150.0));
        assert!(project(&game, [0.0, -10.0, 0.0]).is_none());
        assert!(project(&((0, 0, 800, 600), None), [0.0, 10.0, 0.0]).is_none());
    }
}

#[cfg(test)]
mod cost {
    use std::time::Instant;

    use glue5_runtime::sprites::{Anchor, Group};

    use super::*;

    /// The stress list (as the stress mod's): 5,000 shapes, 500 texts, 300
    /// sprites from a sheet, 100 labels in the world.
    fn stress() -> List {
        let mut list = List::default();
        let image = Image::new(256, 64, vec![200; 256 * 64 * 4]);
        let names: Vec<Arc<str>> = (0..50).map(|i| format!("Label {i}").into()).collect();
        let numbers: Vec<Arc<str>> = (0..50).map(|i| format!("{}", i * 7).into()).collect();
        let at = |i: usize| [(i as f32 * 0.618_034) % 1.0, (i as f32 * 0.414_213_5) % 1.0];
        let stroke = Stroke { color: [255; 4], width: 0.001 };
        let mut push = |shape| list.sprites.push(sprites::Sprite { shape, additive: false, group: NONE });
        for i in 1..=2000 {
            push(Shape::Rect { center: at(i), size: [0.01, 0.01], rotation: 0.0, fill: [48, 128, 255, 128], gradient: None, rounding: if i % 2 == 0 { 0.004 } else { 0.0 }, stroke: None });
        }
        for i in 2001..=3500 {
            push(Shape::Circle { center: at(i), radius: 0.006, fill: Some([255, 128, 48, 128]), stroke: (i % 3 == 0).then_some(stroke) });
        }
        for i in 3501..=4500 {
            let a = at(i);
            push(Shape::Line { from: a, to: [a[0] + 0.02, a[1] + 0.01], stroke });
        }
        for i in 4501..=5000 {
            push(Shape::Arc { center: at(i), radius: 0.01, from: -90.0, to: 90.0 + (i % 180) as f32, stroke: Stroke { width: 0.002, ..stroke } });
        }
        for i in 1..=500 {
            let text = if i <= 450 { names[i % 50].clone() } else { numbers[i % 50].clone() };
            push(Shape::Text { text, at: at(i), size: 0.015, rotation: 0.0, align: Align::Left, color: [255; 4], wrap: 0.0, font: None });
        }
        for i in 1..=300 {
            let f = (i % 4) as f32 / 4.0;
            push(Shape::Image { image: image.clone(), center: at(i + 300), size: [0.03, 0.0], rotation: 0.0, tint: [255; 4], uv: [f, 0.0, f + 0.25, 1.0], nine: None });
        }
        for i in 0..100 {
            let a = i as f32 * 0.7;
            list.groups.push(Group {
                parent: NONE,
                origin: [0.0, 0.0],
                scale: 1.0,
                rotation: 0.0,
                clip: None,
                anchor: Some(Anchor { position: [a.cos() * 20.0, 30.0 + a.sin() * 20.0, 1.0], scaled_at: 20.0, max_distance: 300.0 }),
            });
            list.sprites.push(sprites::Sprite {
                shape: Shape::Text { text: names[i % 50].clone(), at: [0.0, 0.0], size: 0.02, rotation: 0.0, align: Align::Center, color: [255; 4], wrap: 0.0, font: None },
                additive: false,
                group: i as u32,
            });
        }
        list
    }

    /// What the stress list costs at 1920×1080: laying it out (CPU, every
    /// frame, as with labels in the world), and drawing it (GPU: a frame's
    /// pass with it, less the same pass without). Run with
    /// `cargo test -r -p glue5-gpu -- --ignored sprites_cost --nocapture`.
    #[test]
    #[ignore]
    fn sprites_cost() {
        let adapter = pollster::block_on(crate::instance().request_adapter(&Default::default())).unwrap();
        let (device, queue) = pollster::block_on(adapter.request_device(&Default::default())).unwrap();
        let size = [1920, 1080];
        let format = wgpu::TextureFormat::Bgra8Unorm;
        let target = device.create_texture(&wgpu::TextureDescriptor {
            label: None,
            size: wgpu::Extent3d { width: size[0], height: size[1], depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            view_formats: &[],
        });
        let view = target.create_view(&Default::default());
        sprites::publish(stress());
        // A camera looking north from the south of the labels.
        let camera = [[1.0, 0.0, 0.0, 0.0], [0.0, 0.0, -0.1, 1.0], [0.0, 1.7, 0.0, 0.0], [0.0, -1.7, 0.1, 0.0]];
        let game = ((0, 0, size[0], size[1]), Some(camera));
        let mut sprites = Sprites::default();
        let frame = |sprites: &mut Sprites, draw: bool| {
            let mut encoder = device.create_command_encoder(&Default::default());
            let t = Instant::now();
            let shown = draw && sprites.prepare(&device, &queue, &mut encoder, format, size, game);
            let cpu = t.elapsed();
            let mut pass = crate::present::begin_pass(&mut encoder, &view, wgpu::Color::BLACK).forget_lifetime();
            if shown {
                sprites.render(&mut pass, size);
            }
            drop(pass);
            let t = Instant::now();
            queue.submit([encoder.finish()]);
            device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
            (cpu, t.elapsed())
        };
        for _ in 0..10 {
            frame(&mut sprites, true);
        }
        let n = 100;
        let (mut cpu, mut with, mut without) = (0.0, 0.0, 0.0);
        for _ in 0..n {
            let (c, g) = frame(&mut sprites, true);
            cpu += c.as_secs_f64();
            with += g.as_secs_f64();
            without += frame(&mut sprites, false).1.as_secs_f64();
        }
        let ms = |s: f64| s * 1000.0 / n as f64;
        let batches = sprites.layout.primitives.len();
        let (vertices, indices) = sprites.layout.primitives.iter().fold((0, 0), |(v, i), p| match &p.primitive {
            Primitive::Mesh(m) => (v + m.vertices.len(), i + m.indices.len()),
            Primitive::Callback(_) => (v, i),
        });
        println!(
            "sprites: layout + uploads {:.2} ms CPU a frame; pass {:.2} ms (without {:.2}): {:.2} ms GPU; {batches} draw calls, {vertices} vertices, {} triangles",
            ms(cpu),
            ms(with),
            ms(without),
            ms(with - without),
            indices / 3
        );
    }
}
