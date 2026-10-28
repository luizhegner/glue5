//! Shared rows and controls, sized from the space available rather than the platform.
use super::TextKind;
use super::theme::{MUTED, density, expressive};
use egui::*;

pub(super) fn focus(r: Response) -> Response {
    if r.gained_focus() {
        r.scroll_to_me(None);
    }
    r
}

pub(super) fn close_button(ui: &mut Ui) -> Response {
    let scale = density(ui);
    let (r, response) = ui.allocate_exact_size(vec2(44.0 * scale, 44.0 * scale), Sense::click());
    response.widget_info(|| WidgetInfo::labeled(WidgetType::Button, ui.is_enabled(), "Close"));
    let c = r.center();
    ui.painter().rect_filled(r, 0, if response.hovered() { expressive::SURFACE_HIGH } else { expressive::SURFACE });
    for sign in [-1.0, 1.0] {
        ui.painter().line_segment(
            [c + vec2(-5.0 * scale, -5.0 * scale * sign), c + vec2(5.0 * scale, 5.0 * scale * sign)],
            Stroke::new(1.6, expressive::ON_SURFACE),
        );
    }
    if response.has_focus() {
        ui.painter().rect_stroke(r, 0, Stroke::new(1.0, expressive::PRIMARY), StrokeKind::Inside);
    }
    focus(response).on_hover_text("Close")
}

#[derive(Clone, Copy)]
pub(super) enum Control {
    Slider,
    Choice,
    Toggle,
    Action,
    /// A few buttons side by side (`action_buttons`).
    Actions,
}

pub(super) fn setting<R>(
    ui: &mut Ui,
    label: &str,
    kind: Control,
    subtitle: Option<&str>,
    add: impl FnOnce(&mut Ui) -> R,
) -> R {
    let scale = density(ui);
    ui.push_id(label, |ui| {
        let (surface, outline) = match kind {
            Control::Action | Control::Actions => (expressive::PRIMARY_CONTAINER, expressive::PRIMARY),
            Control::Slider | Control::Choice | Control::Toggle => (expressive::SURFACE, expressive::OUTLINE),
        };
        Frame::new()
            .fill(surface)
            .stroke(Stroke::new(1.0, outline))
            .corner_radius(0)
            .inner_margin((12.0 * scale) as i8)
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                let available = ui.available_width();
                let width = match kind {
                    Control::Slider => (available * 0.55).min(260.0 * scale),
                    Control::Choice => (available * 0.60).min(180.0 * scale),
                    Control::Toggle => 44.0 * scale,
                    Control::Action => (available * 0.55).min(180.0 * scale),
                    Control::Actions => (available * 0.62).min(320.0 * scale),
                };
                let label_width = (available - width - ui.spacing().item_spacing.x).max(1.0);
                ui.horizontal_top(|ui| {
                    let (rect, response) = ui.allocate_exact_size(vec2(label_width, 44.0 * scale), Sense::hover());
                    response.widget_info(|| WidgetInfo::labeled(WidgetType::Label, ui.is_enabled(), label));
                    paint_label(
                        ui,
                        rect,
                        label,
                        14.0 * scale,
                        if subtitle.is_some() { -8.0 * scale } else { 0.0 },
                        expressive::ON_SURFACE,
                    );
                    if let Some(text) = subtitle {
                        paint_label(ui, rect, text, 11.0 * scale, 10.0 * scale, expressive::ON_SURFACE_VARIANT);
                        response.on_hover_text(text);
                    } else {
                        response.on_hover_text(label);
                    }
                    ui.allocate_ui_with_layout(vec2(width, 44.0 * scale), Layout::top_down(Align::Min), |ui| {
                        ui.set_width(width);
                        ui.spacing_mut().slider_width = (width - 56.0 * scale).max(24.0 * scale);
                        ui.spacing_mut().interact_size.y = 44.0 * scale;
                        add(ui)
                    })
                    .inner
                })
                .inner
            })
            .inner
    })
    .inner
}

fn paint_label(ui: &Ui, rect: Rect, text: &str, size: f32, offset: f32, color: Color32) {
    let galley = WidgetText::from(RichText::new(text).size(size).color(color)).into_galley(
        ui,
        Some(TextWrapMode::Truncate),
        rect.width(),
        FontId::proportional(size),
    );
    let pos = pos2(rect.left(), rect.center().y + offset - galley.size().y * 0.5);
    ui.painter().galley(pos, galley, color);
}

/// A line of text in a field of its own, its name on the left (if any): the
/// choice control's look. Anywhere on the field gives the text the focus.
/// `kind` picks the system keyboard on touch screens, where an empty code or
/// address field also offers to paste (their keyboard has no way to).
pub(super) fn text_field(ui: &mut Ui, label: &str, kind: TextKind, text: &mut String, hint: &str, max: usize) -> Response {
    let scale = density(ui);
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 44.0 * scale), Sense::hover());
    let radius = 0.0;
    ui.painter().rect_filled(rect, radius, expressive::SURFACE);
    let field = ui.interact(rect, ui.id().with(label), Sense::click());
    let mut inner = rect.shrink2(vec2(14.0 * scale, 0.0));
    let label_width = if label.is_empty() { 0.0 } else { 52.0 * scale };
    ui.painter().text(inner.left_center(), Align2::LEFT_CENTER, label, FontId::proportional(13.0 * scale), MUTED);
    if crate::paths::TOUCH && matches!(kind, TextKind::Code | TextKind::Address) && text.is_empty() {
        let paste = Rect::from_min_max(pos2(inner.right() - 56.0 * scale, rect.top()), pos2(rect.right(), rect.bottom()));
        let response = ui.interact(paste, ui.id().with((label, "paste")), Sense::click());
        response.widget_info(|| WidgetInfo::labeled(WidgetType::Button, ui.is_enabled(), "Paste"));
        let color = if response.hovered() || response.has_focus() { Color32::WHITE } else { MUTED };
        ui.painter().text(paste.center(), Align2::CENTER_CENTER, "Paste", FontId::proportional(13.0 * scale), color);
        if response.clicked()
            && let Some(pasted) = super::clipboard_text()
        {
            *text = pasted.trim().chars().take(max).collect();
        }
        inner.max.x = paste.left();
    }
    let edit_rect = Rect::from_min_max(pos2(inner.left() + label_width, inner.top()), inner.max);
    let edit = ui
        .scope_builder(UiBuilder::new().max_rect(edit_rect).layout(Layout::left_to_right(Align::Center)), |ui| {
            ui.add(
                TextEdit::singleline(text)
                    .frame(Frame::NONE)
                    .hint_text(RichText::new(hint).color(MUTED))
                    .char_limit(max)
                    .font(FontId::proportional(15.0 * scale))
                    .margin(Vec2::ZERO)
                    .desired_width(edit_rect.width()),
            )
        })
        .inner;
    if field.clicked() {
        edit.request_focus();
    }
    if edit.has_focus() {
        ui.painter().rect_stroke(rect, radius, Stroke::new(1.0, expressive::PRIMARY), StrokeKind::Inside);
        ui.ctx().data_mut(|d| d.insert_temp(Id::new(super::TEXT_KIND), kind));
    }
    focus(edit)
}

/// Options side by side, the chosen one white (Settings' tabs, ...).
/// Returns whether the value changed.
pub(super) fn segmented<T: PartialEq + Copy>(ui: &mut Ui, value: &mut T, options: &[(T, &str)], height: f32) -> bool {
    let scale = density(ui);
    let previous = *value;
    Frame::new()
        .fill(expressive::SURFACE_LOW)
        .stroke(Stroke::new(1.0, expressive::OUTLINE))
        .corner_radius(0)
        .inner_margin((4.0 * scale) as i8)
        .show(ui, |ui| {
            let gap = 4.0 * scale;
            let w = (ui.available_width() - gap * (options.len() as f32 - 1.0)) / options.len() as f32;
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = gap;
                for &(option, label) in options {
                    let on = *value == option;
                    let (rect, _) = ui.allocate_exact_size(vec2(w, height), Sense::hover());
                    let response = ui.interact(rect, ui.id().with(label), Sense::click());
                    response.widget_info(|| WidgetInfo::selected(WidgetType::Button, ui.is_enabled(), on, label));
                    let radius = 0.0;
                    let fill = match (on, response.hovered() || response.has_focus()) {
                        (true, _) => expressive::PRIMARY,
                        (false, true) => expressive::SURFACE_HIGH,
                        (false, false) => Color32::TRANSPARENT,
                    };
                    ui.painter().rect_filled(rect, radius, fill);
                    let font = FontId::new(15.0 * scale, FontFamily::Proportional);
                    ui.painter().text(
                        rect.center(),
                        Align2::CENTER_CENTER,
                        label,
                        font,
                        if on { expressive::ON_PRIMARY } else { expressive::ON_SURFACE_VARIANT },
                    );
                    if response.has_focus() {
                        ui.painter().rect_stroke(rect, radius, Stroke::new(1.0, expressive::PRIMARY), StrokeKind::Outside);
                    }
                    if focus(response).clicked() {
                        *value = option;
                    }
                }
            });
        });
    *value != previous
}

pub(super) fn action_button(ui: &mut Ui, text: &str) -> Response {
    ui.with_layout(Layout::right_to_left(Align::Center), |ui| button(ui, text)).inner
}

/// The same button where the layout puts it (in a card's text, not a row's end).
pub(super) fn inline_button(ui: &mut Ui, text: &str) -> Response {
    button(ui, text)
}

/// Buttons side by side at the end of a row, in the order of `texts`:
/// their responses in that order. (`action_button` twice would draw the
/// second over the first.)
pub(super) fn action_buttons(ui: &mut Ui, texts: &[&str]) -> Vec<Response> {
    let mut responses = ui
        .with_layout(Layout::right_to_left(Align::Center), |ui| {
            texts.iter().rev().map(|text| button(ui, text)).collect::<Vec<_>>()
        })
        .inner;
    responses.reverse();
    responses
}

fn button(ui: &mut Ui, text: &str) -> Response {
    let scale = density(ui);
    focus(
        ui.add(
            Button::new(RichText::new(text).size(13.0 * scale).color(expressive::PRIMARY))
                .fill(expressive::SURFACE_HIGH)
                .corner_radius(0)
                .min_size(vec2(0.0, 40.0 * scale)),
        ),
    )
}

pub(super) fn choice<T: PartialEq + Copy>(ui: &mut Ui, value: &mut T, options: &[(T, &str)]) -> Response {
    let scale = density(ui);
    let previous = *value;
    let index = options.iter().position(|(v, _)| v == value).unwrap_or(0);
    let mut response = Frame::new()
        .fill(expressive::SURFACE_HIGH)
        .corner_radius(0)
        .inner_margin((3.0 * scale) as i8)
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 2.0 * scale;
                ui.spacing_mut().interact_size = vec2(38.0 * scale, 38.0 * scale);
                let value_width = (ui.available_width() - 80.0 * scale).max(1.0);
                let arrow = |text| {
                    Button::new(text)
                        .fill(Color32::TRANSPARENT)
                        .corner_radius(0)
                        .min_size(vec2(38.0 * scale, 38.0 * scale))
                };
                let left = focus(ui.add(arrow("‹"))).on_hover_text("Previous value");
                ui.allocate_ui_with_layout(
                    vec2(value_width, 38.0 * scale),
                    Layout::centered_and_justified(Direction::LeftToRight),
                    |ui| {
                        ui.add(Label::new(RichText::new(options[index].1).size(13.0 * scale)).truncate());
                    },
                );
                let right = focus(ui.add(arrow("›"))).on_hover_text("Next value");
                if left.clicked() {
                    *value = options[(index + options.len() - 1) % options.len()].0;
                }
                if right.clicked() {
                    *value = options[(index + 1) % options.len()].0;
                }
                left | right
            })
            .inner
        })
        .inner;
    if *value != previous {
        response.mark_changed();
    }
    response
}

pub(super) fn slider(
    ui: &mut Ui,
    value: &mut f32,
    range: std::ops::RangeInclusive<f32>,
    step: f64,
    suffix: &str,
    decimals: usize,
) -> Response {
    let scale = density(ui);
    ui.horizontal(|ui| {
        let start = *range.start();
        let end = *range.end();
        let (r, mut response) =
            ui.allocate_exact_size(vec2(ui.spacing().slider_width, 44.0 * scale), Sense::click_and_drag());
        let old = *value;
        if response.clicked() {
            response.request_focus();
        }
        if let Some(pos) = response.interact_pointer_pos() {
            let t = ((pos.x - r.left() - 5.0 * scale) / (r.width() - 10.0 * scale).max(1.0)).clamp(0.0, 1.0);
            *value = ((start + t * (end - start)) / step as f32).round() * step as f32;
        }
        if response.has_focus() {
            ui.memory_mut(|m| {
                m.set_focus_lock_filter(response.id, EventFilter { horizontal_arrows: true, ..Default::default() })
            });
            let delta = ui.input(|i| i.num_presses(Key::ArrowRight) as f32 - i.num_presses(Key::ArrowLeft) as f32);
            *value += delta * step as f32;
        }
        *value = value.clamp(start, end);
        if *value != old {
            response.mark_changed();
        }
        response.widget_info(|| WidgetInfo::slider(ui.is_enabled(), *value as f64, ""));
        let rail = Rect::from_min_max(
            pos2(r.left() + 5.0 * scale, r.center().y - 2.0 * scale),
            pos2(r.right() - 5.0 * scale, r.center().y + 2.0 * scale),
        );
        let x = rail.left() + rail.width() * ((*value - start) / (end - start));
        ui.painter().rect_filled(rail, 0, expressive::OUTLINE);
        ui.painter().rect_filled(Rect::from_min_max(rail.min, pos2(x, rail.bottom())), 0, expressive::PRIMARY);
        ui.painter().rect_filled(
            Rect::from_center_size(pos2(x, r.center().y), vec2(11.0 * scale, 11.0 * scale)),
            0,
            expressive::PRIMARY,
        );
        if response.has_focus() {
            ui.painter().rect_stroke(r, 0, Stroke::new(1.0, expressive::PRIMARY), StrokeKind::Inside);
        }
        let (r, _) = ui.allocate_exact_size(vec2(46.0 * scale, 44.0 * scale), Sense::hover());
        let shown = if suffix == "%" { *value * 100.0 } else { *value };
        ui.painter().text(
            r.right_center(),
            Align2::RIGHT_CENTER,
            format!("{:.*}{}", decimals, shown, suffix),
            FontId::proportional(14.0 * scale),
            Color32::WHITE,
        );
        focus(response)
    })
    .inner
}

pub(super) fn toggle(ui: &mut Ui, on: &mut bool) -> Response {
    let scale = density(ui);
    let (r, mut response) = ui.allocate_exact_size(vec2(44.0 * scale, 44.0 * scale), Sense::click());
    if response.clicked() {
        *on = !*on;
        response.mark_changed();
    }
    response.widget_info(|| WidgetInfo::selected(WidgetType::Checkbox, ui.is_enabled(), *on, ""));
    let t = ui.ctx().animate_bool_responsive(response.id, *on);
    ui.painter().rect_filled(
        Rect::from_center_size(r.center(), vec2(40.0 * scale, 24.0 * scale)),
        0,
        if *on { expressive::PRIMARY } else { expressive::OUTLINE },
    );
    ui.painter().rect_filled(
        Rect::from_center_size(
            pos2(egui::lerp((r.left() + 14.0 * scale)..=(r.right() - 14.0 * scale), t), r.center().y),
            vec2(12.0 * scale, 12.0 * scale),
        ),
        0,
        if *on { expressive::ON_PRIMARY } else { expressive::ON_SURFACE_VARIANT },
    );
    if response.has_focus() {
        ui.painter().rect_stroke(r, 0, Stroke::new(1.0, expressive::PRIMARY), StrokeKind::Inside);
    }
    focus(response)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::theme;

    #[test]
    fn slider_keyboard_keeps_focus_and_reports_changes() {
        let ctx = Context::default();
        theme::install(&ctx);
        let mut value = 0.75;
        let mut id = Id::NULL;
        let mut changed = false;
        for frame in 0..3 {
            let events = if frame == 2 {
                vec![Event::Key {
                    key: Key::ArrowRight,
                    physical_key: None,
                    pressed: true,
                    repeat: false,
                    modifiers: Modifiers::NONE,
                }]
            } else {
                vec![]
            };
            let mut output = ctx.run_ui(
                RawInput {
                    screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(390.0, 844.0))),
                    events,
                    ..Default::default()
                },
                |root| {
                    CentralPanel::default().show(root, |ui| {
                        let response = slider(ui, &mut value, 0.5..=1.0, 0.05, "%", 0);
                        id = response.id;
                        if frame < 2 {
                            response.request_focus();
                        }
                        changed = response.changed();
                    });
                },
            );
            output.textures_delta.clear();
        }
        assert!((value - 0.8).abs() < 0.001);
        assert!(changed);
        assert_eq!(ctx.memory(|m| m.focused()), Some(id));
    }

    #[test]
    fn controls_and_action_buttons_share_the_right_edge() {
        for width in [248.0, 318.0, 640.0] {
            let ctx = Context::default();
            theme::install(&ctx);
            let mut rect = Rect::NOTHING;
            let mut right = 0.0;
            let mut output = ctx.run_ui(
                RawInput {
                    screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(width + 40.0, 500.0))),
                    ..Default::default()
                },
                |root| {
                    CentralPanel::default().show(root, |ui| {
                        ui.set_width(width);
                        right = ui.max_rect().right() - 12.0;
                        setting(
                            ui,
                            "Game folder",
                            Control::Action,
                            Some("A very long folder path to truncate"),
                            |ui| {
                                rect = action_button(ui, "Choose…").rect;
                            },
                        );
                        assert!((rect.right() - right).abs() < 1.0, "{width}: {rect:?}, {:?}", ui.min_rect());
                    });
                },
            );
            output.textures_delta.clear();
        }
    }
}
