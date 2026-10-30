//! The start page: the shell's PLAY tab. The four ways into the game as rows,
//! the chosen one repeated as the one action of the list, and what the app
//! knows about it below (the game's files, the source, the last run) — the
//! pause menu's list, in place of cards and a bottom rail.
use egui::Response;

use super::theme::density;
use super::widgets::{action_value, choice, info_text, row, value_text};
use super::{Action, Mode, Page, Status, WARN, shell};
use crate::settings::{Settings, Source};
use crate::sfx::{self, Sound};

/// The four modes: their name, their line in the list, and the description
/// panel's help.
const MODES: [(Mode, &str, &str); 4] = [
    (Mode::Story, "Story mode", "The story of Los Santos: three lives, one city"),
    (Mode::Online, "GTA Online", "Freemode with friends and rivals: your own character"),
    (Mode::Sandbox, "Sandbox", "The whole map, no missions, nothing to prove"),
    (Mode::Benchmark, "Benchmark", "A guided flight over the city, then the frame times"),
];

pub(super) fn show(
    ctx: &egui::Context,
    settings: &mut Settings,
    status: &Status,
    selected: &mut Mode,
    _gamepad_prompts: bool,
    starting: Option<Mode>,
    actions: &mut Vec<Action>,
    go: &mut Option<Option<Page>>,
) {
    // The start page keeps what Play starts, as it always has.
    settings.online = *selected == Mode::Online;
    settings.sandbox = *selected == Mode::Sandbox;
    let prepared = status.game_dir.is_some() && status.preparing.is_none();
    let going = starting.or(status.preparing.map(|_| status.mode));

    shell::frame(ctx, "PLAY", |ui| {
        let scale = density(ui);
        for (mode, name, help) in MODES {
            let here = *selected == mode;
            let response: Response = row(ui, name, help, |ui| {
                if here {
                    action_value(ui, "SELECTED")
                } else {
                    value_text(ui, "›")
                }
            });
            if response.clicked() && !here {
                *selected = mode;
                settings.online = mode == Mode::Online;
                settings.sandbox = mode == Mode::Sandbox;
                sfx::want(Sound::Focus);
            }
        }

        // Describe the actual chosen mode, not an unselected row under the
        // pointer. Other controls below can still provide their own help.
        if let Some((_, name, help)) = MODES.iter().find(|(mode, _, _)| *mode == *selected) {
            super::widgets::describe(ui.ctx(), name, help);
        }

        // GTA Online's own line: the name other players see.
        if *selected == Mode::Online {
            row(ui, "Player name", "The name other players see, made up on the first visit", |ui| {
                super::widgets::text_field(ui, "", super::TextKind::Name, &mut settings.online_name, "Player name", 24)
            });
        }

        ui.add_space(10.0 * scale);
        // The one action of the list: what A does, whatever line is lit.
        let label = if let Some(mode) = going {
            let which = MODES.iter().find(|m| m.0 == mode).map_or("", |m| m.1);
            status.preparing.map_or_else(|| format!("STARTING {}", which.to_uppercase()), |p| format!("PREPARING  {:.0}%", p * 100.0))
        } else {
            format!("PLAY {}", MODES.iter().find(|m| m.0 == *selected).map_or("", |m| m.1).to_uppercase())
        };
        let play = row(ui, &label, "Starts the game with the line selected above", |ui| {
            if prepared && going.is_none() { action_value(ui, "A") } else { value_text(ui, "—") }
        });
        if play.clicked() {
            if prepared && going.is_none() {
                // The one loud sound: the game is about to start.
                sfx::want(Sound::Launch);
                actions.push(Action::Play(*selected));
            } else {
                sfx::want(Sound::Error);
            }
        }
        if status.preparing.is_none() && status.game_dir.is_none() {
            ui.add_space(4.0 * scale);
            ui.label(egui::RichText::new("Choose a game folder below, or connect to a server").size(11.5 * scale).color(WARN));
        }

        ui.add_space(10.0 * scale);
        // What the app knows: the game's files, where they come from, how the
        // last run went. Two rows at most in normal use.
        let files = status.game_dir.map_or_else(
            || match status.source {
                Source::Server => "Not here: from a server".to_owned(),
                Source::Folder => "Not found".to_owned(),
            },
            |dir| dir.file_name().map_or_else(|| "Found".to_owned(), |name| name.to_string_lossy().into_owned()),
        );
        let folder_help = status.problem.unwrap_or("The game's folder, on this device");
        // The folder's own name, when there is one: it is what the player
        // recognizes, and the path itself stays out of the menu.
        let action = if status.game_dir.is_some() { format!("{files} ›") } else { "FIND FILES".to_owned() };
        if row(ui, "Game files", folder_help, |ui| action_value(ui, &action)).clicked() {
            actions.push(if crate::paths::MOBILE { Action::FindGame } else { Action::ChooseGame });
        }
        if crate::paths::TV {
            info_text(ui, "Source", "This device always streams the game", "SERVER");
        } else {
            let before = settings.source;
            row(ui, "Source", "Folder: the game on this device. Server: from a computer that shares it", |ui| {
                choice(ui, &mut settings.source, &[(Source::Folder, "Folder"), (Source::Server, "Server")])
            });
            if settings.source != before && settings.source == Source::Server {
                *go = Some(Some(Page::Servers));
            }
        }
        if let Some((address, state)) = status.stream {
            let (name, _) = super::pages::stream_state(state);
            if row(ui, "Server", &format!("{name}: {address}"), |ui| action_value(ui, "OPEN")).clicked() {
                *go = Some(Some(Page::Servers));
            }
        }
        if let Some(failure) = status.last_failure {
            info_text(ui, "Last session", failure, "STOPPED");
        }
        if let Some(result) = status.benchmark
            && *selected == Mode::Benchmark
        {
            info_text(
                ui,
                "Last run",
                &format!("{:.0} average, {:.0} lowest, {:.0} % under 33 ms", result.avg_fps, result.min_fps, result.smooth * 100.0),
                "BENCHMARK",
            );
        }
    });
}
