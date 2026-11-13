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
    [(Page::Start, "PLAY"), (Page::Settings, "SETTINGS"), (Page::Friends, "FRIENDS"), (Page::Servers, "GLUE SERVER"), (Page::Mods, "MODS"), (Page::Credits, "CREDITS")];

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
const KEYBOARD: &str = "shell-keyboard-height";
const ARTWORK_READY: &str = "shell-artwork-ready";

pub(super) fn set_artwork_ready(ctx: &Context, ready: bool) {
    ctx.data_mut(|d| d.insert_temp(Id::new(ARTWORK_READY), ready));
}

pub(super) fn set_keyboard_height(ctx: &Context, height: f32) {
    ctx.data_mut(|d| d.insert_temp(Id::new(KEYBOARD), height.max(0.0)));
}

/// Keep cutout protection at the sides without turning Android gesture
/// margins into a letterboxed menu. Only an actual keyboard shortens it.
fn control_rect(viewport: Rect, safe: Rect, keyboard: f32) -> Rect {
    let left = safe.left().clamp(viewport.left(), viewport.right());
    let right = safe.right().clamp(left, viewport.right());
    Rect::from_min_max(pos2(left, viewport.top()),
        pos2(right, (viewport.bottom() - keyboard.max(0.0)).clamp(viewport.top(), viewport.bottom())))
}

fn full_strip(viewport: Rect, controls: Rect) -> Rect {
    Rect::from_min_max(pos2(viewport.left(), controls.top()), pos2(viewport.right(), controls.bottom()))
}

fn list_rect(body: Rect, _artwork: bool, _scale: f32) -> Rect {
    body
}

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

/// Paint and blur fill the output; interactive controls use the safe area.
pub(super) fn panel_rect(viewport: Rect, _safe: Rect) -> Rect {
    viewport
}

/// The shell around a page: the scene behind it, the header, the tabs, the
/// page's rows in the list column (scrolling), the description of the line the
/// pointer is on, and the footer. True when the player asked to go back.
pub(super) fn frame(ctx: &Context, title: &str, add: impl FnOnce(&mut Ui)) -> bool {
    let viewport = output_rect(ctx);
    let keyboard = ctx.data(|d| d.get_temp::<f32>(Id::new(KEYBOARD))).unwrap_or(0.0);
    let area = control_rect(viewport, ctx.content_rect(), keyboard);
    let scale = theme::menu_scale(ctx, area);
    let full_panel = panel_rect(viewport, area);
    let panel_w = area.width();
    let pad = metrics::PAD * scale;
    let mut back = false;
    let page = current();
    let (running, _) = session(ctx);
    let home = page == Some(Page::Start) && !running;
    let artwork = home && ctx.data(|d| d.get_temp::<bool>(Id::new(ARTWORK_READY))).unwrap_or(false);
    let can_go_back = page != Some(Page::Start);
    egui::Area::new(Id::new(("shell", title)))
        .order(Order::Middle)
        .fixed_pos(area.min)
        .constrain(false)
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
            // 186/255, measured in the game's movies). The background is
            // softened across the full output. Preserve the clip, including
            // safe-area insets, so the panel reaches the phone's edges.
            let mut backdrop = ctx.layer_painter(ui.layer_id());
            backdrop.set_clip_rect(viewport);
            backdrop.rect_filled(full_panel, 0, if artwork { Color32::from_black_alpha(36) } else { palette::BLOCK });
            let panel = Rect::from_min_size(area.min, vec2(panel_w, area.height()));
            let columns = if (panel_w - pad * 2.0) / TABS.len() as f32 >= 88.0 * scale { TABS.len() } else { 3 };
            let normal_tabs_h = metrics::TABS * scale * TABS.len().div_ceil(columns) as f32;
            let footer_h = metrics::FOOTER * scale;
            // A keyboard can leave less height than the normal chrome alone.
            // Keep the editor/list and Back usable; do not shrink touch targets
            // or let zero/negative body bounds expand the Area off-screen.
            let compact = area.height() < metrics::HEADER * scale + normal_tabs_h + footer_h + 2.0 * metrics::ROW * scale;
            let header_h = if compact { 0.0 } else { metrics::HEADER * scale };
            let tabs_h = if compact { 0.0 } else { normal_tabs_h };
            let header = Rect::from_min_size(panel.min, vec2(panel.width(), header_h));
            let tabs = Rect::from_min_size(pos2(panel.left(), header.bottom()), vec2(panel.width(), tabs_h));
            let footer = Rect::from_min_size(pos2(panel.left(), panel.bottom() - footer_h), vec2(panel.width(), footer_h));
            let body = Rect::from_min_max(pos2(panel.left() + pad, tabs.bottom() + pad * 0.5), pos2(panel.right() - pad, footer.top() - pad * 0.5));

            // Let the build's artwork breathe on PLAY. Other pages keep
            // the full width needed by settings, diagnostics and lists.
            let list = list_rect(body,artwork,scale);
            if home {
                backdrop.rect_filled(full_strip(viewport, header), 0, palette::BLOCK);
                backdrop.rect_filled(full_strip(viewport, tabs), 0, palette::BLOCK);
                backdrop.rect_filled(full_strip(viewport, footer), 0, palette::BLOCK);
                if artwork { painter.rect_filled(list.expand2(vec2(pad*0.5,0.0)), 0, palette::BLOCK); }
            }

            // --- header: the product, the page, the build -------------------
            if !compact {
            let brand = painter.layout_no_wrap("GLUE 5".to_owned(), FontId::new(17.0 * scale, fonts::heavy()), palette::TEXT_HI);
            painter.galley(pos2(header.left() + pad, header.center().y - brand.size().y * 0.5), brand.clone(), palette::TEXT_HI);
            let edge = full_strip(viewport, header);
            backdrop.line_segment([edge.left_bottom(), edge.right_bottom()], Stroke::new(1.0, palette::LINE));

            // --- the tabs ---------------------------------------------------
            ui.scope_builder(UiBuilder::new().max_rect(tabs.shrink2(vec2(pad, 0.0))).layout(Layout::top_down(Align::Min)), |ui| {
                theme::apply_density(ui, scale);
                let tab_w = (tabs.width() - pad * 2.0) / columns as f32;
                for row in TABS.chunks(columns) {
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = 0.0;
                        for &(target, name) in row {
                            let label = name;
                            let selected = page == Some(target) || (page == Some(Page::Saves) && target == Page::Start);
                            if widgets::tab_button(ui, label, selected, vec2(tab_w, metrics::TABS * scale)).clicked() {
                                if page != Some(target) { nav(Some(target)); }
                            }
                        }
                    });
                }
            });
            let edge = full_strip(viewport, tabs);
            backdrop.line_segment([edge.left_bottom(), edge.right_bottom()], Stroke::new(1.0, palette::LINE));
            }

            ui.scope_builder(UiBuilder::new().max_rect(list).layout(Layout::top_down(Align::Min)), |ui| {
                theme::apply_density(ui, scale);
                ui.set_width(list.width());
                ui.spacing_mut().item_spacing.y = 0.0;
                widgets::scroll_vertical()
                    .id_salt(("shell-list", title))
                    .auto_shrink([false, false])
                    .max_height(list.height())
                    .show(ui, |ui| {
                        ui.set_width(list.width());
                        add(ui);
                    });
            });

            // --- footer -----------------------------------------------------
            let edge = full_strip(viewport, footer);
            backdrop.line_segment([edge.left_top(), edge.right_top()], Stroke::new(1.0, palette::LINE));
            // PLAY is the app's root, not a dismissible overlay. Do not show
            // a Close/Back control that has nowhere to go on that page.
            let hint = input_hint(ctx);
            let action_width = if running {
                ((footer.width() - 2.0 * pad - 16.0 * scale) / 3.0).min(104.0 * scale).max(1.0)
            } else { 104.0 * scale };
            let exit_width = if running { 2.0 * (action_width + 8.0 * scale) } else { 0.0 };
            if running {
                let exit = Rect::from_min_size(pos2(footer.left() + pad, footer.top()), vec2(action_width, footer.height()));
                let response = ui.scope_builder(UiBuilder::new().max_rect(exit).layout(Layout::centered_and_justified(Direction::LeftToRight)), |ui| {
                    widgets::tab_button(ui, "EXIT", false, exit.size())
                }).inner;
                if response.clicked() {
                    super::popup::ask(super::popup::Request::quit());
                }
                let home = Rect::from_min_size(pos2(exit.right() + 8.0 * scale, footer.top()), vec2(action_width, footer.height()));
                let response = ui.scope_builder(UiBuilder::new().max_rect(home).layout(Layout::centered_and_justified(Direction::LeftToRight)), |ui| {
                    widgets::tab_button(ui, "HOME", false, home.size())
                }).inner;
                if response.clicked() { nav(Some(Page::Start)); }
            }
            let text_space = Rect::from_min_max(
                pos2(footer.left() + pad + exit_width, footer.top()),
                pos2((footer.right() - pad - if can_go_back { action_width + 12.0 * scale } else { 0.0 }).max(footer.left() + pad + exit_width), footer.bottom()));
            footer_text(&painter, text_space, hint, crate::version::LABEL, scale);
            if can_go_back {
                let close = Rect::from_min_size(pos2(footer.right() - pad - action_width, footer.top()), vec2(action_width, footer.height()));
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

/// One footer line, not a second floating badge below it. Identity has
/// priority over the optional hint when the keyboard leaves little room.
fn footer_text(painter: &Painter, rect: Rect, hint: &str, identity: &str, scale: f32) {
    let font = FontId::new(10.5 * scale, fonts::strong());
    let mut job = text::LayoutJob::simple_singleline(identity.into(), font.clone(), palette::TEXT_MID);
    job.wrap = text::TextWrapping::truncate_at_width(rect.width().max(1.0));
    let identity = painter.layout_job(job);
    let left = rect.right() - identity.size().x;
    painter.galley(pos2(left, rect.center().y - identity.size().y * 0.5), identity, palette::TEXT_MID);
    let hint_width = left - rect.left() - 16.0 * scale;
    if hint_width >= 80.0 * scale {
        let mut job = text::LayoutJob::simple_singleline(hint.into(), font, palette::TEXT_MID);
        job.wrap = text::TextWrapping::truncate_at_width(hint_width);
        let hint = painter.layout_job(job);
        painter.galley(pos2(rect.left(), rect.center().y - hint.size().y * 0.5), hint, palette::TEXT_MID);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identity_and_input_hint_share_the_footer_line() {
        let ctx = Context::default(); theme::install(&ctx);
        for _ in 0..4 {
            let mut output = ctx.run_ui(RawInput {screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(915.0,412.0))), ..Default::default()}, |root| {
                begin(Some(Page::Start)); set_session(root.ctx(),false,false);
                frame(root.ctx(),"PLAY",|_|{});
            });
            output.textures_delta.clear();
            let text_rect = |label:&str| output.shapes.iter().find_map(|s| match &s.shape {
                Shape::Text(t) if t.galley.text()==label => Some(Rect::from_min_size(t.pos,t.galley.size())), _=>None,
            });
            if let (Some(identity),Some(hint)) = (text_rect(crate::version::LABEL),text_rect(input_hint(&ctx))) {
                assert!((identity.center().y-hint.center().y).abs()<0.1);
                assert!(identity.bottom()<=412.0 && identity.top()>=368.0);
                assert!(hint.right()<identity.left());
                return;
            }
        }
        panic!("footer text not drawn");
    }

    #[test]
    fn no_artwork_means_no_reserved_empty_half_screen() {
        let body=Rect::from_min_size(pos2(24.0,110.0),vec2(900.0,280.0));
        assert_eq!(list_rect(body,false,1.0),body);
        assert_eq!(list_rect(body,true,1.0),body);
        let narrow=Rect::from_min_size(body.min,vec2(360.0,280.0));
        assert_eq!(list_rect(narrow,true,1.0),narrow);
    }

    #[test]
    fn gesture_margins_do_not_letterbox_the_menu_but_keyboard_does() {
        let viewport=Rect::from_min_size(Pos2::ZERO,vec2(915.0,412.0));
        let safe=Rect::from_min_max(pos2(58.0,30.0),pos2(885.0,380.0));
        let controls=control_rect(viewport,safe,0.0);
        assert_eq!(controls,Rect::from_min_max(pos2(58.0,0.0),pos2(885.0,412.0)));
        assert_eq!(control_rect(viewport,safe,180.0).bottom(),232.0);
        assert_eq!(control_rect(viewport,safe,1000.0).height(),0.0);
        let strip=full_strip(viewport,Rect::from_min_max(pos2(58.0,40.0),pos2(885.0,84.0)));
        assert_eq!(strip,Rect::from_min_max(pos2(0.0,40.0),pos2(915.0,84.0)));
    }

    #[test]
    fn header_uses_the_top_edge_not_the_android_gesture_inset() {
        let ctx=Context::default(); theme::install(&ctx);
        let viewport=Rect::from_min_size(Pos2::ZERO,vec2(915.0,412.0));
        for _ in 0..4 {
            let mut output=ctx.run_ui(egui::RawInput {
                screen_rect:Some(viewport),
                safe_area_insets:Some(egui::SafeAreaInsets(egui::epaint::MarginF32{left:58.0,right:30.0,top:30.0,bottom:32.0})),
                ..Default::default()
            },|root| {
                set_output_rect(root.ctx(),viewport); begin(Some(Page::Start));
                frame(root.ctx(),"PLAY",|_|{});
            });
            output.textures_delta.clear();
            let title=output.shapes.iter().find_map(|shape| match &shape.shape {
                egui::Shape::Text(text) if text.galley.text()=="GLUE 5" => Some(text.pos), _=>None,
            });
            if let Some(title)=title { assert!(title.y<30.0,"header still inside gesture frame: {title:?}"); return; }
        }
        panic!("header not drawn");
    }

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
    fn shell_panel_fills_the_output_including_safe_area_insets() {
        let output = Rect::from_min_size(Pos2::ZERO, vec2(1568.0, 704.0));
        let safe = Rect::from_min_max(pos2(100.0, 52.0), pos2(1468.0, 650.0));
        let panel = panel_rect(output, safe);
        assert_eq!(panel.left_top(), output.left_top());
        assert_eq!(panel.left_bottom(), output.left_bottom());
        assert_eq!(panel.right(), output.right());
        assert_eq!(panel.bottom(), 704.0);
    }
}
