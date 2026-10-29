//! The visual system of every GLUE 5 menu: Manrope, a dark shell of flat
//! panels and 1 px rules, square corners, and one limes accent.
//!
//! Modeled on the GTA V pause menu's structure (a header, a strip of tabs, a
//! list of rows, a description, a footer of buttons) and on Arena.ai's shell
//! (1 px borders, small radii, dense rows, a solid white selection) — see
//! `docs/ui-redesign-gta-v.md`.
use egui::*;

/// The shell's own near-black; also the text of a selected row.
pub(super) const INK: Color32 = Color32::from_rgb(14, 15, 17);
pub(super) const MUTED: Color32 = Color32::from_rgb(185, 183, 177);

#[allow(dead_code, reason = "the shell's tokens are adopted page by page")]
pub(super) mod palette {
    use egui::Color32;

    /// The page behind everything, and the text on a selected row.
    pub(in crate::ui) const INK_950: Color32 = Color32::from_rgb(14, 15, 17);
    /// A raised strip (the tab bar's trough, the footer).
    pub(in crate::ui) const INK_800: Color32 = Color32::from_rgb(20, 21, 24);
    pub(in crate::ui) const INK_700: Color32 = Color32::from_rgb(30, 32, 36);
    /// 1 px rules: every frame, the tab trough, the description panel.
    pub(in crate::ui) const LINE: Color32 = Color32::from_rgb(44, 47, 53);
    /// The finer rules between rows.
    pub(in crate::ui) const SEPARATOR: Color32 = Color32::from_rgb(34, 36, 42);
    pub(in crate::ui) const TEXT_HI: Color32 = Color32::from_rgb(242, 242, 240);
    pub(in crate::ui) const TEXT_MID: Color32 = Color32::from_rgb(185, 183, 177);
    pub(in crate::ui) const TEXT_LOW: Color32 = Color32::from_rgb(130, 129, 127);
    /// The one accent, used for emphasis only: the active tab's underline, a
    /// changed value, the mouse cursor, a destructive action's label.
    pub(in crate::ui) const LIME: Color32 = Color32::from_rgb(168, 216, 52);
    pub(in crate::ui) const ACCENT: Color32 = LIME;
}

/// The names the older pages were written with, resolved to the shell's
/// tokens: one dark surface, one rule, one accent. Kept so that pages not yet
/// rebuilt on the shell's rows stay on the same palette.
#[allow(dead_code, reason = "pages not yet rebuilt on the shell's rows")]
pub(super) mod expressive {
    use egui::Color32;

    pub(in crate::ui) const PRIMARY: Color32 = super::palette::LIME;
    /// Text on the accent (and on a selected row).
    pub(in crate::ui) const ON_PRIMARY: Color32 = super::palette::INK_950;
    pub(in crate::ui) const PRIMARY_CONTAINER: Color32 = super::palette::INK_700;
    pub(in crate::ui) const SECONDARY: Color32 = super::palette::TEXT_MID;
    pub(in crate::ui) const TERTIARY: Color32 = super::palette::LIME;
    pub(in crate::ui) const TERTIARY_CONTAINER: Color32 = super::palette::INK_700;
    pub(in crate::ui) const SURFACE_LOW: Color32 = super::palette::INK_950;
    pub(in crate::ui) const SURFACE: Color32 = super::palette::INK_800;
    pub(in crate::ui) const SURFACE_HIGH: Color32 = super::palette::INK_700;
    pub(in crate::ui) const OUTLINE: Color32 = super::palette::LINE;
    pub(in crate::ui) const ON_SURFACE: Color32 = super::palette::TEXT_HI;
    pub(in crate::ui) const ON_SURFACE_VARIANT: Color32 = super::palette::TEXT_MID;
}

/// Fonts, by the weight they carry (three files, three roles).
#[allow(dead_code, reason = "the three weights are used as the shell is adopted")]
pub(super) mod fonts {
    use egui::FontFamily;

    /// 400: descriptions, values, anything read as a sentence.
    pub(in crate::ui) fn regular() -> FontFamily {
        FontFamily::Proportional
    }

    /// 600: almost everything that is a control — rows, tabs, footers, labels.
    pub(in crate::ui) fn strong() -> FontFamily {
        FontFamily::Name("strong".into())
    }

    /// 800: the title, a tab's name on the page, the scene's caption.
    pub(in crate::ui) fn heavy() -> FontFamily {
        FontFamily::Name("heavy".into())
    }
}

/// The vertical rhythm, in points, for the 915 × 394 landscape target (Galaxy
/// S21+, the smallest screen this must hold up on). `layout_scale` shrinks it
/// on anything smaller and grows it a little on tablets.
#[allow(dead_code, reason = "the shell's rhythm is adopted page by page")]
pub(super) mod metrics {
    /// The header: the product's name, the page's name, the date.
    pub(in crate::ui) const HEADER: f32 = 48.0;
    /// The tab strip, and its height in the layout.
    pub(in crate::ui) const TABS: f32 = 38.0;
    /// One row of a list, one setting.
    pub(in crate::ui) const ROW: f32 = 38.0;
    /// The footer's buttons.
    pub(in crate::ui) const FOOTER: f32 = 36.0;
    /// Left and right margin of everything on the page.
    pub(in crate::ui) const PAD: f32 = 18.0;
    /// The list's share of the width, the description's the rest.
    pub(in crate::ui) const LIST: f32 = 0.58;
    /// The vertical screen (descriptions and help) is a fifth of the width.
    pub(in crate::ui) const GAP: f32 = 14.0;
}

#[allow(dead_code, reason = "used as pages adopt the shell's type")]
pub(super) fn note(text: &str, size: f32) -> RichText {
    RichText::new(text).font(FontId::new(size, fonts::regular())).color(palette::TEXT_MID)
}

#[allow(dead_code, reason = "used as pages adopt the shell's type")]
pub(super) fn label(text: &str, size: f32) -> RichText {
    RichText::new(text).font(FontId::new(size, fonts::strong())).color(palette::TEXT_HI)
}

pub(super) fn heading(text: &str, size: f32) -> RichText {
    RichText::new(text).font(FontId::new(size, fonts::heavy())).color(palette::TEXT_HI)
}

pub(super) fn install(ctx: &Context) {
    ctx.set_theme(Theme::Dark);
    let mut fonts = FontDefinitions::default();
    for (name, bytes) in [
        ("regular", include_bytes!("../../assets/ui/Manrope-Regular.ttf").as_slice()),
        ("strong", include_bytes!("../../assets/ui/Manrope-SemiBold.ttf").as_slice()),
        ("heavy", include_bytes!("../../assets/ui/Manrope-ExtraBold.ttf").as_slice()),
    ] {
        fonts.font_data.insert(name.into(), FontData::from_static(bytes).into());
    }
    fonts.families.get_mut(&FontFamily::Proportional).unwrap().insert(0, "regular".into());
    for family in ["strong", "heavy", "condensed"] {
        let mut chain = vec![family.into()];
        chain.extend(fonts.families[&FontFamily::Proportional].clone());
        fonts.families.insert(FontFamily::Name(family.into()), chain);
    }
    ctx.set_fonts(fonts);
    ctx.all_styles_mut(|s| {
        s.text_styles.insert(TextStyle::Body, FontId::new(14.0, fonts::strong()));
        s.text_styles.insert(TextStyle::Button, FontId::new(14.0, fonts::strong()));
        s.text_styles.insert(TextStyle::Heading, FontId::new(21.0, fonts::heavy()));
        s.text_styles.insert(TextStyle::Small, FontId::new(11.0, fonts::strong()));
        s.text_styles.insert(TextStyle::Monospace, FontId::new(13.0, fonts::strong()));
        s.spacing.item_spacing = vec2(8.0, 8.0);
        s.spacing.button_padding = vec2(10.0, 6.0);
        s.spacing.interact_size.y = metrics::ROW;
        s.visuals.override_text_color = Some(palette::TEXT_HI);
        s.visuals.panel_fill = palette::INK_950;
        s.visuals.window_fill = palette::INK_950;
        s.visuals.extreme_bg_color = palette::INK_800;
        // Square corners everywhere: the shell is a frame of flat panels, and
        // only the smallest elements (a focus ring, a slider handle) round.
        s.visuals.window_corner_radius = 0.into();
        s.visuals.menu_corner_radius = 0.into();
        s.visuals.window_stroke = Stroke::new(1.0, palette::LINE);
        // Selection is the menu's one loud thing: a solid light row, dark text.
        s.visuals.selection.bg_fill = palette::TEXT_HI;
        s.visuals.selection.stroke = Stroke::new(1.0, palette::TEXT_HI);
        for w in [&mut s.visuals.widgets.inactive, &mut s.visuals.widgets.hovered, &mut s.visuals.widgets.active] {
            w.corner_radius = 0.into();
            w.bg_fill = Color32::TRANSPARENT;
            w.weak_bg_fill = Color32::TRANSPARENT;
            w.bg_stroke = Stroke::NONE;
            w.fg_stroke = Stroke::new(1.0, palette::TEXT_HI);
        }
        s.visuals.widgets.hovered.bg_fill = Color32::from_rgba_unmultiplied(242, 242, 240, 20);
        s.visuals.widgets.active.bg_stroke = Stroke::new(1.0, palette::LIME);
        s.visuals.hyperlink_color = palette::TEXT_HI;
    });
}

/// Density follows available space, including landscape phones and split views.
/// 394 pt tall (the S21+ in landscape) is 1.0.
pub(super) fn layout_scale(rect: Rect) -> f32 {
    (rect.height() / 394.0).min(rect.width() / 780.0).clamp(0.72, 1.15)
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
