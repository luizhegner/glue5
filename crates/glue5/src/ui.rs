//! The application's own UI, drawn with egui over the game (before the game
//! runs, alone): the start page, the settings, the credits, and on touch
//! screens the settings button and the touch controls. The game's menus
//! stay the game's.
//!
//! One responsive layout, fitted to the safe area. The same widgets
//! everywhere, all reachable from a controller or a keyboard: the D-pad,
//! left stick or arrows move the focus, A / Space / Enter activates, B /
//! Esc goes back.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use egui::{
    Align2, Color32, FontId, Id, LayerId, Order, Pos2, Rect, SafeAreaInsets, Vec2, ViewportId, pos2, vec2,
};
use sdl3::event::Event;
use sdl3::keyboard::Scancode;
use sdl3::mouse::MouseButton;
use sdl3::video::Window;

use crate::settings::{PerfOverlay, Settings};

mod chip;
mod credits;
mod friends;
mod mods;
mod pages;
mod perf;
mod servers;
mod theme;
mod widgets;
use credits::credits_page;
use friends::{FriendsPage, friends_page};
use mods::{ModsPage, mods_page};
pub use mods::StudioInfo;
use pages::{settings_page, start_page};
use servers::{ServersPage, servers_page};

use crate::touch::Touch;

/// A page of the application's.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Page {
    /// Before the game: its files, the ways into it, Settings, Credits.
    Start,
    Settings,
    /// Who made what the application is built on, and their licenses.
    Credits,
    /// The player's friend code, requests, friends.
    Friends,
    /// Where the game's files stream from: the server in use, the ones on
    /// the network, one by its address, this device's copy.
    Servers,
    /// The mods, GLUE 5 Studio, the mods' log.
    Mods,
}

/// How the game starts.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    Story,
    /// The whole map, free roam, no missions.
    Sandbox,
    /// GTA Online's freemode, with the lobby's other players.
    Online,
    /// The game's fly-through of the map, its results on the start page.
    Benchmark,
}

/// What the pages ask the application to do.
pub enum Action {
    Play(Mode),
    Quit,
    /// The system's folder picker, for the game (desktop).
    ChooseGame,
    /// The game's files, on a phone: Android's folder picker for a folder of
    /// the player's own, read where it is (nothing is copied); elsewhere, and
    /// if the picker will not open, look again at the known places (the files
    /// may have been copied in meanwhile).
    FindGame,
    /// Android's document picker: a GTA V profile folder.
    ImportSave,
    /// Android's document picker: a folder whose contents mirror the game
    /// folder (for example an omitted audio pack).
    ImportResources,
    /// The game's graphics settings back to the engine's defaults.
    ResetGraphics,
    /// Join a friend (by its key) playing GTA Online in `lobby`, from the
    /// start page.
    JoinFriend { key: String, lobby: String },
    /// Back to the game from the friends (a join asked for).
    CloseFriends,
    /// Delete the stream's copy of the game.
    ClearCopy,
    /// Look for servers on the network.
    FindServers,
    /// Stream from this server (an address found or typed).
    UseServer(String),
}

/// What a text field of the app's takes: the system keyboard follows it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum TextKind {
    #[default]
    Text,
    /// A friend code: letters and digits, in capitals.
    Code,
    /// A server's address or name.
    Address,
    /// The player's name.
    Name,
}

/// Where the focused text field keeps its [`TextKind`] (egui's memory).
const TEXT_KIND: &str = "text kind";

/// The system clipboard's text (a paste asked for by a tap).
fn clipboard_text() -> Option<String> {
    unsafe {
        let text = sdl3::sys::clipboard::SDL_GetClipboardText();
        if text.is_null() {
            return None;
        }
        let owned = std::ffi::CStr::from_ptr(text).to_string_lossy().into_owned();
        sdl3::sys::stdinc::SDL_free(text.cast());
        Some(owned).filter(|t| !t.is_empty())
    }
}

/// What the stream says over the game.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StreamAlert {
    /// The engine's reads have waited for the server for a moment.
    Waiting,
    /// The server does not answer (the game plays on from the copy).
    Offline,
}

/// A server found on the network, and what it answered when asked whether
/// it serves this game (nothing yet while it is asked).
pub type FoundServer = (glue5_stream::discovery::Found, Option<glue5_stream::Check>);

/// Navigation from a controller.
#[derive(Clone, Copy, Debug)]
pub enum Nav {
    Up,
    Down,
    Left,
    Right,
    Activate,
    Back,
}

/// What the running game started with: settings that differ apply when it
/// starts again.
pub struct Started {
    pub game_dir: PathBuf,
    pub source: crate::settings::Source,
    pub max_fps: u32,
    pub low_memory: bool,
    pub mods: bool,
}

/// What the pages show about the application.
pub struct Status<'a> {
    /// The game's folder, if found.
    pub game_dir: Option<&'a Path>,
    /// Why it was not (or why it would not start).
    pub problem: Option<&'a str>,
    /// The game runs (and started with these).
    pub started: Option<&'a Started>,
    /// Why the last game stopped, if it did.
    pub last_failure: Option<&'a str>,
    /// The game's graphics settings went back to their defaults.
    pub graphics_reset: bool,
    /// The window, in pixels.
    pub window: (u32, u32),
    /// The game's back buffer, while it runs.
    pub back_buffer: Option<(u32, u32)>,
    /// The upscalers this GPU has.
    pub upscalers: &'a [crate::settings::Upscaling],
    /// This GPU can generate frames (with MetalFX Temporal).
    pub frame_generation: bool,
    /// Apple's Metal HUD can show (it was on at launch).
    #[cfg_attr(not(target_vendor = "apple"), allow(dead_code))]
    pub metal_hud: bool,
    /// The display's refresh rate at most, in Hz.
    pub refresh_rate: f32,
    /// Play waits for the game's pipelines: how far they are (0 to 1).
    pub preparing: Option<f32>,
    /// The mode Play starts (or the last one played).
    pub mode: Mode,
    /// The last benchmark's results.
    pub benchmark: Option<&'a crate::benchmark::Results>,
    /// The game running is GTA Online.
    pub online: bool,
    /// Friend requests and game invites waiting for an answer.
    pub friend_requests: usize,
    /// Where the game's files come from (the setting).
    pub source: crate::settings::Source,
    /// The stream from a server, if one is set: its address and state.
    pub stream: Option<(&'a str, &'a glue5_stream::Status)>,
    /// This computer sharing its game folder (desktop).
    pub sharing: Option<crate::sharing::State>,
    /// The game runs with mods.
    pub mods: bool,
    /// A mod has a menu (a button for them beside the settings button).
    pub mod_menus: bool,
    /// GLUE 5 Studio, serving.
    pub studio: Option<&'a StudioInfo>,
    /// Servers found on the network, and whether a search runs.
    pub servers: &'a [FoundServer],
    pub searching: bool,
    /// A sign over the game: it waits for the server, or the server is away.
    pub stream_alert: Option<StreamAlert>,
}

/// The settings button over the game, sized like the touch controls: at the
/// top's middle, beside their camera view and pause buttons.
pub const GEAR_RADIUS: f32 = 20.0;

/// Where the settings button is, in the safe area `safe`, at the touch
/// controls' scale.
pub fn gear_center(safe: Rect, scale: f32) -> Pos2 {
    pos2(safe.center().x + 54.0 * scale, safe.min.y + 30.0 * scale)
}

const WARN: Color32 = Color32::from_rgb(240, 180, 100);

#[derive(Clone, Copy, Default, PartialEq, Eq)]
enum SettingsTab {
    #[default]
    Video,
    Controls,
    Game,
}

pub struct Ui {
    ctx: egui::Context,
    background: Option<egui::TextureHandle>,
    tab: SettingsTab,
    overlay: glue5_gpu::Overlay,
    events: Vec<egui::Event>,
    pointer: Pos2,
    start: Instant,
    page: Option<Page>,
    /// Where the settings go back to: the start page before the game.
    home: Option<Page>,
    /// Where the credits, the friends and the servers go back to: the page
    /// they were opened from.
    behind: Option<Page>,
    friends: FriendsPage,
    servers: ServersPage,
    mods: ModsPage,
    /// The last submission drew something (an empty one clears it).
    drawn: bool,
    note: Option<String>,
    /// The render scale while its slider is dragged (it applies when let go).
    scale_edit: Option<f32>,
    /// A mode was chosen: this frame shows that it starts, then it does.
    starting: Option<Mode>,
    /// GTA Online's card on the start page is open (its lobby and name).
    online_open: bool,
    /// The settings button as last drawn (centre, radius), in points.
    gear: Option<(Pos2, f32)>,
    /// The mods' menus button, likewise.
    menus_button: Option<(Pos2, f32)>,
    /// When egui wants to draw again (before the game, frames are drawn
    /// only for input or for this).
    repaint_at: Option<Instant>,
    perf: perf::PerfHud,
    /// Notifications over everything (Social Club's overlay showed them),
    /// the oldest first, and where they were last drawn (to tap them).
    toasts: std::collections::VecDeque<Toast>,
    toast_area: Option<Rect>,
    /// The touch controls as last drawn: their taps are the game's, even
    /// under a notification.
    controls: Vec<Rect>,
    /// The stream's sign over the game.
    chip: chip::Chip,
    /// Where the focused text field was last drawn (the system keyboard
    /// keeps it in view).
    text_rect: Option<Rect>,
    /// The settings button's icon.
    gear_icon: Option<egui::TextureHandle>,
}

/// A notification: as Social Club's, a title, a name and a line; the
/// friends' tab it opens; until when it shows.
struct Toast {
    toast: friends::Toast,
    /// A mod's: tapped, it opens the mods' menus (else the friends).
    from_mod: bool,
    since: Instant,
    until: Instant,
}

/// How long a notification shows (each one more after the ones before),
/// how many at most, how wide and high, how long it fades.
const TOAST_FOR: Duration = Duration::from_millis(3500);
const TOASTS: usize = 3;
const TOAST_WIDTH: f32 = 300.0;
/// A notification's height at most (title, name, line; avatar beside).
const TOAST_HEIGHT: f32 = 72.0;
const TOAST_FADE: f32 = 0.35;

impl Ui {
    pub fn new(overlay: glue5_gpu::Overlay) -> Self {
        let ctx = egui::Context::default();
        theme::install(&ctx);
        let background = theme::background(&ctx);
        let gear_icon = crate::touch::load_icon(&ctx, "settings");
        Ui {
            ctx,
            background,
            tab: SettingsTab::default(),
            overlay,
            events: Vec::new(),
            pointer: Pos2::ZERO,
            start: Instant::now(),
            page: Some(Page::Start),
            home: Some(Page::Start),
            behind: Some(Page::Start),
            friends: FriendsPage::default(),
            servers: ServersPage::default(),
            mods: ModsPage::default(),
            drawn: false,
            note: None,
            scale_edit: None,
            starting: None,
            online_open: false,
            gear: None,
            repaint_at: Some(Instant::now()),
            perf: Default::default(),
            toasts: Default::default(),
            toast_area: None,
            controls: Vec::new(),
            chip: Default::default(),
            menus_button: None,
            text_rect: None,
            gear_icon,
        }
    }

    pub fn page(&self) -> Option<Page> {
        self.page
    }

    pub fn is_open(&self) -> bool {
        self.page.is_some()
    }

    /// The game runs: no start page any more.
    pub fn game_started(&mut self) {
        self.home = None;
        self.starting = None;
        self.show(None);
    }

    /// A friends' notification, over the game or the pages.
    pub fn notify(&mut self, notice: &glue5_runtime::friends::Notice) {
        let now = Instant::now();
        while self.toasts.len() >= TOASTS {
            self.toasts.pop_front();
        }
        // Each leaves its time after the one before it: they go one at a time.
        let after = self.toasts.back().map_or(now, |t| t.until.max(now));
        let toast = friends::toast(notice);
        self.toasts.push_back(Toast { toast, from_mod: false, since: now, until: after + TOAST_FOR });
        self.repaint();
    }

    /// A mod's notification (`ui.notify`).
    pub fn notify_mod(&mut self, module: &str, text: &str) {
        let now = Instant::now();
        while self.toasts.len() >= TOASTS {
            self.toasts.pop_front();
        }
        let after = self.toasts.back().map_or(now, |t| t.until.max(now));
        let toast = friends::Toast { title: "Mod", name: module.to_owned(), text: text.to_owned(), tab: Default::default() };
        self.toasts.push_back(Toast { toast, from_mod: true, since: now, until: after + TOAST_FOR });
        self.repaint();
    }

    /// The mods' menus (F7); again, back.
    pub fn toggle_mod_menus(&mut self) {
        if self.page == Some(Page::Mods) {
            self.back();
        } else {
            self.mods.show_menus();
            self.show(Some(Page::Mods));
        }
    }

    /// The friends (the Home key, Back + RB); again, back.
    pub fn toggle_friends(&mut self) {
        if self.page == Some(Page::Friends) {
            self.back();
        } else {
            self.show(Some(Page::Friends));
        }
    }

    /// The friends at the tab of the newest notification (one was tapped).
    pub fn open_toast(&mut self) {
        let from_mod = self.toasts.back().is_some_and(|t| t.from_mod);
        if let Some(t) = self.toasts.back() {
            self.friends.tab = t.toast.tab;
        }
        self.toasts.clear();
        if from_mod {
            self.mods.show_menus();
            self.show(Some(Page::Mods));
        } else {
            self.show(Some(Page::Friends));
        }
    }

    /// Whether a finger at `x`, `y` (0..1, as SDL gives them) is on a
    /// notification.
    pub fn toast_hit(&self, x: f32, y: f32, window: &Window) -> bool {
        let size = screen_size(window);
        let p = pos2(x * size.x, y * size.y);
        self.toast_area.is_some_and(|r| r.contains(p)) && !self.controls.iter().any(|r| r.contains(p))
    }

    /// The settings (from the game or the start page); again, back.
    pub fn toggle_settings(&mut self) {
        if self.page == Some(Page::Settings) {
            self.back();
        } else {
            self.show(Some(Page::Settings));
        }
    }

    /// From the settings back to the start page, or the game; from the
    /// credits back to where they were opened; on the start page, GTA
    /// Online's card closes.
    pub fn back(&mut self) {
        match self.page {
            Some(Page::Start) if self.online_open => {
                self.online_open = false;
                self.repaint();
            }
            Some(Page::Settings) => self.show(self.home),
            Some(Page::Credits | Page::Friends | Page::Servers | Page::Mods) => self.show(self.behind),
            _ => {}
        }
    }

    fn show(&mut self, page: Option<Page>) {
        let secondary = |p: Option<Page>| matches!(p, Some(Page::Credits | Page::Friends | Page::Servers | Page::Mods));
        if secondary(page) && self.page != page {
            // One opened from another goes back where the first came from
            // (else back would go between the two for ever).
            if !secondary(self.page) {
                self.behind = self.page;
            }
            if page == Some(Page::Servers) {
                self.servers.opened();
            }
        }
        self.page = page;
        self.note = None;
        self.scale_edit = None;
        self.ctx.memory_mut(|m| {
            if let Some(id) = m.focused() {
                m.surrender_focus(id);
            }
        });
        self.repaint();
    }

    /// A message shown in the settings (e.g. "applies at the next start").
    pub fn note(&mut self, text: impl Into<String>) {
        self.note = Some(text.into());
        self.repaint();
    }

    /// Draw again at the next frame (the window changed, ...).
    pub fn repaint(&mut self) {
        self.repaint_in(Duration::ZERO);
    }

    pub fn repaint_in(&mut self, delay: Duration) {
        let at = Instant::now() + delay;
        self.repaint_at = Some(self.repaint_at.map_or(at, |t| t.min(at)));
    }

    /// Before the game: whether a frame should be drawn now, and how long
    /// the application can wait for input otherwise.
    pub fn wants_frame(&self) -> bool {
        !self.events.is_empty() || self.repaint_at.is_some_and(|t| Instant::now() >= t)
    }

    /// A text field of the app's has the focus: text input on (the system
    /// keyboard on touch screens).
    pub fn wants_text(&self) -> bool {
        self.page.is_some() && self.ctx.egui_wants_keyboard_input()
    }

    /// What the focused text field takes, and where it is (in points).
    pub fn text_field(&self) -> Option<(TextKind, Rect)> {
        let kind = self.ctx.data(|d| d.get_temp::<TextKind>(Id::new(TEXT_KIND))).unwrap_or_default();
        self.text_rect.filter(|_| self.wants_text()).map(|rect| (kind, rect))
    }

    pub fn idle_for(&self) -> Duration {
        let most = Duration::from_millis(250);
        self.repaint_at.map_or(most, |t| t.saturating_duration_since(Instant::now()).min(most))
    }

    /// Whether a finger at `x`, `y` (0..1, as SDL gives them) is on the
    /// settings button.
    pub fn gear_hit(&self, x: f32, y: f32, window: &Window) -> bool {
        let size = screen_size(window);
        self.gear.is_some_and(|(c, r)| c.distance(pos2(x * size.x, y * size.y)) <= r * 1.4)
    }

    /// Whether a finger at `x`, `y` (0..1) is on the mods' menus button.
    pub fn menus_hit(&self, x: f32, y: f32, window: &Window) -> bool {
        let size = screen_size(window);
        self.menus_button.is_some_and(|(c, r)| c.distance(pos2(x * size.x, y * size.y)) <= r * 1.4)
    }

    /// Whether a finger at `x`, `y` (0..1) is on the stream's sign.
    pub fn chip_hit(&self, x: f32, y: f32, window: &Window) -> bool {
        let size = screen_size(window);
        self.chip.hit(pos2(x * size.x, y * size.y))
    }

    /// The Servers page (from the stream's sign over the game).
    pub fn show_servers(&mut self) {
        self.show(Some(Page::Servers));
    }

    /// Input for a page while one is open.
    pub fn event(&mut self, event: &Event, window: &Window) {
        let size = screen_size(window);
        let scale = points_per_window_unit(window);
        let button = |b: &MouseButton| match b {
            MouseButton::Right => Some(egui::PointerButton::Secondary),
            MouseButton::Middle => Some(egui::PointerButton::Middle),
            MouseButton::Left => Some(egui::PointerButton::Primary),
            _ => None,
        };
        let modifiers = egui::Modifiers::default();
        match event {
            Event::MouseMotion { x, y, .. } => self.pointer_to(pos2(*x, *y) * scale),
            Event::MouseButtonDown { mouse_btn, x, y, .. } | Event::MouseButtonUp { mouse_btn, x, y, .. } => {
                let pressed = matches!(event, Event::MouseButtonDown { .. });
                if let Some(button) = button(mouse_btn) {
                    self.pointer_to(pos2(*x, *y) * scale);
                    self.events.push(egui::Event::PointerButton { pos: self.pointer, button, pressed, modifiers });
                }
            }
            Event::MouseWheel { x, y, .. } => self.events.push(egui::Event::MouseWheel {
                unit: egui::MouseWheelUnit::Line,
                delta: vec2(*x, *y),
                modifiers,
                phase: egui::TouchPhase::Move,
            }),
            // A finger is a touch (egui scrolls by dragging only on a touch
            // screen) and the pointer.
            // A finger the system took (an edge gesture, a call) lets go too.
            Event::FingerDown { touch_id, finger_id, x, y, pressure, .. }
            | Event::FingerUp { touch_id, finger_id, x, y, pressure, .. }
            | Event::FingerCanceled { touch_id, finger_id, x, y, pressure, .. } => {
                let pressed = matches!(event, Event::FingerDown { .. });
                let phase = match event {
                    Event::FingerDown { .. } => egui::TouchPhase::Start,
                    Event::FingerUp { .. } => egui::TouchPhase::End,
                    _ => egui::TouchPhase::Cancel,
                };
                let at = pos2(x * size.x, y * size.y);
                self.touch(*touch_id, *finger_id, phase, at, *pressure);
                // A cancelled touch lets go away from where it was: egui
                // takes a release close to its press for a click.
                self.pointer_to(if phase == egui::TouchPhase::Cancel { pos2(-1.0e4, -1.0e4) } else { at });
                self.events.push(egui::Event::PointerButton {
                    pos: self.pointer,
                    button: egui::PointerButton::Primary,
                    pressed,
                    modifiers,
                });
                if !pressed {
                    self.events.push(egui::Event::PointerGone);
                }
            }
            Event::FingerMotion { touch_id, finger_id, x, y, pressure, .. } => {
                let p = pos2(x * size.x, y * size.y);
                self.touch(*touch_id, *finger_id, egui::TouchPhase::Move, p, *pressure);
                self.pointer_to(p);
            }
            Event::TextInput { text, .. } => self.events.push(egui::Event::Text(text.clone())),
            // Pasting and copying into the app's text fields.
            Event::KeyDown { scancode: Some(sc @ (Scancode::V | Scancode::C | Scancode::X)), keymod, .. }
                if keymod.intersects(sdl3::keyboard::Mod::LCTRLMOD | sdl3::keyboard::Mod::RCTRLMOD)
                    || keymod.intersects(sdl3::keyboard::Mod::LGUIMOD | sdl3::keyboard::Mod::RGUIMOD) =>
            {
                self.events.push(match sc {
                    Scancode::V => match window.subsystem().clipboard().clipboard_text() {
                        Ok(text) => egui::Event::Paste(text),
                        Err(_) => return,
                    },
                    Scancode::C => egui::Event::Copy,
                    _ => egui::Event::Cut,
                });
            }
            Event::KeyDown { scancode: Some(sc), keymod, .. } | Event::KeyUp { scancode: Some(sc), keymod, .. } => {
                let pressed = matches!(event, Event::KeyDown { .. });
                if let Some(key) = key(*sc) {
                    let shift = keymod.intersects(sdl3::keyboard::Mod::LSHIFTMOD | sdl3::keyboard::Mod::RSHIFTMOD);
                    self.key(key, pressed, egui::Modifiers { shift, ..Default::default() });
                }
            }
            _ => {}
        }
    }

    fn touch(&mut self, device: u64, finger: u64, phase: egui::TouchPhase, pos: Pos2, force: f32) {
        self.events.push(egui::Event::Touch {
            device_id: egui::TouchDeviceId(device),
            id: egui::TouchId(finger),
            phase,
            pos,
            force: Some(force),
        });
    }

    fn pointer_to(&mut self, p: Pos2) {
        self.pointer = p;
        self.events.push(egui::Event::PointerMoved(p));
    }

    fn key(&mut self, key: egui::Key, pressed: bool, modifiers: egui::Modifiers) {
        self.events.push(egui::Event::Key { key, physical_key: None, pressed, repeat: false, modifiers });
    }

    pub fn nav(&mut self, nav: Nav) {
        let key = match nav {
            Nav::Up => egui::Key::ArrowUp,
            Nav::Down => egui::Key::ArrowDown,
            Nav::Left => egui::Key::ArrowLeft,
            Nav::Right => egui::Key::ArrowRight,
            Nav::Activate => egui::Key::Enter,
            Nav::Back => return self.back(),
        };
        // Nothing has the focus yet: the first move gives it to the page's
        // first control (egui moves it from a focused widget only).
        let key = if self.ctx.memory(|m| m.focused()).is_none() { egui::Key::Tab } else { key };
        self.key(key, true, Default::default());
        self.key(key, false, Default::default());
    }

    /// Draws the open page, or over the game the touch controls (if given),
    /// the settings button (if given, at this opacity) and the performance
    /// overlay (if on).
    pub fn frame(
        &mut self,
        window: &Window,
        settings: &mut Settings,
        status: &Status,
        touch: Option<&Touch>,
        gear: Option<f32>,
    ) -> Vec<Action> {
        let mut actions = Vec::new();
        let gear = gear.filter(|_| self.page.is_none());
        let hud = status.started.is_some() && self.page.is_none() && settings.overlay == PerfOverlay::Glue5;
        if !hud {
            self.perf.hide();
        }
        let alert = status.stream_alert.filter(|_| self.page.is_none());
        let now = Instant::now();
        self.toasts.retain(|t| now < t.until);
        if self.toasts.is_empty() {
            self.toast_area = None;
        }
        if self.page.is_none()
            && touch.is_none()
            && gear.is_none()
            && alert.is_none()
            && self.toasts.is_empty()
            && (!hud || !self.perf.due(settings.overlay_detail))
        {
            if self.drawn && !hud {
                self.overlay.submit(Vec::new(), Default::default(), 1.0);
                self.drawn = false;
            }
            self.events.clear();
            self.gear = None;
            self.menus_button = None;
            self.chip = Default::default();
            return actions;
        }
        // Over the game, one pass per frame shown: the renderer draws only
        // the newest submission at each present, the loop turns faster (its
        // input stays queued for the next pass).
        if status.started.is_some() && self.overlay.waiting() {
            return actions;
        }
        // Play was chosen at the last frame: this one shows that the game
        // starts, then it does.
        let starting = std::mem::take(&mut self.starting);
        let ppp = pixels_per_point(window);
        let screen = Rect::from_min_size(Pos2::ZERO, screen_size(window));
        let safe = safe_rect(window);
        let mut raw = egui::RawInput {
            screen_rect: Some(screen),
            safe_area_insets: Some(SafeAreaInsets(egui::epaint::MarginF32 {
                left: safe.min.x - screen.min.x,
                right: screen.max.x - safe.max.x,
                top: safe.min.y - screen.min.y,
                bottom: screen.max.y - safe.max.y,
            })),
            time: Some(self.start.elapsed().as_secs_f64()),
            events: std::mem::take(&mut self.events),
            focused: true,
            ..Default::default()
        };
        raw.viewports.entry(ViewportId::ROOT).or_default().native_pixels_per_point = Some(ppp);

        let ctx = self.ctx.clone();
        // The page to show next, or back to the previous one.
        let mut go = None;
        let mut back = false;
        ctx.begin_pass(raw);
        if let Some(touch) = touch {
            touch.draw(&ctx.layer_painter(LayerId::background()), settings.touch_opacity);
        }
        let scale = crate::touch::control_scale(screen.size(), settings.touch_size);
        self.gear = gear.map(|alpha| draw_gear(&ctx, safe, scale, alpha, self.gear_icon.as_ref()));
        self.menus_button = gear.filter(|_| status.mod_menus).map(|alpha| draw_menus_button(&ctx, safe, scale, alpha));
        let overlay = hud.then(|| self.perf.draw(&ctx, safe, settings, status));
        // The notifications and the stream's sign keep off everything else
        // drawn over the game.
        self.controls = touch.map_or_else(Vec::new, Touch::occupied);
        let mut taken = self.controls.clone();
        taken.extend(self.gear.into_iter().chain(self.menus_button).map(|(c, r)| Rect::from_center_size(c, Vec2::splat(2.0 * r))));
        taken.extend(overlay);
        // Under the settings button when it shows.
        // Under the top's buttons (the touch controls' camera view and
        // pause, the settings), where they show.
        let below = if self.gear.is_some() { (30.0 + GEAR_RADIUS) * scale + 8.0 } else { 12.0 };
        self.toast_area = draw_toasts(&ctx, safe, below, &taken, &self.toasts, now);
        if !self.toasts.is_empty() {
            // Through egui: its delay decides the next frame after this one.
            ctx.request_repaint_after(Duration::from_millis(33));
        }
        // The sign, legible whatever the touch controls' opacity.
        taken.extend(self.toast_area);
        let alpha = gear.unwrap_or(1.0).max(0.75);
        self.chip.draw(&ctx, safe, alert, self.gear, &taken, scale, alpha);
        if self.page.is_some() && status.started.is_none() {
            theme::backdrop(&ctx, self.background.as_ref());
        }
        match self.page {
            Some(Page::Start) => {
                let open = &mut self.online_open;
                self.starting = start_page(&ctx, settings, status, starting, open, &mut actions, &mut go);
            }
            Some(Page::Settings) => {
                let note = self.note.as_deref();
                let (tab, scale_edit) = (&mut self.tab, &mut self.scale_edit);
                back = settings_page(&ctx, settings, status, note, tab, scale_edit, &mut actions, &mut go);
            }
            Some(Page::Credits) => back = credits_page(&ctx),
            Some(Page::Friends) => {
                let game = friends::Context { started: status.started.is_some(), online: status.online };
                back = friends_page(&ctx, &mut self.friends, game, &mut actions);
            }
            Some(Page::Servers) => back = servers_page(&ctx, &mut self.servers, settings, status, &mut actions),
            Some(Page::Mods) => back = mods_page(&ctx, &mut self.mods, settings, status),
            None => {}
        }
        let mut output = ctx.end_pass();
        self.text_rect = output.platform_output.ime.as_ref().map(|ime| ime.rect);
        for command in std::mem::take(&mut output.platform_output.commands) {
            match command {
                egui::OutputCommand::OpenUrl(open) => {
                    if let Err(e) = sdl3::url::open_url(&open.url) {
                        log::warn!("cannot open {}: {e}", open.url);
                    }
                }
                egui::OutputCommand::CopyText(text) => {
                    let _ = window.subsystem().clipboard().set_clipboard_text(&text);
                }
                _ => {}
            }
        }
        let delay = output.viewport_output.get(&ViewportId::ROOT).map_or(Duration::MAX, |v| v.repaint_delay);
        self.repaint_at = (delay < Duration::from_secs(10)).then(|| Instant::now() + delay);
        let primitives = ctx.tessellate(output.shapes, output.pixels_per_point);
        self.overlay.submit(primitives, output.textures_delta, output.pixels_per_point);
        self.drawn = true;
        if back {
            self.back();
        } else if let Some(page) = go {
            self.show(page);
        }
        if self.starting.is_some() {
            self.repaint();
        } else if status.preparing.is_some() {
            self.repaint_in(Duration::from_millis(200));
        }
        if let Some(mode) = starting {
            actions.push(Action::Play(mode));
        }
        actions
    }
}

/// The notifications, at the safe area's top right (`below` its top), the
/// newest at the bottom, fading in and out: where they are, if any.
fn draw_toasts(
    ctx: &egui::Context,
    safe: Rect,
    below: f32,
    taken: &[Rect],
    toasts: &std::collections::VecDeque<Toast>,
    now: Instant,
) -> Option<Rect> {
    if toasts.is_empty() {
        return None;
    }
    let scale = theme::layout_scale(safe);
    let width = (TOAST_WIDTH * scale).min(safe.width() - 32.0);
    // All of them where they fit, else the newest alone; nowhere free (the
    // touch controls fill the top): none over the game, the page has them.
    let (at, shown) = [toasts.len(), 1].into_iter().find_map(|n| {
        let height = n as f32 * TOAST_HEIGHT * scale + (n - 1) as f32 * 8.0 * scale;
        toast_place(safe, below, vec2(width, height), taken).map(|at| (at, n))
    })?;
    let response = egui::Area::new(Id::new("toasts"))
        .order(Order::Tooltip)
        .fixed_pos(at)
        .interactable(false)
        .show(ctx, |ui| {
            ui.set_width(width);
            ui.spacing_mut().item_spacing.y = 8.0 * scale;
            for t in toasts.iter().skip(toasts.len() - shown) {
                let shown = now.duration_since(t.since).as_secs_f32();
                let left = t.until.saturating_duration_since(now).as_secs_f32();
                ui.scope(|ui| {
                    ui.multiply_opacity((shown.min(left) / TOAST_FADE).clamp(0.0, 1.0));
                    friends::draw_toast(ui, &t.toast, width);
                });
            }
        });
    Some(response.response.rect)
}

/// Where the notifications go (their top left), the first place where they
/// cover nothing else on screen (`taken`): in the top's middle under its
/// buttons (the touch controls' camera view and pause, the settings; the
/// game shows nothing there), in the top right or left corner, else lower
/// in those three places, down to the middle of the screen at most.
fn toast_place(safe: Rect, below: f32, size: Vec2, taken: &[Rect]) -> Option<Pos2> {
    let starts = [
        pos2(safe.center().x - size.x / 2.0, safe.top() + below),
        pos2(safe.right() - size.x - 16.0, safe.top() + 12.0),
        pos2(safe.left() + 16.0, safe.top() + 12.0),
    ];
    let lowest = safe.center().y - size.y / 2.0;
    let free = |at: Pos2| {
        let rect = Rect::from_min_size(at, size);
        safe.contains_rect(rect) && !taken.iter().any(|t| t.intersects(rect.expand(4.0)))
    };
    (0..)
        .map(|step| step as f32 * 8.0)
        .take_while(|down| starts.iter().any(|s| s.y + down <= lowest))
        .flat_map(|down| starts.map(|s| s + vec2(0.0, down)))
        .find(|&at| at.y <= lowest && free(at))
}

// Points: the UI's unit, the same physical size everywhere. As SDL's
// README-highdpi describes, a point is the window's display scale in pixels;
// SDL's window coordinates are points on Apple platforms but pixels on
// Android and Windows (where the display's content scale enlarges the UI).

/// Pixels per point.
fn pixels_per_point(window: &Window) -> f32 {
    window.display_scale().max(0.5)
}

/// Points per unit of SDL's window coordinates (event and safe area
/// positions).
fn points_per_window_unit(window: &Window) -> f32 {
    window.pixel_density() / pixels_per_point(window)
}

/// The window's size in points.
pub fn screen_size(window: &Window) -> Vec2 {
    let (w, h) = window.size_in_pixels();
    vec2(w as f32, h as f32) / pixels_per_point(window)
}

/// A rectangle in points, in SDL's window coordinates.
pub fn window_rect(window: &Window, r: Rect) -> sdl3::rect::Rect {
    let r = Rect::from_min_max(r.min / points_per_window_unit(window), r.max / points_per_window_unit(window));
    sdl3::rect::Rect::new(r.min.x as i32, r.min.y as i32, r.width().max(1.0) as u32, r.height().max(1.0) as u32)
}

/// The part of the window clear of notches and rounded corners, in points.
pub fn safe_rect(window: &Window) -> Rect {
    match window.safe_area() {
        Ok(r) if r.width() > 0 && r.height() > 0 => {
            let scale = points_per_window_unit(window);
            Rect::from_min_size(
                pos2(r.x() as f32, r.y() as f32) * scale,
                vec2(r.width() as f32, r.height() as f32) * scale,
            )
        }
        _ => Rect::from_min_size(Pos2::ZERO, screen_size(window)),
    }
}

/// The settings button: the touch controls' look, at their size.
fn draw_gear(ctx: &egui::Context, safe: Rect, scale: f32, alpha: f32, icon: Option<&egui::TextureHandle>) -> (Pos2, f32) {
    let c = gear_center(safe, scale);
    let r = GEAR_RADIUS * scale;
    let look = crate::touch::Look::new(alpha, scale, glue5_runtime::input::Character::Other);
    let painter = ctx.layer_painter(LayerId::new(Order::Foreground, Id::new("gear")));
    painter.circle(c, r, look.fill(false), look.stroke(false));
    match icon {
        Some(icon) => {
            let uv = Rect::from_min_max(Pos2::ZERO, pos2(1.0, 1.0));
            painter.image(icon.id(), Rect::from_center_size(c, Vec2::splat(r * 1.15)), uv, look.icon);
        }
        None => {
            painter.text(c, Align2::CENTER_CENTER, "⚙", FontId::proportional(r * 0.95), look.icon);
        }
    }
    (c, r)
}

/// The mods' menus button, the settings button's right neighbour: sliders.
fn draw_menus_button(ctx: &egui::Context, safe: Rect, scale: f32, alpha: f32) -> (Pos2, f32) {
    let c = gear_center(safe, scale) + vec2(54.0 * scale, 0.0);
    let r = GEAR_RADIUS * scale;
    let look = crate::touch::Look::new(alpha, scale, glue5_runtime::input::Character::Other);
    let painter = ctx.layer_painter(LayerId::new(Order::Foreground, Id::new("gear")));
    painter.circle(c, r, look.fill(false), look.stroke(false));
    let stroke = egui::Stroke::new(r * 0.1, look.icon);
    for (dy, knob) in [(-0.24, 0.18), (0.24, -0.2)] {
        let y = c.y + dy * r;
        painter.line_segment([pos2(c.x - 0.48 * r, y), pos2(c.x + 0.48 * r, y)], stroke);
        painter.circle_filled(pos2(c.x + knob * r, y), r * 0.15, look.icon);
    }
    (c, r)
}

fn key(sc: Scancode) -> Option<egui::Key> {
    Some(match sc {
        Scancode::Tab => egui::Key::Tab,
        Scancode::Return | Scancode::KpEnter => egui::Key::Enter,
        Scancode::Space => egui::Key::Space,
        Scancode::Backspace => egui::Key::Backspace,
        Scancode::Delete => egui::Key::Delete,
        Scancode::Home => egui::Key::Home,
        Scancode::End => egui::Key::End,
        Scancode::Up => egui::Key::ArrowUp,
        Scancode::Down => egui::Key::ArrowDown,
        Scancode::Left => egui::Key::ArrowLeft,
        Scancode::Right => egui::Key::ArrowRight,
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::touch::Touch;
    use glue5_runtime::input::{Activity, Character, Driving, PlayerContext};

    /// Every touch layout, at every controls' size, on phones and a tablet:
    /// the notifications, where they show, cover no control and are inside
    /// the safe area.
    #[test]
    fn notifications_cover_no_control() {
        let activities = [
            Activity::OnFoot,
            Activity::Driving(Driving::Car),
            Activity::Driving(Driving::Heli),
            Activity::Driving(Driving::Plane),
            Activity::Passenger,
            Activity::Parachute,
        ];
        let screens = [
            (vec2(874.0, 402.0), [62.0, 0.0, 62.0, 21.0]),
            (vec2(844.0, 390.0), [47.0, 0.0, 47.0, 21.0]),
            (vec2(1194.0, 834.0), [0.0, 24.0, 0.0, 20.0]),
        ];
        let (mut placed, mut tried) = (0, 0);
        for (screen, [l, t, r, b]) in screens {
            let safe = Rect::from_min_max(pos2(l, t), pos2(screen.x - r, screen.y - b));
            for scale in [0.7, 1.0, 1.5] {
                let controls = crate::touch::control_scale(screen, scale);
                let gear = Rect::from_center_size(gear_center(safe, controls), Vec2::splat(2.0 * GEAR_RADIUS * controls));
                let below = (30.0 + GEAR_RADIUS) * controls + 8.0;
                for activity in activities {
                    for (vehicle_near, phone) in [(false, false), (true, false), (false, true)] {
                        let context = PlayerContext {
                            activity,
                            vehicle_near,
                            phone,
                            menu: false,
                            cutscene: false,
                            bar: false,
                            scripted: false,
                            character: Character::Other,
                            gun: false,
                            cursor: false,
                            map: false,
                            pointer: false,
                            alert: false,
                            hidden: false,
                        };
                        let mut touch = Touch::new();
                        touch.layout(screen, safe, scale);
                        touch.set_context(context);
                        let mut taken = touch.occupied();
                        taken.push(gear);
                        let width = (TOAST_WIDTH * theme::layout_scale(safe)).min(safe.width() - 32.0);
                        let size = vec2(width, TOAST_HEIGHT * theme::layout_scale(safe));
                        tried += 1;
                        let Some(at) = toast_place(safe, below, size, &taken) else { continue };
                        placed += 1;
                        let rect = Rect::from_min_size(at, size);
                        assert!(safe.contains_rect(rect), "{screen:?} {scale} {context:?}: {rect:?}");
                        for t in &taken {
                            assert!(!t.intersects(rect), "{screen:?} {scale} {context:?}: {rect:?} on {t:?}");
                        }
                    }
                }
            }
        }
        // They show almost always.
        assert!(placed * 10 >= tried * 9, "shown {placed} of {tried} times");
    }
}

