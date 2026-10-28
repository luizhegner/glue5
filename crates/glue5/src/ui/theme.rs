//! Shared typography, color tokens and layout density for the app UI.
use egui::*;

pub(super) const INK: Color32 = Color32::from_rgb(8, 11, 9);
pub(super) const MUTED: Color32 = Color32::from_rgb(165, 173, 164);

/// Shared charcoal and green tokens for Home and every utility page.
pub(super) mod expressive {
    use egui::Color32;
    pub(in crate::ui) const PRIMARY: Color32 = Color32::from_rgb(99, 143, 76);
    pub(in crate::ui) const ON_PRIMARY: Color32 = Color32::from_rgb(8, 12, 9);
    pub(in crate::ui) const PRIMARY_CONTAINER: Color32 = Color32::from_rgb(28, 42, 29);
    pub(in crate::ui) const SECONDARY: Color32 = Color32::from_rgb(132, 153, 126);
    pub(in crate::ui) const TERTIARY: Color32 = Color32::from_rgb(169, 184, 147);
    pub(in crate::ui) const TERTIARY_CONTAINER: Color32 = Color32::from_rgb(35, 45, 31);
    pub(in crate::ui) const SURFACE_LOW: Color32 = Color32::from_rgb(8, 11, 9);
    pub(in crate::ui) const SURFACE: Color32 = Color32::from_rgb(15, 19, 16);
    pub(in crate::ui) const SURFACE_HIGH: Color32 = Color32::from_rgb(26, 32, 27);
    pub(in crate::ui) const OUTLINE: Color32 = Color32::from_rgb(64, 72, 64);
    pub(in crate::ui) const ON_SURFACE: Color32 = Color32::from_rgb(241, 243, 239);
    pub(in crate::ui) const ON_SURFACE_VARIANT: Color32 = Color32::from_rgb(180, 187, 178);
}

#[allow(dead_code, reason = "shared colors used across UI surfaces")]
pub(super) mod palette {
    use egui::Color32;
    pub(in crate::ui) const ACCENT: Color32 = Color32::from_rgb(99, 143, 76);
}


/// Shared warm-accent, dark-sky tokens for settings and utility pages. The
/// same palette family is used by the home stage and bottom navigation.
pub(super) mod expressive {
    use egui::Color32;
    pub(in crate::ui) const PRIMARY: Color32 = Color32::from_rgb(255, 145, 90);
    pub(in crate::ui) const ON_PRIMARY: Color32 = Color32::from_rgb(39, 22, 17);
    pub(in crate::ui) const PRIMARY_CONTAINER: Color32 = Color32::from_rgb(75, 42, 27);
    pub(in crate::ui) const SECONDARY: Color32 = Color32::from_rgb(90, 185, 171);
    pub(in crate::ui) const TERTIARY: Color32 = Color32::from_rgb(255, 197, 61);
    pub(in crate::ui) const TERTIARY_CONTAINER: Color32 = Color32::from_rgb(75, 55, 24);
    pub(in crate::ui) const SURFACE_LOW: Color32 = Color32::from_rgb(16, 19, 25);
    pub(in crate::ui) const SURFACE: Color32 = Color32::from_rgb(20, 24, 33);
    pub(in crate::ui) const SURFACE_HIGH: Color32 = Color32::from_rgb(30, 36, 48);
    pub(in crate::ui) const OUTLINE: Color32 = Color32::from_rgb(76, 78, 83);
    pub(in crate::ui) const ON_SURFACE: Color32 = Color32::from_rgb(244, 241, 234);
    pub(in crate::ui) const ON_SURFACE_VARIANT: Color32 = Color32::from_rgb(169, 164, 151);
}

/// The app's shared night-sky palette: quiet surfaces, clear text and warm
/// mode accents, without copying any game's screen.
#[allow(dead_code, reason = "the members the components use land with them")]
pub(super) mod palette {
    use egui::Color32;
    pub(in crate::ui) const INK_950: Color32 = Color32::from_rgb(11, 13, 18);
    pub(in crate::ui) const INK_800: Color32 = Color32::from_rgb(20, 24, 33);
    pub(in crate::ui) const INK_700: Color32 = Color32::from_rgb(30, 36, 48);
    pub(in crate::ui) const LINE: Color32 = Color32::from_rgba_premultiplied(36, 36, 36, 36);
    pub(in crate::ui) const TEXT_HI: Color32 = Color32::from_rgb(244, 241, 234);
    pub(in crate::ui) const TEXT_MID: Color32 = Color32::from_rgb(169, 164, 151);
    pub(in crate::ui) const TEXT_LOW: Color32 = Color32::from_rgb(110, 106, 96);
    pub(in crate::ui) const SUN: Color32 = Color32::from_rgb(255, 122, 61);
    pub(in crate::ui) const SEA: Color32 = Color32::from_rgb(46, 196, 182);
    pub(in crate::ui) const AMBER: Color32 = Color32::from_rgb(255, 197, 61);
    pub(in crate::ui) const RED: Color32 = Color32::from_rgb(229, 72, 77);
}

pub(super) fn heading(text: &str, size: f32) -> RichText {
    RichText::new(text).font(FontId::proportional(size))
}

pub(super) fn install(ctx: &Context) {
    ctx.set_theme(Theme::Dark);
    // Use egui's neutral proportional sans-serif instead of a condensed display face.
    ctx.all_styles_mut(|s| {
        s.text_styles.insert(TextStyle::Body, FontId::proportional(14.0));
        s.text_styles.insert(TextStyle::Button, FontId::proportional(14.0));
        s.text_styles.insert(TextStyle::Small, FontId::proportional(11.0));
        s.spacing.item_spacing = vec2(10.0, 10.0);
        s.spacing.button_padding = vec2(12.0, 8.0);
        s.spacing.interact_size.y = 44.0;
        s.visuals.override_text_color = Some(expressive::ON_SURFACE);
        s.visuals.panel_fill = INK;
        s.visuals.window_fill = expressive::SURFACE_LOW;
        s.visuals.window_corner_radius = 0.into();
        s.visuals.window_stroke = Stroke::new(1.0, expressive::OUTLINE);
        s.visuals.selection.bg_fill = expressive::PRIMARY_CONTAINER;
        s.visuals.selection.stroke = Stroke::new(1.0, expressive::PRIMARY);
        for w in [&mut s.visuals.widgets.inactive, &mut s.visuals.widgets.hovered, &mut s.visuals.widgets.active] {
            w.corner_radius = 0.into();
            w.bg_fill = expressive::SURFACE;
            w.weak_bg_fill = expressive::SURFACE_HIGH;
            w.bg_stroke = Stroke::NONE;
            w.fg_stroke = Stroke::new(1.0, expressive::ON_SURFACE);
        }
        s.visuals.widgets.hovered.bg_fill = expressive::SURFACE_HIGH;
        s.visuals.widgets.active.bg_stroke = Stroke::new(1.0, expressive::PRIMARY);
    });
}

/// Density follows available space, including landscape phones and split views.
pub(super) fn layout_scale(rect: Rect) -> f32 {
    (rect.height() / 560.0).min(rect.width() / 780.0).clamp(0.75, 1.0)
}

pub(super) fn density(ui: &Ui) -> f32 {
    ui.style().text_styles[&TextStyle::Body].size / 14.0
}

pub(super) fn apply_density(ui: &mut Ui, scale: f32) {
    let style = ui.style_mut();
    for font in style.text_styles.values_mut() {
        font.size *= scale;
    }
    style.spacing.item_spacing *= scale;
    style.spacing.button_padding *= scale;
    style.spacing.interact_size *= scale;
}
