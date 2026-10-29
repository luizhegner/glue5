//! What the application draws over the game — its settings and touch
//! controls — as egui primitives, rendered into the window after the game's
//! frame.

use std::sync::{Arc, Mutex};

use egui::TexturesDelta;
use egui::epaint::ClippedPrimitive;

/// Shared by the application (which submits) and the renderer (which draws
/// the latest submission at each present).
#[derive(Clone, Default)]
pub struct Overlay(Arc<Mutex<Frame>>);

#[derive(Default)]
struct Frame {
    primitives: Arc<Vec<ClippedPrimitive>>,
    /// Texture changes accumulate until a present applies them.
    textures: TexturesDelta,
    pixels_per_point: f32,
    /// Submitted since the renderer last took it.
    waiting: bool,
    /// Panel right edge as a fraction of the full output width; None means
    /// no acrylic blur. The UI itself is drawn afterwards, sharp.
    background_blur: Option<f32>,
}

impl Drop for Frame {
    fn drop(&mut self) {
        // Pending uploads can be discarded once the overlay is no longer used.
        self.textures.clear();
    }
}

impl Overlay {
    /// Replaces what is drawn (an empty list draws nothing).
    pub fn submit(&self, primitives: Vec<ClippedPrimitive>, textures: TexturesDelta, pixels_per_point: f32) {
        let mut frame = self.0.lock().unwrap();
        frame.primitives = Arc::new(primitives);
        frame.textures.append(textures);
        frame.pixels_per_point = pixels_per_point;
        frame.waiting = true;
    }

    /// Blur the game only beneath the panel, whose edge is relative to the
    /// output width. `None` restores the normal image.
    pub fn set_background_blur(&self, panel_right: Option<f32>) {
        self.0.lock().unwrap().background_blur = panel_right.map(|v| v.clamp(0.0, 1.0));
    }

    fn background_blur(&self) -> Option<f32> {
        self.0.lock().unwrap().background_blur
    }

    /// A submission the renderer has not drawn yet: a newer one would only
    /// replace it (the application can skip making one).
    pub fn waiting(&self) -> bool {
        self.0.lock().unwrap().waiting
    }

    fn take(&self) -> (Arc<Vec<ClippedPrimitive>>, TexturesDelta, f32) {
        let mut frame = self.0.lock().unwrap();
        frame.waiting = false;
        (frame.primitives.clone(), std::mem::take(&mut frame.textures), frame.pixels_per_point)
    }
}

/// The renderer side.
pub struct OverlayRenderer {
    overlay: Overlay,
    renderer: Option<(wgpu::TextureFormat, egui_wgpu::Renderer)>,
}

impl OverlayRenderer {
    pub fn new(overlay: Overlay) -> Self {
        OverlayRenderer { overlay, renderer: None }
    }

    pub(crate) fn background_blur(&self) -> Option<f32> {
        self.overlay.background_blur()
    }

    /// The latest submission's texture and buffer uploads, recorded into
    /// `encoder` before the pass that [`OverlayRenderer::render`]s it.
    pub fn prepare(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        format: wgpu::TextureFormat,
        size: [u32; 2],
    ) -> Option<Prepared> {
        let (primitives, textures, pixels_per_point) = self.overlay.take();
        if primitives.is_empty() && textures.is_empty() {
            return None;
        }
        if self.renderer.as_ref().is_none_or(|(f, _)| *f != format) {
            self.renderer = Some((format, egui_wgpu::Renderer::new(device, format, Default::default())));
        }
        let renderer = &mut self.renderer.as_mut().unwrap().1;
        for (id, deltas) in &textures.set {
            for delta in deltas {
                renderer.update_texture(device, queue, *id, delta);
            }
        }
        let screen = egui_wgpu::ScreenDescriptor { size_in_pixels: size, pixels_per_point };
        if !primitives.is_empty() {
            renderer.update_buffers(device, queue, encoder, &primitives, &screen);
        }
        Some(Prepared { primitives, textures, screen })
    }

    /// Draws a prepared submission into `pass` (over the game's frame), then
    /// frees the textures it no longer uses.
    pub fn render(&mut self, pass: &mut wgpu::RenderPass<'static>, mut prepared: Prepared) {
        let renderer = &mut self.renderer.as_mut().unwrap().1;
        if !prepared.primitives.is_empty() {
            renderer.render(pass, &prepared.primitives, &prepared.screen);
        }
        for id in &prepared.textures.free {
            renderer.free_texture(id);
        }
        prepared.textures.clear();
    }
}

/// A submission whose uploads are recorded, to draw.
pub struct Prepared {
    primitives: Arc<Vec<ClippedPrimitive>>,
    textures: TexturesDelta,
    screen: egui_wgpu::ScreenDescriptor,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backdrop_blur_can_be_enabled_and_restored() {
        let overlay = Overlay::default();
        assert_eq!(overlay.background_blur(), None);
        overlay.set_background_blur(Some(0.62));
        assert_eq!(overlay.background_blur(), Some(0.62));
        overlay.set_background_blur(Some(1.5));
        assert_eq!(overlay.background_blur(), Some(1.0));
        overlay.set_background_blur(None);
        assert_eq!(overlay.background_blur(), None);
    }

    #[test]
    fn closing_before_the_first_present_discards_pending_uploads() {
        let overlay = Overlay::default();
        let mut textures = TexturesDelta::default();
        textures.push(
            egui::TextureId::Managed(0),
            egui::epaint::ImageDelta::full(
                egui::ColorImage::new([1, 1], vec![egui::Color32::WHITE]),
                egui::TextureOptions::LINEAR,
            ),
        );
        overlay.submit(Vec::new(), textures, 1.0);
        // TexturesDelta asserts on drop unless consumed or explicitly cleared.
        drop(overlay);
    }
}
