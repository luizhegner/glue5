//! The credits: what Glue 5 is, whose game it runs, and the open-source
//! projects it is built on, with their licenses and websites.
use super::pages::sheet;
use super::theme;
use super::widgets::{Control, action_button, setting};
use egui::RichText;


/// A project: its name, what it does here, its license, its website.
const PROJECTS: &[(&str, &str, &str, &str)] = &[
    ("WABT", "wasm2c: the game's code, to C", "Apache-2.0", "https://github.com/WebAssembly/wabt"),
    ("wgpu", "Graphics on Metal and Vulkan", "MIT / Apache-2.0", "https://wgpu.rs"),
    ("naga", "The game's shaders, translated", "MIT / Apache-2.0", "https://github.com/gfx-rs/wgpu"),
    ("SDL", "Window, input, sound, gamepads", "Zlib", "https://libsdl.org"),
    ("sdl3-rs", "SDL from Rust", "MIT", "https://github.com/vhspace/sdl3-rs"),
    ("egui", "This interface", "MIT / Apache-2.0", "https://github.com/emilk/egui"),
    ("Barlow", "Typeface, by Jeremy Tribby", "SIL OFL 1.1", "https://github.com/jpt/barlow"),
    (
        "game-icons.net",
        "Touch icons, by Lorc, Delapouite, John Colburn and Guard13007",
        "CC BY 3.0",
        "https://game-icons.net",
    ),
    ("Phosphor Icons", "Touch icons", "MIT", "https://phosphoricons.com"),
    ("Font Awesome Free", "Touch icons", "CC BY 4.0", "https://fontawesome.com"),
    ("Material Symbols", "Touch icons, by Google", "Apache-2.0", "https://fonts.google.com/icons"),
    ("Lucide", "Touch icons", "ISC", "https://lucide.dev"),
    ("Tabler Icons", "Touch icons", "MIT", "https://tabler.io/icons"),
    ("MingCute", "Touch icons", "Apache-2.0", "https://www.mingcute.com"),
    ("etcpak", "Textures for GPUs without BC", "BSD-3-Clause", "https://github.com/wolfpld/etcpak"),
    ("bcdec_rs", "Texture decoding", "MIT", "https://github.com/ScanMountGoat/image_dds"),
    ("snow", "The Noise protocol: friends' encrypted channel", "Apache-2.0 / MIT", "https://github.com/mcginty/snow"),
    ("Zstandard", "The game's small files, packed to stream", "BSD-3-Clause", "https://facebook.github.io/zstd"),
    #[cfg(target_os = "android")]
    (
        "libadrenotools",
        "GPU drivers added for Adreno GPUs, with liblinkernsbypass, by Billy Laws",
        "BSD-2-Clause",
        "https://github.com/bylaws/libadrenotools",
    ),
    #[cfg(target_vendor = "apple")]
    ("objc2", "Apple's frameworks from Rust", "MIT", "https://github.com/madsmtm/objc2"),
    #[cfg(windows)]
    ("mimalloc", "Memory allocator", "MIT", "https://github.com/microsoft/mimalloc"),
    ("Rust", "The language, its compiler and library", "MIT / Apache-2.0", "https://www.rust-lang.org"),
];

/// True when closed.
pub(super) fn credits_page(ctx: &egui::Context) -> bool {
    sheet(ctx, "credits-window", "CREDITS", |ui| {
        let scale = theme::density(ui);
        let text = |ui: &mut egui::Ui, text: &str| {
            ui.label(RichText::new(text).size(12.5 * scale).color(theme::MUTED));
        };
        egui::Frame::new()
            .fill(theme::expressive::SURFACE)
            .corner_radius(14)
            .inner_margin((16.0 * scale) as i8)
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.spacing_mut().item_spacing.y = 6.0 * scale;
                ui.horizontal(|ui| {
                    ui.label(theme::heading("GLUE 5", 26.0 * scale));
                    ui.label(
                        RichText::new(crate::version::LABEL)
                            .size(12.0 * scale)
                            .color(theme::MUTED),
                    );
                });
                text(
                    ui,
                    "GTA V's WebAssembly build as a native application for phones, tablets and computers: \
                     the game's code recompiled for this device, with its graphics, sound, controls and \
                     online play. Free software, under the GNU General Public License, version 3 or later.",
                );
            });
        text(
            ui,
            "Grand Theft Auto V is © Rockstar Games and Take-Two Interactive. Glue 5 is an independent \
             project, neither affiliated with nor endorsed by them, and includes none of the game's files: \
             it runs your own copy.",
        );
        ui.add_space(4.0 * scale);
        ui.label(theme::heading("OPEN SOURCE", 17.0 * scale).color(theme::MUTED));
        for &(name, what, license, url) in PROJECTS {
            setting(ui, name, Control::Action, Some(&format!("{what} · {license}")), |ui| {
                if action_button(ui, "Website").on_hover_text(url).clicked() {
                    ui.ctx().open_url(egui::OpenUrl::new_tab(url));
                }
            });
        }
        text(
            ui,
            "With the libraries these use, each under its own license (MIT, Apache-2.0, BSD, Zlib, \
             Unicode, …). GTA Online's players and friends find each other through the BitTorrent DHT, \
             kept by the people who run it.",
        );
    })
}
