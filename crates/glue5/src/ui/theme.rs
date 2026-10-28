//! Shared typography, color tokens and layout density for the app UI.
use egui::*;

pub(super) const INK: Color32 = Color32::from_rgb(14, 15, 17);
pub(super) const MUTED: Color32 = Color32::from_rgb(157, 160, 166);

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
    RichText::new(text).font(FontId::new(size, FontFamily::Name("condensed".into())))
}

pub(super) fn install(ctx: &Context) {
    ctx.set_theme(Theme::Dark);
    let mut fonts = FontDefinitions::default();
    for (name, bytes) in [
        ("ui", include_bytes!("../../assets/ui/Barlow-SemiBold.ttf").as_slice()),
        ("condensed", include_bytes!("../../assets/ui/BarlowCondensed-Bold.ttf").as_slice()),
    ] {
        fonts.font_data.insert(name.into(), FontData::from_static(bytes).into());
    }
    fonts.families.get_mut(&FontFamily::Proportional).unwrap().insert(0, "ui".into());
    let mut headings = vec!["condensed".into()];
    headings.extend(fonts.families[&FontFamily::Proportional].clone());
    fonts.families.insert(FontFamily::Name("condensed".into()), headings);
    ctx.set_fonts(fonts);
    ctx.all_styles_mut(|s| {
        s.text_styles.insert(TextStyle::Body, FontId::proportional(14.0));
        s.text_styles.insert(TextStyle::Button, FontId::proportional(14.0));
        s.text_styles.insert(TextStyle::Small, FontId::proportional(11.0));
        s.spacing.item_spacing = vec2(10.0, 10.0);
        s.spacing.button_padding = vec2(12.0, 8.0);
        s.spacing.interact_size.y = 44.0;
        s.visuals.override_text_color = Some(Color32::WHITE);
        s.visuals.panel_fill = INK;
        s.visuals.window_fill = Color32::from_rgba_unmultiplied(12, 13, 15, 242);
        s.visuals.window_corner_radius = 22.into();
        s.visuals.window_stroke = Stroke::new(1.0, Color32::from_gray(76));
        s.visuals.selection.bg_fill = Color32::from_gray(90);
        s.visuals.selection.stroke = Stroke::new(1.0, Color32::WHITE);
        for w in [&mut s.visuals.widgets.inactive, &mut s.visuals.widgets.hovered, &mut s.visuals.widgets.active] {
            w.corner_radius = 14.into();
            w.bg_fill = Color32::from_gray(39);
            w.weak_bg_fill = Color32::from_gray(34);
            w.bg_stroke = Stroke::NONE;
            w.fg_stroke = Stroke::new(1.5, Color32::WHITE);
        }
        s.visuals.widgets.hovered.bg_fill = Color32::from_gray(60);
        s.visuals.widgets.active.bg_stroke = Stroke::new(2.0, Color32::WHITE);
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
