//! Test the actual production widgets without SDL, wgpu or generated game C.
//! Only platform services and menu data below are fixtures, never UI copies.
#![allow(dead_code)]

mod paths {
    pub const TOUCH: bool = cfg!(feature = "touch");
    pub const MOBILE: bool = TOUCH;
    pub const TV: bool = false;
    pub fn user_dir() -> std::io::Result<std::path::PathBuf> {
        Err(std::io::ErrorKind::NotFound.into())
    }
    pub fn fonts_dir(path: &std::path::Path) -> std::path::PathBuf { path.join("fonts") }
}
mod version { pub const LABEL: &str = "0.1.0 · cedar"; }
mod sfx {
    pub enum Sound { Focus, Adjust, Confirm, Tab, Launch, Error, ToggleOn, ToggleOff }
    pub fn want(_: Sound) {}
}
mod settings {
    #[derive(Clone, Copy, PartialEq, Default)]
    pub enum Source { #[default] Folder, Server }
    #[derive(Default)]
    pub struct Settings { pub online: bool, pub sandbox: bool, pub online_name: String, pub source: Source }
}

#[path = "ui.rs"]
mod ui;
mod raster;
#[path="../../../crates/glue5/src/saves.rs"]
mod saves;

fn main() {
    ui::preview();
}
