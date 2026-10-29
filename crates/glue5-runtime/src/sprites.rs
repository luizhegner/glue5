//! What host code draws over the game's frames (mods' images, rectangles
//! and text), handed to the renderer a frame at a time. Positions are
//! fractions of the screen; the renderer draws the latest list into the
//! window's image after the game's frame is scaled to it (upscaled and
//! generated frames too), under the application's own UI.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use crate::Locked;

/// An image to draw: RGBA, 8 bits a channel, sRGB values with the alpha
/// premultiplied, with its smaller levels (mipmaps: an image drawn smaller
/// than it is reads one of them, sharper and with less memory traffic).
/// Its pixels go to the GPU the first time it is drawn and are dropped
/// here then.
pub struct Image {
    id: u64,
    width: u32,
    height: u32,
    levels: u32,
    /// The levels one after the other, the largest first.
    pixels: Mutex<Option<Vec<u8>>>,
}

impl Image {
    /// `pixels`: `width` × `height` × 4 bytes. The smaller levels are made
    /// here (a 2×2 average each), on the caller's thread.
    pub fn new(width: u32, height: u32, mut pixels: Vec<u8>) -> Arc<Image> {
        assert_eq!(pixels.len(), width as usize * height as usize * 4, "an image's pixels");
        let levels = 32 - width.max(height).max(1).leading_zeros();
        pixels.reserve_exact(pixels.len() / 3 + 4 * levels as usize);
        let (mut w, mut h, mut at) = (width as usize, height as usize, 0);
        for _ in 1..levels {
            let (nw, nh) = ((w / 2).max(1), (h / 2).max(1));
            for y in 0..nh {
                for x in 0..nw {
                    let px = |dx: usize, dy: usize| at + 4 * ((2 * y + dy).min(h - 1) * w + (2 * x + dx).min(w - 1));
                    let (a, b, c, d) = (px(0, 0), px(1, 0), px(0, 1), px(1, 1));
                    for i in 0..4 {
                        let sum = u32::from(pixels[a + i]) + u32::from(pixels[b + i]) + u32::from(pixels[c + i]) + u32::from(pixels[d + i]);
                        pixels.push(((sum + 2) / 4) as u8);
                    }
                }
            }
            at += 4 * w * h;
            (w, h) = (nw, nh);
        }
        static NEXT: AtomicU64 = AtomicU64::new(1);
        let id = NEXT.fetch_add(1, Ordering::Relaxed);
        Arc::new(Image { id, width, height, levels, pixels: Mutex::new(Some(pixels)) })
    }

    /// Its levels, the full size first.
    pub fn levels(&self) -> u32 {
        self.levels
    }

    /// What an image of this size takes, its levels included.
    pub fn bytes_for(width: u32, height: u32) -> usize {
        let (mut w, mut h, mut bytes) = (width as usize, height as usize, 0);
        loop {
            bytes += 4 * w * h;
            if w == 1 && h == 1 {
                return bytes;
            }
            (w, h) = ((w / 2).max(1), (h / 2).max(1));
        }
    }

    /// Unique for the process's life.
    pub fn id(&self) -> u64 {
        self.id
    }

    pub fn size(&self) -> (u32, u32) {
        (self.width, self.height)
    }

    /// What it takes on the GPU (and here until then), its levels included.
    pub fn bytes(&self) -> usize {
        Image::bytes_for(self.width, self.height)
    }

    /// The pixels, for the upload (none after the first call).
    pub fn take_pixels(&self) -> Option<Vec<u8>> {
        self.pixels.locked().take()
    }
}

/// A font (TrueType or OpenType) texts may use.
pub struct Font {
    id: u64,
    data: Arc<[u8]>,
}

impl Font {
    /// `data`: a font file, checked by the caller (its tables read).
    pub fn new(data: Vec<u8>) -> Arc<Font> {
        static NEXT: AtomicU64 = AtomicU64::new(1);
        Arc::new(Font { id: NEXT.fetch_add(1, Ordering::Relaxed), data: data.into() })
    }

    /// Unique for the process's life.
    pub fn id(&self) -> u64 {
        self.id
    }

    pub fn data(&self) -> &Arc<[u8]> {
        &self.data
    }
}

impl Drop for Font {
    fn drop(&mut self) {
        FONTS_DROPPED.fetch_add(1, Ordering::Release);
    }
}

/// Changes whenever a font is dropped (the renderer's atlas can let it go).
pub fn fonts_dropped() -> u64 {
    FONTS_DROPPED.load(Ordering::Acquire)
}

static FONTS_DROPPED: AtomicU64 = AtomicU64::new(0);

impl Drop for Image {
    fn drop(&mut self) {
        // The renderer frees its texture.
        DROPPED.fetch_add(1, Ordering::Release);
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Align {
    Left,
    Center,
    Right,
}

/// RGBA, not premultiplied.
pub type Rgba = [u8; 4];

/// A line along an outline, or one drawn by itself.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Stroke {
    pub color: Rgba,
    /// A fraction of the screen's height.
    pub width: f32,
}

/// What is drawn. Positions are fractions of the screen (0, 0 its top
/// left, 1, 1 its bottom right); lengths that are not along x or y (radii,
/// line widths, rounding, text sizes) fractions of its height. Rotations
/// are degrees, clockwise.
#[derive(Clone)]
pub enum Shape {
    /// `size` wide and high (a height of 0 keeps the proportions of what is
    /// drawn of it), its pixels multiplied by `tint`. `uv` is the part of it
    /// drawn (u0, v0, u1, v1, fractions of its size: a sprite sheet's frame);
    /// `nine` slices it nine ways (left, top, right, bottom borders, in its
    /// pixels): the corners keep their size, as on a 1080-pixel-high screen,
    /// the edges and the middle stretch.
    Image { image: Arc<Image>, center: [f32; 2], size: [f32; 2], rotation: f32, tint: Rgba, uv: [f32; 4], nine: Option<[f32; 4]> },
    /// Filled with `fill`, or from `fill` to `gradient.0` top to bottom (left
    /// to right when `gradient.1`).
    Rect {
        center: [f32; 2],
        size: [f32; 2],
        rotation: f32,
        fill: Rgba,
        gradient: Option<(Rgba, bool)>,
        rounding: f32,
        stroke: Option<Stroke>,
    },
    /// From `at` (its top; which end of it is there, `align` says), `size`
    /// high; lines broken to fit `wrap` (a fraction of the screen's width;
    /// 0: only at line breaks); in `font` (none: the app's).
    Text { text: Arc<str>, at: [f32; 2], size: f32, rotation: f32, align: Align, color: Rgba, wrap: f32, font: Option<Arc<Font>> },
    Line { from: [f32; 2], to: [f32; 2], stroke: Stroke },
    /// The list's `points[points]`; a closed one may be filled (convex).
    Path { points: std::ops::Range<u32>, closed: bool, fill: Option<Rgba>, stroke: Option<Stroke> },
    Circle { center: [f32; 2], radius: f32, fill: Option<Rgba>, stroke: Option<Stroke> },
    /// From `from` to `to` degrees, clockwise from the top.
    Arc { center: [f32; 2], radius: f32, from: f32, to: f32, stroke: Stroke },
}

#[derive(Clone)]
pub struct Sprite {
    pub shape: Shape,
    /// Added to what is under it (light, glows) rather than blended over.
    pub additive: bool,
    /// The list's `groups[group]` it is in ([`NONE`]: none).
    pub group: u32,
}

/// No group.
pub const NONE: u32 = u32::MAX;

/// What is drawn in a group is placed in its coordinates: moved to its
/// origin, scaled and turned around it, cut to its clip.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Group {
    /// The group it is in ([`NONE`]: none); always an earlier one.
    pub parent: u32,
    /// Where its 0, 0 is, in its parent's coordinates.
    pub origin: [f32; 2],
    pub scale: f32,
    pub rotation: f32,
    /// What shows of it: centre x, y, width, height in its coordinates (on
    /// screen, the box around them when it is turned).
    pub clip: Option<[f32; 4]>,
    /// Its 0, 0 is this point of the world, where the frame shown sees it
    /// (`origin` and the groups around it then do not place it).
    pub anchor: Option<Anchor>,
}

/// A point of the world a group follows on screen.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Anchor {
    pub position: [f32; 3],
    /// When not 0: the group is scaled by this distance (m) over the
    /// camera's (larger nearer, smaller farther).
    pub scaled_at: f32,
    /// When not 0: farther than this (m), it is not drawn.
    pub max_distance: f32,
}

/// What is drawn in a frame, in drawing order. Kept and reused: a list the
/// renderer is done with comes back from [`spare`].
#[derive(Default)]
pub struct List {
    pub sprites: Vec<Sprite>,
    /// The paths' points.
    pub points: Vec<[f32; 2]>,
    pub groups: Vec<Group>,
}

impl List {
    pub fn clear(&mut self) {
        self.sprites.clear();
        self.points.clear();
        self.groups.clear();
    }

    pub fn is_empty(&self) -> bool {
        self.sprites.is_empty()
    }
}

/// The latest list (none: nothing to draw), and the one before, for reuse.
static LATEST: Mutex<Option<Arc<List>>> = Mutex::new(None);
static SPARE: Mutex<Option<Arc<List>>> = Mutex::new(None);
/// Changes with the list.
static GENERATION: AtomicU64 = AtomicU64::new(0);
/// The latest list has groups in the world: the renderer needs each
/// frame's camera.
static WORLD: AtomicBool = AtomicBool::new(false);
/// Images dropped so far.
static DROPPED: AtomicU64 = AtomicU64::new(0);
/// The window's image, in pixels (width << 32 | height), as last drawn.
static SCREEN: AtomicU64 = AtomicU64::new(0);

/// Replaces what is drawn, from the next frame shown (an empty list draws
/// nothing).
pub fn publish(list: List) {
    let mut latest = LATEST.locked();
    if list.is_empty() && latest.is_none() {
        return;
    }
    WORLD.store(list.groups.iter().any(|g| g.anchor.is_some()), Ordering::Relaxed);
    let old = std::mem::replace(&mut *latest, (!list.is_empty()).then(|| Arc::new(list)));
    GENERATION.fetch_add(1, Ordering::Release);
    drop(latest);
    if old.is_some() {
        *SPARE.locked() = old;
    }
}

/// An empty list to fill, the one the renderer was done with if it is
/// (its memory kept).
pub fn spare() -> List {
    let spare = SPARE.locked().take();
    match spare.map(Arc::try_unwrap) {
        Some(Ok(mut list)) => {
            list.clear();
            list
        }
        _ => List::default(),
    }
}

/// Whether the latest list has groups in the world (each frame's camera
/// places them).
pub fn in_world() -> bool {
    WORLD.load(Ordering::Relaxed)
}

/// The list to draw.
pub fn latest() -> Option<Arc<List>> {
    LATEST.locked().clone()
}

/// Changes whenever [`latest`] would give another list.
pub fn generation() -> u64 {
    GENERATION.load(Ordering::Acquire)
}

/// Changes whenever an image is dropped (its texture can go).
pub fn dropped() -> u64 {
    DROPPED.load(Ordering::Acquire)
}

/// The renderer's: the size of the window's image the sprites go on.
pub fn set_screen(width: u32, height: u32) {
    SCREEN.store(u64::from(width) << 32 | u64::from(height), Ordering::Relaxed);
}

/// The size of the window's image, in pixels (0, 0 before a frame is shown).
pub fn screen() -> (u32, u32) {
    let s = SCREEN.load(Ordering::Relaxed);
    ((s >> 32) as u32, s as u32)
}

#[cfg(test)]
mod tests {
    use super::Image;

    /// Each level halves the one before (2×2 averages, an odd edge
    /// repeated), down to a pixel; they count in its bytes.
    #[test]
    fn mipmaps() {
        let mut pixels = Vec::new();
        for v in [0u8, 40, 80, 120, 160, 200] {
            pixels.extend([v, v, v, 255]);
        }
        let image = Image::new(3, 2, pixels);
        assert_eq!(image.levels(), 2);
        assert_eq!(image.bytes(), Image::bytes_for(3, 2));
        assert_eq!(Image::bytes_for(3, 2), 24 + 4);
        let all = image.take_pixels().unwrap();
        assert_eq!(all.len(), image.bytes());
        // (0 + 40 + 120 + 160) / 4
        assert_eq!(&all[24..], &[80, 80, 80, 255]);
        assert_eq!(Image::bytes_for(16, 8), 4 * (128 + 32 + 8 + 2 + 1));
    }
}
