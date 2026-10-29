//! The one shell every menu is drawn in, modeled on the GTA V pause menu: a
//! header, a strip of tabs, a list of rows with a description beside it, and a
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

/// Asked for by a tab (or by a page's own link).
pub(super) fn nav(page: Option<Page>) {
    NAV.with_borrow_mut(|nav| *nav = Some(page));
}

/// What a tab asked for, once per frame.
pub(super) fn take_nav() -> Option<Option<Page>> {
    NAV.with_borrow_mut(Option::take)
}

/// The shell around a page: the scene behind it, the header, the tabs, the
/// page's rows in the list column (scrolling), the description of the line the
/// pointer is on, and the footer. True when the player asked to go back.
pub(super) fn frame(ctx: &Context, title: &str, add: impl FnOnce(&mut Ui)) -> bool {
    let area = ctx.content_rect();
    let scale = theme::layout_scale(area);
    let panel_w = (area.width() * 0.62).max(area.width() * 0.55);
    let pad = metrics::PAD * scale;
    let mut back = false;
    let page = current();

    egui::Area::new(Id::new(("shell", title)))
        .order(Order::Middle)
        .fixed_pos(area.min)
        .interactable(true)
        .show(ctx, |ui| {
            ui.set_width(area.width());
            ui.set_height(area.height());
            ui.spacing_mut().item_spacing = Vec2::ZERO;
            // Nothing the pages draw leaves the screen.
            ui.set_clip_rect(area);
            let painter = ui.painter().clone();
            // The scene shows through: the whole screen is darkened, the panel
            // (62 %) is nearly solid, and the art stays visible at the right.
            painter.rect_filled(area, 0, Color32::from_rgba_unmultiplied(9, 10, 12, 168));
            let panel = Rect::from_min_size(area.min, vec2(panel_w.min(area.width()), area.height()));
            painter.rect_filled(panel, 0, Color32::from_rgba_unmultiplied(14, 15, 17, 240));
            painter.line_segment([panel.right_top(), panel.right_bottom()], Stroke::new(1.0, palette::LINE));

            let header_h = metrics::HEADER * scale;
            let tabs_h = metrics::TABS * scale;
            let footer_h = metrics::FOOTER * scale;
            let header = Rect::from_min_size(panel.min, vec2(panel.width(), header_h));
            let tabs = Rect::from_min_size(pos2(panel.left(), header.bottom()), vec2(panel.width(), tabs_h));
            let footer = Rect::from_min_size(pos2(panel.left(), panel.bottom() - footer_h), vec2(panel.width(), footer_h));
            let body = Rect::from_min_max(pos2(panel.left() + pad, tabs.bottom() + pad * 0.5), pos2(panel.right() - pad, footer.top() - pad * 0.5));

            // --- header: the product, the page, the build -------------------
            let brand = painter.layout_no_wrap("GLUE 5".to_owned(), FontId::new(17.0 * scale, fonts::heavy()), palette::TEXT_HI);
            painter.galley(pos2(header.left() + pad, header.center().y - brand.size().y * 0.5), brand.clone(), palette::TEXT_HI);
            painter.text(
                pos2(header.left() + pad + brand.size().x + 12.0 * scale, header.center().y),
                Align2::LEFT_CENTER,
                title,
                FontId::new(12.0 * scale, fonts::strong()),
                palette::TEXT_MID,
            );
            painter.text(
                pos2(header.right() - pad, header.center().y),
                Align2::RIGHT_CENTER,
                crate::version::LABEL,
                FontId::new(10.5 * scale, fonts::regular()),
                palette::TEXT_LOW,
            );
            painter.line_segment([header.left_bottom(), header.right_bottom()], Stroke::new(1.0, palette::LINE));

            // --- the tabs ---------------------------------------------------
            let tab_count = TABS.len() as f32;
            let tab_w = ((tabs.width() - pad * 2.0) / tab_count).min(112.0 * scale);
            for (index, (target, name)) in TABS.iter().enumerate() {
                let rect = Rect::from_min_size(pos2(tabs.left() + pad + index as f32 * tab_w, tabs.top() + 4.0 * scale), vec2(tab_w, tabs.height() - 8.0 * scale));
                let here = page == Some(*target);
                let response = ui.interact(rect, Id::new(("shell-tab", name)), Sense::click());
                response.widget_info(|| WidgetInfo::selected(WidgetType::Button, true, here, *name));
                if here {
                    painter.rect_filled(rect, 0, palette::TEXT_HI);
                } else if response.hovered() || response.has_focus() {
                    painter.rect_filled(rect, 0, palette::INK_800);
                }
                painter.text(
                    rect.center(),
                    Align2::CENTER_CENTER,
                    *name,
                    FontId::new(11.5 * scale, fonts::strong()),
                    if here { palette::INK_950 } else { palette::TEXT_MID },
                );
                if response.has_focus() && !here {
                    painter.rect_stroke(rect, 0, Stroke::new(1.0, palette::LIME), StrokeKind::Inside);
                }
                if response.clicked() && !here {
                    // `Ui::show` plays the page turn's own sound.
                    nav(Some(*target));
                }
            }
            painter.line_segment([tabs.left_bottom(), tabs.right_bottom()], Stroke::new(1.0, palette::LINE));

            // --- the list, and the description beside it --------------------
            let list_w = (body.width() * 0.57).max(1.0);
            let list = Rect::from_min_size(body.min, vec2(list_w, body.height()));
            let desc = Rect::from_min_max(pos2(list.right() + metrics::GAP * scale, body.top()), body.max);
            ui.scope_builder(UiBuilder::new().max_rect(list).layout(Layout::top_down(Align::Min)), |ui| {
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

            painter.line_segment([pos2(desc.left() - metrics::GAP * scale * 0.5, desc.top()), pos2(desc.left() - metrics::GAP * scale * 0.5, desc.bottom())], Stroke::new(1.0, palette::SEPARATOR));
            description(ctx, &painter, desc, scale);

            // --- footer -----------------------------------------------------
            painter.line_segment([footer.left_top(), footer.right_top()], Stroke::new(1.0, palette::LINE));
            let hint = "A   SELECT      B   BACK      Y   SETTINGS";
            painter.text(pos2(footer.left() + pad, footer.center().y), Align2::LEFT_CENTER, hint, FontId::new(10.5 * scale, fonts::strong()), palette::TEXT_LOW);
            let close = Rect::from_min_size(pos2(footer.right() - pad - 30.0 * scale, footer.top() + 4.0 * scale), vec2(30.0 * scale, footer.height() - 8.0 * scale));
            let response = ui
                .scope_builder(UiBuilder::new().max_rect(close).layout(Layout::centered_and_justified(Direction::LeftToRight)), |ui| {
                    // The footer's height, not a row's: a button that asked
                    // for a whole row would push the shell past the screen.
                    ui.spacing_mut().interact_size.y = (close.height() - 2.0 * scale).max(1.0);
                    widgets::close_button(ui)
                })
                .inner;
            if response.clicked() {
                back = true;
            }
        });
    back
}

/// The description of the line under the pointer (its name and its help, which
/// the rows leave in egui's memory): the panel's right column, as the pause
/// menu's help box.
fn description(ctx: &Context, painter: &Painter, rect: Rect, scale: f32) {
    let Some((label, help)) = widgets::description(ctx) else {
        painter.text(
            pos2(rect.left(), rect.top()),
            Align2::LEFT_TOP,
            "MOVE THE POINTER OR THE SELECTION\nFOR A DESCRIPTION",
            FontId::new(10.5 * scale, fonts::strong()),
            palette::TEXT_LOW,
        );
        return;
    };
    let mut y = rect.top();
    let title = painter.layout(label, FontId::new(14.0 * scale, fonts::heavy()), palette::TEXT_HI, rect.width());
    let height = title.size().y;
    painter.galley(pos2(rect.left(), y), title, palette::TEXT_HI);
    y += height + 8.0 * scale;
    if !help.is_empty() {
        let body = painter.layout(help, FontId::new(12.0 * scale, fonts::regular()), palette::TEXT_MID, rect.width());
        let body_h = body.size().y;
        painter.galley(pos2(rect.left(), y), body, palette::TEXT_MID);
        y += body_h;
    }
    // The rest of the column is the page's, if it has anything to say there.
    let _ = y;
}
