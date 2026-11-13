//! The start page: the shell's PLAY tab. The four ways into the game as rows,
//! the chosen one repeated as the one action of the list, and what the app
//! knows about it below (the game's files, the source, the last run) — the
//! pause menu's list, in place of cards and a bottom rail.
use super::theme::{density, fonts, palette};
use super::widgets::{action_value, info_text, row, tab_button};
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
    show_with_spinner(ctx, settings, status, selected, None, _gamepad_prompts, starting, actions, go);
}

pub(super) fn show_with_spinner(
    ctx: &egui::Context,
    settings: &mut Settings,
    status: &Status,
    selected: &mut Mode,
    spinner: Option<&egui::TextureHandle>,
    _gamepad_prompts: bool,
    starting: Option<Mode>,
    actions: &mut Vec<Action>,
    go: &mut Option<Option<Page>>,
) {
    // The start page keeps what Play starts, as it always has.
    if status.started.is_some() {
        *selected = status.mode;
    } else {
        settings.online = *selected == Mode::Online;
        settings.sandbox = *selected == Mode::Sandbox;
    }
    let prepared = status.game_dir.is_some() && status.preparing.is_none() && status.started.is_none();
    let going = starting.or(status.preparing.map(|_| status.mode));
    let loading_label = going.map(|mode| {
        let which = MODES.iter().find(|m| m.0 == mode).map_or("", |m| m.1);
        status
            .preparing
            .map_or_else(|| format!("STARTING {}", which.to_uppercase()), |p| format!("PREPARING {:.0}%", p * 100.0))
    });

    shell::frame(ctx, "PLAY", |ui| {
        let scale = density(ui);
        let loading = going.is_some();
        ui.add_enabled_ui(!loading && status.started.is_none(), |ui| {
            let gap = 4.0 * scale;
            let width = ((ui.available_width()-gap*3.0)/4.0).max(1.0);
            ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = gap;
            for &(mode, name, _) in &MODES {
                if tab_button(ui, name, *selected == mode, egui::vec2(width,44.0*scale)).clicked() {
                    *selected = mode;
                    settings.online = mode == Mode::Online;
                    settings.sandbox = mode == Mode::Sandbox;
                    sfx::want(Sound::Focus);
                }
            }
            });
        });
        if let Some((_,_,help)) = MODES.iter().find(|(mode,_,_)| *mode==*selected) {
            ui.add_space(8.0*scale);
            ui.label(egui::RichText::new(*help).font(egui::FontId::new(11.5*scale,fonts::regular())).color(palette::TEXT_MID));
            ui.add_space(8.0*scale);
        }
        // The one action of the list: what A does, whatever line is lit.
        let label = if status.started.is_some() {
            "RESUME GAME".to_owned()
        } else if let Some(mode) = going {
            let which = MODES.iter().find(|m| m.0 == mode).map_or("", |m| m.1);
            status.preparing.map_or_else(|| format!("STARTING {}", which.to_uppercase()), |p| format!("PREPARING  {:.0}%", p * 100.0))
        } else {
            format!("PLAY {}", MODES.iter().find(|m| m.0 == *selected).map_or("", |m| m.1).to_uppercase())
        };
        let play = ui.add_enabled_ui(status.started.is_some() || (prepared && !loading), |ui| {
            tab_button(ui, &label, true, egui::vec2(ui.available_width(), 48.0 * scale))
        }).inner;
        if play.clicked() {
            if status.started.is_some() {
                *go = Some(None);
            } else {
                sfx::want(Sound::Launch);
                actions.push(Action::Play(*selected));
            }
        }
        if status.started.is_some() {
            ui.label("Your session is still running. Resume returns to it; Exit closes GLUE 5.");
        }
        if let Some(progress) = status.preparing {
            super::pages::rail(ui, progress);
        }
        if status.preparing.is_none() && status.game_dir.is_none() {
            ui.add_space(4.0 * scale);
            let message = status.problem.unwrap_or(match settings.source {
                Source::Folder => "Choose the folder containing your WebAssembly game files below.",
                Source::Server => "Open Servers below to connect and prepare the game files.",
            });
            ui.label(egui::RichText::new(message).size(11.5 * scale).color(WARN));
        }

        if *selected == Mode::Story && status.started.is_none() {
            ui.add_space(8.0 * scale);
            ui.add_enabled_ui(!loading, |ui| {
                if row(ui,"Story saves","Choose a save to start the story, or import a profile",|ui|action_value(ui,"OPEN SAVES")).clicked() {
                    *go = Some(Some(Page::Saves));
                }
            });
        }

        ui.add_space(8.0 * scale);
        ui.add_enabled_ui(!loading && status.started.is_none(), |ui| {
        // GTA Online's own line: the name other players see.
        if *selected == Mode::Online {
            row(ui, "Player name", "The name other players see, made up on the first visit", |ui| {
                super::widgets::text_field(ui, "", super::TextKind::Name, &mut settings.online_name, "Player name", 24)
            });
        }

        // What the app knows: the game's files, where they come from, how the
        // last run went. Two rows at most in normal use.
        let files = status.game_dir.map_or_else(
            || match status.source {
                Source::Server => "Not here: from a server".to_owned(),
                Source::Folder => "Not found".to_owned(),
            },
            |dir| dir.file_name().map_or_else(|| "Found".to_owned(), |name| name.to_string_lossy().into_owned()),
        );
        let from_server = settings.source == Source::Server;
        let folder_help = status.problem.unwrap_or(if from_server {
            "Game files stream from your GLUE server, not GTA Online. Open GLUE SERVER to manage the connection and cached files."
        } else {
            "The WebAssembly game's folder on this device; not a PC installation."
        });
        // The folder's own name, when there is one: it is what the player
        // recognizes, and the path itself stays out of the menu.
        let action = if from_server { "OPEN GLUE SERVER".to_owned() }
            else if status.game_dir.is_some() { files }
            else { "FIND FILES".to_owned() };
        if row(ui, "Game files", folder_help, |ui| action_value(ui, &action)).clicked() {
            if from_server {
                *go = Some(Some(Page::Servers));
            } else {
                actions.push(if crate::paths::MOBILE { Action::FindGame } else { Action::ChooseGame });
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
    });
    let safe = ctx.content_rect();
    let scene_width = safe.right() - shell::panel_rect(shell::output_rect(ctx), safe).right();
    if let Some(label) = loading_label.as_deref()
        && scene_width >= 352.0 * super::theme::layout_scale(safe)
    {
        loading_prompt(ctx, spinner, label);
    }
}

/// Where the loading prompt sits: thin against the safe area's bottom right,
/// on the scene beside the shell's panel (the menu's controls are in the
/// panel, on the left).
fn loading_prompt_rect(safe: egui::Rect, size: egui::Vec2, scale: f32) -> egui::Rect {
    let margin = 16.0 * scale;
    let min = egui::pos2(
        (safe.right() - size.x - margin).max(safe.left()),
        (safe.bottom() - size.y - margin).max(safe.top()),
    );
    egui::Rect::from_min_size(min, size)
}

/// The game's loading prompt while Play prepares or starts: one thin black
/// strap at the bottom right, its words at the left and the turning spinner
/// at its right. It answers to nothing.
fn loading_prompt(ctx: &egui::Context, spinner: Option<&egui::TextureHandle>, text: &str) {
    let safe = ctx.content_rect();
    if safe.width() <= 0.0 || safe.height() <= 0.0 {
        return;
    }
    let scale = super::theme::layout_scale(safe);
    let painter = ctx.layer_painter(egui::LayerId::new(egui::Order::Foreground, egui::Id::new("loading-prompt")));
    let pad_x = 12.0 * scale;
    let spinner_size = spinner.map_or(0.0, |_| 17.0 * scale);
    let spinner_gap = if spinner.is_some() { 10.0 * scale } else { 0.0 };
    let max_width = (320.0 * scale).min((safe.width() - 32.0 * scale).max(1.0));
    let text_width = (max_width - pad_x * 2.0 - spinner_size - spinner_gap).max(1.0);
    let galley = painter.layout(
        text.to_owned(),
        egui::FontId::new(12.0 * scale, fonts::strong()),
        palette::TEXT_HI,
        text_width,
    );
    let size = egui::vec2(
        (galley.size().x + pad_x * 2.0 + spinner_size + spinner_gap).min(max_width),
        (galley.size().y + 16.0 * scale).max(32.0 * scale),
    );
    let rect = loading_prompt_rect(safe, size, scale);
    painter.rect_filled(rect, 0, palette::BLOCK);
    painter.rect_stroke(rect, 0, egui::Stroke::new(1.0, palette::LINE), egui::StrokeKind::Inside);
    let text_h = galley.size().y;
    painter.galley(egui::pos2(rect.left() + pad_x, rect.center().y - text_h * 0.5), galley, palette::TEXT_HI);
    if let Some(spinner) = spinner {
        let spinner_rect = egui::Rect::from_center_size(
            egui::pos2(rect.right() - pad_x - spinner_size * 0.5, rect.center().y),
            egui::Vec2::splat(spinner_size),
        );
        let time = ctx.input(|i| i.time) as f32;
        super::spinner::paint(&painter, spinner, spinner_rect, -time * std::f32::consts::PI);
        ctx.request_repaint_after(std::time::Duration::from_millis(33));
    }
}

/// Initial resource preparation is separate from starting story mode. Its
/// backdrop uses the output; text keeps the shell's existing content bounds.
pub(super) fn preparation(ctx:&egui::Context,output:egui::Rect,spinner:Option<&egui::TextureHandle>,progress:Option<f32>) {
    let painter=ctx.layer_painter(egui::LayerId::new(egui::Order::Foreground,egui::Id::new("prepare-interface")));
    painter.with_clip_rect(output).rect_filled(output,0,egui::Color32::BLACK);
    let safe=ctx.content_rect();
    let scale=super::theme::menu_scale(ctx,safe);
    let left=safe.left()+24.0*scale;
    let top=safe.top()+safe.height()*0.35;
    painter.text(egui::pos2(left,top),egui::Align2::LEFT_TOP,"Preparing the interface",
        egui::FontId::new(22.0*scale,fonts::regular()),palette::TEXT_HI);
    let detail=if let Some(progress)=progress { format!("Receiving game resources · {:.0}%",progress.clamp(0.0,1.0)*100.0) }
        else { "Opening artwork from the selected game build".to_owned() };
    let galley=painter.layout(detail,egui::FontId::new(13.0*scale,fonts::regular()),palette::TEXT_MID,
        (safe.right()-left-24.0*scale).max(1.0));
    painter.galley(egui::pos2(left,top+40.0*scale),galley,palette::TEXT_MID);
    loading_prompt(ctx,spinner,"PREPARING INTERFACE");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loading_prompt_hugs_the_bottom_right_of_the_safe_area() {
        let safe = egui::Rect::from_min_max(egui::pos2(10.0, 20.0), egui::pos2(410.0, 220.0));
        let rect = loading_prompt_rect(safe, egui::vec2(180.0, 32.0), 1.0);
        assert_eq!(rect.left(), 214.0);
        assert_eq!(rect.top(), 172.0);
        assert_eq!(rect.right(), 394.0);
        assert_eq!(rect.bottom(), 204.0);
    }
}
