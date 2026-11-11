//! The credits: what Glue 5 is, whose game it runs, and the open-source
//! projects it is built on, with their licenses and websites.
use super::{shell, theme};
use super::widgets::{action_value, row};
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
    ("Manrope", "Fallback interface typeface", "SIL OFL 1.1", "https://github.com/google/fonts/tree/main/ofl/manrope"),
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
    shell::frame(ctx, "CREDITS", |ui| {
        let scale = theme::density(ui);
        let text = |ui: &mut egui::Ui, text: &str| {
            ui.label(RichText::new(text).font(egui::FontId::new(12.5 * scale,theme::fonts::regular())).color(theme::palette::TEXT_MID));
        };
        egui::Frame::new()
            .fill(theme::palette::BLOCK)
            .corner_radius(0)
            .inner_margin((12.0 * scale) as i8)
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.set_max_width((600.0*scale).min(ui.available_width()));
                ui.spacing_mut().item_spacing.y = 8.0 * scale;
                ui.label(theme::heading("GLUE 5", 26.0 * scale));
                text(ui,crate::version::LABEL);
                text(
                    ui,
                    "A native runtime for GTA V's WebAssembly build. This Android fork focuses on \
                     Galaxy S21+ and its Mali-G78 GPU.",
                );
            });
        section(ui,"ANDROID FORK");
        link(ui,"Luiz Hegner","Fork maintainer · Galaxy S21+ / Mali-G78","GITHUB","https://github.com/luizhegner");
        link(ui,"GLUE 5 for Android","Source code, changes and issues for this fork","REPOSITORY","https://github.com/luizhegner/glue5");

        section(ui,"ORIGINAL PROJECT");
        link(ui,"GLUE 5 · mempage4k and contributors","Original runtime and project this fork builds on","GITHUB","https://github.com/mempage4k/glue5");

        section(ui,"GAME AND LICENSE");
        text(ui,"Free software under the GNU General Public License, version 3 or later.");
        ui.add_space(8.0*scale);
        text(
            ui,
            "Grand Theft Auto V is © Rockstar Games and Take-Two Interactive. Glue 5 is an independent \
             project, neither affiliated with nor endorsed by them. Your own game files are required.",
        );
        section(ui,"OPEN SOURCE");
        for &(name, what, license, url) in PROJECTS {
            link(ui,name,&format!("{what} · {license}"),"WEBSITE",url);
        }
        text(
            ui,
            "With the libraries these use, each under its own license (MIT, Apache-2.0, BSD, Zlib, \
             Unicode, …). GTA Online's players and friends find each other through the BitTorrent DHT, \
             kept by the people who run it.",
        );
    })
}

fn section(ui:&mut egui::Ui,title:&str) {
    let scale=theme::density(ui);
    ui.add_space(16.0*scale);
    ui.label(theme::label(title,12.0*scale).color(theme::palette::TEXT_MID));
    ui.add_space(8.0*scale);
}

fn link(ui:&mut egui::Ui,name:&str,description:&str,action:&str,url:&str) {
    ui.push_id((name,url),|ui| {
        if row(ui,name,description,|ui|action_value(ui,action)).on_hover_text(url).clicked() {
            ui.ctx().open_url(egui::OpenUrl::new_tab(url));
        }
    });
}
