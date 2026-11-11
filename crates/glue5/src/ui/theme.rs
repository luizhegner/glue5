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
    /// The movies' white: RGB 240, not 255 — text and the focused block.
    pub(in crate::ui) const TEXT_HI: Color32 = Color32::from_rgb(240, 240, 240);
    /// The pause menu's block: pure black at 186/255 (73%), measured in the
    /// game's movies — rows, tabs and the panel itself.
    pub(in crate::ui) const BLOCK: Color32 = Color32::from_rgba_premultiplied(0, 0, 0, 186);
    pub(in crate::ui) const TEXT_MID: Color32 = Color32::from_rgb(185, 183, 177);
    pub(in crate::ui) const TEXT_LOW: Color32 = Color32::from_rgb(130, 129, 127);
    /// The one accent, used for emphasis only: the active tab's underline, a
    /// changed value, the mouse cursor, a destructive action's label.
    pub(in crate::ui) const LIME: Color32 = Color32::from_rgb(168, 216, 52);
    pub(in crate::ui) const ACCENT: Color32 = LIME;
    /// The accent's older name, kept for the overlay and the stage's lines.
    pub(in crate::ui) const SUN: Color32 = LIME;
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
/// S21+). Narrow layouts reflow instead of shrinking touch targets;
/// tablets grow the rhythm slightly.
#[allow(dead_code, reason = "the shell's rhythm is adopted page by page")]
pub(super) mod metrics {
    /// The header: the product's name, the page's name, the date.
    pub(in crate::ui) const HEADER: f32 = 44.0;
    /// The tab strip, and its height in the layout.
    pub(in crate::ui) const TABS: f32 = 44.0;
    /// One row of a list, one setting.
    pub(in crate::ui) const ROW: f32 = 44.0;
    /// The footer's buttons.
    pub(in crate::ui) const FOOTER: f32 = 44.0;
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
    install_with_game_font(ctx, None);
}

fn install_with_game_font(ctx: &Context, game_font: Option<Vec<u8>>) {
    ctx.set_theme(Theme::Dark);
    let mut fonts = FontDefinitions::default();
    for (name, bytes) in [
        ("regular", include_bytes!("../../assets/ui/Manrope-Regular.ttf").as_slice()),
        ("strong", include_bytes!("../../assets/ui/Manrope-SemiBold.ttf").as_slice()),
        ("heavy", include_bytes!("../../assets/ui/Manrope-ExtraBold.ttf").as_slice()),
    ] {
        fonts.font_data.insert(name.into(), FontData::from_static(bytes).into());
    }
    // The selected build is the only source of game fonts. Manrope remains
    // a fallback for setup, missing resources and glyphs absent from Chalet.
    let player = game_font.is_some();
    if let Some(bytes) = game_font {
        fonts.font_data.insert("player".into(), FontData::from_owned(bytes).into());
    }
    let first: &str = if player { "player" } else { "regular" };
    fonts.families.get_mut(&FontFamily::Proportional).unwrap().insert(0, first.into());
    // Each named family starts at a font file that exists: a chain whose first
    // name has no data is a panic when a glyph is laid out. Manrope has no
    // condensed cut, so "condensed" (the old Barlow Condensed) is the bold one.
    for (family, face) in [("strong", "strong"), ("heavy", "heavy"), ("condensed", "heavy")] {
        // The game's faces are a single weight: with them, emphasis is size
        // and the condensed cut, exactly as in the original menus.
        let face = if player { "player" } else { face };
        let mut chain = vec![face.into()];
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
        s.visuals.selection.stroke = Stroke::new(1.0, palette::INK_950);
        for w in [&mut s.visuals.widgets.inactive, &mut s.visuals.widgets.hovered, &mut s.visuals.widgets.active] {
            w.corner_radius = 0.into();
            w.bg_fill = Color32::TRANSPARENT;
            w.weak_bg_fill = Color32::TRANSPARENT;
            w.bg_stroke = Stroke::NONE;
            w.fg_stroke = Stroke::new(1.0, palette::TEXT_HI);
        }
        s.visuals.widgets.hovered.bg_fill = Color32::from_rgba_unmultiplied(240, 240, 240, 20);
        s.visuals.widgets.active.bg_stroke = Stroke::new(1.0, palette::LIME);
        s.visuals.hyperlink_color = palette::TEXT_HI;
    });
}

/// The game folder can become available after startup (notably when a stream
/// connects). Check periodically, not by reading the file on every frame.
pub(super) fn update_game_font(ctx: &Context, game: Option<&std::path::Path>, build: &str) {
    let root = game.map(std::path::Path::to_path_buf);
    let root_id = Id::new("ui-font-game-root");
    let loaded_id = Id::new("ui-font-game-loaded");
    let poll_id = Id::new("ui-font-game-poll");
    let previous = ctx.data(|d| d.get_temp::<Option<std::path::PathBuf>>(root_id));
    if previous.as_ref() != Some(&root) {
        if previous.is_some() { install(ctx); }
        ctx.data_mut(|d| {
            d.insert_temp(root_id, root.clone());
            d.insert_temp(loaded_id, false);
            d.insert_temp(poll_id, -10.0f64);
        });
    }
    let Some(root) = root else { return };
    if ctx.data(|d| d.get_temp::<bool>(loaded_id)).unwrap_or(false) { return; }
    let now = ctx.input(|i| i.time);
    let last = ctx.data(|d| d.get_temp::<f64>(poll_id)).unwrap_or(-10.0);
    if now - last < 2.0 { return; }
    ctx.data_mut(|d| d.insert_temp(poll_id, now));
    let path = root.join("b").join(build).join("title/chalet.woff");
    // Refuse oversized files before allocating them.
    let bytes = std::fs::metadata(&path).ok().filter(|m| m.len() <= 8 * 1024 * 1024)
        .and_then(|_| std::fs::read(&path).ok());
    if let Some(font) = bytes.as_deref().and_then(super::woff::decode) {
        log::info!("fonts: loaded Chalet from the game build");
        install_with_game_font(ctx, Some(font));
        ctx.data_mut(|d| d.insert_temp(loaded_id, true));
    }
}

/// Density follows width, not the height left above the on-screen keyboard.
pub(super) fn layout_scale(rect: Rect) -> f32 {
    // Opening the keyboard must not shrink every label and touch target.
    (rect.width() / 844.0).clamp(1.0, 1.15)
}

pub(super) fn set_ui_scale(ctx: &egui::Context, value: f32) {
    let value = if value.is_finite() { value.clamp(0.85, 1.30) } else { 1.0 };
    ctx.data_mut(|d| d.insert_temp(egui::Id::new("glue5-ui-scale"), value));
}

pub(super) fn menu_scale(ctx: &egui::Context, rect: Rect) -> f32 {
    layout_scale(rect) * ctx.data(|d| d.get_temp::<f32>(egui::Id::new("glue5-ui-scale"))).unwrap_or(1.0)
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
