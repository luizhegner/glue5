//! Home and the floating settings window share the same responsive controls.
use super::widgets::{Control, action_button, choice, focus, setting, slider, text_field, toggle};
use super::{Action, DebugMarker, Mode, Page, SettingsTab, Status, TextKind, WARN, icons, theme};
use crate::paths::{self, MOBILE, TOUCH, TV};
use crate::settings::{
    self, Corner, OverlayDetail, PerfOverlay, RENDER_SCALES, Settings, Source, TouchAim, TouchLayout, TouchMode,
    TouchFire, TouchSteer, Upscaling,
};
use crate::sharing::State as Sharing;
use glue5_stream::Phase;
use egui::{Align, Align2, Color32, Id, Layout, Rect, RichText, Sense, Stroke, StrokeKind, UiBuilder, pos2, vec2};

/// The only platform differences are capabilities, not layout.
fn open_hint() -> &'static str {
    if TOUCH {
        "Settings: ⚙ or Back + Start"
    } else if TV {
        "Settings: Back + Start"
    } else {
        "Settings: F1 or Back + Start"
    }
}

/// The ways into the game, the start page's main column: a name and what
/// it is.
const MODES: [(Mode, &str, &str); 4] = [
    (Mode::Story, "STORY MODE", "Michael, Franklin and Trevor's story, from your saves."),
    (Mode::Online, "GTA ONLINE", "Freemode with other players, matched automatically, no servers."),
    (Mode::Sandbox, "SANDBOX", "The whole map, free roam, no missions."),
    (Mode::Benchmark, "BENCHMARK", "The game's fly-through of the map: how it runs here."),
];

/// A mode's card at least, and the title's row, in points at full density
/// (to place the page before it is laid out).
const MODE_HEIGHT: f32 = 66.0;
const TITLE_HEIGHT: f32 = 58.0;
const GAP: f32 = 8.0;

/// A status dot: the game's files are there.
pub(super) const READY: Color32 = theme::palette::ACCENT;

/// The start page: the title with the app's own buttons, the game's modes
/// and, quieter, what the app knows. Two columns where there is room, one
/// on an upright phone. Returns the mode chosen, if one was. `starting`:
/// the mode chosen at the last frame; `online_open`: GTA Online's card is
/// open (its lobby and the player's name).
#[allow(clippy::too_many_arguments)]
#[allow(dead_code, reason = "the start page is the shell's PLAY tab now; kept until its last test moves over")]
pub(super) fn start_page(
    ctx: &egui::Context,
    settings: &mut Settings,
    status: &Status,
    starting: Option<Mode>,
    online_open: &mut bool,
    actions: &mut Vec<Action>,
    go: &mut Option<Option<Page>>,
) -> Option<Mode> {
    let safe = ctx.content_rect();
    let scale = theme::layout_scale(safe);
    let edge = 16.0 * scale;
    let available = (safe.width() - 2.0 * edge).clamp(1.0, 1100.0).floor();
    let gap = 32.0 * scale;
    // Upright screens read better in one column: a phone, a tablet.
    let two_columns = available >= 600.0 * scale && safe.width() >= safe.height();
    let width = if two_columns { available } else { available.min(520.0) };
    let menu_width = if two_columns { (420.0 * scale).min((width - gap) * 0.5).floor() } else { width };
    // A little above the middle when there is room: the title and the
    // modes (the other cards come and go), on one column all of it.
    let modes = MODES.len() as f32 * (MODE_HEIGHT + GAP) * scale;
    let content = (TITLE_HEIGHT + 20.0) * scale + modes + if two_columns { 0.0 } else { 160.0 * scale };
    let top = ((safe.height() - content) * 0.4).max(edge * 0.5).floor();
    let height = (safe.height() - top - edge * 0.5).max(1.0).floor();
    let mut chosen = None;
    egui::Area::new(Id::new("start"))
        .default_size(vec2(width, height))
        .constrain(false)
        .fixed_pos(pos2(safe.center().x - width * 0.5, safe.top() + top))
        .show(ctx, |ui| {
            theme::apply_density(ui, scale);
            ui.set_width(width);
            ui.set_height(height);
            // The buttons go in last, so that the first move of a
            // controller lands on the modes.
            let (title_row, _) = ui.allocate_exact_size(vec2(width, TITLE_HEIGHT * scale), Sense::hover());
            ui.painter().text(
                title_row.left_center(),
                Align2::LEFT_CENTER,
                "GLUE 5",
                egui::FontId::proportional(40.0 * scale),
                Color32::WHITE,
            );
            ui.add_space(20.0 * scale);
            let remaining = (ui.max_rect().bottom() - ui.cursor().top()).max(1.0).floor();
            if two_columns {
                ui.horizontal_top(|ui| {
                    ui.spacing_mut().item_spacing.x = gap;
                    column(ui, "home-modes", menu_width, remaining, |ui| {
                        chosen = mode_list(ui, settings, status, starting, online_open, menu_width);
                    });
                    column(ui, "home-status", width - menu_width - gap, remaining, |ui| {
                        info(ui, settings, status, actions, go)
                    });
                });
            } else {
                column(ui, "home", width, remaining, |ui| {
                    // Without the game's files, why first.
                    if status.game_dir.is_none() {
                        info(ui, settings, status, actions, go);
                        ui.add_space(8.0 * scale);
                    }
                    chosen = mode_list(ui, settings, status, starting, online_open, width);
                    if status.game_dir.is_some() {
                        ui.add_space(8.0 * scale);
                        info(ui, settings, status, actions, go);
                    }
                });
            }
            let labels = width >= 720.0 * scale;
            ui.scope_builder(UiBuilder::new().max_rect(title_row).layout(Layout::right_to_left(Align::Center)), |ui| {
                ui.spacing_mut().item_spacing.x = 8.0 * scale;
                if !MOBILE && icon_button(ui, "🗙", "Quit", labels).clicked() {
                    actions.push(Action::Quit);
                }
                // Streamed (always, on the Apple TV), the files come from the
                // Servers page.
                let (icon, files) = match status.source {
                    Source::Server => ("🖧", "Server"),
                    Source::Folder if MOBILE => ("🗁", "Find files"),
                    Source::Folder => ("🗁", "Game folder"),
                };
                if icon_button(ui, icon, files, labels).clicked() {
                    match status.source {
                        Source::Server => *go = Some(Some(Page::Servers)),
                        Source::Folder => actions.push(if MOBILE { Action::FindGame } else { Action::ChooseGame }),
                    }
                }
                if icon_button(ui, "★", "Credits", labels).clicked() {
                    *go = Some(Some(Page::Credits));
                }
                if icon_button(ui, "🛠", "Mods", labels).clicked() {
                    *go = Some(Some(Page::Mods));
                }
                // Asked to be friends: how many, on the button.
                let friends = match status.friend_requests {
                    0 => "Friends".to_owned(),
                    n => format!("Friends ({n})"),
                };
                if icon_button(ui, "♡", &friends, labels || status.friend_requests > 0).clicked() {
                    *go = Some(Some(Page::Friends));
                }
                if icon_button(ui, "⚙", "Settings", labels).clicked() {
                    *go = Some(Some(Page::Settings));
                }
            });
        });
    chosen
}

/// A column of the start page, scrolling if it has to.
fn column(ui: &mut egui::Ui, id: &str, width: f32, height: f32, add: impl FnOnce(&mut egui::Ui)) {
    ui.allocate_ui_with_layout(vec2(width, height), Layout::top_down(Align::Min), |ui| {
        ui.set_width(width);
        ui.spacing_mut().item_spacing = vec2(10.0, GAP) * theme::density(ui);
        egui::ScrollArea::vertical()
            .content_margin(0)
            .id_salt(id)
            .max_height(height)
            .auto_shrink([false, false])
            .show(ui, add);
    });
}

/// The modes, one card each. The one on its way (chosen at the last frame,
/// or waiting for its pipelines) says so; the others wait. GTA Online opens
/// in place first (`open`): the lobby and the player's name, then Play.
fn mode_list(
    ui: &mut egui::Ui,
    settings: &mut Settings,
    status: &Status,
    starting: Option<Mode>,
    open: &mut bool,
    width: f32,
) -> Option<Mode> {
    let going = starting.or(status.preparing.map(|_| status.mode));
    let ready = status.game_dir.is_some() && going.is_none();
    *open &= ready;
    let mut chosen = None;
    for (mode, name, about) in MODES {
        let id = Id::new(("mode", name));
        let look = if mode == status.mode { Look::Primary } else { Look::Plain };
        let online = mode == Mode::Online;
        // Fades the open card's controls in.
        let shown = if online { ui.ctx().animate_bool_responsive(id.with("open"), *open) } else { 0.0 };
        if going == Some(mode) {
            match status.preparing {
                Some(done) => mode_card(
                    ui,
                    id,
                    width,
                    &format!("PREPARING {:.0}%", done * 100.0),
                    "Compiling the game's shaders for this device: minutes the first time, seconds after.",
                    look,
                    None,
                    Some(done),
                ),
                None => mode_card(ui, id, width, "STARTING…", about, look, None, None),
            };
        } else if online && *open {
            if online_card(ui, id, width, about, shown, settings, open) {
                chosen = Some(mode);
            }
        } else {
            let chevron = if online { Chevron::Opens } else { Chevron::Starts };
            let card = ui.add_enabled_ui(ready, |ui| mode_card(ui, id, width, name, about, look, Some(chevron), None));
            if card.inner.clicked() {
                if online {
                    *open = true;
                    ui.data_mut(|d| d.insert_temp(id.with("reveal"), true));
                    if settings.online_name.trim().is_empty() {
                        settings.online_name = settings::default_online_name();
                    }
                    ui.ctx().request_repaint();
                } else {
                    chosen = Some(mode);
                }
            }
        }
    }
    chosen
}

/// How a mode's card looks.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Look {
    Plain,
    /// White: the last one played.
    Primary,
    /// The open GTA Online card's top (the card draws the background).
    Open,
}

/// What the card's chevron says: it starts the game, it opens, it closes.
#[derive(Clone, Copy)]
enum Chevron {
    Starts,
    Opens,
    Closes,
}

/// A mode: its name and what it is, a chevron, the whole card a button
/// (without a chevron, it is on its way). `progress`: a bar along the
/// bottom. `id` stays the same whatever the card says.
#[allow(clippy::too_many_arguments)]
fn mode_card(
    ui: &mut egui::Ui,
    id: Id,
    width: f32,
    name: &str,
    about: &str,
    look: Look,
    chevron: Option<Chevron>,
    progress: Option<f32>,
) -> egui::Response {
    let scale = theme::density(ui);
    let pad = vec2(20.0, 12.0) * scale;
    let (ink, soft) =
        if look == Look::Primary { (theme::expressive::ON_PRIMARY, theme::expressive::ON_SURFACE_VARIANT) } else { (theme::expressive::ON_SURFACE, theme::MUTED) };
    let text_width = (width - 2.0 * pad.x - 24.0 * scale).max(1.0);
    let galley = |text: egui::RichText, wrap| {
        egui::WidgetText::from(text).into_galley(ui, Some(wrap), text_width, egui::TextStyle::Body)
    };
    let title = galley(theme::heading(name, 22.0 * scale).color(ink), egui::TextWrapMode::Truncate);
    let about = galley(RichText::new(about).size(12.0 * scale).color(soft), egui::TextWrapMode::Wrap);
    let text_height = title.size().y + 2.0 * scale + about.size().y;
    let height = (text_height + 2.0 * pad.y).max(MODE_HEIGHT * scale);
    let interactive = chevron.is_some() && ui.is_enabled();
    let (rect, _) = ui.allocate_exact_size(vec2(width, height), Sense::hover());
    let response = ui.interact(rect, id, if interactive { Sense::click() } else { Sense::hover() });
    response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, ui.is_enabled(), name));
    let lit = interactive && (response.hovered() || response.has_focus());
    let fill = match (look, lit) {
        (Look::Primary, false) => theme::expressive::PRIMARY,
        (Look::Primary, true) => theme::expressive::SECONDARY,
        (Look::Plain, false) => theme::expressive::SURFACE,
        (Look::Plain, true) => theme::expressive::SURFACE_HIGH,
        (Look::Open, false) => Color32::TRANSPARENT,
        (Look::Open, true) => theme::expressive::SURFACE_HIGH,
    };
    let radius = 14.0 * scale;
    let painter = ui.painter();
    painter.rect_filled(rect, radius, fill);
    let top = rect.center().y - text_height * 0.5;
    painter.galley(pos2(rect.left() + pad.x, top), title.clone(), ink);
    painter.galley(pos2(rect.left() + pad.x, top + title.size().y + 2.0 * scale), about, soft);
    if let Some(chevron) = chevron.filter(|_| interactive) {
        let at = pos2(rect.right() - pad.x - 4.0 * scale, rect.center().y);
        let towards = match chevron {
            Chevron::Starts => vec2(1.0, 0.0),
            Chevron::Opens => vec2(0.0, 1.0),
            Chevron::Closes => vec2(0.0, -1.0),
        };
        let (side, depth) = (towards.rot90() * 6.0 * scale, towards * 3.5 * scale);
        let stroke = Stroke::new(2.0 * scale, soft);
        painter.add(egui::Shape::line(vec![at - depth + side, at + depth, at - depth - side], stroke));
    }
    if let Some(done) = progress {
        let rail = Rect::from_min_max(
            pos2(rect.left() + pad.x, rect.bottom() - 7.0 * scale),
            pos2(rect.right() - pad.x, rect.bottom() - 4.0 * scale),
        );
        painter.rect_filled(rail, 0, ink.gamma_multiply(0.2));
        let filled = Rect::from_min_max(rail.min, pos2(egui::lerp(rail.x_range(), done.clamp(0.0, 1.0)), rail.max.y));
        painter.rect_filled(filled, 0, ink);
    }
    if response.has_focus() {
        focus_ring(ui, rect, radius);
    }
    focus(response)
}

/// A clear orange focus outline, kept just outside a focused control.
fn focus_ring(ui: &egui::Ui, rect: Rect, radius: f32) {
    let gap = 3.0 * theme::density(ui);
    let stroke = Stroke::new(1.0, theme::palette::ACCENT);
    ui.painter().rect_stroke(rect.expand(gap), radius + gap, stroke, StrokeKind::Outside);
}

/// GTA Online, open: the player's name, and Play, faded in by `shown`. Its
/// top closes it (`open` false). True when Play is pressed.
fn online_card(
    ui: &mut egui::Ui,
    id: Id,
    width: f32,
    about: &str,
    shown: f32,
    settings: &mut Settings,
    open: &mut bool,
) -> bool {
    let scale = theme::density(ui);
    let mut play = false;
    let card = egui::Frame::new()
        .fill(theme::expressive::SURFACE_LOW)
        .stroke(Stroke::new(1.0, theme::expressive::OUTLINE))
        .corner_radius(14)
        .show(ui, |ui| {
            let width = width - 2.0;
            ui.set_width(width);
            ui.spacing_mut().item_spacing.y = 0.0;
            if mode_card(ui, id, width, "GTA ONLINE", about, Look::Open, Some(Chevron::Closes), None).clicked() {
                *open = false;
            }
            ui.scope(|ui| {
                ui.multiply_opacity(shown);
                let margin = |v: f32| (v * scale) as i8;
                egui::Frame::new()
                    .inner_margin(egui::Margin {
                        left: margin(20.0),
                        right: margin(20.0),
                        top: 0,
                        bottom: margin(20.0),
                    })
                    .show(ui, |ui| {
                        ui.spacing_mut().item_spacing.y = 10.0 * scale;
                        let what = "Sessions with the players online, matched automatically; invite-only, friend \
                                    and solo sessions from the game's pause menu, and your friends from ♡ Friends.";
                        ui.label(RichText::new(what).size(12.0 * scale).color(theme::MUTED));
                        text_field(ui, "Name", TextKind::Name, &mut settings.online_name, "Player", 16);
                        ui.add_space(2.0 * scale);
                        play = play_button(ui, "PLAY").clicked();
                    });
            });
        });
    // Just opened: all of it in view (a phone on its side scrolls).
    if ui.data_mut(|d| d.remove_temp::<bool>(id.with("reveal"))).is_some() {
        ui.scroll_to_rect(card.response.rect, None);
    }
    play
}

/// The orange, rounded button that starts the game.
fn play_button(ui: &mut egui::Ui, label: &str) -> egui::Response {
    let scale = theme::density(ui);
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 50.0 * scale), Sense::hover());
    let response = ui.interact(rect, ui.id().with("play"), Sense::click());
    response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, ui.is_enabled(), label));
    let lit = response.hovered() || response.has_focus();
    let radius = 14.0 * scale;
    let painter = ui.painter();
    painter.rect_filled(rect, radius, if lit { theme::expressive::SECONDARY } else { theme::expressive::PRIMARY });
    let font = egui::FontId::proportional(18.0 * scale);
    painter.text(rect.center(), Align2::CENTER_CENTER, label, font, theme::INK);
    if response.has_focus() {
        focus_ring(ui, rect, radius);
    }
    focus(response)
}

/// The app's own buttons, small and half-transparent next to the modes:
/// an icon, with its name where there is room.
fn icon_button(ui: &mut egui::Ui, icon: &str, label: &str, with_label: bool) -> egui::Response {
    let scale = theme::density(ui);
    let size = (40.0 * scale).max(36.0);
    let text = if with_label { format!("{icon}  {label}") } else { icon.to_owned() };
    let button = egui::Button::new(
        RichText::new(text).size(if with_label { 14.0 } else { 18.0 } * scale).color(Color32::from_white_alpha(220)),
    )
    .fill(theme::expressive::SURFACE_HIGH)
    .min_size(vec2(size, size))
    .corner_radius(14);
    let response = focus(ui.add(button));
    if with_label { response } else { response.on_hover_text(label) }
}

/// What the app knows: the game's files, the last benchmark, why the last
/// session stopped.
fn info(
    ui: &mut egui::Ui,
    settings: &mut Settings,
    status: &Status,
    actions: &mut Vec<Action>,
    go: &mut Option<Option<Page>>,
) {
    card(ui, |ui| game_files(ui, settings, status, go));
    if settings.mods {
        card(ui, |ui| super::mods::home_card(ui, settings, status, go));
    }
    if let Some(sharing) = &status.sharing {
        card(ui, |ui| sharing_card(ui, sharing));
    }
    if let Some(results) = status.benchmark {
        card(ui, |ui| last_benchmark(ui, results));
    }
    if let Some(why) = status.last_failure {
        card(ui, |ui| last_failure(ui, why, status.graphics_reset, actions));
    }
    ui.label(RichText::new(open_hint()).small().color(theme::MUTED));
}

fn card<R>(ui: &mut egui::Ui, add: impl FnOnce(&mut egui::Ui) -> R) -> R {
    let scale = theme::density(ui);
    egui::Frame::new()
        .fill(theme::expressive::SURFACE)
        .corner_radius(14)
        .inner_margin(egui::Margin::symmetric((20.0 * scale) as i8, (16.0 * scale) as i8))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.spacing_mut().item_spacing.y = 6.0 * scale;
            add(ui)
        })
        .inner
}

/// A card's title, and on its right a status: a dot and a word.
pub(super) fn card_title(ui: &mut egui::Ui, title: &str, state: Option<(&str, Color32)>) {
    let scale = theme::density(ui);
    let (row, _) = ui.allocate_exact_size(vec2(ui.available_width(), 24.0 * scale), Sense::hover());
    let painter = ui.painter();
    let mut title_width = row.width();
    if let Some((word, color)) = state {
        let text = painter.layout_no_wrap(word.into(), egui::FontId::proportional(11.0 * scale), Color32::WHITE);
        let pill = Rect::from_min_max(
            pos2(row.right() - text.size().x - 28.0 * scale, row.center().y - 11.0 * scale),
            pos2(row.right(), row.center().y + 11.0 * scale),
        );
        title_width = pill.left() - row.left() - 8.0 * scale;
        painter.rect_filled(pill, 10, theme::expressive::SURFACE_HIGH);
        painter.circle_filled(pos2(pill.left() + 11.0 * scale, pill.center().y), 3.5 * scale, color);
        painter.galley(pos2(pill.left() + 19.0 * scale, pill.center().y - text.size().y * 0.5), text, Color32::WHITE);
    }
    // A long title (a computer's name) ends with … before the pill.
    let font = egui::FontId::new(17.0 * scale, egui::FontFamily::Name("condensed".into()));
    let mut job = egui::text::LayoutJob::simple_singleline(title.into(), font, Color32::WHITE);
    job.wrap = egui::text::TextWrapping::truncate_at_width(title_width.max(1.0));
    let galley = painter.layout_job(job);
    painter.galley(pos2(row.left(), row.center().y - galley.size().y * 0.5), galley, Color32::WHITE);
}

fn game_files(ui: &mut egui::Ui, settings: &mut Settings, status: &Status, go: &mut Option<Option<Page>>) {
    if status.source == Source::Server {
        return streamed_files(ui, status, go);
    }
    let state = if status.game_dir.is_some() { ("Ready", READY) } else { ("Missing", WARN) };
    card_title(ui, "GAME FILES", Some(state));
    if let Some(dir) = status.game_dir {
        let text = dir.display().to_string();
        ui.add(egui::Label::new(RichText::new(&text).small().color(theme::MUTED)).truncate()).on_hover_text(text);
    } else {
        if let Some(problem) = status.problem {
            ui.label(RichText::new(problem).small().color(WARN));
        }
        ui.label(RichText::new(paths::where_the_game_goes()).small().color(theme::MUTED));
        ui.label(RichText::new(paths::game_layout()).monospace().small().color(theme::MUTED));
        ui.label(RichText::new(paths::or_stream()).small().color(theme::MUTED));
        if ui.horizontal(|ui| action_button(ui, "Stream from a server")).inner.clicked() {
            settings.source = Source::Server;
            *go = Some(Some(Page::Servers));
        }
    }
}

pub(super) fn small(ui: &mut egui::Ui, text: &str, color: Color32) {
    ui.label(RichText::new(text).small().color(color));
}

/// A thin bar: how far something is (0 to 1).
pub(super) fn rail(ui: &mut egui::Ui, done: f32) {
    let scale = theme::density(ui);
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 4.0 * scale), Sense::hover());
    ui.painter().rect_filled(rect, 0, theme::expressive::OUTLINE);
    let filled = Rect::from_min_size(rect.min, vec2(rect.width() * done.clamp(0.0, 1.0), rect.height()));
    ui.painter().rect_filled(filled, 0, theme::expressive::PRIMARY);
}

pub(super) fn bytes(n: u64) -> String {
    if n >= 1 << 30 { format!("{:.1} GB", n as f64 / 1e9) } else { format!("{:.0} MB", n as f64 / 1e6) }
}

/// The game's files from a server: where from, how far.
fn streamed_files(ui: &mut egui::Ui, status: &Status, go: &mut Option<Option<Page>>) {
    let Some((server, s)) = status.stream else {
        card_title(ui, "GAME FILES", Some(("No server", WARN)));
        small(ui, "Choose the computer that shares the game.", theme::MUTED);
        if ui.horizontal(|ui| action_button(ui, "Choose a server")).inner.clicked() {
            *go = Some(Some(Page::Servers));
        }
        return;
    };
    card_title(ui, "GAME FILES", Some(stream_state(s)));
    let mut from = format!("From {}", server_name(status, server));
    if s.cache_bytes > 0 {
        from.push_str(&format!(" · {} on this device", bytes(s.cache_bytes)));
    }
    small(ui, &from, theme::MUTED);
    stream_progress(ui, s);
}

/// The stream's state in a word, for a card's pill.
pub(super) fn stream_state(s: &glue5_stream::Status) -> (&'static str, Color32) {
    match &s.phase {
        Phase::Failed(_) => ("Stopped", WARN),
        Phase::Ready if !s.connected => ("Offline", WARN),
        Phase::Ready => ("Ready", READY),
        Phase::Syncing { .. } => ("Taking files", theme::MUTED),
        Phase::Connecting => ("Connecting", WARN),
    }
}

/// What the stream is doing: a line, and a bar while it takes something.
pub(super) fn stream_progress(ui: &mut egui::Ui, s: &glue5_stream::Status) {
    match &s.phase {
        Phase::Failed(why) => small(ui, why, WARN),
        Phase::Connecting => match &s.error {
            Some(e) => small(ui, e, WARN),
            None => small(ui, "Looking for the server…", theme::MUTED),
        },
        Phase::Syncing { what, done, total } => {
            let what = if *what == "text" { "small files" } else { "shaders" };
            let done = *done as f32 / (*total).max(1) as f32;
            small(ui, &format!("Taking the {what}, once: {:.0}%", done * 100.0), theme::MUTED);
            rail(ui, done);
        }
        Phase::Ready if s.complete => small(ui, "The whole game is on this device.", theme::MUTED),
        Phase::Ready if !s.connected => small(ui, "Playing from this device's copy until the server answers.", theme::MUTED),
        Phase::Ready if s.filling => {
            let done = s.cache_bytes as f32 / s.total_bytes.max(1) as f32;
            let line = format!("Downloading the whole game: {} of {}", bytes(s.cache_bytes), bytes(s.total_bytes));
            small(ui, &line, theme::MUTED);
            if s.no_room {
                small(ui, "Not enough space on this device: the download waits for some.", WARN);
            }
            rail(ui, done);
        }
        Phase::Ready if s.boot.0 < s.boot.1 => {
            let done = s.boot.0 as f32 / s.boot.1 as f32;
            small(ui, &format!("Taking what every start reads: {:.0}%", done * 100.0), theme::MUTED);
            rail(ui, done);
        }
        Phase::Ready => {}
    }
}

/// A server's name, if the network search found it, else its address.
pub(super) fn server_name(status: &Status, address: &str) -> String {
    status.servers.iter().find(|s| is_server(s, address)).map_or_else(|| address.to_owned(), |(f, _)| f.name.clone())
}

/// Whether a server found is the one at `address`: by its name, or by an
/// address its name stands for (a Mac found as `name.local`, typed as its IP).
pub(super) fn is_server((found, check): &super::FoundServer, address: &str) -> bool {
    let ip = address.parse::<std::net::SocketAddr>().ok().or_else(|| {
        let ip = address.parse::<std::net::IpAddr>().ok()?;
        Some(std::net::SocketAddr::new(ip, glue5_stream::PORT))
    });
    found.address == address
        || ip.is_some_and(|ip| matches!(check, Some(glue5_stream::Check::Ready { at, .. }) if at.contains(&ip)))
}

/// The server in use in a few words: its name, and how it is doing.
fn server_summary(settings: &Settings, status: &Status) -> String {
    let server = settings.server.trim();
    if server.is_empty() {
        return "None chosen yet".to_owned();
    }
    let name = server_name(status, server);
    match status.stream.filter(|(s, _)| *s == server) {
        Some((_, s)) => format!("{name} · {}", stream_state(s).0),
        // The game runs from another one; this one is for the next start.
        None if status.started.is_some() => format!("{name} · next start"),
        None => name,
    }
}

/// This computer serving its game folder.
fn sharing_card(ui: &mut egui::Ui, sharing: &Sharing) {
    match sharing {
        Sharing::Preparing(done) => {
            card_title(ui, "SHARING", Some(("Preparing", theme::MUTED)));
            small(ui, "Packing the small files and the shaders, the first time.", theme::MUTED);
            if let Some(done) = done {
                rail(ui, *done);
            }
        }
        Sharing::On(addresses, announced) => {
            card_title(ui, "SHARING", Some(("On", READY)));
            if addresses.is_empty() {
                small(ui, "No network address found: is this computer on a network?", WARN);
            } else {
                if *announced {
                    small(ui, "Other devices find this computer in their Servers page, or play from one of these addresses:", theme::MUTED);
                } else {
                    // Another program has the port announcements use.
                    small(ui, "Other devices do not find this computer by themselves: type one of these addresses in their Servers page:", WARN);
                }
                for a in addresses {
                    ui.label(RichText::new(a).monospace().color(Color32::WHITE));
                }
            }
        }
        Sharing::Failed(why) => {
            card_title(ui, "SHARING", Some(("Stopped", WARN)));
            small(ui, why, WARN);
        }
    }
}

fn graphics_reset_note() -> RichText {
    RichText::new("Graphics reset. Backup: settings.xml.bak").small().color(theme::MUTED)
}

fn last_benchmark(ui: &mut egui::Ui, results: &crate::benchmark::Results) {
    let scale = theme::density(ui);
    card_title(ui, "LAST BENCHMARK", None);
    ui.label(theme::heading(&format!("{:.0} FPS", results.avg_fps), 34.0 * scale));
    ui.label(
        RichText::new(format!(
            "On average; {:.0} at least, {:.0}% of the frames in under 33 ms.",
            results.min_fps,
            results.smooth * 100.0
        ))
        .small()
        .color(theme::MUTED),
    );
}

fn last_failure(ui: &mut egui::Ui, why: &str, reset: bool, actions: &mut Vec<Action>) {
    card_title(ui, "LAST SESSION", Some(("Stopped", WARN)));
    ui.label(RichText::new(why).small().color(theme::MUTED));
    if reset {
        ui.label(graphics_reset_note());
    } else if ui.horizontal(|ui| action_button(ui, "Reset graphics")).inner.clicked() {
        actions.push(Action::ResetGraphics);
    }
    #[cfg(target_os = "android")]
    if ui.horizontal(|ui| action_button(ui, "Capture GLUE 5 logs via Shizuku")).inner.clicked() {
        actions.push(Action::CaptureSessionLogcat);
    }
}

/// The settings, over the start page or the game. True when closed.
#[allow(clippy::too_many_arguments)]
/// The settings' nine categories, in the pause menu's rhythm.
const CATEGORIES: [(SettingsTab, &str); 9] = [
    (SettingsTab::Graphics, "GRAPHICS"),
    (SettingsTab::Display, "DISPLAY"),
    (SettingsTab::Audio, "AUDIO"),
    (SettingsTab::Controls, "CONTROLS"),
    (SettingsTab::Gameplay, "GAMEPLAY"),
    (SettingsTab::Files, "FILES"),
    (SettingsTab::Network, "NETWORK"),
    (SettingsTab::Advanced, "ADVANCED"),
    (SettingsTab::Diagnostics, "DIAGNOSTICS"),
];

pub(super) fn settings_page(
    ctx: &egui::Context,
    settings: &mut Settings,
    status: &Status,
    note: Option<&str>,
    tab: &mut SettingsTab,
    scale_edit: &mut Option<f32>,
    actions: &mut Vec<Action>,
    go: &mut Option<Option<Page>>,
) -> bool {
    sheet(ctx, "settings-window", "SETTINGS", |ui| {
        let scale = theme::density(ui);
        categories(ui, tab, scale);
        ui.add_space(6.0 * scale);
        if let Some(note) = note {
            ui.label(RichText::new(note).size(11.5 * scale).color(WARN));
        }
        settings_items(ui, settings, status, *tab, scale_edit, actions, go);
    })
}

/// The strip of categories above the list: one line, scrolled sideways when
/// nine names do not fit, the current one solid light.
fn categories(ui: &mut egui::Ui, tab: &mut SettingsTab, scale: f32) {
    egui::ScrollArea::horizontal().id_salt("settings-categories").auto_shrink([false, false]).show(ui, |ui| {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 1.0 * scale;
            for &(value, name) in CATEGORIES.iter() {
                let here = *tab == value;
                let galley = ui.painter().layout_no_wrap(
                    name.to_owned(),
                    egui::FontId::new(11.0 * scale, theme::fonts::strong()),
                    theme::palette::TEXT_MID,
                );
                let (rect, response) =
                    ui.allocate_exact_size(vec2(galley.size().x + 20.0 * scale, 28.0 * scale), Sense::click());
                response.widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::Button, true, here, name));
                let painter = ui.painter();
                if here {
                    painter.rect_filled(rect, 0, theme::palette::TEXT_HI);
                } else if response.hovered() || response.has_focus() {
                    painter.rect_filled(rect, 0, theme::palette::INK_800);
                }
                painter.galley(
                    pos2(rect.center().x - galley.size().x * 0.5, rect.center().y - galley.size().y * 0.5),
                    galley,
                    if here { theme::palette::INK_950 } else { theme::palette::TEXT_MID },
                );
                if response.has_focus() && !here {
                    painter.rect_stroke(rect, 0, Stroke::new(1.0, theme::palette::LIME), StrokeKind::Inside);
                }
                if focus(response).clicked() && !here {
                    *tab = value;
                }
            }
        });
    });
    // A hairline under the strip, as the pause menu has under its tabs.
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 1.0), Sense::hover());
    ui.painter().rect_filled(rect, 0, theme::palette::SEPARATOR);
}

/// A line with a switch: whether it changed.
fn switch_row(ui: &mut egui::Ui, label: &str, help: &str, on: &mut bool) -> bool {
    let before = *on;
    super::widgets::row(ui, label, help, |ui| toggle(ui, on));
    *on != before
}

/// A line with a value picked from a few: whether it changed.
fn pick_row<T: PartialEq + Copy>(
    ui: &mut egui::Ui,
    label: &str,
    help: &str,
    value: &mut T,
    options: &[(T, &str)],
) -> bool {
    let before = *value;
    super::widgets::row(ui, label, help, |ui| choice(ui, value, options));
    *value != before
}

/// A line with a slider: whether it changed.
fn slide_row(
    ui: &mut egui::Ui,
    label: &str,
    help: &str,
    value: &mut f32,
    range: std::ops::RangeInclusive<f32>,
    step: f64,
    suffix: &str,
    decimals: usize,
) -> bool {
    let before = *value;
    super::widgets::row(ui, label, help, |ui| slider(ui, value, range, step, suffix, decimals));
    (*value - before).abs() > f32::EPSILON
}

/// A line that does something when it is taken: whether it was.
fn action_row(ui: &mut egui::Ui, label: &str, help: &str, text: &str) -> bool {
    let response = super::widgets::row(ui, label, help, |ui| super::widgets::action_value(ui, text));
    response.clicked()
}

/// A line that only says something (a path, a resolution, a state).
fn info_row(ui: &mut egui::Ui, label: &str, help: &str, text: &str) {
    super::widgets::row(ui, label, help, |ui| super::widgets::value_text(ui, text));
}

fn settings_items(
    ui: &mut egui::Ui,
    settings: &mut Settings,
    status: &Status,
    tab: SettingsTab,
    scale_edit: &mut Option<f32>,
    actions: &mut Vec<Action>,
    go: &mut Option<Option<Page>>,
) {
    let scale = theme::density(ui);
    let _ = scale; // Android's diagnostics only
    let started = status.started;
    match tab {
        SettingsTab::Graphics => {
            let mut value = scale_edit.unwrap_or(settings.render_scale());
            let (rw, rh) = settings::render_size(status.window, value, settings.low_memory);
            let help = format!("{rw} × {rh} · the game is drawn at this scale and stretched to the screen");
            let r = super::widgets::row(ui, "Resolution scale", &help, |ui| {
                slider(ui, &mut value, RENDER_SCALES, 0.05, "%", 0)
            });
            if r.dragged() {
                *scale_edit = Some(value);
            } else if r.changed() || r.drag_stopped() {
                settings.render_scale = Some(settings::round_scale(value));
                *scale_edit = None;
            }
            if status.upscalers.len() > 1 {
                let options: Vec<_> = status
                    .upscalers
                    .iter()
                    .map(|&u| {
                        let name = match u {
                            Upscaling::Bicubic => "Bicubic",
                            Upscaling::MetalFx => "MetalFX",
                            Upscaling::MetalFxTemporal => "MetalFX Temporal",
                            Upscaling::Fsr => "FSR 2",
                        };
                        (u, name)
                    })
                    .collect();
                pick_row(ui, "Upscaling", "How the frame is stretched to the screen", &mut settings.upscaler, &options);
            }
            if settings.upscaler == Upscaling::Fsr {
                slide_row(ui, "Sharpening", "FSR 2's sharpening", &mut settings.sharpness, 0.0..=1.0, 0.05, "%", 0);
            }
            if status.frame_generation && settings.upscaler == Upscaling::MetalFxTemporal {
                switch_row(
                    ui,
                    "Frame generation",
                    "A frame between each two of the game's: twice the frames, half a frame of latency",
                    &mut settings.frame_generation,
                );
            }
            switch_row(ui, "VSync", "Wait for the display between frames", &mut settings.vsync);
            let unlimited = if settings.vsync { "Display" } else { "Unlimited" };
            let mut rates = vec![(0, unlimited), (60, "60")];
            if status.refresh_rate >= 119.0 {
                rates.push((40, "40"));
            }
            rates.push((30, "30"));
            let differs = started.is_some_and(|s| s.max_fps != settings.max_fps);
            if pick_row(ui, "Frame rate", next_start(differs).unwrap_or("Frames per second at most"), &mut settings.max_fps, &rates) && differs {
                // A change that needs the next start says so once.
            }
            let low_differs = started.is_some_and(|s| s.low_memory != settings.low_memory);
            if switch_row(
                ui,
                "Low memory",
                next_start(low_differs).unwrap_or("Smaller engine pools; also lowers graphics once"),
                &mut settings.low_memory,
            ) && settings.low_memory
            {
                settings.low_preset_written = false;
            }
            let reset_help = if status.graphics_reset {
                "The game's graphics settings are already the defaults"
            } else {
                "The game's own graphics settings, back to the engine's defaults"
            };
            if action_row(ui, "Restore graphics", reset_help, "RESET") && !status.graphics_reset && status.game_dir.is_some() {
                actions.push(Action::ResetGraphics);
            }
        }
        SettingsTab::Display => {
            info_row(
                ui,
                "Screen",
                "The window, and the display's refresh rate (the app fits every menu to it)",
                &format!("{} × {} · {:.0} Hz", status.window.0, status.window.1, status.refresh_rate),
            );
            if let Some((w, h)) = status.back_buffer {
                info_row(ui, "Game buffer", "What the game draws, before it is stretched", &format!("{w} × {h}"));
            }
            switch_row(
                ui,
                "Reduce motion",
                "Stop the start page's parallax and its scene changes",
                &mut settings.reduce_motion,
            );
            info_row(
                ui,
                "Menu scale",
                "Every menu follows the screen's height; the rows stay at the size of a thumb",
                &format!("{:.0} %", theme::layout_scale(ui.ctx().content_rect()) * 100.0),
            );
        }
        SettingsTab::Audio => {
            slide_row(ui, "Music", "The start page's track, before the game", &mut settings.music_volume, 0.0..=1.0, 0.05, "%", 0);
            switch_row(
                ui,
                "Muffle with the menu",
                "The music keeps playing behind a menu, dulled instead of stopped",
                &mut settings.muffle_music,
            );
            switch_row(ui, "Menu sounds", "The ticks and clicks of moving through the menus", &mut settings.menu_sounds);
            if settings.menu_sounds {
                slide_row(ui, "Sounds volume", "Beside the music, which has its own", &mut settings.sfx_volume, 0.0..=1.0, 0.05, "%", 0);
                let found = crate::sfx::installed(&crate::paths::user_dir().unwrap_or_default(), status.game_dir);
                info_row(
                    ui,
                    "Sound files",
                    "The menu sounds are the player's own: a sfx folder beside scenes, or glue5-sfx beside the game",
                    if found { "FOUND" } else { "NONE" },
                );
            }
        }
        SettingsTab::Controls => {
            pick_row(
                ui,
                "Touch controls",
                "Auto: shown on touch screens, hidden while a controller or keyboard is used",
                &mut settings.touch,
                &[(TouchMode::Auto, "Auto"), (TouchMode::Always, "On"), (TouchMode::Off, "Off")],
            );
            pick_row(
                ui,
                "Layout",
                "Modern: the actions used all the time on screen, the rest on the More button's fan. Classic: every action on screen",
                &mut settings.touch_layout,
                &[(TouchLayout::Modern, "Modern"), (TouchLayout::Classic, "Classic")],
            );
            pick_row(
                ui,
                "Car steering",
                "Stick: the left thumb steers where it lands. Arrows: left and right buttons (slide between them)",
                &mut settings.touch_steer,
                &[(TouchSteer::Stick, "Stick"), (TouchSteer::Arrows, "Arrows")],
            );
            slide_row(ui, "Size", "The on-screen controls' size", &mut settings.touch_size, 0.7..=1.5, 0.1, "×", 1);
            slide_row(ui, "Opacity", "How solid they are over the game", &mut settings.touch_opacity, 0.2..=1.0, 0.01, "%", 0);
            slide_row(ui, "Camera speed", "How fast a finger's drag turns the camera", &mut settings.touch_camera, 0.25..=3.0, 0.05, "×", 2);
            pick_row(
                ui,
                "Aim assist",
                "Lock-on follows the nearest target; Snap snaps, then you aim; Slow-down slows over targets",
                &mut settings.touch_aim,
                &[
                    (TouchAim::LockOn, "Lock-on"),
                    (TouchAim::Snap, "Snap"),
                    (TouchAim::SlowDown, "Slow-down"),
                    (TouchAim::Off, "Off"),
                ],
            );
            pick_row(
                ui,
                "Aim & fire",
                "Auto: holding Fire with a gun aims too. Manual: holding Aim shows a second Fire button on the left",
                &mut settings.touch_fire,
                &[(TouchFire::Auto, "Auto"), (TouchFire::Manual, "Manual")],
            );
            if crate::haptics::SUPPORTED {
                switch_row(ui, "Haptics", "Taps and the game's rumble, on devices that have it", &mut settings.haptics);
            }
        }
        SettingsTab::Gameplay => {
            // The same two flags the start page keeps: what Play starts.
            let mut mode = if settings.online {
                Mode::Online
            } else if settings.sandbox {
                Mode::Sandbox
            } else {
                Mode::Story
            };
            if pick_row(
                ui,
                "Mode",
                "What PLAY starts with: the story, GTA Online, or the whole map without missions",
                &mut mode,
                &[(Mode::Story, "Story"), (Mode::Online, "Online"), (Mode::Sandbox, "Sandbox")],
            ) {
                settings.online = mode == Mode::Online;
                settings.sandbox = mode == Mode::Sandbox;
            }
            super::widgets::row(ui, "Player name", "The name other players see in GTA Online", |ui| {
                text_field(ui, "", TextKind::Name, &mut settings.online_name, "Player name", 24)
            });
            switch_row(ui, "Mods", "The mods in the mods folder run with the game", &mut settings.mods);
            if action_row(ui, "Mods' menus", "The mods' own menus, over the game (F7)", "OPEN") {
                *go = Some(Some(Page::Mods));
            }
        }
        SettingsTab::Files => {
            if !TV {
                let differs = started.is_some_and(|s| s.source != settings.source);
                if pick_row(
                    ui,
                    "Game files",
                    next_start(differs).unwrap_or("Folder: the game on this device. Server: from a computer that shares it"),
                    &mut settings.source,
                    &[(Source::Folder, "Folder"), (Source::Server, "Server")],
                ) {

                }
            }
            match settings.source {
                Source::Folder => game_folder(ui, settings, status, actions),
                Source::Server => {
                    let summary = server_summary(settings, status);
                    if action_row(ui, "Server", &format!("{summary} · the others on this network, this device's copy"), "OPEN") {
                        *go = Some(Some(Page::Servers));
                    }
                }
            }
            if action_row(
                ui,
                "Download the whole game",
                "Stream it all to this device while it plays, instead of what it reads",
                if settings.download_all { "ON" } else { "OFF" },
            ) {
                settings.download_all = !settings.download_all;
            }
            if !MOBILE {
                switch_row(
                    ui,
                    "Share this folder",
                    "Other devices on this network may stream the game from this computer",
                    &mut settings.share,
                );
            }
            if status.game_dir.is_some() && action_row(ui, "Clear the local copy", "Deletes the copy this device streams, not your game folder", "CLEAR") {
                actions.push(Action::ClearCopy);
            }
            #[cfg(target_os = "android")]
            {
                if started.is_none() {
                    if action_row(ui, "Import save", "A GTA V profile folder from this phone", "CHOOSE…") {
                        actions.push(Action::ImportSave);
                    }
                    if action_row(
                        ui,
                        "Import game resources",
                        "A folder whose data/ and b/ paths match the game; existing files at the same path are replaced",
                        "CHOOSE…",
                    ) && status.source == Source::Folder
                        && status.game_dir.is_some()
                    {
                        actions.push(Action::ImportResources);
                    }
                }
            }
        }
        SettingsTab::Network => {
            let asking = match status.friend_requests {
                0 => "Your friends, requests and invites".to_owned(),
                n => format!("{n} waiting: friend requests, game invites"),
            };
            if action_row(ui, "Friends", &asking, "OPEN") {
                *go = Some(Some(Page::Friends));
            }
            switch_row(ui, "Friends' notifications", "Online, requests and invites, over the game", &mut settings.friend_notices);
            if action_row(ui, "Join a friend", "With the key a friend gave you", "OPEN") {
                *go = Some(Some(Page::Friends));
            }
            if let Some((address, state)) = status.stream {
                let (name, _) = stream_state(state);
                info_row(ui, "Server", "The computer this device streams from", &format!("{name} · {address}"));
            }
        }
        SettingsTab::Advanced => {
            let mut overlays = vec![(PerfOverlay::Off, "Off")];
            if cfg!(target_vendor = "apple") {
                overlays.push((PerfOverlay::MetalHud, "Metal HUD"));
            }
            overlays.push((PerfOverlay::Glue5, "Glue 5"));
            let metal_differs = settings.overlay == PerfOverlay::MetalHud && !status.metal_hud;
            if pick_row(
                ui,
                "Performance overlay",
                next_start(metal_differs).unwrap_or("Frames, the GPU and the frame's time, over the game"),
                &mut settings.overlay,
                &overlays,
            ) && metal_differs
            {
                // It shows from the next launch: the row says so.
            }
            if settings.overlay == PerfOverlay::Glue5 {
                pick_row(
                    ui,
                    "Overlay detail",
                    "How much it shows",
                    &mut settings.overlay_detail,
                    &[
                        (OverlayDetail::Minimal, "Minimal"),
                        (OverlayDetail::Standard, "Standard"),
                        (OverlayDetail::Full, "Full"),
                    ],
                );
                pick_row(
                    ui,
                    "Overlay corner",
                    "Where it sits over the game",
                    &mut settings.overlay_corner,
                    &[
                        (Corner::TopLeft, "Top left"),
                        (Corner::TopRight, "Top right"),
                        (Corner::BottomLeft, "Bottom left"),
                        (Corner::BottomRight, "Bottom right"),
                    ],
                );
            }
            switch_row(ui, "GLUE 5 Studio", "With mods, the game serves the Studio's pages", &mut settings.studio);
            if settings.studio {
                switch_row(ui, "Studio on the network", "Other devices may open it, with its pairing code", &mut settings.studio_lan);
                info_row(
                    ui,
                    "Pairing code",
                    "A browser paired once stays paired when the game starts again",
                    if settings.studio_code.is_empty() { "NONE YET" } else { settings.studio_code.as_str() },
                );
            }
        }
        SettingsTab::Diagnostics => {
            #[cfg(target_os = "android")]
            {
                ui.label(
                    RichText::new(
                        "Only entries from the GLUE 5 app process are captured; other apps are excluded. These logs may contain app paths or diagnostic data.",
                    )
                    .size(11.5 * scale)
                    .color(theme::palette::TEXT_MID),
                );
                if action_row(ui, "Start a capture", "Ask Shizuku for permission, then begin capturing", "START") {
                    actions.push(Action::StartLogcatCapture);
                }
                if action_row(ui, "Save the capture", "Stop the live stream and open save/share options", "SAVE") {
                    actions.push(Action::StopLogcatCapture);
                }
                if action_row(ui, "Previous session", "The last GLUE 5 process capture, through Shizuku", "CAPTURE") {
                    actions.push(Action::CaptureSessionLogcat);
                }
                if action_row(
                    ui,
                    "Import report",
                    "Copy the Android import paths, outcomes, save-file names and GPU details",
                    "COPY",
                ) {
                    actions.push(Action::CopyImportDiagnostics);
                }
                for (label, marker) in [
                    ("Screen freezes", DebugMarker::ScreenFrozen),
                    ("Audio continues", DebugMarker::AudioContinues),
                    ("Controls still respond", DebugMarker::ControlsRespond),
                    ("Controls stop responding", DebugMarker::ControlsUnresponsive),
                ] {
                    if action_row(ui, label, "Adds a timestamped line to the GLUE 5 log", "MARK") {
                        actions.push(Action::MarkDebugEvent(marker));
                    }
                }
            }
            let version = concat!("Glue 5 ", env!("CARGO_PKG_VERSION"), " and the projects it builds on");
            if action_row(ui, "Credits", version, "VIEW") {
                *go = Some(Some(Page::Credits));
            }
            info_row(ui, "Version", "This build", crate::version::LABEL);
            if started.is_some() && !MOBILE && action_row(ui, "Stop the game", "Back to this menu", "QUIT") {
                actions.push(Action::Quit);
            }
        }
    }
}

fn next_start(differs: bool) -> Option<&'static str> {
    differs.then_some("Next start")
}

fn game_folder(ui: &mut egui::Ui, settings: &mut Settings, status: &Status, actions: &mut Vec<Action>) {
    let started = status.started;
    let dir = if started.is_some() { settings.game_dir.as_deref() } else { status.game_dir };
    let mut shown = dir.map_or_else(|| "Not found".to_owned(), |d| d.display().to_string());
    if started.is_some_and(|s| s.source == Source::Folder && Some(s.game_dir.as_path()) != settings.game_dir.as_deref()) {
        shown.push_str(" · next start");
    }
    setting(ui, "Game folder", Control::Action, Some(&shown), |ui| {
        if action_button(ui, if MOBILE { "Find files" } else { "Choose…" }).clicked() {
            actions.push(if MOBILE { Action::FindGame } else { Action::ChooseGame });
        }
    });
    if !MOBILE {
        let shown = match &status.sharing {
            // Turned on while the game runs: it starts with the start page.
            None if settings.share && status.started.is_some() => {
                "Phones, tablets and TVs play it from here · next start".to_owned()
            }
            None => "Phones, tablets and TVs play it from here".to_owned(),
            Some(Sharing::Preparing(_)) => "Preparing…".to_owned(),
            Some(Sharing::On(a, _)) => a.first().map_or_else(|| "No network address".to_owned(), |a| format!("On: {a}")),
            Some(Sharing::Failed(why)) => why.clone(),
        };
        setting(ui, "Share on the network", Control::Toggle, Some(&shown), |ui| toggle(ui, &mut settings.share));
    }
}

#[cfg(test)]
mod tests {
    use super::{Mode, Page, SettingsTab, Status, settings_page, start_page, theme};
    use crate::settings::{PerfOverlay, Settings, Upscaling};
    use egui::{Id, Pos2, Rect, SafeAreaInsets, vec2};
    use std::path::Path;

    fn status<'a>() -> Status<'a> {
        Status {
            game_dir: Some(Path::new("game")),
            problem: None,
            started: None,
            last_failure: None,
            graphics_reset: false,
            window: (1440, 900),
            back_buffer: None,
            upscalers: &[Upscaling::Bicubic, Upscaling::MetalFx, Upscaling::MetalFxTemporal, Upscaling::Fsr],
            frame_generation: true,
            metal_hud: false,
            refresh_rate: 120.0,
            preparing: None,
            mode: Mode::Story,
            benchmark: None,
            online: false,
            friend_requests: 0,
            source: crate::settings::Source::Folder,
            stream: None,
            sharing: None,
            mods: false,
            mod_menus: false,
            studio: None,
            servers: &[],
            searching: false,
            stream_alert: None,
        }
    }

    #[test]
    fn start_page_icons_are_in_the_font() {
        let ctx = egui::Context::default();
        theme::install(&ctx);
        ctx.begin_pass(Default::default());
        ctx.end_pass().textures_delta.clear();
        assert!(ctx.fonts_mut(|f| f.has_glyphs(&egui::FontId::proportional(14.0), "⚙🗁🖧🗙★♡🛠›")));
    }

    #[test]
    fn settings_fill_the_safe_area_at_all_sizes() {
        for size in
            [vec2(1440.0, 900.0), vec2(768.0, 1024.0), vec2(390.0, 844.0), vec2(320.0, 480.0), vec2(844.0, 390.0)]
        {
            let ctx = egui::Context::default();
            theme::install(&ctx);
            let screen = Rect::from_min_size(Pos2::ZERO, size);
            for tab in [
                SettingsTab::Graphics,
                SettingsTab::Display,
                SettingsTab::Audio,
                SettingsTab::Controls,
                SettingsTab::Gameplay,
                SettingsTab::Files,
                SettingsTab::Network,
                SettingsTab::Advanced,
                SettingsTab::Diagnostics,
            ] {
                let mut settings = Settings::default();
                let mut tab = tab;
                for _ in 0..4 {
                    let mut output = ctx.run_ui(
                        egui::RawInput {
                            screen_rect: Some(screen),
                            safe_area_insets: Some(SafeAreaInsets(egui::epaint::MarginF32 {
                                top: 24.0,
                                bottom: 16.0,
                                left: 8.0,
                                right: 8.0,
                            })),
                            ..Default::default()
                        },
                        |root| {
                            settings_page(
                                root.ctx(),
                                &mut settings,
                                &status(),
                                None,
                                &mut tab,
                                &mut None,
                                &mut Vec::new(),
                                &mut None,
                            );
                        },
                    );
                    output.textures_delta.clear();
                }
                // Full screen: the shell is the safe area, not a window in it.
                let shell = ctx.memory(|m| m.area_rect(Id::new(("shell", "SETTINGS")))).unwrap();
                let safe = ctx.content_rect();
                assert!(
                    (shell.min - safe.min).length() < 2.0 && (shell.max - safe.max).length() < 2.0,
                    "{size:?} {tab:?}: the shell {shell:?} is not the safe area {safe:?}"
                );
            }
        }
    }

    #[test]
    fn a_phone_reaches_the_last_graphics_row_by_scrolling() {
        let ctx = egui::Context::default();
        theme::install(&ctx);
        let mut settings = Settings::default();
        let mut tab = SettingsTab::Graphics;
        let mut seen = false;
        for pass in 0..8 {
            let mut output = ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(874.0, 402.0))),
                    time: Some(pass as f64),
                    events: if pass == 4 {
                        vec![
                            egui::Event::PointerMoved(Pos2::new(300.0, 300.0)),
                            egui::Event::MouseWheel {
                                unit: egui::MouseWheelUnit::Point,
                                delta: vec2(0.0, -600.0),
                                phase: egui::TouchPhase::Move,
                                modifiers: egui::Modifiers::default(),
                            },
                        ]
                    } else {
                        Vec::new()
                    },
                    safe_area_insets: Some(SafeAreaInsets(egui::epaint::MarginF32 {
                        left: 62.0,
                        right: 62.0,
                        top: 0.0,
                        bottom: 21.0,
                    })),
                    ..Default::default()
                },
                |root| {
                    settings_page(
                        root.ctx(),
                        &mut settings,
                        &status(),
                        None,
                        &mut tab,
                        &mut None,
                        &mut Vec::new(),
                        &mut None,
                    );
                },
            );
            output.textures_delta.clear();
            if pass == 7 {
                // Every row that is drawn sits inside what is on screen.
                for shape in &output.shapes {
                    if let egui::Shape::Text(text) = &shape.shape
                        && text.galley.text() == "Restore graphics"
                    {
                        seen = shape.clip_rect.contains_rect(text.visual_bounding_rect());
                    }
                }
            }
        }
        assert!(seen, "the last graphics row is not reachable by scrolling");
    }

    #[test]
    fn a_controller_lands_on_the_modes_first_and_opens_gta_online() {
        let ctx = egui::Context::default();
        theme::install(&ctx);
        let key = |key| {
            [true, false].map(|pressed| egui::Event::Key {
                key,
                physical_key: None,
                pressed,
                repeat: false,
                modifiers: Default::default(),
            })
        };
        let mut settings = Settings::default();
        let mut open = false;
        let mut chosen = None;
        let keys = [None, Some(egui::Key::Tab), Some(egui::Key::ArrowDown), Some(egui::Key::Enter), None];
        for key in keys.map(|k| k.map_or(Vec::new(), |k| key(k).to_vec())) {
            let raw = egui::RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(1440.0, 900.0))),
                events: key,
                ..Default::default()
            };
            ctx.run_ui(raw, |root| {
                chosen = start_page(root.ctx(), &mut settings, &status(), None, &mut open, &mut Vec::new(), &mut None)
            })
            .textures_delta
            .clear();
        }
        // GTA Online opens first, its card keeps the focus, a name is made up.
        assert_eq!(chosen, None);
        assert!(open);
        assert_eq!(ctx.memory(|m| m.focused()), Some(Id::new(("mode", "GTA ONLINE"))));
        assert!(!settings.online_name.is_empty());

        // Down past the name to Play, which starts.
        let keys = vec![egui::Key::ArrowDown, egui::Key::ArrowDown, egui::Key::Enter];
        for k in keys.into_iter().map(Some).chain([None]) {
            let raw = egui::RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(1440.0, 900.0))),
                events: k.map_or(Vec::new(), |k| key(k).to_vec()),
                ..Default::default()
            };
            ctx.run_ui(raw, |root| {
                let page =
                    start_page(root.ctx(), &mut settings, &status(), None, &mut open, &mut Vec::new(), &mut None);
                chosen = chosen.or(page);
            })
            .textures_delta
            .clear();
        }
        assert_eq!(chosen, Some(Mode::Online));
    }

    #[test]
    fn credits_fit_the_safe_area() {
        for size in [vec2(1440.0, 900.0), vec2(390.0, 844.0), vec2(844.0, 390.0), vec2(320.0, 480.0)] {
            let ctx = egui::Context::default();
            theme::install(&ctx);
            for _ in 0..4 {
                let raw =
                    egui::RawInput { screen_rect: Some(Rect::from_min_size(Pos2::ZERO, size)), ..Default::default() };
                ctx.run_ui(raw, |root| {
                    assert!(!crate::ui::credits_page(root.ctx()));
                })
                .textures_delta
                .clear();
            }
            let window = ctx.memory(|m| m.area_rect(Id::new(("shell", "CREDITS")))).unwrap();
            assert!(ctx.content_rect().contains_rect(window), "{size:?}: {window:?}");
        }
    }

    #[test]
    fn friends_fit_the_safe_area() {
        for size in [vec2(1440.0, 900.0), vec2(390.0, 844.0), vec2(844.0, 390.0), vec2(320.0, 480.0)] {
            let ctx = egui::Context::default();
            theme::install(&ctx);
            let mut page = crate::ui::FriendsPage::default();
            for _ in 0..4 {
                let raw =
                    egui::RawInput { screen_rect: Some(Rect::from_min_size(Pos2::ZERO, size)), ..Default::default() };
                ctx.run_ui(raw, |root| {
                    let game = crate::ui::friends::Context { started: false, online: false };
                    assert!(!crate::ui::friends_page(root.ctx(), &mut page, game, &mut Vec::new()));
                })
                .textures_delta
                .clear();
            }
            let window = ctx.memory(|m| m.area_rect(Id::new(("shell", "FRIENDS")))).unwrap();
            assert!(ctx.content_rect().contains_rect(window), "{size:?}: {window:?}");
        }
    }

    #[test]
    fn home_reflows_when_the_same_window_is_resized() {
        let ctx = egui::Context::default();
        theme::install(&ctx);
        for size in
            [vec2(1440.0, 900.0), vec2(390.0, 844.0), vec2(320.0, 480.0), vec2(900.0, 500.0), vec2(1440.0, 900.0)]
        {
            let screen = Rect::from_min_size(Pos2::ZERO, size);
            for _ in 0..4 {
                let mut output =
                    ctx.run_ui(egui::RawInput { screen_rect: Some(screen), ..Default::default() }, |root| {
                        start_page(
                            root.ctx(),
                            &mut Settings::default(),
                            &status(),
                            None,
                            &mut false,
                            &mut Vec::new(),
                            &mut None,
                        );
                    });
                output.textures_delta.clear();
            }
            let home = ctx.memory(|m| m.area_rect(Id::new("start"))).unwrap();
            assert!(screen.contains_rect(home), "{size:?}: {home:?}");
        }
    }

    /// The start page, the Game settings and the Servers page in each state
    /// of the stream (and of sharing) lay out within the screen, the windows
    /// within the safe area.
    #[test]
    fn streaming_states_fit() {
        use crate::settings::Source;
        use glue5_stream::Phase;
        let base = glue5_stream::Status {
            phase: Phase::Ready,
            connected: true,
            error: None,
            cache_bytes: 3_700_000_000,
            boot: (300 << 20, 550 << 20),
            waiting: 0,
            total_bytes: 21_000_000_000,
            filling: false,
            complete: false,
            no_room: false,
        };
        let states = [
            glue5_stream::Status { phase: Phase::Connecting, error: Some("connection refused".into()), ..base.clone() },
            glue5_stream::Status { phase: Phase::Syncing { what: "text", done: 3 << 20, total: 9 << 20 }, ..base.clone() },
            base.clone(),
            glue5_stream::Status { filling: true, no_room: true, ..base.clone() },
            glue5_stream::Status { complete: true, connected: false, ..base.clone() },
            glue5_stream::Status { phase: Phase::Failed("the server has game build x".into()), ..base.clone() },
        ];
        use glue5_stream::Check;
        let found = |name: &str, check| (glue5_stream::discovery::Found { name: name.into(), address: format!("{name}.local:47100") }, check);
        let found = [
            found("Mac", Some(Check::Ready { ms: 3, at: vec!["192.168.1.20:47100".parse().unwrap()] })),
            found("A computer with a very long name that does not fit", Some(Check::Unusable("the server has game build x, this app runs y".into()))),
            found("Linux", Some(Check::Unreachable("connection refused".into()))),
            found("Asking", None),
        ];
        for size in [vec2(1440.0, 900.0), vec2(390.0, 844.0), vec2(844.0, 390.0)] {
            for (i, stream) in states.iter().enumerate() {
                let ctx = egui::Context::default();
                theme::install(&ctx);
                let screen = Rect::from_min_size(Pos2::ZERO, size);
                let mut settings = Settings::default();
                (settings.source, settings.server) = (Source::Server, "192.168.1.20".into());
                let mut status = status();
                status.source = Source::Server;
                status.stream = Some(("192.168.1.20", stream));
                status.servers = &found;
                status.sharing = Some(crate::sharing::State::On(vec!["10.0.0.2".into()], true));
                let mut tab = SettingsTab::Files;
                let mut servers = crate::ui::ServersPage::default();
                for page in 0..12 {
                    let mut output =
                        ctx.run_ui(egui::RawInput { screen_rect: Some(screen), ..Default::default() }, |root| {
                            let (actions, go) = (&mut Vec::new(), &mut None);
                            if page < 4 {
                                start_page(root.ctx(), &mut settings, &status, None, &mut false, actions, go);
                            } else if page < 8 {
                                settings_page(root.ctx(), &mut settings, &status, None, &mut tab, &mut None, actions, go);
                            } else {
                                crate::ui::servers_page(root.ctx(), &mut servers, &mut settings, &status, actions);
                            }
                        });
                    output.textures_delta.clear();
                }
                let home = ctx.memory(|m| m.area_rect(Id::new("start"))).unwrap();
                assert!(screen.contains_rect(home), "{size:?}, state {i}: {home:?}");
                for title in ["SETTINGS", "SERVERS"] {
                    let window = ctx.memory(|m| m.area_rect(Id::new(("shell", title)))).unwrap();
                    assert!(ctx.content_rect().contains_rect(window), "{size:?}, state {i}, {title}: {window:?}");
                }
            }
        }
    }

    /// A server found by name is known under the addresses its name stands for.
    #[test]
    fn a_server_found_is_known_by_its_address_too() {
        use glue5_stream::Check;
        let found = glue5_stream::discovery::Found { name: "Mac".into(), address: "mac.local:47100".into() };
        let at = vec!["10.0.0.2:47100".parse().unwrap(), "[fe80::1]:47100".parse().unwrap()];
        let server = (found, Some(Check::Ready { ms: 2, at }));
        for (address, same) in
            [("mac.local:47100", true), ("10.0.0.2:47100", true), ("10.0.0.2", true), ("10.0.0.2:47101", false), ("10.0.0.3", false)]
        {
            assert_eq!(super::is_server(&server, address), same, "{address}");
        }
        assert!(!super::is_server(&(server.0.clone(), None), "10.0.0.2"));
    }

    /// Streamed, the start page's files button opens the Servers page; the
    /// page, opened, searches the network.
    #[test]
    fn the_files_button_opens_the_servers_page() {
        let ctx = egui::Context::default();
        theme::install(&ctx);
        let mut settings = Settings::default();
        settings.source = crate::settings::Source::Server;
        let mut status = status();
        status.source = crate::settings::Source::Server;
        let screen = Rect::from_min_size(Pos2::ZERO, vec2(1440.0, 900.0));
        let mut button = None;
        let mut go = None;
        for pass in 0..4 {
            let events = match (pass, button) {
                (3, Some(at)) => vec![
                    egui::Event::PointerMoved(at),
                    egui::Event::PointerButton { pos: at, button: egui::PointerButton::Primary, pressed: true, modifiers: Default::default() },
                    egui::Event::PointerButton { pos: at, button: egui::PointerButton::Primary, pressed: false, modifiers: Default::default() },
                ],
                _ => Vec::new(),
            };
            let output = ctx.run_ui(egui::RawInput { screen_rect: Some(screen), events, ..Default::default() }, |root| {
                start_page(root.ctx(), &mut settings, &status, None, &mut false, &mut Vec::new(), &mut go);
            });
            // The button with the server's icon and name.
            button = button.or_else(|| {
                output.shapes.iter().find_map(|s| match &s.shape {
                    egui::Shape::Text(t) if t.galley.text().contains("Server") => Some(t.visual_bounding_rect().center()),
                    _ => None,
                })
            });
            let mut output = output;
            output.textures_delta.clear();
        }
        assert!(button.is_some(), "no Server button");
        assert_eq!(go, Some(Some(Page::Servers)));

        let mut page = crate::ui::ServersPage::default();
        page.opened();
        let mut actions = Vec::new();
        ctx.run_ui(egui::RawInput { screen_rect: Some(screen), ..Default::default() }, |root| {
            crate::ui::servers_page(root.ctx(), &mut page, &mut settings, &status, &mut actions);
        })
        .textures_delta
        .clear();
        assert!(matches!(actions[..], [crate::ui::Action::FindServers]));
    }
}
