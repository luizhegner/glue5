//! Over the game, while it streams: a small marker when the game waits
//! for the server (a turning ring) or the server does not answer (orange alert),
//! saying so in a few words for a moment after it changes. It sits beside the
//! settings button, or in the first free place after it, and stays clear of
//! everything else on screen (the touch controls, the overlay); a tap opens
//! the Servers page.
use std::time::{Duration, Instant};

use egui::{Align2, Color32, FontId, Id, LayerId, Order, Pos2, Rect, Shape, Stroke, Vec2, pos2, vec2};

use super::{StreamAlert, theme};

/// The square marker's half-size, in points at controls' size 1.
const HALF_SIZE: f32 = 13.0;
/// Space kept between the sign and anything else.
const GAP: f32 = 8.0;
/// How long the words show after the state changes.
const SAY: Duration = Duration::from_secs(3);
const ALERT: Color32 = theme::palette::ACCENT;
const ALERT_INK: Color32 = theme::expressive::ON_PRIMARY;

#[derive(Default)]
pub(super) struct Chip {
    /// What it shows, and since when.
    shown: Option<(StreamAlert, Instant)>,
    /// Where it was last drawn: the sign, and the words if apart (taps).
    rects: Vec<Rect>,
}

impl Chip {
    /// A finger at `p` is on the sign.
    pub(super) fn hit(&self, p: Pos2) -> bool {
        self.rects.iter().any(|r| r.expand(6.0).contains(p))
    }

    /// Draws the sign for `alert` (none: nothing) inside `safe`, off
    /// `taken` (what else is on screen), next to the settings button
    /// (`gear`: its centre and radius) if there is one. `scale`: the touch
    /// controls' size; `alpha`: their opacity.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn draw(
        &mut self,
        ctx: &egui::Context,
        safe: Rect,
        alert: Option<StreamAlert>,
        gear: Option<(Pos2, f32)>,
        taken: &[Rect],
        scale: f32,
        alpha: f32,
    ) {
        self.rects.clear();
        let Some(alert) = alert else {
            self.shown = None;
            return;
        };
        let now = Instant::now();
        if self.shown.is_none_or(|(a, _)| a != alert) {
            self.shown = Some((alert, now));
        }
        let since = self.shown.map_or(now, |(_, t)| t);
        let (r, gap) = (HALF_SIZE * scale, GAP * scale);
        let free = |rect: Rect| safe.contains_rect(rect) && !taken.iter().any(|t| t.intersects(rect.expand(gap * 0.5)));
        let marker_rect = |c: Pos2| Rect::from_center_size(c, Vec2::splat(2.0 * r));
        let Some(c) = places(safe, gear, r, gap).into_iter().find(|&c| free(marker_rect(c))) else {
            return;
        };
        let painter = ctx.layer_painter(LayerId::new(Order::Foreground, Id::new("server chip")));
        let white = |a: f32| Color32::from_white_alpha((a * alpha * 255.0) as u8);
        let (fill, stroke) = (Color32::from_black_alpha((0.6 * alpha * 255.0) as u8), match alert {
            StreamAlert::Waiting => white(0.6),
            StreamAlert::Offline => ALERT,
        });
        // The words, for a moment after a change: in the sign opened to the
        // right (beside the settings button) or the left, else under it,
        // wherever they cover nothing.
        let words = match alert {
            StreamAlert::Waiting => "Waiting for the server",
            StreamAlert::Offline => "Server not answering",
        };
        let galley = painter.layout_no_wrap(words.into(), FontId::proportional(12.0 * scale), white(1.0));
        let text = galley.size();
        let wide = text.x + 2.0 * r + 12.0 * scale;
        let right = Rect::from_min_max(pos2(c.x - r, c.y - r), pos2(c.x - r + wide, c.y + r));
        let left = Rect::from_min_max(pos2(c.x + r - wide, c.y - r), pos2(c.x + r, c.y + r));
        let under = Rect::from_min_max(
            pos2(c.x + r - (text.x + 20.0 * scale), c.y + r + gap * 0.5),
            pos2(c.x + r, c.y + r + gap * 0.5 + 2.0 * r),
        );
        let place = (now - since < SAY).then(|| [right, left, under].into_iter().find(|&w| free(w))).flatten();
        let opened = place.filter(|&w| w != under);
        let body = opened.unwrap_or_else(|| marker_rect(c));
        let stroke = Stroke::new(1.5 * scale, stroke);
        painter.rect(body, 0, fill, stroke, egui::StrokeKind::Inside);
        self.rects.push(body);
        if let Some(w) = place {
            if w == under {
                painter.rect(under, 0, fill, stroke, egui::StrokeKind::Inside);
                self.rects.push(under);
            }
            let at = if w == right {
                pos2(c.x + r + 2.0 * scale, c.y)
            } else if w == left {
                pos2(w.min.x + 10.0 * scale, c.y)
            } else {
                pos2(w.min.x + 10.0 * scale, w.center().y)
            };
            painter.galley(at - vec2(0.0, text.y * 0.5), galley, white(1.0));
        }
        match alert {
            // A ring turning, ten steps a second (frames come as the game's
            // anyway; the time is the UI's).
            StreamAlert::Waiting => {
                let turn = (ctx.input(|i| i.time) * 10.0).floor() as f32 / 10.0 * std::f32::consts::TAU * 0.8;
                let ring = r * 0.5;
                painter.circle_stroke(c, ring, Stroke::new(2.0 * scale, white(0.25)));
                let points =
                    (0..=12).map(|i| turn + i as f32 / 12.0 * std::f32::consts::TAU * 0.3).map(|a| c + ring * Vec2::angled(a));
                painter.add(Shape::line(points.collect(), Stroke::new(2.0 * scale, white(1.0))));
            }
            StreamAlert::Offline => {
                painter.rect_filled(Rect::from_center_size(c, Vec2::splat(2.0 * r * 0.62)), 0, ALERT);
                painter.text(c, Align2::CENTER_CENTER, "!", FontId::proportional(11.0 * scale), ALERT_INK);
            }
        }
        ctx.request_repaint_after(Duration::from_millis(100));
    }
}

/// Where the sign may go, in order: right of the settings button (the last
/// of the top's buttons: the touch controls' camera view and pause, the
/// settings), under it, the top corners, under the middle of the top.
fn places(safe: Rect, gear: Option<(Pos2, f32)>, r: f32, gap: f32) -> Vec<Pos2> {
    let mut out = Vec::new();
    if let Some((g, gr)) = gear {
        out.push(pos2(g.x + gr + gap + r, g.y));
        out.push(pos2(g.x, g.y + gr + gap + r));
    }
    let corner = 12.0 * r / HALF_SIZE + r;
    out.push(pos2(safe.max.x - corner, safe.min.y + corner));
    out.push(pos2(safe.min.x + corner, safe.min.y + corner));
    out.push(pos2(safe.center().x, safe.min.y + 4.0 * corner));
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::touch::Touch;
    use glue5_runtime::input::{Activity, Character, Driving, PlayerContext};

    /// Every touch layout, at every controls' size, on phones, a tablet and a
    /// window, with the overlay in a top corner or not: the sign and its
    /// words are inside the safe area and on no control.
    #[test]
    fn the_sign_covers_no_control() {
        let activities = [
            Activity::OnFoot,
            Activity::Driving(Driving::Car),
            Activity::Driving(Driving::Bicycle),
            Activity::Driving(Driving::Heli),
            Activity::Driving(Driving::Plane),
            Activity::Driving(Driving::Submarine),
            Activity::Passenger,
            Activity::Parachute,
        ];
        let mut contexts = Vec::new();
        for activity in activities {
            for (vehicle_near, phone, menu, cutscene) in
                [(false, false, false, false), (true, false, false, false), (false, true, false, false), (false, false, true, false), (false, false, false, true)]
            {
                contexts.push(PlayerContext { activity, vehicle_near, phone, menu, cutscene, bar: false, scripted: false, character: Character::Other, gun: false, cursor: false, map: false, pointer: false, alert: false, hidden: false });
            }
        }
        // Screens in points, and their safe areas' insets (left, top, right, bottom).
        let screens = [
            (vec2(874.0, 402.0), [62.0, 0.0, 62.0, 21.0]),
            (vec2(844.0, 390.0), [47.0, 0.0, 47.0, 21.0]),
            (vec2(1194.0, 834.0), [0.0, 24.0, 0.0, 20.0]),
            (vec2(1280.0, 720.0), [0.0, 0.0, 0.0, 0.0]),
        ];
        let mut checked = 0;
        for (screen, [l, t, r, b]) in screens {
            let safe = Rect::from_min_max(pos2(l, t), pos2(screen.x - r, screen.y - b));
            for scale in [0.7, 1.0, 1.5] {
                for overlay in [None, Some(Rect::from_min_size(safe.min + vec2(8.0, 8.0), vec2(228.0, 210.0))),
                    Some(Rect::from_min_size(pos2(safe.max.x - 236.0, safe.min.y + 8.0), vec2(228.0, 210.0)))]
                {
                    for &context in &contexts {
                        let mut touch = Touch::new();
                        touch.layout(screen, safe, scale);
                        touch.set_context(context);
                        for alert in [StreamAlert::Waiting, StreamAlert::Offline] {
                            let ctx = egui::Context::default();
                            super::super::theme::install(&ctx);
                            let mut chip = Chip::default();
                            let mut taken = touch.occupied();
                            let raw = egui::RawInput { screen_rect: Some(Rect::from_min_size(Pos2::ZERO, screen)), ..Default::default() };
                            let mut out = ctx.run_ui(raw, |ui| {
                                let controls = crate::touch::control_scale(screen, scale);
                                let gear = super::super::draw_gear(ui.ctx(), safe, controls, 1.0, None);
                                taken.push(Rect::from_center_size(gear.0, Vec2::splat(2.0 * gear.1)));
                                taken.extend(overlay);
                                chip.draw(ui.ctx(), safe, Some(alert), Some(gear), &taken, controls, 1.0);
                            });
                            out.textures_delta.clear();
                            if chip.rects.is_empty() {
                                continue;
                            }
                            checked += 1;
                            for drawn in &chip.rects {
                                assert!(safe.contains_rect(*drawn), "{screen:?} {scale} {context:?}: {drawn:?} outside {safe:?}");
                                for t in &taken {
                                    assert!(!t.intersects(*drawn), "{screen:?} {scale} {context:?} {overlay:?}: {drawn:?} on {t:?}");
                                }
                            }
                        }
                    }
                }
            }
        }
        // It found a place almost always (not drawing is the last resort).
        assert!(checked >= 2800, "drawn {checked} of 2880 times");
    }
}
