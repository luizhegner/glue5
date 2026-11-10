//! Paint and input use the same button shape; squared corners are touchable.
use egui::{Pos2, Rect, Vec2, vec2};

pub const MIN_HIT_RADIUS: f32 = 28.0;

pub fn round(icon: &str) -> bool {
    matches!(icon, "fire" | "aim" | "weapons" | "radio" | "characters")
}

pub fn size(icon: &str, radius: f32) -> Vec2 {
    if matches!(icon, "gas" | "brake") { vec2(2.0, 1.4) * radius }
    else { Vec2::splat(2.0 * radius) }
}

pub fn hit_radius(radius: f32) -> f32 {
    (radius * 1.2).max(MIN_HIT_RADIUS)
}

/// The menu D-pad must keep its minimum-size targets apart even when the
/// visual controls shrink. Move the entire cross inside the safe area.
pub fn menu_dpad_position(area: Rect, desired: Pos2, hit: f32, visual_step: f32, direction: Vec2) -> Pos2 {
    let step = visual_step.max(2.0 * hit + 2.0);
    let padding = step + hit;
    let min = area.min + Vec2::splat(padding);
    let max = (area.max - Vec2::splat(padding)).max(min);
    desired.clamp(min, max) + direction * step
}

pub fn hit(icon: &str, center: Pos2, radius: f32, point: Pos2) -> bool {
    let reach = hit_radius(radius);
    if round(icon) { center.distance(point) <= reach }
    else { Rect::from_center_size(center, Vec2::splat(2.0*reach)).contains(point) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn secondary_tiles_keep_full_control_width_and_accept_their_corners() {
        for scale in [0.574,0.82,1.0,1.23,1.5] {
            for icon in ["jump","reload","phone","pause","gas","brake"] {
                let radius = 22.0 * scale;
                let size = size(icon,radius);
                assert!(size.x >= 2.0*radius);
                let corner = size * 0.49;
                assert!(hit(icon,Pos2::ZERO,radius,Pos2::ZERO+corner));
                assert!(hit_radius(radius) >= 28.0);
            }
        }
    }

    #[test]
    fn combat_remains_round_and_does_not_capture_square_outer_corners() {
        assert!(round("fire"));
        assert!(!hit("fire",Pos2::ZERO,24.0,egui::pos2(28.0,28.0)));
        assert!(hit("reload",Pos2::ZERO,24.0,egui::pos2(28.0,28.0)));
    }

    #[test]
    fn menu_dpad_targets_do_not_overlap_when_controls_shrink() {
        let area = Rect::from_min_max(egui::pos2(47.0, 0.0), egui::pos2(797.0, 369.0));
        for scale in [0.574, 0.82, 1.0, 1.23] {
            let hit = hit_radius(22.0 * scale);
            let desired = egui::pos2(area.left() + 80.0 * scale, area.bottom() - 135.0 * scale);
            let targets: Vec<_> = [vec2(-1.0,0.0),vec2(1.0,0.0),vec2(0.0,-1.0),vec2(0.0,1.0)].into_iter()
                .map(|d| Rect::from_center_size(menu_dpad_position(area,desired,hit,62.0*scale,d),Vec2::splat(2.0*hit))).collect();
            for (i, target) in targets.iter().enumerate() {
                assert!(area.contains_rect(*target));
                assert!(targets[i+1..].iter().all(|other| !target.intersects(*other)));
            }
        }
    }
}
