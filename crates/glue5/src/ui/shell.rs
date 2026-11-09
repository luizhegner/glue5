//! The one shell every menu is drawn in, modeled on the GTA V pause menu: a
//! header, responsive tabs, rows with inline help, and a
//! footer of buttons — full screen, never a floating window.
//!
//! The pages supply the rows ([`super::pages::sheet`]); the shell draws the
//! chrome, the scene behind it, and says when a tab was asked for. The tab
//! strip is the app's pages, so the same chrome carries the start page, the
//! settings, the friends, the servers, the mods and the credits.
use std::cell::RefCell;

use egui::*;

use super::theme::{self, fonts, metrics, palette};
use super::{Page, widgets};

/// The tabs, in the pause menu's order: what the player came to do first, then
/// the lists, the mods, and the credits last.
pub(super) const TABS: [(Page, &str); 6] =
    [(Page::Start, "PLAY"), (Page::Settings, "SETTINGS"), (Page::Friends, "FRIENDS"), (Page::Servers, "SERVERS"), (Page::Mods, "MODS"), (Page::Credits, "CREDITS")];

thread_local! {
    /// The page being drawn, for the tab strip to mark.
    static PAGE: RefCell<Option<Page>> = const { RefCell::new(None) };
    /// A tab was clicked: go there (`None`: back to the game).
    static NAV: RefCell<Option<Option<Page>>> = const { RefCell::new(None) };
}

/// Before the pages are drawn: which one is open.
pub(super) fn begin(page: Option<Page>) {
    PAGE.with_borrow_mut(|current| *current = page);
}

pub(super) fn current() -> Option<Page> {
    PAGE.with_borrow(|page| *page)
}

/// The full output rectangle from the SDL window, kept distinct from egui's
/// safe content rectangle on devices with display cutouts or system bars.
const OUTPUT_RECT: &str = "glue5-full-output-rect";
const SESSION: &str = "shell-session";
const BLOCKED: &str = "shell-modal-open";

pub(super) fn set_blocked(ctx: &Context, blocked: bool) {
    ctx.data_mut(|d| d.insert_temp(Id::new(BLOCKED), blocked));
}

pub(super) fn set_session(ctx: &Context, running: bool, gamepad: bool) {
    ctx.data_mut(|d| d.insert_temp(Id::new(SESSION), (running, gamepad)));
}

fn session(ctx: &Context) -> (bool, bool) {
    ctx.data(|d| d.get_temp(Id::new(SESSION))).unwrap_or_default()
}

pub(super) fn input_hint(ctx: &Context) -> &'static str {
    if session(ctx).1 { "A  Select  ·  B  Back" }
    else if crate::paths::TOUCH { "Tap to select · Swipe to scroll" }
    else { "Enter  Select  ·  Esc  Back" }
}

pub(super) fn set_output_rect(ctx: &Context, rect: Rect) {
    ctx.data_mut(|data| data.insert_temp(Id::new(OUTPUT_RECT), rect));
}

pub(super) fn output_rect(ctx: &Context) -> Rect {
    ctx.data(|data| data.get_temp(Id::new(OUTPUT_RECT)))
        .unwrap_or_else(|| ctx.viewport_rect())
}

/// Asked for by a tab (or by a page's own link).
pub(super) fn nav(page: Option<Page>) {
    NAV.with_borrow_mut(|nav| *nav = Some(page));
}

/// What a tab asked for, once per frame.
pub(super) fn take_nav() -> Option<Option<Page>> {
    NAV.with_borrow_mut(Option::take)
}

/// The same edge-to-edge panel is used for paint, scene clipping and the
/// game-frame blur. Interactive controls remain inside `safe`.
pub(super) fn panel_rect(viewport: Rect, safe: Rect) -> Rect {
    let right = (viewport.left() + viewport.width() * 0.62)
        .max(safe.left() + (720.0 * theme::layout_scale(safe)).min(safe.width()))
        .clamp(safe.left(), safe.right());
    Rect::from_min_max(viewport.min, pos2(right, viewport.bottom()))
}

/// The shell around a page: the scene behind it, the header, the tabs, the
/// page's rows in the list column (scrolling), the description of the line the
/// pointer is on, and the footer. True when the player asked to go back.
pub(super) fn frame(ctx: &Context, title: &str, add: impl FnOnce(&mut Ui)) -> bool {
    let area = ctx.content_rect();
    let scale = theme::layout_scale(area);
    let viewport = output_rect(ctx);
    let full_panel = panel_rect(viewport, area);
    let panel_w = full_panel.right() - area.left();
    let pad = metrics::PAD * scale;
    let mut back = false;
    let page = current();
    let (running, _) = session(ctx);
    let can_go_back = running || page != Some(Page::Start);
    egui::Area::new(Id::new(("shell", title)))
        .order(Order::Middle)
        .fixed_pos(area.min)
        .interactable(true)
        .show(ctx, |ui| {
            if ctx.data(|d| d.get_temp::<bool>(Id::new(BLOCKED))).unwrap_or(false) {
                ui.disable();
            }
            ui.set_width(area.width());
            ui.set_height(area.height());
            ui.spacing_mut().item_spacing = Vec2::ZERO;
            // Nothing the pages draw leaves the screen.
            ui.set_clip_rect(area);
            let painter = ui.painter().clone();
            // Translucent black — the pause menu's block colour (0, 0, 0 at
            // 186/255, measured in the game's movies): the scene beneath has
            // been lightly softened inside this same rectangle and stays
            // sharp outside it. Preserve the full output clip, including
            // safe-area insets, so the panel reaches the phone's edges.
            let mut backdrop = ctx.layer_painter(ui.layer_id());
            backdrop.set_clip_rect(viewport);
            backdrop.rect_filled(full_panel, 0, palette::BLOCK);
            let panel = Rect::from_min_size(area.min, vec2(panel_w, area.height()));
            let header_h = metrics::HEADER * scale;
            let columns = if (panel_w - pad * 2.0) / TABS.len() as f32 >= 88.0 * scale { TABS.len() } else { 3 };
            let tabs_h = metrics::TABS * scale * TABS.len().div_ceil(columns) as f32;
            let footer_h = metrics::FOOTER * scale;
            let header = Rect::from_min_size(panel.min, vec2(panel.width(), header_h));
            let tabs = Rect::from_min_size(pos2(panel.left(), header.bottom()), vec2(panel.width(), tabs_h));
            let footer = Rect::from_min_size(pos2(panel.left(), panel.bottom() - footer_h), vec2(panel.width(), footer_h));
            let body = Rect::from_min_max(pos2(panel.left() + pad, tabs.bottom() + pad * 0.5), pos2(panel.right() - pad, footer.top() - pad * 0.5));

            // --- header: the product, the page, the build -------------------
            let brand = painter.layout_no_wrap("GLUE 5".to_owned(), FontId::new(17.0 * scale, fonts::heavy()), palette::TEXT_HI);
            painter.galley(pos2(header.left() + pad, header.center().y - brand.size().y * 0.5), brand.clone(), palette::TEXT_HI);
            painter.text(
                pos2(header.right() - pad, header.center().y),
                Align2::RIGHT_CENTER,
                crate::version::LABEL,
                FontId::new(10.5 * scale, fonts::regular()),
                palette::TEXT_LOW,
            );
            painter.line_segment([header.left_bottom(), header.right_bottom()], Stroke::new(1.0, palette::LINE));

            // --- the tabs ---------------------------------------------------
            ui.scope_builder(UiBuilder::new().max_rect(tabs.shrink2(vec2(pad, 0.0))).layout(Layout::top_down(Align::Min)), |ui| {
                theme::apply_density(ui, scale);
                let tab_w = (tabs.width() - pad * 2.0) / columns as f32;
                for row in TABS.chunks(columns) {
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = 0.0;
                        for &(target, name) in row {
                            let label = if running && target == Page::Start { "RESUME" } else { name };
                            if widgets::tab_button(ui, label, page == Some(target), vec2(tab_w, metrics::TABS * scale)).clicked() {
                                if running && target == Page::Start { nav(None); }
                                else if page != Some(target) { nav(Some(target)); }
                            }
                        }
                    });
                }
            });
            painter.line_segment([tabs.left_bottom(), tabs.right_bottom()], Stroke::new(1.0, palette::LINE));

            let list = body;
            ui.scope_builder(UiBuilder::new().max_rect(list).layout(Layout::top_down(Align::Min)), |ui| {
                theme::apply_density(ui, scale);
                ui.set_width(list.width());
                ui.spacing_mut().item_spacing.y = 0.0;
                ScrollArea::vertical()
                    .id_salt(("shell-list", title))
                    .auto_shrink([false, false])
                    .max_height(list.height())
                    .show(ui, |ui| {
                        ui.set_width(list.width());
                        add(ui);
                    });
            });

            // --- footer -----------------------------------------------------
            painter.line_segment([footer.left_top(), footer.right_top()], Stroke::new(1.0, palette::LINE));
            // PLAY is the app's root, not a dismissible overlay. Do not show
            // a Close/Back control that has nowhere to go on that page.
            let hint = input_hint(ctx);
            let hint_width = (footer.width() - 2.0 * pad - if can_go_back { 116.0 * scale } else { 0.0 }).max(1.0);
            let mut hint_job = text::LayoutJob::simple_singleline(hint.into(), FontId::new(10.5 * scale, fonts::strong()), palette::TEXT_MID);
            hint_job.wrap = text::TextWrapping::truncate_at_width(hint_width);
            let hint_text = painter.layout_job(hint_job);
            painter.galley(pos2(footer.left() + pad, footer.center().y - hint_text.size().y * 0.5), hint_text, palette::TEXT_MID);
            if can_go_back {
                let close = Rect::from_min_size(pos2(footer.right() - pad - 104.0 * scale, footer.top()), vec2(104.0 * scale, footer.height()));
                let response = ui
                    .scope_builder(UiBuilder::new().max_rect(close).layout(Layout::centered_and_justified(Direction::LeftToRight)), |ui| {
                        ui.spacing_mut().interact_size.y = (close.height() - 2.0 * scale).max(1.0);
                        widgets::tab_button(ui, if running { "RESUME" } else { "BACK" }, false, close.size())
                    })
                    .inner;
                if response.clicked() {
                    if running { nav(None); } else { back = true; }
                }
            }
        });
    back
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn panel_reaches_viewport_edges_even_with_safe_area_insets() {
        use egui::{RawInput, SafeAreaInsets, Shape};

        let ctx = Context::default();
        theme::install(&ctx);
        let screen = Rect::from_min_size(Pos2::ZERO, vec2(844.0, 390.0));
        for _ in 0..4 {
            let mut output = ctx.run_ui(
                RawInput {
                    screen_rect: Some(screen),
                    safe_area_insets: Some(SafeAreaInsets(egui::epaint::MarginF32 {
                        left: 54.0,
                        right: 30.0,
                        top: 20.0,
                        bottom: 18.0,
                    })),
                    ..Default::default()
                },
                |root| {
                    set_output_rect(root.ctx(), screen);
                    frame(root.ctx(), "PLAY", |_| {});
                },
            );
            // Nothing here paints: the font atlas' delta is dropped as it is
            // in the other tests that run a frame (egui panics otherwise).
            output.textures_delta.clear();
            let panel = panel_rect(screen, ctx.content_rect());
            let acrylic = output.shapes.iter().find(|shape| match &shape.shape {
                Shape::Rect(rect) => {
                    rect.rect == panel
                        && rect.fill == palette::BLOCK
                }
                _ => false,
            });
            if let Some(acrylic) = acrylic {
                assert_eq!(acrylic.clip_rect, screen, "acrylic must not be clipped to the safe area");
                return;
            }
        }
        panic!("acrylic panel was not painted");
    }

    #[test]
    fn shell_panel_bleeds_to_the_output_edges_but_stays_safe_on_the_right() {
        let output = Rect::from_min_size(Pos2::ZERO, vec2(1568.0, 704.0));
        let safe = Rect::from_min_max(pos2(100.0, 52.0), pos2(1468.0, 650.0));
        let panel = panel_rect(output, safe);
        assert_eq!(panel.left_top(), output.left_top());
        assert_eq!(panel.left_bottom(), output.left_bottom());
        assert!((panel.right() - 972.16).abs() < 0.1);
        assert_eq!(panel.bottom(), 704.0);
    }
}
