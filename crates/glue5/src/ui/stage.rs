//! What is behind the start page and the pages over it: a drawn backdrop
//! (night over the coast, the thin lines of the accent, drifting almost out
//! of sight) and, when the player has put them in a folder, the mode's scene
//! over it. Drawn at every frame on the background layer, with no state but
//! the scenes'.
use std::f64::consts::TAU;
use std::path::{Path, PathBuf};
use std::time::Duration;

use egui::{Color32, Context, Id, LayerId, Mesh, Painter, Pos2, Rect, Stroke, pos2, vec2};

use super::theme::{self, palette};
use super::scenes;

/// The start page's scene, if any, tells it which side the text goes.
pub(super) const PANEL_RIGHT: &str = "home-panel-right";
/// And the page tells the stage where the scenes are (for its status).
pub(super) const SCENES_NOTE: &str = "home-scenes-note";

pub(super) struct Stage {
    scenes: scenes::Scenes,
}

impl Stage {
    pub(super) fn new(user_dir: Option<PathBuf>) -> Self {
        Stage { scenes: scenes::Scenes::new(user_dir) }
    }

    /// The game has started: the pictures are let go.
    pub(super) fn release(&mut self) {
        self.scenes.release();
    }

    /// Drop any CPU-side images that finished decoding after the game took over.
    pub(super) fn discard_pending(&mut self) {
        self.scenes.discard_pending();
    }

    /// `home`: the start page is over it (else a settings page and the like:
    /// only the farthest layer, dimmed); `motion`: the lines and the layers
    /// drift. The scene itself comes from the scenes' own clock, not from the
    /// game's mode.
    pub(super) fn draw(&mut self, ctx: &Context, game_dir: Option<&Path>, home: bool, motion: bool) {
        let rect = ctx.viewport_rect();
        let safe = ctx.content_rect();
        let scale = theme::layout_scale(safe);
        let t = if motion { ctx.input(|i| i.time) } else { 0.0 };
        self.scenes.locate(game_dir);
        self.scenes.poll(ctx);
        self.scenes.show(ctx);

        let mut painter = ctx.layer_painter(LayerId::background());
        painter.set_clip_rect(rect);
        quad(&painter, rect, [palette::INK_950, palette::INK_950, palette::INK_800, palette::INK_800]);
        let motion_of = self.scenes.motion(motion);
        let lines_now = |p: &Painter| lines(p, rect, t, scale);
        let drew = self.scenes.draw(ctx, &painter, rect, t, scale, home, &motion_of, &lines_now);
        if !drew {
            lines(&painter, rect, t, scale);
        }
        let panel_right = self.scenes.panel_right();
        if drew {
            if home {
                legibility(&painter, rect, panel_right.unwrap_or(true));
            } else {
                painter.rect_filled(rect, 0, Color32::from_black_alpha(170));
            }
        }
        if home {
            band(&painter, rect, safe.bottom() - 66.0 * scale);
        }
        ctx.data_mut(|d| {
            d.insert_temp(Id::new(PANEL_RIGHT), panel_right.unwrap_or(true));
            d.insert_temp(Id::new(SCENES_NOTE), self.scenes.report());
        });
        if motion {
            // Slow: it moves a few points a second.
            ctx.request_repaint_after(Duration::from_millis(33));
        }
    }
}

/// A quad with a colour at each corner (top left, top right, bottom right,
/// bottom left).
fn quad(painter: &Painter, rect: Rect, colors: [Color32; 4]) {
    let mut mesh = Mesh::default();
    for (pos, color) in [rect.left_top(), rect.right_top(), rect.right_bottom(), rect.left_bottom()].into_iter().zip(colors) {
        mesh.colored_vertex(pos, color);
    }
    mesh.add_triangle(0, 1, 2);
    mesh.add_triangle(0, 2, 3);
    painter.add(mesh);
}

/// Black over a scene where the text goes, and under the bar, so that
/// whatever the picture is, it reads.
fn legibility(painter: &Painter, rect: Rect, panel_right: bool) {
    let dark = |a: u8| Color32::from_rgba_unmultiplied(6, 8, 12, a);
    let reach = rect.width() * 0.72;
    let (from, to) = if panel_right {
        (rect.right() - reach, rect.right())
    } else {
        (rect.left() + reach, rect.left())
    };
    let strip = Rect::from_min_max(pos2(from.min(to), rect.top()), pos2(from.max(to), rect.bottom()));
    let (near, far) = (dark(205), dark(0));
    let colors = if panel_right { [far, near, near, far] } else { [near, far, far, near] };
    quad(painter, strip, colors);
    // The foot of the screen, where the bar is.
    let foot = Rect::from_min_max(pos2(rect.left(), rect.bottom() - rect.height() * 0.3), rect.max);
    quad(painter, foot, [dark(0), dark(0), dark(190), dark(190)]);
}

/// The bar's band along the bottom: dark, with a hairline on top.
fn band(painter: &Painter, rect: Rect, top: f32) {
    let band = Rect::from_min_max(pos2(rect.left(), top), rect.max);
    painter.rect_filled(band, 0, Color32::from_rgba_unmultiplied(7, 9, 13, 196));
    painter.line_segment(
        [pos2(rect.left(), top), pos2(rect.right(), top)],
        Stroke::new(1.0, Color32::from_white_alpha(22)),
    );
}

/// The lines the start page is known by: diagonals along the left, which
/// slide a step in a few seconds, and arcs of the accent from the top right
/// that swing a few points, each at its own pace (so the whole is never the
/// same twice, and nothing in it is quick enough to catch).
fn lines(painter: &Painter, rect: Rect, t: f64, scale: f32) {
    let (w, h) = (rect.width(), rect.height());
    let band = w * 0.24;
    let step = (band / 9.0).max(8.0);
    let drift = (t as f32 * 2.0 * scale) % step;
    let mut x = rect.left() - h + drift - step;
    while x < rect.left() + band {
        // Fades out at the band's end instead of stopping.
        let edge = ((rect.left() + band - x) / (band * 0.25)).clamp(0.0, 1.0);
        let alpha = (9.0 * edge).round() as u8;
        if alpha > 0 {
            painter.line_segment(
                [pos2(x, rect.bottom()), pos2(x + h, rect.top())],
                Stroke::new(1.0, Color32::from_white_alpha(alpha)),
            );
        }
        x += step;
    }
    let swing = |period: f64, phase: f64, amount: f32| ((t * TAU / period + phase).sin() as f32) * amount * scale;
    let accent = |alpha: u8| {
        let c = palette::SUN;
        Color32::from_rgba_unmultiplied(c.r(), c.g(), c.b(), alpha)
    };
    let arcs: [(Pos2, f32, f32, u8, [f64; 3]); 2] = [
        (pos2(rect.right() + w * 0.12, rect.top() - h * 0.30), h * 0.85, 2.0, 20, [53.0, 71.0, 89.0]),
        (pos2(rect.right() + w * 0.04, rect.top() - h * 0.48), h * 1.12, 1.0, 11, [71.0, 47.0, 97.0]),
    ];
    for (i, (centre, radius, width, alpha, periods)) in arcs.into_iter().enumerate() {
        let phase = i as f64 * 1.9;
        let move_by = vec2(swing(periods[0], phase, 5.0), swing(periods[1], phase + 1.3, 3.0));
        let grow = swing(periods[2], phase + 0.7, 4.0);
        painter.circle_stroke(centre + move_by, radius + grow, Stroke::new(width, accent(alpha)));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui::{RawInput, SafeAreaInsets, Shape};

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
        let meshes: Vec<_> = output.shapes.iter().filter(|s| matches!(s.shape, Shape::Mesh(_))).collect();
        assert!(!meshes.is_empty());
        assert!(meshes.iter().all(|s| s.clip_rect == viewport));
    }

    #[test]
    fn a_still_page_is_the_same_at_any_time_and_a_moving_one_is_not() {
        let painted = |t: f64, still: bool| {
            let ctx = Context::default();
            let rect = Rect::from_min_size(Pos2::ZERO, vec2(800.0, 400.0));
            let mut output = ctx.run_ui(RawInput { screen_rect: Some(rect), ..Default::default() }, |ui| {
                let painter = ui.painter().clone();
                lines(&painter, rect, if still { 0.0 } else { t }, 1.0);
            });
            output.textures_delta.clear();
            format!("{:?}", output.shapes.iter().map(|s| &s.shape).collect::<Vec<&Shape>>())
        };
        assert_eq!(painted(5.0, true), painted(90.0, true));
        assert_ne!(painted(5.0, false), painted(25.0, false));
    }
}
