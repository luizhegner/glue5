//! Mods: switched on or off here; the mods in the user directory's `mods`
//! folder, started with the game or during it; their menus while it runs;
//! GLUE 5 Studio (the web IDE the app serves) with its address, QR code
//! and pairing code; the mods' log.

use egui::{Color32, CornerRadius, RichText, Sense, vec2};
use glue5_mods::ModState;

use super::pages::{card_title, sheet, small};
use super::widgets::{Control, action_button, action_buttons, choice, inline_button, segmented, setting, slider, toggle};
use super::{Status, WARN, theme};
use crate::settings::Settings;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Tab {
    Menus,
    #[default]
    Mods,
    Studio,
    Log,
}

/// What the page keeps between frames.
#[derive(Default)]
pub struct ModsPage {
    tab: Tab,
}

impl ModsPage {
    /// Opens on the mods' menus.
    pub fn show_menus(&mut self) {
        self.tab = Tab::Menus;
    }
}

/// The Studio as it serves, for the page.
pub struct StudioInfo {
    /// The page's address, the pairing code in it.
    pub url: String,
    /// The pairing code as people type it.
    pub code: String,
    pub lan: bool,
}

const ERR: Color32 = Color32::from_rgb(240, 149, 149);

/// The mods in a line, for Settings: off, on, what runs.
pub(super) fn summary(settings: &Settings, status: &Status) -> String {
    match status.started.as_ref() {
        Some(s) if s.mods != settings.mods => {
            return if settings.mods { "On from the next start".into() } else { "Off from the next start".into() };
        }
        _ => {}
    }
    if !settings.mods {
        return "Off · scripts that change the game".into();
    }
    let mods = glue5_mods::mods();
    let mut parts = vec!["On".to_owned()];
    match mods.len() {
        0 => {}
        1 => parts.push("1 mod".into()),
        n => parts.push(format!("{n} mods")),
    }
    let running = mods.iter().filter(|m| m.state == ModState::Running).count();
    if running > 0 {
        parts.push(format!("{running} running"));
    }
    if status.studio.is_some() {
        parts.push("Studio on".into());
    }
    parts.join(" · ")
}

/// The start page's card while mods are on: what there is, and the way
/// into the Studio.
pub(super) fn home_card(ui: &mut egui::Ui, settings: &Settings, status: &Status, go: &mut Option<Option<super::Page>>) {
    let state = match status.started.as_ref() {
        Some(s) if !s.mods => ("Next start", WARN),
        _ => ("On", super::pages::READY),
    };
    card_title(ui, "MODS", Some(state));
    let mods = glue5_mods::mods();
    let running = mods.iter().filter(|m| m.state == ModState::Running).count();
    let mut line = match mods.len() {
        0 => "No mods yet".to_owned(),
        1 => "1 mod".to_owned(),
        n => format!("{n} mods"),
    };
    if running > 0 {
        line.push_str(&format!(" · {running} running"));
    }
    match &status.studio {
        Some(info) => {
            line.push_str(if info.lan { " · Studio on this network" } else { " · Studio on this device" });
            small(ui, &line, theme::MUTED);
            let address = info.url.split('?').next().unwrap_or(&info.url).trim_end_matches('/').to_owned();
            ui.label(RichText::new(format!("{address}  ·  code {}", info.code)).monospace().color(Color32::WHITE));
            let [open, copy, page] = <[_; 3]>::try_from(
                ui.horizontal(|ui| action_buttons(ui, &["Open Studio", "Copy address", "Mods"])).inner,
            )
            .unwrap();
            if open.on_hover_text("In this device's browser").clicked() {
                ui.ctx().open_url(egui::OpenUrl::new_tab(&info.url));
            }
            if copy.clicked() {
                ui.ctx().copy_text(info.url.clone());
            }
            if page.clicked() {
                *go = Some(Some(super::Page::Mods));
            }
        }
        None => {
            if !settings.studio {
                line.push_str(" · Studio off");
            }
            small(ui, &line, theme::MUTED);
            if ui.horizontal(|ui| action_button(ui, "Mods")).inner.clicked() {
                *go = Some(Some(super::Page::Mods));
            }
        }
    }
}

/// True when closed.
pub(super) fn mods_page(ctx: &egui::Context, page: &mut ModsPage, settings: &mut Settings, status: &Status) -> bool {
    // Mods change by themselves (they end, fail, the Studio starts them).
    ctx.request_repaint_after(std::time::Duration::from_millis(500));
    sheet(ctx, "mods-window", "MODS", |ui| {
        let scale = theme::density(ui);
        let running_with = status.started.as_ref().map(|s| s.mods);
        let note = match running_with {
            Some(with) if with != settings.mods => {
                Some(if settings.mods { "On from the next start of the game" } else { "Off from the next start; running mods keep going until then" })
            }
            _ => None,
        };
        setting(ui, "Mods", Control::Toggle, Some(note.unwrap_or("Luau scripts that change the game, run with it")), |ui| {
            toggle(ui, &mut settings.mods);
        });
        if !settings.mods && !status.mods {
            ui.add_space(4.0 * scale);
            panel(ui, |ui| {
                card_title(ui, "WHAT MODS ARE", None);
                small(
                    ui,
                    "A mod is a folder of Luau scripts that call the game's own functions: spawn a car, change \
                     the weather, add a menu of its own. Turn mods on, then write them in GLUE 5 Studio from a \
                     browser on your computer or tablet, or let Claude Code write them.",
                    theme::MUTED,
                );
            });
            return;
        }
        ui.add_space(6.0 * scale);
        // The menus are there while the game runs with mods.
        let mut tabs = vec![(Tab::Mods, "MODS"), (Tab::Studio, "STUDIO"), (Tab::Log, "LOG")];
        if status.mods {
            tabs.insert(0, (Tab::Menus, "MENUS"));
        } else if page.tab == Tab::Menus {
            page.tab = Tab::Mods;
        }
        segmented(ui, &mut page.tab, &tabs, 40.0 * scale);
        ui.add_space(10.0 * scale);
        match page.tab {
            Tab::Menus => menus_tab(ui),
            Tab::Mods => mods_tab(ui, page, status),
            Tab::Studio => studio_tab(ui, settings, status),
            Tab::Log => log_tab(ui),
        }
    })
}

/// The mods' menus, each under its title: their items with the app's own
/// widgets; what the player does goes to the mod.
fn menus_tab(ui: &mut egui::Ui) {
    let scale = theme::density(ui);
    let menus = glue5_mods::menus();
    if menus.is_empty() {
        muted(ui, "No menus. A mod makes one with ui.menu(\"Title\"); F7 opens them during the game.");
        return;
    }
    for menu in &menus {
        ui.add_space(4.0 * scale);
        ui.horizontal(|ui| {
            ui.label(theme::heading(&menu.title.to_uppercase(), 17.0 * scale).color(Color32::WHITE));
            ui.label(RichText::new(&menu.module).size(12.0 * scale).color(theme::MUTED));
        });
        ui.push_id(menu.id, |ui| {
            for item in &menu.items {
                ui.push_id(item.id, |ui| menu_item(ui, &menu.module, item));
            }
        });
    }
}

fn menu_item(ui: &mut egui::Ui, module: &str, item: &glue5_mods::MenuItem) {
    use glue5_mods::{MenuInput, MenuKind};
    let note = item.note.as_deref();
    match &item.kind {
        MenuKind::Toggle(on) => {
            let mut v = *on;
            setting(ui, &item.label, Control::Toggle, note, |ui| {
                if toggle(ui, &mut v).changed() {
                    glue5_mods::menu_input(module, item.id, MenuInput::Toggle(v));
                }
            });
        }
        MenuKind::Slider { value, min, max, step } => {
            let mut v = *value as f32;
            let decimals = if step.fract() == 0.0 { 0 } else { 2 };
            setting(ui, &item.label, Control::Slider, note, |ui| {
                if slider(ui, &mut v, *min as f32..=*max as f32, *step, "", decimals).changed() {
                    glue5_mods::menu_input(module, item.id, MenuInput::Value(f64::from(v)));
                }
            });
        }
        MenuKind::Choice { options, index } => {
            let mut i = *index;
            let labelled: Vec<(usize, &str)> = options.iter().enumerate().map(|(n, o)| (n, o.as_str())).collect();
            setting(ui, &item.label, Control::Choice, note, |ui| {
                if choice(ui, &mut i, &labelled).changed() {
                    glue5_mods::menu_input(module, item.id, MenuInput::Choose(i));
                }
            });
        }
        MenuKind::Button => {
            setting(ui, &item.label, Control::Action, note, |ui| {
                if action_button(ui, "Run").clicked() {
                    glue5_mods::menu_input(module, item.id, MenuInput::Press);
                }
            });
        }
        MenuKind::Label => muted(ui, &item.label),
    }
}

fn mods_tab(ui: &mut egui::Ui, page: &mut ModsPage, status: &Status) {
    let held = glue5_mods::held_back();
    if !held.is_empty() {
        small(
            ui,
            &format!("{} did not start with the game: it was running when the game stopped last time.", held.join(", ")),
            WARN,
        );
    }
    let mods = glue5_mods::mods();
    if mods.is_empty() {
        panel(ui, |ui| {
            card_title(ui, "NO MODS YET", None);
            small(ui, "Write one in GLUE 5 Studio, from a browser on this network.", theme::MUTED);
            if let Some(dir) = glue5_mods::dir().filter(|_| !crate::paths::MOBILE) {
                small(ui, &format!("Or put a folder with a main.luau in {}", dir.display()), theme::MUTED);
            }
            ui.add_space(4.0 * theme::density(ui));
            if inline_button(ui, "Open Studio").clicked() {
                page.tab = Tab::Studio;
            }
        });
        return;
    }
    // Before the game: which start with it.
    if !status.mods {
        for m in &mods {
            let mut on = m.info.autostart;
            let note = if on { "Starts with the game" } else { "Not started with the game" };
            setting(ui, &m.info.title, Control::Toggle, Some(note), |ui| {
                if toggle(ui, &mut on).changed()
                    && let Err(e) = glue5_mods::set_autostart(&m.name, on)
                {
                    log::warn!("mods: {e}");
                }
            });
        }
        return;
    }
    for m in &mods {
        let subtitle = match m.state {
            ModState::Running if m.waiting => "Waiting".to_owned(),
            ModState::Running => format!("Running · {:.1} ms per frame", m.cpu_ms),
            ModState::Error => format!("Error · {}", m.error.as_deref().unwrap_or("")),
            ModState::Stopped if m.info.autostart => "Stopped · starts with the game".to_owned(),
            ModState::Stopped => "Stopped".to_owned(),
        };
        let dot = match m.state {
            ModState::Running => "• ",
            _ => "",
        };
        setting(ui, &format!("{dot}{}", m.info.title), Control::Action, Some(&subtitle), |ui| {
            let running = m.state == ModState::Running;
            if action_button(ui, if running { "Stop" } else { "Start" }).clicked() {
                if running {
                    glue5_mods::stop(&m.name);
                } else {
                    glue5_mods::start(&m.name);
                }
            }
        });
    }
    let running: Vec<_> = mods.iter().filter(|m| m.state == ModState::Running).collect();
    if !running.is_empty() {
        let total: f32 = running.iter().map(|m| m.cpu_ms).sum();
        setting(ui, &format!("{} running", running.len()), Control::Action, Some(&format!("{total:.1} ms per frame")), |ui| {
            if action_button(ui, "Stop all").clicked() {
                for m in &running {
                    glue5_mods::stop(&m.name);
                }
            }
        });
    }
    if glue5_mods::tainted() {
        small(ui, "A native failed inside the game: restart the game before long.", WARN);
    }
}

fn studio_tab(ui: &mut egui::Ui, settings: &mut Settings, status: &Status) {
    setting(ui, "GLUE 5 Studio", Control::Toggle, Some("Write and run mods from a browser"), |ui| {
        toggle(ui, &mut settings.studio);
    });
    let others = if settings.studio_lan { "Browsers on this network connect with the code" } else { "Only a browser on this device" };
    setting(ui, "Other devices", Control::Toggle, Some(others), |ui| {
        toggle(ui, &mut settings.studio_lan);
    });
    match &status.studio {
        Some(info) => {
            // The address as typed; copied with the code in it.
            let address = info.url.split('?').next().unwrap_or(&info.url).trim_end_matches('/');
            let open = if info.lan { "Open it in a browser on this network" } else { "Open it in a browser on this device" };
            setting(ui, address, Control::Actions, Some(open), |ui| {
                let [go, copy] = <[_; 2]>::try_from(action_buttons(ui, &["Open", "Copy"])).unwrap();
                if go.on_hover_text("In this device's browser").clicked() {
                    ui.ctx().open_url(egui::OpenUrl::new_tab(&info.url));
                }
                if copy.clicked() {
                    ui.ctx().copy_text(info.url.clone());
                }
            });
            setting(ui, &info.code, Control::Actions, Some("Pairing code: the page asks for it once"), |ui| {
                let [new, copy] = <[_; 2]>::try_from(action_buttons(ui, &["New", "Copy"])).unwrap();
                if new.on_hover_text("A new code: browsers paired with this one must pair again").clicked() {
                    settings.studio_code.clear();
                }
                if copy.clicked() {
                    ui.ctx().copy_text(info.code.clone());
                }
            });
            if info.lan {
                qr_row(ui, &info.url);
            }
        }
        None if settings.studio => small(ui, "Starting…", theme::MUTED),
        None => {}
    }
    muted(ui, "Whoever has the code can run code in the game: make a new one if someone should no longer have it.");
}

/// The address as a QR code, in a row like the settings': a tablet's or a
/// phone's camera opens the Studio, paired.
fn qr_row(ui: &mut egui::Ui, url: &str) {
    let scale = theme::density(ui);
    egui::Frame::new()
        .fill(theme::expressive::SURFACE)
        .corner_radius(0)
        .inner_margin((12.0 * scale) as i8)
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                qr(ui, url, 112.0 * scale);
                ui.add_space(8.0 * scale);
                ui.vertical(|ui| {
                    ui.label(RichText::new("Scan to open").size(14.0 * scale).color(Color32::WHITE));
                    ui.label(
                        RichText::new("With a tablet's or a phone's camera: the Studio opens already paired.")
                            .size(11.0 * scale)
                            .color(theme::MUTED),
                    );
                });
            });
        });
}

fn log_tab(ui: &mut egui::Ui) {
    let scale = theme::density(ui);
    // The last lines only (a copy of the whole log each frame held the lock
    // the mods' update takes).
    let lines = glue5_mods::recent_logs(80);
    if lines.is_empty() {
        muted(ui, "Nothing yet.");
    }
    panel(ui, |ui| {
        for l in &lines {
            let color = if l.level == glue5_mods::LogLevel::Error { ERR } else { Color32::from_gray(213) };
            ui.label(RichText::new(format!("{} · {}", l.module, l.text)).monospace().size(12.0 * scale).color(color));
        }
    });
}

/// `text` as a QR code, dark on white.
fn qr(ui: &mut egui::Ui, text: &str, size: f32) {
    // Encoded once per address (the page is drawn every frame).
    thread_local! {
        static CODE: std::cell::RefCell<Option<(String, usize, Vec<qrcode::Color>)>> = const { std::cell::RefCell::new(None) };
    }
    let code = CODE.with_borrow_mut(|cached| {
        if cached.as_ref().is_none_or(|(t, ..)| t != text) {
            *cached = qrcode::QrCode::new(text.as_bytes()).ok().map(|c| (text.to_string(), c.width(), c.to_colors()));
        }
        cached.as_ref().map(|(_, modules, colors)| (*modules, colors.clone()))
    });
    let Some((modules, colors)) = code else { return };
    let (rect, _) = ui.allocate_exact_size(vec2(size, size), Sense::hover());
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, CornerRadius::same(0), Color32::WHITE);
    let quiet = 2.0;
    let cell = size / (modules as f32 + 2.0 * quiet);
    for (i, c) in colors.iter().enumerate() {
        if *c == qrcode::Color::Dark {
            let (x, y) = ((i % modules) as f32, (i / modules) as f32);
            let min = rect.min + vec2((x + quiet) * cell, (y + quiet) * cell);
            painter.rect_filled(egui::Rect::from_min_size(min, vec2(cell + 0.5, cell + 0.5)), 0, theme::INK);
        }
    }
}

fn panel(ui: &mut egui::Ui, add: impl FnOnce(&mut egui::Ui)) {
    let scale = theme::density(ui);
    egui::Frame::new()
        .fill(theme::expressive::SURFACE)
        .corner_radius(0)
        .inner_margin((14.0 * scale) as i8)
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.spacing_mut().item_spacing.y = 6.0 * scale;
            add(ui);
        });
}

fn muted(ui: &mut egui::Ui, text: &str) {
    ui.label(RichText::new(text).size(12.0 * theme::density(ui)).color(theme::MUTED));
}
