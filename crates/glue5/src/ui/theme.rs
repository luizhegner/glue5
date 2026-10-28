//! Shared visual system: Barlow typography, Claude's night-and-sun palette,
//! rounded surfaces, and layout density for every page.
use egui::*;

pub(super) const INK: Color32 = Color32::from_rgb(14, 15, 17);
pub(super) const MUTED: Color32 = Color32::from_rgb(169, 164, 151);

/// Claude's start-page palette, carried through Home, Settings and every
/// utility page: deep blue-black surfaces, warm readable text, one orange accent.
#[allow(dead_code, reason = "shared design tokens may be adopted by components independently")]
pub(super) mod palette {
    use egui::Color32;

    pub(in crate::ui) const INK_950: Color32 = Color32::from_rgb(11, 13, 18);
    pub(in crate::ui) const INK_800: Color32 = Color32::from_rgb(20, 24, 33);
    pub(in crate::ui) const INK_700: Color32 = Color32::from_rgb(34, 39, 49);
    pub(in crate::ui) const LINE: Color32 = Color32::from_rgba_unmultiplied(115, 117, 124, 78);
    pub(in crate::ui) const TEXT_HI: Color32 = Color32::from_rgb(244, 241, 234);
    pub(in crate::ui) const TEXT_MID: Color32 = Color32::from_rgb(169, 164, 151);
    pub(in crate::ui) const TEXT_LOW: Color32 = Color32::from_rgb(130, 129, 127);
    /// The accent: the selected mode, controls and primary actions.
    pub(in crate::ui) const SUN: Color32 = Color32::from_rgb(255, 122, 61);
    pub(in crate::ui) const SUN_HI: Color32 = Color32::from_rgb(255, 146, 92);
    pub(in crate::ui) const ACCENT: Color32 = SUN;
}

/// Compatibility tokens used by shared widgets; they intentionally resolve
/// to one accent rather than introducing unrelated colors between sections.
pub(super) mod expressive {
    use egui::Color32;
    pub(in crate::ui) const PRIMARY: Color32 = super::palette::SUN;
    pub(in crate::ui) const ON_PRIMARY: Color32 = super::palette::INK_950;
    pub(in crate::ui) const PRIMARY_CONTAINER: Color32 = Color32::from_rgb(54, 32, 25);
    pub(in crate::ui) const SECONDARY: Color32 = super::palette::SUN_HI;
    pub(in crate::ui) const TERTIARY: Color32 = super::palette::SUN;
    pub(in crate::ui) const TERTIARY_CONTAINER: Color32 = Color32::from_rgb(48, 31, 26);
    pub(in crate::ui) const SURFACE_LOW: Color32 = Color32::from_rgb(11, 13, 18);
    pub(in crate::ui) const SURFACE: Color32 = Color32::from_rgb(20, 24, 33);
    pub(in crate::ui) const SURFACE_HIGH: Color32 = Color32::from_rgb(34, 39, 49);
    pub(in crate::ui) const OUTLINE: Color32 = Color32::from_rgb(76, 78, 84);
    pub(in crate::ui) const ON_SURFACE: Color32 = super::palette::TEXT_HI;
    pub(in crate::ui) const ON_SURFACE_VARIANT: Color32 = super::palette::TEXT_MID;
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
        s.text_styles.insert(TextStyle::Heading, FontId::new(24.0, FontFamily::Name("condensed".into())));
        s.text_styles.insert(TextStyle::Small, FontId::proportional(11.0));
        s.spacing.item_spacing = vec2(10.0, 10.0);
        s.spacing.button_padding = vec2(12.0, 8.0);
        s.spacing.interact_size.y = 44.0;
        s.visuals.override_text_color = Some(palette::TEXT_HI);
        s.visuals.panel_fill = INK;
        s.visuals.window_fill = Color32::from_rgba_unmultiplied(12, 13, 17, 246);
        s.visuals.window_corner_radius = 22.into();
        s.visuals.window_stroke = Stroke::new(1.0, expressive::OUTLINE);
        s.visuals.selection.bg_fill = expressive::PRIMARY_CONTAINER;
        s.visuals.selection.stroke = Stroke::new(1.0, expressive::PRIMARY);
        for w in [&mut s.visuals.widgets.inactive, &mut s.visuals.widgets.hovered, &mut s.visuals.widgets.active] {
            w.corner_radius = 14.into();
            w.bg_fill = expressive::SURFACE;
            w.weak_bg_fill = expressive::SURFACE_HIGH;
            w.bg_stroke = Stroke::NONE;
            w.fg_stroke = Stroke::new(1.5, palette::TEXT_HI);
        }
        s.visuals.widgets.hovered.bg_fill = expressive::SURFACE_HIGH;
        s.visuals.widgets.active.bg_stroke = Stroke::new(2.0, expressive::PRIMARY);
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
