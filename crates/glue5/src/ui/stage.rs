//! The menu backdrop comes from the selected WebAssembly build.
use std::path::{Path, PathBuf};
use std::time::Duration;

use egui::{Context, Id, LayerId};

use super::theme::palette;
use super::title;

/// The start page's scene, if any, tells it which side the text goes.
pub(super) const PANEL_RIGHT: &str = "home-panel-right";
/// And the page tells the stage where the scenes are (for its status).
pub(super) const SCENES_NOTE: &str = "home-scenes-note";

pub(super) struct Stage {
    title: title::Title,
}

impl Stage {
    pub(super) fn new(_user_dir: Option<PathBuf>) -> Self {
        Stage { title: title::Title::new() }
    }

    /// The game has started: the pictures are let go.
    pub(super) fn release(&mut self) {
        self.title.release();
    }

    /// Drop any CPU-side images that finished decoding after the game took over.
    pub(super) fn discard_pending(&mut self) {
        self.title.discard_pending();
    }

    pub(super) fn preparing(&self) -> bool { self.title.preparing() }

    /// The whole scene remains visible on all tabs. `motion` controls drift;
    /// the scene clock is independent of the chosen game mode.
    pub(super) fn draw(&mut self, ctx: &Context, game_dir: Option<&Path>, _home: bool, motion: bool) {
        let rect = super::shell::output_rect(ctx);
        let mut painter = ctx.layer_painter(LayerId::background());
        painter.set_clip_rect(rect);
        painter.rect_filled(rect, 0, palette::INK_950);
        let drew = self.title.draw(ctx, &painter, rect, game_dir, glue5_runtime::GAME_BUILD, motion);
        ctx.data_mut(|d| {
            d.insert_temp(Id::new(PANEL_RIGHT), false);
            d.insert_temp(Id::new(SCENES_NOTE), if drew { "Artwork from the selected game build" } else { "Game artwork unavailable" }.to_owned());
        });
        super::shell::set_artwork_ready(ctx, drew);
        if motion && drew {
            // Slow: it moves a few points a second.
            ctx.request_repaint_after(Duration::from_millis(33));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui::{Pos2, Rect, RawInput, SafeAreaInsets, Shape, vec2};
    use crate::ui::theme;

    fn run(stage: &mut Stage, viewport: Rect, home: bool, motion: bool) -> egui::FullOutput {
        let ctx = Context::default();
        theme::install(&ctx);
        let mut output = ctx.run_ui(
            RawInput {
                screen_rect: Some(viewport),
                safe_area_insets: Some(SafeAreaInsets(egui::epaint::MarginF32 {
                    left: 62.0,
                    right: 62.0,
                    top: 0.0,
                    bottom: 21.0,
                })),
                ..Default::default()
            },
            |ui| stage.draw(ui.ctx(), None, home, motion),
        );
        output.textures_delta.clear();
        output
    }

    #[test]
    fn the_backdrop_is_not_clipped_to_the_safe_area() {
        let viewport = Rect::from_min_size(Pos2::ZERO, vec2(874.0, 402.0));
        let output = run(&mut Stage::new(None), viewport, true, false);
        let background = output.shapes.iter().find(|s| matches!(&s.shape,
            Shape::Rect(r) if r.rect == viewport && r.fill == palette::INK_950)).expect("neutral backdrop");
        assert_eq!(background.clip_rect, viewport);
    }
}
