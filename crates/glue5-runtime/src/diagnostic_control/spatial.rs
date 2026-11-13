//! Coordinates/bearings shared by structured diagnostic context and tests.
pub fn relative(from: [f32; 3], to: [f32; 3], heading: f32) -> Option<(f32, f32)> {
    if !from.into_iter().chain(to).chain([heading]).all(f32::is_finite) {
        return None;
    }
    let dx = to[0] - from[0];
    let dy = to[1] - from[1];
    let distance = dx.hypot(dy);
    // GTA: zero points north (+y), positive headings turn toward west (-x).
    let bearing = (-dx).atan2(dy).to_degrees();
    Some((distance, (bearing - heading + 180.0).rem_euclid(360.0) - 180.0))
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn bearings_follow_gta_axes_and_wrap() {
        assert_eq!(relative([0.0; 3], [0.0, 10.0, 0.0], 0.0), Some((10.0, 0.0)));
        assert_eq!(relative([0.0; 3], [10.0, 0.0, 0.0], 0.0), Some((10.0, -90.0)));
        assert_eq!(relative([0.0; 3], [-10.0, 0.0, 0.0], 90.0), Some((10.0, 0.0)));
        assert!(relative([f32::NAN; 3], [0.0; 3], 0.0).is_none());
    }
}
