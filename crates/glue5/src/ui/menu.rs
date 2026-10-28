//! The full-screen game-menu home: mode context in the stage and a compact,
//! horizontally scrollable lower navigation rail.
use egui::{Align, Align2, Color32, Id, Layout, Rect, RichText, Sense, Stroke, StrokeKind, pos2, vec2};

use super::{Action, Mode, Page, Status, WARN, icons, theme};
use crate::{paths::MOBILE, settings::{Settings, Source}};

const MODES: [(Mode, &str, &str, &str); 4] = [
    (Mode::Story, "STORY", "THE STORY OF LOS SANTOS", "Three lives. One city. The story begins here."),
    (Mode::Online, "ONLINE", "YOUR NEXT CHAPTER", "Build a life in Los Santos with friends and rivals."),
    (Mode::Sandbox, "SANDBOX", "LOS SANTOS IS YOURS", "Explore the whole map. No missions, no rush."),
    (Mode::Benchmark, "BENCHMARK", "TEST YOUR SETUP", "Take a guided flight over the city and review performance."),
];

pub(super) fn show(
    ctx: &egui::Context,
    settings: &mut Settings,
    status: &Status,
    selected: &mut Mode,
    gamepad_prompts: bool,
    starting: Option<Mode>,
    actions: &mut Vec<Action>,
    go: &mut Option<Option<Page>>,
) {
    let safe = ctx.content_rect();
    settings.online = *selected == Mode::Online;
    settings.sandbox = *selected == Mode::Sandbox;
    let scale = theme::layout_scale(safe);
    let edge = (20.0 * scale).max(10.0);

    // Status stays deliberately quiet and never leaks the selected filesystem path.
    egui::Area::new(Id::new("home-status"))
        .fixed_pos(pos2(safe.left() + edge, safe.top() + edge * 0.45))
        .order(egui::Order::Foreground)
        .show(ctx, |ui| {
            let label = if status.preparing.is_some() {
                format!("PREPARING  {:.0}%", status.preparing.unwrap_or_default() * 100.0)
            } else if status.game_dir.is_some() {
                "GAME FILES · READY".to_owned()
            } else {
                match status.source {
                    Source::Server => "GAME FILES · CONNECT SERVER".to_owned(),
                    Source::Folder => "GAME FILES · NOT SET".to_owned(),
                }
            };
            let response = ui.add(egui::Button::new(RichText::new(label).size(11.0 * scale).strong().color(if status.game_dir.is_some() { theme::palette::TEXT_MID } else { WARN }))
                .fill(Color32::from_black_alpha(42)).stroke(Stroke::new(0.6, Color32::from_white_alpha(24))).corner_radius(12));
            let response = response.on_hover_text("Open game file settings");
            if let Some(progress) = status.preparing {
                let bar = Rect::from_min_size(pos2(response.rect.left(), response.rect.bottom() + 5.0 * scale), vec2(122.0 * scale, 2.0 * scale));
                ui.painter().rect_filled(bar, 1.0, Color32::from_white_alpha(30));
                ui.painter().rect_filled(Rect::from_min_size(bar.min, vec2(bar.width() * progress.clamp(0.0, 1.0), bar.height())), 1.0, accent(*selected));
            }
            if response.clicked() {
                match status.source {
                    Source::Server => *go = Some(Some(Page::Servers)),
                    Source::Folder => actions.push(if MOBILE { Action::FindGame } else { Action::ChooseGame }),
                }
            }
        });

    egui::Area::new(Id::new("home-brand"))
        .anchor(Align2::RIGHT_TOP, vec2(-edge, edge * 0.6))
        .interactable(false)
        .show(ctx, |ui| {
            ui.label(RichText::new("GLUE 5").font(egui::FontId::new(17.0 * scale, egui::FontFamily::Name("condensed".into()))).color(Color32::from_white_alpha(180)));
        });

    // Context panel: mode title and description are the visual focus, not a list of cards.
    let (mode, eyebrow, title, description) = MODES.iter().find(|item| item.0 == *selected).copied().unwrap_or(MODES[0]);
    let going = starting.or(status.preparing.map(|_| status.mode));
    let side_panel = safe.width() >= safe.height() && mode != Mode::Benchmark;
    let (anchor, offset) = if side_panel {
        (Align2::LEFT_CENTER, vec2(edge, -16.0 * scale))
    } else {
        (Align2::CENTER_CENTER, vec2(0.0, -16.0 * scale))
    };
    egui::Area::new(Id::new("home-context"))
        .anchor(anchor, offset)
        .order(egui::Order::Middle)
        .show(ctx, |ui| {
            let available_width = (safe.width() - 2.0 * edge).max(1.0);
            let width = if side_panel {
                ((available_width * 0.46).clamp(260.0, 640.0)).min(available_width)
            } else {
                available_width.min(640.0)
            };
            ui.set_width(width);
            ui.vertical_centered(|ui| {
                let accent = accent(mode);
                ui.label(RichText::new(eyebrow).size(11.0 * scale).strong().color(accent));
                ui.add_space(10.0 * scale);
                ui.label(RichText::new(title).font(egui::FontId::new(44.0 * scale, egui::FontFamily::Name("condensed".into()))).color(theme::palette::TEXT_HI));
                ui.add_space(8.0 * scale);
                ui.label(RichText::new(description).size(14.0 * scale).color(theme::palette::TEXT_MID));
                if mode == Mode::Benchmark
                    && let Some(result) = status.benchmark
                {
                    let summary = format!("LAST RUN  ·  {:.0} AVG FPS  ·  {:.0} MIN FPS  ·  {:.0}% UNDER 33 MS", result.avg_fps, result.min_fps, result.smooth * 100.0);
                    ui.add_space(8.0 * scale);
                    ui.label(RichText::new(summary).size(10.0 * scale).color(theme::palette::TEXT_MID));
                }
                if let Some(failure) = status.last_failure {
                    ui.add_space(8.0 * scale);
                    ui.label(RichText::new(format!("LAST SESSION  ·  {failure}")).size(10.0 * scale).color(WARN));
                }
                ui.add_space(24.0 * scale);
                let ready = status.game_dir.is_some() && going.is_none();
                if mode == Mode::Online {
                    ui.set_max_width((width * 0.74).max(250.0));
                    ui.horizontal(|ui| {
                        ui.label(RichText::new("PLAYER").size(10.0 * scale).strong().color(theme::palette::TEXT_LOW));
                        ui.add_space(8.0 * scale);
                        ui.add(egui::TextEdit::singleline(&mut settings.online_name).hint_text("Player name").desired_width(ui.available_width()).margin(vec2(10.0, 8.0)));
                    });
                    ui.add_space(12.0 * scale);
                }
                let button_text = if going == Some(mode) {
                    status.preparing.map_or("STARTING…".to_owned(), |p| format!("PREPARING  {:.0}%", p * 100.0))
                } else if mode == Mode::Benchmark { "RUN BENCHMARK".to_owned() } else { "PLAY".to_owned() };
                let button_width = (width * 0.46).clamp(190.0, 300.0).min(width);
                let (rect, _) = ui.allocate_exact_size(vec2(button_width, 54.0 * scale), Sense::hover());
                let response = ui.interact(rect, ui.id().with("home-play"), Sense::click());
                let hot = response.hovered() || response.has_focus();
                ui.painter().rect_filled(rect, 8.0, if ready { if hot { Color32::from_rgb(255, 142, 86) } else { Color32::from_rgb(238, 111, 57) } } else { Color32::from_gray(53) });
                ui.painter().text(rect.center(), Align2::CENTER_CENTER, button_text, egui::FontId::new(17.0 * scale, egui::FontFamily::Name("condensed".into())), if ready { Color32::WHITE } else { theme::palette::TEXT_LOW });
                if response.has_focus() { ui.painter().rect_stroke(rect.expand(3.0), 10.0, Stroke::new(2.0, Color32::WHITE), StrokeKind::Outside); }
                response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, ready, "Play"));
                if response.clicked() && ready { actions.push(Action::Play(mode)); }
                if !ready {
                    ui.add_space(9.0 * scale);
                    let hint = if status.game_dir.is_some() { "Preparing the game…" } else { "Choose a game folder to continue" };
                    ui.label(RichText::new(hint).size(11.0 * scale).color(theme::palette::TEXT_LOW));
                }
            });
        });

    bottom_rail(ctx, safe, scale, settings, status, selected, go);

    if gamepad_prompts {
        let prompt = "←  →   SELECT MODE      A   PLAY";
        egui::Area::new(Id::new("gamepad-prompts"))
            .anchor(Align2::CENTER_BOTTOM, vec2(0.0, -(78.0 * scale)))
            .interactable(false)
            .show(ctx, |ui| { ui.label(RichText::new(prompt).size(10.0 * scale).strong().color(Color32::from_white_alpha(125))); });
    }
}

fn bottom_rail(
    ctx: &egui::Context,
    safe: Rect,
    scale: f32,
    settings: &mut Settings,
    status: &Status,
    selected: &mut Mode,
    go: &mut Option<Option<Page>>,
) {
    let height = 66.0 * scale;
    let selected_before = ctx.data(|data| data.get_temp::<Mode>(Id::new("home-nav-selected")));
    let selection_changed = selected_before != Some(*selected);
    ctx.data_mut(|data| data.insert_temp(Id::new("home-nav-selected"), *selected));
    let rect = Rect::from_min_max(pos2(safe.left(), safe.bottom() - height), safe.right_bottom());
    let response = egui::Area::new(Id::new("home-navigation"))
        .fixed_pos(rect.min)
        .order(egui::Order::Foreground)
        .show(ctx, |ui| {
            ui.set_width(rect.width());
            ui.set_height(rect.height());
            ui.painter().rect_filled(rect, 0, Color32::from_rgba_unmultiplied(7, 9, 13, 232));
            ui.painter().line_segment([rect.left_top(), rect.right_top()], Stroke::new(0.7, Color32::from_white_alpha(32)));
            ui.allocate_ui_with_layout(rect.size(), Layout::left_to_right(Align::Center), |ui| {
                ui.add_space(12.0 * scale);
                egui::ScrollArea::horizontal().id_salt("home-nav-scroll").auto_shrink([false, false]).show(ui, |ui| {
                    ui.spacing_mut().item_spacing.x = 5.0 * scale;
                    for (mode, label, _, _) in MODES {
                        let active = *selected == mode;
                        let width = (82.0 * scale).max(72.0);
                        let (item, _) = ui.allocate_exact_size(vec2(width, 45.0 * scale), Sense::hover());
                        let response = ui.interact(item, ui.id().with(("mode-tab", label)), Sense::click());
                        if active && selection_changed { ui.scroll_to_rect(item, Some(Align::Center)); }
                        if active {
                            ui.painter().rect_filled(item, 6.0, Color32::from_white_alpha(13));
                            ui.painter().rect_filled(Rect::from_min_size(pos2(item.left() + 8.0, item.bottom() - 2.0), vec2(item.width() - 16.0, 2.0)), 1.0, accent(mode));
                        }
                        ui.painter().text(item.center(), Align2::CENTER_CENTER, label, egui::FontId::new(13.0 * scale, egui::FontFamily::Name("condensed".into())), if active { Color32::WHITE } else { theme::palette::TEXT_LOW });
                        response.widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::Button, true, active, label));
                        if response.clicked() {
                            *selected = mode;
                            settings.online = mode == Mode::Online;
                            settings.sandbox = mode == Mode::Sandbox;
                        }
                        if response.has_focus() { ui.painter().rect_stroke(item.expand(2.0), 7.0, Stroke::new(1.5, Color32::WHITE), StrokeKind::Outside); }
                    }
                    ui.add_space(9.0 * scale);
                    ui.painter().line_segment([pos2(ui.cursor().left(), rect.top() + 13.0 * scale), pos2(ui.cursor().left(), rect.bottom() - 13.0 * scale)], Stroke::new(1.0, Color32::from_white_alpha(36)));
                    ui.add_space(9.0 * scale);
                    action_item(ui, icons::Icon::Game, "MODS", 54.0 * scale, go, Page::Mods, None, scale);
                    action_item(ui, icons::Icon::Community, "FRIENDS", 64.0 * scale, go, Page::Friends, Some(status.friend_requests), scale);
                    action_item(ui, icons::Icon::Settings, "SETTINGS", 64.0 * scale, go, Page::Settings, None, scale);
                });
            });
        });
    let _ = response;
}

fn action_item(
    ui: &mut egui::Ui,
    icon: icons::Icon,
    label: &str,
    width: f32,
    go: &mut Option<Option<Page>>,
    page: Page,
    badge: Option<usize>,
    scale: f32,
) {
    let (rect, _) = ui.allocate_exact_size(vec2(width, 48.0 * scale), Sense::hover());
    let response = ui.interact(rect, ui.id().with(("rail-action", label)), Sense::click());
    if response.hovered() { ui.painter().rect_filled(rect, 6.0, Color32::from_white_alpha(10)); }
    let center = pos2(rect.center().x, rect.center().y - 5.0 * scale);
    icons::tile(ui, Rect::from_center_size(center, vec2(24.0, 24.0) * scale), icon, Color32::from_white_alpha(196), Color32::TRANSPARENT);
    ui.painter().text(pos2(rect.center().x, rect.bottom() - 4.0 * scale), Align2::CENTER_BOTTOM, label, egui::FontId::new(9.5 * scale, egui::FontFamily::Proportional), theme::palette::TEXT_LOW);
    if let Some(n) = badge.filter(|n| *n > 0) {
        let badge_rect = Rect::from_center_size(pos2(rect.center().x + 10.0 * scale, rect.center().y - 14.0 * scale), vec2(18.0 * scale, 15.0 * scale));
        ui.painter().rect_filled(badge_rect, 8.0, Color32::from_rgb(229, 72, 77));
        ui.painter().text(badge_rect.center(), Align2::CENTER_CENTER, n.to_string(), egui::FontId::proportional(9.0 * scale), Color32::WHITE);
    }
    response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, label));
    if response.clicked() {
        *go = Some(Some(page));
    }
    if response.has_focus() { ui.painter().rect_stroke(rect.expand(2.0), 7.0, Stroke::new(1.5, Color32::WHITE), StrokeKind::Outside); }
}

fn accent(mode: Mode) -> Color32 {
    match mode {
        Mode::Story => theme::palette::SUN,
        Mode::Online => theme::palette::SEA,
        Mode::Sandbox => theme::palette::AMBER,
        Mode::Benchmark => theme::palette::RED,
    }
}
