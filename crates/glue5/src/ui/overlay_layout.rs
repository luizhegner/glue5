//! Geometry shared by the live overlay and the lightweight CI checks.
use egui::{Pos2, Rect, Vec2, pos2};

pub const GEAR_RADIUS: f32 = 20.0;

pub fn gear_center(safe: Rect, scale: f32) -> Pos2 {
    pos2((safe.right() - 30.0 * scale).max(safe.left() + 30.0 * scale), safe.top() + 30.0 * scale)
}

/// Reserve the complete toolbar row even while it is fading/temporarily hidden,
/// so the build identity does not jump into an interactive button's position.
pub fn build_identity_rect(safe: Rect, size: Vec2, scale: f32) -> Option<Rect> {
    let top = gear_center(safe, scale).y + GEAR_RADIUS * scale + 8.0;
    if size.x > safe.width() - 8.0 || top + size.y > safe.bottom() - 4.0 {
        return None;
    }
    Some(Rect::from_min_size(pos2(safe.right() - size.x - 4.0, top), size))
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui::vec2;

    #[test]
    fn identity_stays_below_settings_and_mods_at_every_control_scale() {
        for (screen, inset) in [(vec2(960.0,432.0),24.0),(vec2(844.0,390.0),47.0),(vec2(390.0,844.0),0.0)] {
            let safe = Rect::from_min_max(pos2(inset,0.0),pos2(screen.x-inset,screen.y-20.0));
            for scale in [0.574,0.82,1.0,1.23,1.5] {
                for size in [vec2(200.0,18.0),vec2(safe.width()-16.0,40.0)] {
                    let identity = build_identity_rect(safe,size,scale).unwrap();
                    assert!(safe.contains_rect(identity));
                    for dx in [0.0,-54.0*scale] {
                        let button = Rect::from_center_size(gear_center(safe,scale)+vec2(dx,0.0),Vec2::splat(2.0*GEAR_RADIUS*scale));
                        assert!(!identity.intersects(button));
                    }
                }
            }
        }
    }
}
