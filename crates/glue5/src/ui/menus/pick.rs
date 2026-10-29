//! A choice's or a multi's options on a page of their own: a search over
//! long lists, one row per option, a check on the chosen ones. Only the
//! rows in view are drawn (lists may have a thousand options).

use egui::*;
use glue5_mods::{MenuInput, MenuItem, MenuKind};

use super::super::TextKind;
use super::super::theme::{INK, MUTED, density};
use super::super::widgets::{focus, inline_button, text_field};
use super::rows::{FILL, check, truncated};

/// More options than this: a search field.
const SEARCH_FROM: usize = 12;
/// More options than this: All and None (a multi's).
const ALL_FROM: usize = 6;

pub(super) enum Picked {
    /// A choice's option was picked (None: the one already chosen): the
    /// page closes; `keys`: by a key or a controller.
    Chosen { input: Option<MenuInput>, keys: bool },
    /// A multi's option was ticked or unticked.
    Toggled(MenuInput),
}

fn options(item: &MenuItem) -> &[String] {
    match &item.kind {
        MenuKind::Choice { options, .. } | MenuKind::Multi { options, .. } => options,
        _ => &[],
    }
}

/// The options that match `search` (all when it is empty), by index.
pub(super) fn filter(item: &MenuItem, search: &str) -> Vec<usize> {
    let options = options(item);
    let search = search.trim().to_lowercase();
    if search.is_empty() || options.len() <= SEARCH_FROM {
        return (0..options.len()).collect();
    }
    options.iter().enumerate().filter(|(_, o)| o.to_lowercase().contains(&search)).map(|(i, _)| i).collect()
}

/// A multi's part of the nav bar: how many are chosen, and All / None
/// (of the options shown) on long lists.
pub(super) fn header(ui: &mut Ui, item: &MenuItem, filtered: &[usize]) -> Option<MenuInput> {
    let MenuKind::Multi { options, selected } = &item.kind else { return None };
    let s = density(ui);
    let mut input = None;
    if options.len() > ALL_FROM {
        // Right to left: None, then All.
        if inline_button(ui, "None").clicked() {
            let left: Vec<u16> = selected.iter().copied().filter(|&i| !filtered.contains(&usize::from(i))).collect();
            input = Some(MenuInput::Select(left));
        }
        if inline_button(ui, "All").clicked() {
            let mut all: Vec<u16> = selected.iter().copied().chain(filtered.iter().map(|&i| i as u16)).collect();
            all.sort_unstable();
            all.dedup();
            input = Some(MenuInput::Select(all));
        }
    }
    ui.label(RichText::new(format!("{} of {}", selected.len(), options.len())).size(12.0 * s).color(MUTED));
    input
}

/// The search (long lists) and the options.
pub(super) fn list(
    ui: &mut Ui,
    item: &MenuItem,
    filtered: &[usize],
    search: &mut String,
    landing: Option<&super::Pending>,
) -> Option<Picked> {
    let s = density(ui);
    let options = options(item);
    if options.len() > SEARCH_FROM {
        text_field(ui, "", TextKind::Text, search, "Search", 40);
    }
    if filtered.is_empty() {
        ui.label(RichText::new("No option has that.").size(12.0 * s).color(MUTED));
        return None;
    }
    let (chosen, multi): (Vec<usize>, bool) = match &item.kind {
        MenuKind::Choice { index, .. } => (vec![*index], false),
        MenuKind::Multi { selected, .. } => (selected.iter().map(|&i| usize::from(i)).collect(), true),
        _ => return None,
    };
    // Opened: a choice shows (and with keys focuses) its option; a multi
    // its first.
    let opened = match landing {
        Some(super::Pending::Opened { focus }) => Some(*focus),
        _ => None,
    };
    let target = opened.map(|_| if multi { 0 } else { filtered.iter().position(|&i| i == chosen[0]).unwrap_or(0) });
    let height = 44.0 * s;
    let gap = 6.0 * s;
    let pitch = height + gap;
    let (list, _) =
        ui.allocate_exact_size(vec2(ui.available_width(), filtered.len() as f32 * pitch - gap), Sense::hover());
    // The rows in view, and one more each side: arrows move the focus onto
    // it, which then scrolls into view.
    let clip = ui.clip_rect();
    let first = (((clip.top() - list.top()) / pitch).floor() as isize - 1).clamp(0, filtered.len() as isize) as usize;
    let last = (((clip.bottom() - list.top()) / pitch).ceil() as isize + 1).clamp(0, filtered.len() as isize) as usize;
    let mut picked = None;
    let rows = (first..last).chain(target.filter(|t| !(first..last).contains(t)));
    for n in rows {
        let index = filtered[n];
        let rect = Rect::from_min_size(pos2(list.left(), list.top() + n as f32 * pitch), vec2(list.width(), height));
        let on = chosen.contains(&index);
        let response = option(ui, rect, item.id, index, &options[index], on, multi);
        if target == Some(n) {
            if opened == Some(true) {
                response.request_focus();
            }
            if !multi {
                ui.scroll_to_rect(rect, Some(Align::Center));
            }
        }
        if response.clicked() {
            picked = Some(if multi {
                let mut selected: Vec<u16> =
                    chosen.iter().map(|&i| i as u16).filter(|&i| usize::from(i) != index).collect();
                if !on {
                    selected.push(index as u16);
                    selected.sort_unstable();
                }
                Picked::Toggled(MenuInput::Select(selected))
            } else {
                Picked::Chosen { input: (!on).then_some(MenuInput::Choose(index)), keys: response.has_focus() }
            });
        }
    }
    picked
}

/// An option's row: its text, and a check when chosen (a multi's as a box).
fn option(ui: &mut Ui, rect: Rect, item: u32, index: usize, text: &str, on: bool, multi: bool) -> Response {
    let s = density(ui);
    let response = ui.interact(rect, Id::new(("mod-option", item, index)), Sense::click());
    response.widget_info(|| WidgetInfo::selected(WidgetType::Checkbox, true, on, text));
    let lit = response.hovered() || response.is_pointer_button_down_on();
    let radius = 12.0 * s;
    ui.painter().rect_filled(rect, radius, if lit { Color32::from_gray(33) } else { FILL });
    let mark = Rect::from_center_size(pos2(rect.right() - 24.0 * s, rect.center().y), vec2(18.0 * s, 18.0 * s));
    if multi {
        if on {
            ui.painter().rect_filled(mark, 5.0 * s, Color32::WHITE);
            check(ui, mark.center(), INK);
        } else {
            ui.painter().rect_stroke(mark, 5.0 * s, Stroke::new(1.5, Color32::from_gray(100)), StrokeKind::Inside);
        }
    } else if on {
        check(ui, mark.center(), Color32::WHITE);
    }
    let galley =
        truncated(ui, RichText::new(text).size(14.0 * s).color(Color32::WHITE), mark.left() - rect.left() - 26.0 * s);
    ui.painter().galley(pos2(rect.left() + 14.0 * s, rect.center().y - galley.size().y * 0.5), galley, Color32::WHITE);
    if response.has_focus() {
        ui.painter().rect_stroke(rect, radius, Stroke::new(1.5, Color32::WHITE), StrokeKind::Inside);
    }
    focus(response)
}
