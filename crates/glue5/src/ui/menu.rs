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
    let mut loading = super::loading::Feedback::default();
    show_with_spinner(ctx, settings, status, selected, None, &mut loading, _gamepad_prompts, starting, actions, go);
}

pub(super) fn show_with_spinner(
    ctx: &egui::Context,
    settings: &mut Settings,
    status: &Status,
    selected: &mut Mode,
    spinner: Option<&egui::TextureHandle>,
    loading_feedback: &mut super::loading::Feedback,
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
    // Screen::progress measures the real pipeline preparation. No Screen yet
    // means indeterminate startup, not a synthetic "100%" completion.
    let progress = status.preparing.flatten().filter(|p| p.is_finite()).map(|p| p.clamp(0.0, 1.0));
    let phase = if status.preparing.is_some() { "pipelines" } else { "launch" };
    let indicator = loading_feedback.observe(going.is_some(), phase, progress, std::time::Instant::now());
    let loading_label = going.map(|mode| {
        let which = match mode { Mode::Story => "modo história", Mode::Online => "modo online", Mode::Sandbox => "sandbox", Mode::Benchmark => "benchmark" };
        match progress {
            Some(p) => format!("Preparando gráficos · {:.0}% · {which}", p * 100.0),
            None => format!("Iniciando {which}"),
        }
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
        } else if loading {
            "CARREGANDO".to_owned()
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
        // The running status belongs to the Play list, not a separate
        // foreground popup at the bottom-right (which competed with the
        // footer, safe-area cutouts and long translated messages).
        if let Some(label) = loading_label.as_deref() {
            if indicator.visible {
                ui.add_space(8.0 * scale);
                loading_line(ui, spinner, label, settings.reduce_motion);
                if indicator.quiet {
                    ui.label(egui::RichText::new("Sem atualização de progresso; a etapa pode continuar ativa")
                        .font(egui::FontId::new(11.0 * scale, fonts::regular()))
                        .color(palette::TEXT_MID));
                } else if indicator.details && progress.is_none() {
                    ui.label(egui::RichText::new("Aguardando dados da inicialização")
                        .font(egui::FontId::new(11.0 * scale, fonts::regular()))
                        .color(palette::TEXT_MID));
                }
            }
        }
        if let Some(progress) = progress {
            super::pages::rail(ui, progress);
        }
        if status.preparing.is_none() && status.game_dir.is_none() {
            ui.add_space(4.0 * scale);
            let message = status.problem.unwrap_or(match settings.source {
                Source::Folder => "Choose your WebAssembly game folder in Settings / Files.",
                Source::Server => "Open GLUE SERVER to connect and prepare the game files.",
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
    if loading_label.is_some() {
        // Ensure the 400 ms appearance threshold is reached even if the
        // engine does not report new progress on this frame.
        ctx.request_repaint_after(std::time::Duration::from_millis(100));
    }
}

/// Inline Play status. The shell's scroll area owns all clipping,
/// width constraints and keyboard/safe-area insets. Do not draw a floating
/// foreground prompt over the footer or outside the scrollable panel.
fn loading_line(
    ui: &mut egui::Ui,
    spinner: Option<&egui::TextureHandle>,
    label: &str,
    reduce_motion: bool,
) {
    let scale = density(ui);
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 8.0 * scale;
        if let Some(texture) = spinner {
            let (rect, _) = ui.allocate_exact_size(egui::Vec2::splat(18.0 * scale), egui::Sense::hover());
            let time = ui.input(|i| i.time) as f32;
            let angle = if reduce_motion {
                0.0
            } else {
                -(time * 10.0).floor() * 0.1 * std::f32::consts::PI
            };
            let painter = ui.painter().with_clip_rect(ui.clip_rect());
            super::spinner::paint(&painter, texture, rect, angle);
            if !reduce_motion {
                ui.ctx().request_repaint_after(std::time::Duration::from_millis(100));
            }
        }
        // A label wraps to the remaining width of this row, so a long
        // context/error message never pushes the spinner into the cutout.
        ui.add(egui::Label::new(
            egui::RichText::new(label)
                .font(egui::FontId::new(12.0 * scale, fonts::regular()))
                .color(palette::TEXT_HI),
        ).wrap());
    });
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
    // Keep this status with the actual preparation text, instead of adding
    // a second banner near the bottom-right corner.
    let label_pos = egui::pos2(left,top+72.0*scale);
    let icon_size = 17.0*scale;
    if let Some(spinner) = spinner {
        let rect = egui::Rect::from_min_size(label_pos,egui::Vec2::splat(icon_size));
        let time = ctx.input(|i|i.time) as f32;
        super::spinner::paint(&painter.with_clip_rect(safe),spinner,rect,-(time*10.0).floor()*0.1*std::f32::consts::PI);
        ctx.request_repaint_after(std::time::Duration::from_millis(100));
    }
    let label=painter.layout("PREPARING INTERFACE".to_owned(),egui::FontId::new(12.0*scale,fonts::regular()),palette::TEXT_MID,
        (safe.right()-left-32.0*scale).max(1.0));
    painter.with_clip_rect(safe).galley(egui::pos2(left+icon_size+8.0*scale,label_pos.y),label,palette::TEXT_MID);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loading_line_stays_clipped_to_its_parent_on_narrow_screens() {
        let ctx = egui::Context::default();
        super::super::theme::install(&ctx);
        let safe = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(190.0, 120.0));
        let content = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(150.0, 88.0));
        let mut output = ctx.run_ui(
            egui::RawInput { screen_rect: Some(safe), ..Default::default() },
            |ui| {
                ui.set_width(content.width());
                ui.set_clip_rect(content);
                loading_line(ui, None,
                    "Preparando gráficos · 26% · modo história com uma mensagem longa",
                    true);
            },
        );
        // The isolated egui context has no renderer to consume font texture uploads.
        output.textures_delta.clear();
        let texts: Vec<_> = output.shapes.iter().filter_map(|item| {
            if matches!(&item.shape, egui::Shape::Text(_)) {
                Some(item.clip_rect)
            } else {
                None
            }
        }).collect();
        assert!(!texts.is_empty(), "the loading status must be visible");
        for clip in texts {
            assert!(clip.right() <= content.right(), "status escapes the parent's right edge: {clip:?}");
            assert!(clip.bottom() <= content.bottom(), "status escapes the parent's bottom edge: {clip:?}");
        }
    }
}
