//! Geometry shared by the live overlay and the lightweight CI checks.
use egui::{Pos2, Rect, Vec2, pos2};

pub const GEAR_RADIUS: f32 = 20.0;
/// Space below menu actions for the persistent build identity.
pub const IDENTITY_STRIP: f32 = 24.0;

pub fn draw_build_identity(ctx: &egui::Context, safe: Rect, label: &str) -> Option<Rect> {
    use egui::{Color32, FontId, Id, LayerId, Order, Stroke, vec2};
    let painter = ctx.layer_painter(LayerId::new(Order::Foreground, Id::new("alpha-build-identity")));
    let color = Color32::from_white_alpha(225);
    let galley = painter.layout(label.to_owned(), FontId::proportional(9.0), color, (safe.width() - 16.0).max(1.0));
    let rect = build_identity_rect(safe, galley.size() + vec2(10.0, 6.0), 1.0)?;
    painter.rect_filled(rect, 0, Color32::from_black_alpha(170));
    painter.rect_stroke(rect, 0, Stroke::new(0.5, Color32::from_white_alpha(45)), egui::StrokeKind::Inside);
    painter.galley(rect.min + vec2(5.0, 3.0), galley, color);
    Some(rect)
}

pub fn gear_center(safe: Rect, scale: f32) -> Pos2 {
    pos2((safe.right() - 30.0 * scale).max(safe.left() + 30.0 * scale), safe.top() + 30.0 * scale)
}

/// Keep the build identity at the bottom-right of the supplied output.
/// Gameplay passes the full viewport: passive text must not inherit the
/// controls' gesture/navigation insets and float away from the screen edge.
pub fn build_identity_rect(safe: Rect, size: Vec2, _scale: f32) -> Option<Rect> {
    if size.x > safe.width() - 8.0 || size.y > safe.height() - 8.0 {
        return None;
    }
    Some(Rect::from_min_size(pos2(safe.right() - size.x - 4.0, safe.bottom() - size.y - 4.0), size))
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui::vec2;

    #[test]
    fn identity_stays_in_the_bottom_right_safe_area_at_every_control_scale() {
        for (screen, inset) in [(vec2(960.0,432.0),24.0),(vec2(844.0,390.0),47.0),(vec2(390.0,844.0),0.0)] {
            let safe = Rect::from_min_max(pos2(inset,0.0),pos2(screen.x-inset,screen.y-20.0));
            for scale in [0.574,0.82,1.0,1.23,1.5] {
                for size in [vec2(200.0,18.0),vec2(safe.width()-16.0,40.0)] {
                    let identity = build_identity_rect(safe,size,scale).unwrap();
                    assert!(safe.contains_rect(identity));
                    assert_eq!(identity.right(), safe.right() - 4.0);
                    assert_eq!(identity.bottom(), safe.bottom() - 4.0);
                    for dx in [0.0,-54.0*scale] {
                        let button = Rect::from_center_size(gear_center(safe,scale)+vec2(dx,0.0),Vec2::splat(2.0*GEAR_RADIUS*scale));
                        assert!(!identity.intersects(button));
                    }
                }
            }
        }
    }
}
