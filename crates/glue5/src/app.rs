//! The application: window, renderer, engine and the event loop.
//!
//! It opens on the start page with the renderer already running: the
//! game's files, Play, Settings. What applies when the engine starts (frame
//! rate, memory profile, the game folder, a phone's resolution) can be
//! changed there before it loads, and a game that fails while starting is
//! not started again by itself. Play starts the engine, which then owns the
//! frames; the application's UI goes over them. `glue5 --play` (a restart
//! the game asks for) starts the game without the start page.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::{Result, anyhow};
use glue5_gpu::{Presentation, Screen, Upscaler};
use glue5_runtime::exit::Exit;
use glue5_runtime::input::{Gamepad, InputBlock, mouse};
use glue5_runtime::layout::is_game_dir;
use sdl3::event::{Event, WindowEvent};
use sdl3::keyboard::Scancode;
use sdl3::mouse::MouseButton;

use crate::haptics;
use crate::paths::{self, MOBILE, TOUCH, TV};
use crate::settings::{self, PerfOverlay, Settings, Source, TouchMode, Upscaling};
use crate::touch::{self, Touch};
use crate::ui::{self, Action, Mode, Page, Started, Status, Ui};
use crate::{audio, benchmark, keymap, pads};

/// The settings button's opacity over the game while the touch controls
/// are hidden (a controller in use, or touch off): there, but faint.
const GEAR_ALONE: f32 = 0.35;

/// In the user directory: why the last game stopped, for the start page
/// (gone once the game is started again).
const FAILURE_FILE: &str = "last-failure.txt";

/// How often the pages read the controllers, when the loop reads them.
const PAD_READ: Duration = Duration::from_millis(16);
/// How long the pointer waits to be captured once it should be (`App::pointer`).
const CAPTURE_DELAY: Duration = Duration::from_millis(150);

/// The menus' touch keys, as the keyboard's (by [`touch::MenuKey`]):
/// Backspace (back: the scripts' menus take it too, where Esc would open
/// the pause menu over them), the arrows (left and right change the value
/// of the row picked), Enter.
const MENU_VKS: [u8; 6] = [0x08, 0x25, 0x27, 0x26, 0x28, 0x0D];

pub fn run() -> Result<()> {
    // SDL's event wait on Android pumps events, which queues the poll
    // sentinel, which wakes the wait: it spins for its whole timeout (a big
    // core at 100%). Without the sentinel it sleeps until an event; polling
    // then drains the queue, which is what this loop does anyway.
    #[cfg(target_os = "android")]
    {
        sdl3::hint::set("SDL_POLL_SENTINEL", "0");
        // Deliver Android Back as SDL_SCANCODE_AC_BACK so the app can turn it
        // into Escape (back out of a page or open the in-game pause menu).
        sdl3::hint::set("SDL_ANDROID_TRAP_BACK_BUTTON", "1");
    }
    let sdl = sdl3::init().map_err(|e| anyhow!("SDL: {e}"))?;
    let video = sdl.video().map_err(|e| anyhow!("SDL video: {e}"))?;
    if MOBILE {
        sdl3::hint::set("SDL_ORIENTATIONS", "LandscapeLeft LandscapeRight");
        // Touches drive the touch controls, not a pretend mouse.
        sdl3::hint::set("SDL_TOUCH_MOUSE_EVENTS", "0");
    }
    // The renderer sizes the window's Metal layer when it configures it for
    // the window. SDL resizing the layer as well (macOS), from inside the
    // system's resize loop, had frames drawn for one size shown at another.
    sdl3::hint::set("SDL_VIDEO_METAL_AUTO_RESIZE_DRAWABLE", "0");
    let user_dir = paths::user_dir()?;
    let mut settings = Settings::load(&user_dir);
    if settings.online_name.trim().is_empty() {
        settings.online_name = settings::default_online_name();
    }
    // Friends see the player from the start page on.
    glue5_runtime::friends::start(&user_dir, settings.online_name.trim());
    // Before the GPU opens; turned on later, it shows from the next launch.
    let metal_hud = cfg!(target_vendor = "apple") && settings.overlay == PerfOverlay::MetalHud;
    if metal_hud {
        // SAFETY: Apple's libc locks the environment for getenv and setenv
        // (elsewhere this does nothing).
        unsafe { glue5_gpu::enable_metal_hud() };
    }
    let args = Args::parse();

    let mut builder = video.window("Glue 5", 1280, 720);
    builder.resizable().high_pixel_density();
    if MOBILE {
        builder.fullscreen();
    }
    // (tvOS: made below.)
    if cfg!(target_vendor = "apple") && !TV {
        builder.metal_view();
    }
    let window = builder.build()?;
    let pixels = window.size_in_pixels();
    settings.for_screen(pixels.1);

    let presentation = Presentation::new(pixels, settings.vsync, upscaler(&settings));
    presentation.set_metal_hud(settings.overlay == PerfOverlay::MetalHud);
    let overlay = glue5_gpu::Overlay::default();
    let instance = glue5_gpu::instance();
    #[cfg(target_os = "android")]
    let surfaces = instance.clone();
    // The window outlives the renderer: the process exits with it.
    #[cfg(not(target_os = "tvos"))]
    let surface = unsafe {
        instance.create_surface_unsafe(wgpu::SurfaceTargetUnsafe::from_display_and_window(&window, &window)?)?
    };
    // The sdl3 crate has no window handle for tvOS (UIKit only on iOS): the
    // surface takes the layer of a Metal view of SDL's (the process exits
    // with it too).
    #[cfg(target_os = "tvos")]
    let surface = unsafe {
        let view = sdl3::sys::metal::SDL_Metal_CreateView(window.raw());
        let layer = sdl3::sys::metal::SDL_Metal_GetLayer(view);
        anyhow::ensure!(!layer.is_null(), "no Metal layer for the window: {}", sdl3::get_error());
        instance.create_surface_unsafe(wgpu::SurfaceTargetUnsafe::CoreAnimationLayer(layer))?
    };
    let screen = Screen::open(glue5_gpu::Window {
        instance,
        surface,
        presentation: presentation.clone(),
        overlay: overlay.clone(),
    })?;
    // While the system resizes the window in a loop of its own (macOS,
    // Windows), events wait for it to end, and so does this loop; SDL still
    // calls watches, and from there the renderer follows the window. The
    // game's resolution changes once, when this loop runs again. SDL sends
    // the application's lifecycle only to watches too, from the call that
    // learns of it (not through the queue): the loop follows it from there.
    let watched = presentation.clone();
    let lifecycle = Arc::new(Lifecycle::default());
    let life = lifecycle.clone();
    let _watch = sdl.event().map_err(|e| anyhow!("SDL events: {e}"))?.add_event_watch(move |event: Event| match event {
        Event::Window { win_event: WindowEvent::PixelSizeChanged(w, h), .. } => {
            watched.set_size(w.max(0) as u32, h.max(0) as u32)
        }
        // iOS sends it also while the app is only inactive (Control Center,
        // a system alert): suspended is when it did go to the background.
        Event::AppWillEnterBackground { .. } => life.background.store(true, Ordering::Relaxed),
        Event::AppDidEnterBackground { .. } => {
            life.suspended.store(true, Ordering::Relaxed);
            // The system may reject or abandon frame work while the app is
            // backgrounded; do not interpret those GPU heartbeats as a hang.
            glue5_gpu::set_background(true);
            // Android lets the game run on unseen (iOS suspends it).
            #[cfg(target_os = "android")]
            glue5_runtime::frame_pace::set_background(true);
        }
        Event::AppDidEnterForeground { .. } => {
            life.background.store(false, Ordering::Relaxed);
            life.came_back.store(true, Ordering::Relaxed);
            glue5_gpu::set_background(false);
            #[cfg(target_os = "android")]
            glue5_runtime::frame_pace::set_background(false);
        }
        _ => {}
    });

    // Text input runs only while the game's text box is open (see `update`).
    let text_input = video.text_input();
    let refresh_rate = window_refresh_rate(&window);
    let mode = match () {
        _ if settings.online => Mode::Online,
        _ if settings.sandbox => Mode::Sandbox,
        _ => Mode::Story,
    };
    let last_benchmark = benchmark::last(&user_dir);
    let audio_subsystem = match sdl.audio() {
        Ok(audio) => Some(audio),
        Err(error) => {
            log::warn!("no audio: {error}");
            None
        }
    };
    let menu_music = audio_subsystem.as_ref().and_then(|audio| match audio::MenuMusic::start(audio) {
        Ok(music) => Some(music),
        Err(error) => {
            log::warn!("start-page music unavailable: {error:#}");
            None
        }
    });
    let mut app = App {
        menu_music,
        pads: pads::Pads::new(sdl.gamepad().map_err(|e| anyhow!("SDL gamepad: {e}"))?),
        mouse: sdl.mouse(),
        events: sdl.event_pump().map_err(|e| anyhow!("SDL events: {e}"))?,
        sdl,
        window,
        #[cfg(target_os = "android")]
        asked_rate: None,
        #[cfg(target_os = "ios")]
        thermal: (-1, None),
        text_input,
        typing: None,
        typing_rect: None,
        keyboard_away: false,
        presentation,
        #[cfg(target_os = "android")]
        instance: surfaces,
        user_dir: user_dir.clone(),
        settings,
        ui: Ui::new(overlay, &user_dir, mode),
        touch: Touch::new(),
        touch_input: TOUCH,
        touch_shown: false,
        touch_mouse: Default::default(),
        lifecycle,
        folder: None,
        game_dir: None,
        problem: None,
        stream: None,
        source: Source::Folder,
        stream_for: None,
        sharing: None,
        studio: None,
        studio_failed: None,
        servers: Arc::default(),
        stream_waiting: None,
        last_failure: None,
        graphics_reset: false,
        upscalers: UPSCALERS.iter().copied().filter(|u| screen.has_upscaler(u.upscaler())).collect(),
        frame_generation: screen.has_frame_generation(),
        metal_hud,
        refresh_rate,
        screen: Some(screen),
        play_requested: false,
        mode,
        benchmark: last_benchmark,
        join: None,
        friend_name: String::new(),
        capture_since: None,
        mouse_used: !MOBILE,
        game: None,
        _audio_subsystem: audio_subsystem,
    };
    // Saved settings may name an upscaler this GPU does not provide. Use the
    // first available one so the setting shown and the renderer stay aligned.
    if !app.upscalers.contains(&app.settings.upscaler)
        && let Some(upscaler) = app.upscalers.first()
    {
        app.settings.upscaler = *upscaler;
    }
    app.last_failure = std::fs::read_to_string(app.user_dir.join(FAILURE_FILE)).ok();
    app.find_game(args.game_dir.as_deref());
    if args.play {
        match app.settings.source {
            Source::Folder if app.game_dir.is_some() => app.play()?,
            Source::Folder => {}
            // Once the stream is ready (`update` asks again until then).
            Source::Server => app.play_requested = true,
        }
    }
    app.run()
}

/// The command line (desktop): `glue5 [--play] [game folder]`; the start
/// page is skipped only with `--play` (a folder alone is where the game is:
/// a game that fails is not started again by itself).
struct Args {
    /// Start the game without the start page.
    play: bool,
    game_dir: Option<PathBuf>,
}

impl Args {
    fn parse() -> Args {
        let mut args = Args { play: false, game_dir: None };
        // Phones pass arguments of their own.
        for arg in std::env::args_os().skip(1).filter(|_| !MOBILE) {
            if arg == "--play" {
                args.play = true;
            } else {
                args.game_dir = Some(arg.into());
            }
        }
        // Measurements on a phone, whose screen nobody taps (`mise run ios:run`
        // passes GLUE5_* variables on).
        args.play |= std::env::var_os("GLUE5_PLAY").is_some();
        args
    }
}

/// The application's lifecycle, as SDL tells the event watch.
#[derive(Default)]
struct Lifecycle {
    /// In the background (phones: no GPU work there).
    background: AtomicBool,
    /// Back in the foreground since the loop last looked.
    came_back: AtomicBool,
    /// Was in the background for real (not only inactive) before that.
    suspended: AtomicBool,
}

/// What is being typed into: a field of the app's, or the game's text box.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Typing {
    App(ui::TextKind),
    Game(glue5_runtime::input::TextType),
}

impl Typing {
    /// The system keyboard for it. Codes, names and addresses are not
    /// words: no corrections. The game's keypad is digits, but iPhones
    /// have no Return key on theirs: there, the keyboard's numbers.
    fn options(self) -> sdl3::keyboard::TextInputOptions {
        use glue5_runtime::input::TextType;
        use sdl3::keyboard::{Capitalization, TextInputOptions, TextInputType};
        let (input_type, capitalization, autocorrect) = match self {
            Typing::App(ui::TextKind::Text) => (TextInputType::Text, Capitalization::Sentences, true),
            Typing::App(ui::TextKind::Code) => (TextInputType::Text, Capitalization::Letters, false),
            Typing::App(ui::TextKind::Address) => (TextInputType::Text, Capitalization::None, false),
            Typing::App(ui::TextKind::Name) => (TextInputType::Text, Capitalization::Words, false),
            Typing::Game(TextType::Email) => (TextInputType::Email, Capitalization::None, false),
            Typing::Game(TextType::Password) => (TextInputType::PasswordHidden, Capitalization::None, false),
            Typing::Game(TextType::Numeric) if cfg!(not(target_os = "ios")) => {
                (TextInputType::Number, Capitalization::None, false)
            }
            Typing::Game(TextType::Numeric | TextType::Gamertag | TextType::Filename) => {
                (TextInputType::Text, Capitalization::None, false)
            }
            Typing::Game(TextType::Text | TextType::Alphabet) => (TextInputType::Text, Capitalization::Sentences, false),
        };
        TextInputOptions {
            input_type: Some(input_type),
            capitalization: Some(capitalization),
            autocorrect: Some(autocorrect),
            ..Default::default()
        }
    }
}

struct App {
    /// Kept before SDL in drop order so the mixer closes while audio is alive.
    menu_music: Option<audio::MenuMusic>,
    sdl: sdl3::Sdl,
    window: sdl3::video::Window,
    /// The display rate asked of Android for the window's surface.
    #[cfg(target_os = "android")]
    asked_rate: Option<u32>,
    /// The device's thermal state (iOS: 0 nominal to 3 critical) and when it
    /// was last asked.
    #[cfg(target_os = "ios")]
    thermal: (isize, Option<Instant>),
    events: sdl3::EventPump,
    mouse: sdl3::mouse::MouseUtil,
    text_input: sdl3::keyboard::TextInputUtil,
    /// Typed text is on: while the game's text box or a field of ours is
    /// open; and where it is.
    typing: Option<Typing>,
    typing_rect: Option<egui::Rect>,
    /// The on-screen keyboard, which hides the touch controls, was put away
    /// with a tap above it: the game's text box takes A or B from them.
    keyboard_away: bool,
    pads: pads::Pads,
    presentation: Presentation,
    /// For the window's surfaces after the first.
    #[cfg(target_os = "android")]
    instance: wgpu::Instance,
    user_dir: PathBuf,
    settings: Settings,
    ui: Ui,
    touch: Touch,
    /// Touch is the input in use: the touch controls follow it (touching
    /// shows them, a controller or keyboard hides them; being connected is
    /// not enough, phones often have a paired controller lying around).
    touch_input: bool,
    touch_shown: bool,
    /// What the touch controls last did with the mouse and Esc (the pause
    /// menu's).
    touch_mouse: touch::Mouse,
    lifecycle: Arc<Lifecycle>,
    folder: Option<FolderPick>,
    /// The game's folder, once found.
    game_dir: Option<PathBuf>,
    /// Why it was not, or a folder chosen that is not one.
    problem: Option<String>,
    /// The game streamed from a server (Settings › Game › Game files): its
    /// client, which keeps a copy of the game in the cache folder.
    stream: Option<Arc<glue5_stream::Client>>,
    /// The source `game_dir` is for.
    source: Source,
    /// The server and whether the whole game is wanted, as the stream was
    /// started for (it starts again when they change).
    stream_for: Option<(String, bool)>,
    /// This computer sharing its game folder (desktop).
    sharing: Option<crate::sharing::Sharing>,
    /// GLUE 5 Studio, served while the game runs with mods and the setting
    /// is on, and what the mods page shows of it.
    studio: Option<(glue5_mods::studio::Studio, ui::StudioInfo)>,
    /// The Studio could not start (both ports taken): when, and as what; not
    /// tried again for a while.
    studio_failed: Option<(Instant, (bool, String))>,
    /// Servers found on the network by the last search, and whether one runs.
    servers: Arc<Mutex<(bool, Vec<ui::FoundServer>)>>,
    /// Engine reads have waited for the server since then.
    stream_waiting: Option<Instant>,
    /// Why the last game stopped, if it did.
    last_failure: Option<String>,
    /// The game's graphics settings went back to their defaults.
    graphics_reset: bool,
    /// The upscalers this GPU has.
    upscalers: Vec<Upscaling>,
    /// It can generate frames.
    frame_generation: bool,
    /// Apple's Metal HUD can show: it was on at launch.
    metal_hud: bool,
    /// The display's refresh rate at most, in Hz.
    refresh_rate: f32,
    /// The renderer, until the game takes it over.
    screen: Option<Screen>,
    /// Play was chosen: the game starts once its pipelines are prepared.
    play_requested: bool,
    /// What Play starts.
    mode: Mode,
    /// The last benchmark's results.
    benchmark: Option<benchmark::Results>,
    /// GTA Online starts in a friend's lobby (its name) rather than the
    /// player's.
    join: Option<String>,
    /// The name friends were last told.
    friend_name: String,
    /// Since when the pointer should be captured (see `pointer`).
    capture_since: Option<Instant>,
    /// A mouse has been used in the game: phones capture it only then.
    mouse_used: bool,
    game: Option<Running>,
    /// Kept until after the game and menu streams have been dropped.
    _audio_subsystem: Option<sdl3::AudioSubsystem>,
}

/// The game, once started.
struct Running {
    game: glue5_runtime::Game,
    /// The engine's input block, once it has published it.
    input: Option<InputBlock>,
    started: Started,
    /// The back buffer size last asked of the engine.
    back_buffer: (u32, u32),
    _audio: Option<audio::Audio>,
}

impl App {
    fn run(mut self) -> Result<()> {
        loop {
            if let Some(exit) = glue5_runtime::exit::requested() {
                self.save();
                if exit == Exit::Restart {
                    if MOBILE {
                        // A phone app cannot start itself again: says so before it ends.
                        let text = "The new settings apply when Glue 5 is opened again.";
                        let flag = sdl3::messagebox::MessageBoxFlag::INFORMATION;
                        let _ = sdl3::messagebox::show_simple_message_box(flag, "Glue 5", text, &self.window);
                    }
                    relaunch(true);
                } else if self.mode == Mode::Benchmark {
                    self.benchmark_done();
                }
                return Ok(());
            }
            if let Some(why) = glue5_runtime::exit::failed() {
                self.stopped(why);
                return Ok(());
            }
            if let Some(game) = &mut self.game
                && game.input.is_none()
            {
                game.input = game.game.input();
                if let Some(i) = &game.input {
                    i.focus(true);
                }
            }
            let page = self.ui.page();
            // Before the game the loop sleeps until there is input or the UI
            // animates; with it, it keeps the touch controls and pads current.
            // In the background no frame is drawn, so a repaint due would
            // never be taken: no busy loop for it (iOS: inactive under
            // Control Center or an alert, for as long as that lasts).
            // Before the game on a desktop with a controller, the loop reads
            // it (at the pages' pace) rather than SDL every millisecond.
            let read_pads = !MOBILE && self.game.is_none() && self.pads.any();
            self.pads.read_by_loop(read_pads);
            let wait = if self.game.is_some() {
                Duration::from_millis(8)
            } else if self.menu_music.as_ref().is_some_and(audio::MenuMusic::is_fading) {
                Duration::from_millis(16)
            } else if self.lifecycle.background.load(Ordering::Relaxed) {
                Duration::from_millis(250)
            } else if read_pads {
                self.ui.idle_for().min(PAD_READ)
            } else {
                self.ui.idle_for()
            };
            let first = wait_event(&mut self.events, wait);
            self.pads.read();
            let events: Vec<Event> = first.into_iter().chain(self.events.poll_iter()).collect();
            for event in events {
                if self.event(event) {
                    self.save();
                    return Ok(());
                }
            }
            if self.lifecycle.came_back.swap(false, Ordering::Relaxed) {
                self.came_back();
            }
            if self.update(page)? {
                self.save();
                return Ok(());
            }
        }
    }

    /// One event; true to quit.
    fn event(&mut self, event: Event) -> bool {
        match &event {
            Event::Quit { .. } => return true,
            Event::GamepadAdded { which, .. } => self.pads.added(*which),
            Event::GamepadRemoved { which, .. } => self.pads.removed(*which),
            // A phone's on-screen keyboard types keys too: not a keyboard
            // taking over from the touch controls.
            Event::KeyDown { .. } if MOBILE && self.typing.is_some() => {}
            Event::KeyDown { .. } | Event::MouseButtonDown { .. } | Event::GamepadButtonDown { .. } => {
                self.touch_input = false
            }
            Event::GamepadAxisMotion { value, .. } if value.unsigned_abs() > 16000 => self.touch_input = false,
            Event::Window { win_event: WindowEvent::PixelSizeChanged(..) | WindowEvent::Resized(..), .. } => {
                let (w, h) = self.window.size_in_pixels();
                self.presentation.set_size(w, h);
                self.ui.repaint();
            }
            Event::Window { win_event: WindowEvent::Exposed, .. } => self.ui.repaint(),
            _ => {}
        }
        // The Siri Remote's touch surface moves like a pad's stick (SDL makes
        // the remote a gamepad); its touches are not on the screen.
        if TV
            && matches!(
                event,
                Event::FingerDown { .. } | Event::FingerMotion { .. } | Event::FingerUp { .. } | Event::FingerCanceled { .. }
            )
        {
            return false;
        }
        if let Event::FingerDown { x, y, .. } = event {
            // The settings button takes its finger before the touch controls
            // see it (or they would show, taking pad 0 from a controller).
            if !self.ui.is_open() && self.ui.gear_hit(x, y, &self.window) {
                self.ui.toggle_settings();
                return false;
            }
            // Its neighbour while mods have menus: those menus.
            if !self.ui.is_open() && self.ui.menus_hit(x, y, &self.window) {
                self.ui.toggle_mod_menus();
                return false;
            }
            // A notification tapped: the friends, at its tab.
            if self.ui.toast_hit(x, y, &self.window) {
                self.ui.open_toast();
                return false;
            }
            // So does the stream's sign: the Servers page.
            if !self.ui.is_open() && self.ui.chip_hit(x, y, &self.window) {
                self.ui.show_servers();
                return false;
            }
            self.touch_input = true;
            // A tap above the on-screen keyboard puts it away (it covers the
            // touch controls, and a phone's has no key to hide it).
            if MOBILE && self.typing.is_some() && !self.ui.is_open() {
                self.keyboard_away = true;
                return false;
            }
        }
        if let Event::KeyDown { scancode: Some(Scancode::F1), repeat: false, .. } = event {
            self.ui.toggle_settings();
            return false;
        }
        // F7: the mods' menus (with mods running).
        if let Event::KeyDown { scancode: Some(Scancode::F7), repeat: false, .. } = event
            && glue5_mods::installed()
            && !self.ui.wants_text()
        {
            self.ui.toggle_mod_menus();
            return false;
        }
        // Home, as it opened Social Club's overlay: the friends.
        if let Event::KeyDown { scancode: Some(Scancode::Home), repeat: false, .. } = event
            && !self.ui.wants_text()
        {
            self.ui.toggle_friends();
            return false;
        }
        if self.ui.is_open() {
            if let Event::KeyDown { scancode: Some(sc), repeat: false, .. } = event
                && (sc == Scancode::Escape || sc as i32 == 270)
            {
                self.ui.back();
            } else {
                self.ui.event(&event, &self.window);
            }
            return false;
        }
        match event {
            Event::MouseButtonDown { .. } => self.mouse_used = true,
            Event::FingerDown { finger_id, x, y, .. } => self.touch.finger_down(finger_id, x, y),
            Event::FingerMotion { finger_id, x, y, .. } => self.touch.finger_motion(finger_id, x, y),
            Event::FingerUp { finger_id, .. } => self.touch.finger_up(finger_id),
            Event::FingerCanceled { finger_id, .. } => self.touch.finger_cancel(finger_id),
            _ => {}
        }
        if let Some(game) = &self.game
            && let Some(input) = &game.input
        {
            game_input(input, &event, &self.window, game.back_buffer);
        }
        false
    }

    /// Everything after the events: touch controls, pads, the UI and what
    /// it asks for. True to quit.
    fn update(&mut self, page_before: Option<Page>) -> Result<bool> {
        self.keep_sources();
        self.update_studio();
        let in_game = self.game.is_some();
        // Touch controls: a virtual pad in slot 0 unless a controller has it.
        let show = in_game
            && !self.ui.is_open()
            && match self.settings.touch {
                TouchMode::Always => true,
                TouchMode::Off => false,
                TouchMode::Auto => self.touch_input,
            };
        if show != self.touch_shown {
            self.touch_shown = show;
            self.touch.clear();
            glue5_runtime::input::set_gamepad(0, Gamepad::default());
            if let Some(input) = self.game.as_ref().and_then(|g| g.input.as_ref()) {
                if self.touch_mouse.down {
                    input.mouse_button(mouse::LEFT, false);
                }
                for (held, vk) in self.touch_mouse.keys.into_iter().zip(MENU_VKS) {
                    if held {
                        input.key(vk, false);
                    }
                }
            }
            self.touch_mouse = Default::default();
        }
        glue5_runtime::input::set_touch_play(show);
        glue5_runtime::input::set_touch_targeting(self.settings.touch_aim.targeting());
        if show {
            self.touch.haptics = self.settings.haptics;
            self.touch.camera_speed = self.settings.touch_camera;
            self.touch.set_profile(self.settings.touch_layout);
            self.touch.set_steer(self.settings.touch_steer);
            self.touch.set_fire(self.settings.touch_fire);
            self.touch.aim_locked = glue5_runtime::input::aim_locked();
            let screen = ui::screen_size(&self.window);
            self.touch.layout(screen, ui::safe_rect(&self.window), self.settings.touch_size);
            self.touch.set_context(glue5_runtime::input::player_context());
            // What the game asks to press, where its phone is.
            let phone = glue5_runtime::input::phone_rect(screen.x / screen.y.max(1.0));
            self.touch.set_game(glue5_runtime::prompts::prompts(), phone);
            self.touch.set_pad_map(glue5_runtime::prompts::pad_map());
            self.touch.menu_went_deeper(glue5_runtime::input::menu_went_deeper());
            glue5_runtime::input::set_gamepad(0, self.touch.gamepad());
            for input in self.touch.take_inputs() {
                glue5_runtime::prompts::hold_input(input);
            }
            let look = self.touch.take_look();
            let mouse = self.touch.take_mouse();
            if let Some(game) = &self.game
                && let Some(input) = &game.input
            {
                // The camera, dragged: as a mouse turns it.
                if look != (0, 0) {
                    input.mouse_motion(look.0, look.1);
                }
                // The pause menu: a finger is the mouse.
                if mouse.at != self.touch_mouse.at
                    && let Some(at) = mouse.at
                {
                    let (w, h) = game.back_buffer;
                    input.mouse_position((at.x * w as f32) as i32, (at.y * h as f32) as i32);
                }
                if mouse.down != self.touch_mouse.down {
                    input.mouse_button(mouse::LEFT, mouse.down);
                }
                if mouse.wheel != 0 {
                    input.mouse_wheel(mouse.wheel);
                }
                for ((held, was), vk) in mouse.keys.into_iter().zip(self.touch_mouse.keys).zip(MENU_VKS) {
                    if held != was {
                        input.key(vk, held);
                    }
                }
                self.touch_mouse = mouse;
            }
        }
        // The game's rumble while the touch controls play; stopped as soon as
        // they hide or haptics go off (a vibration plays until told to stop).
        haptics::rumble(if show && self.settings.haptics { glue5_runtime::input::rumble(0) } else { (0, 0) });

        // While the touch controls are pad 0, a controller in slot 0 waits.
        let report = self.pads.update(self.ui.is_open(), show);
        if report.menu {
            self.ui.toggle_settings();
        }
        if report.friends {
            self.ui.toggle_friends();
        }
        for nav in report.nav {
            self.ui.nav(nav);
        }

        // What applies at once (the engine's own frame limiter at its next start).
        self.presentation.set_vsync(self.settings.vsync);
        self.presentation.set_metal_hud(self.settings.overlay == PerfOverlay::MetalHud);
        glue5_gpu::perf::set_on(in_game && self.settings.overlay == PerfOverlay::Glue5);
        // With the overlay: what the frame's HUD phase does, which is what the
        // overlay is for when the question is the HUD's resolution.
        glue5_gpu::census::set_on(in_game && self.settings.overlay == PerfOverlay::Glue5);
        #[cfg(target_os = "ios")]
        self.watch_thermal();
        self.presentation.set_frame_rate(self.settings.max_fps);
        #[cfg(target_os = "android")]
        if self.asked_rate != Some(self.settings.max_fps) {
            self.asked_rate = Some(self.settings.max_fps);
            ask_frame_rate(&self.window, self.settings.max_fps);
        }
        self.presentation.set_upscaler(upscaler(&self.settings));
        self.presentation.set_sharpness(self.settings.sharpness);
        self.presentation.set_frame_generation(self.settings.frame_generation);
        self.resize_game();

        // The UI: over the game at every turn, before it when something changed.
        let mut actions = Vec::new();
        let stream = self.stream.clone();
        let stream = stream.as_ref().map(|c| (c.server(), c.status()));
        let stream_alert = self.stream_alert(stream.as_ref().map(|(_, s)| s));
        let servers = self.servers.lock().unwrap().clone();
        if in_game || (!self.lifecycle.background.load(Ordering::Relaxed) && self.ui.wants_frame()) {
            let status = Status {
                game_dir: self.game_dir.as_deref(),
                problem: self.problem.as_deref(),
                started: self.game.as_ref().map(|g| &g.started),
                last_failure: self.last_failure.as_deref(),
                graphics_reset: self.graphics_reset,
                window: self.window.size_in_pixels(),
                back_buffer: self.game.as_ref().map(|g| g.back_buffer),
                upscalers: &self.upscalers,
                frame_generation: self.frame_generation,
                metal_hud: self.metal_hud,
                refresh_rate: self.refresh_rate,
                preparing: self.play_requested.then(|| self.screen.as_ref().and_then(Screen::progress)).flatten(),
                mode: self.mode,
                benchmark: self.benchmark.as_ref(),
                online: self.game.is_some() && self.mode == Mode::Online,
                friend_requests: glue5_runtime::friends::pending(),
                source: self.settings.source,
                stream: stream.as_ref().map(|(server, status)| (*server, status)),
                sharing: self.sharing.as_ref().map(crate::sharing::Sharing::state),
                mods: glue5_mods::installed(),
                mod_menus: glue5_mods::has_menus(),
                studio: self.studio.as_ref().map(|(_, info)| info),
                servers: &servers.1,
                searching: servers.0,
                stream_alert,
            };
            // The settings button: on touch screens always, so that it stays
            // with a controller in use or the touch controls off.
            let gear =
                (in_game && (TOUCH || show)).then_some(if show { self.settings.touch_opacity } else { GEAR_ALONE });
            actions = self.ui.frame(&self.window, &mut self.settings, &status, show.then_some(&self.touch), gear);
            if let Some(screen) = &mut self.screen
                && !screen.show_ui()
            {
                // Not shown (the window not visible yet, ...): again soon.
                self.ui.repaint_in(Duration::from_millis(100));
            }
        }
        for action in actions {
            match action {
                Action::Quit => return Ok(true),
                Action::Play(mode) => {
                    self.mode = mode;
                    if mode != Mode::Benchmark {
                        self.settings.sandbox = mode == Mode::Sandbox;
                        self.settings.online = mode == Mode::Online;
                    }
                    self.play()?;
                }
                Action::ChooseGame => self.folder = Some(FolderPick::start(&self.window)),
                Action::FindGame => self.find_files(),
                Action::ImportSave => self.import_from_phone(crate::imports::Kind::Save),
                Action::ImportResources => self.import_from_phone(crate::imports::Kind::Resources),
                Action::CopyImportDiagnostics => {
                    #[cfg(target_os = "android")]
                    {
                        let touch_events = self.touch.diagnostics();
                        if let Err(error) = crate::imports::copy_diagnostics(
                            &self.user_dir,
                            self.source,
                            self.game_dir.as_deref(),
                            &self.settings,
                            &touch_events,
                        ) {
                            self.ui.note(format!("Could not copy import diagnostics: {error:#}"));
                        }
                    }
                    #[cfg(not(target_os = "android"))]
                    self.ui.note("Import diagnostics are available in the Android app.");
                }
                Action::CaptureSessionLogcat => {
                    #[cfg(target_os = "android")]
                    if let Err(error) = crate::imports::request_logcat_capture() {
                        self.ui.note(format!("Could not request GLUE 5 process log capture via Shizuku: {error:#}"));
                    }
                    #[cfg(not(target_os = "android"))]
                    self.ui.note("GLUE 5 process log capture via Shizuku is available in the Android app.");
                }
                Action::StartLogcatCapture => {
                    #[cfg(target_os = "android")]
                    if let Err(error) = crate::imports::start_logcat_capture() {
                        self.ui.note(format!("Could not start GLUE 5 process log capture via Shizuku: {error:#}"));
                    }
                    #[cfg(not(target_os = "android"))]
                    self.ui.note("GLUE 5 process log capture via Shizuku is available in the Android app.");
                }
                Action::StopLogcatCapture => {
                    #[cfg(target_os = "android")]
                    if let Err(error) = crate::imports::stop_logcat_capture() {
                        self.ui.note(format!("Could not finish GLUE 5 process log capture via Shizuku: {error:#}"));
                    }
                    #[cfg(not(target_os = "android"))]
                    self.ui.note("GLUE 5 process log capture via Shizuku is available in the Android app.");
                }
                Action::MarkDebugEvent(marker) => {
                    #[cfg(target_os = "android")]
                    match crate::imports::mark_debug_event(marker.tag()) {
                        Ok(()) => self.ui.note(format!("Debug marker recorded: {}", marker.tag())),
                        Err(error) => self.ui.note(format!("Could not mark GLUE 5 debug event: {error:#}")),
                    }
                    #[cfg(not(target_os = "android"))]
                    self.ui.note(format!("GLUE 5 debug event: {}", marker.tag()));
                }
                Action::ResetGraphics => self.reset_graphics(),
                Action::JoinFriend { key, lobby } => {
                    if self.game.is_none() {
                        // GTA Online in the friend's lobby, then into its
                        // session.
                        self.join = Some(lobby);
                        glue5_runtime::friends::follow(&key);
                        self.mode = Mode::Online;
                        self.settings.sandbox = false;
                        self.settings.online = true;
                        self.play()?;
                    }
                }
                Action::CloseFriends => {
                    if self.ui.page() == Some(Page::Friends) {
                        self.ui.back();
                    }
                }
                Action::ClearCopy => self.clear_copy(),
                Action::FindServers => self.find_servers(),
                Action::UseServer(address) => {
                    self.settings.server = address;
                    self.settings.source = Source::Server;
                    self.settings.save(&self.user_dir);
                }
            }
        }
        if self.play_requested {
            self.play()?;
        }
        if let Some(choice) = self.folder.as_ref().and_then(FolderPick::poll) {
            self.folder = None;
            self.chosen(choice);
        }
        if self.game.is_none() {
            // The Android picker's answer for the game's folder, left as a
            // file while the app was away (see imports::take_game_pick).
            if let Some(pick) = crate::imports::take_game_pick(&self.user_dir) {
                self.game_picked(pick);
            }
            for result in crate::imports::install_ready(&self.user_dir, self.game_dir.as_deref()) {
                match result {
                    Ok(message) => self.ui.note(message),
                    Err(error) => self.ui.note(format!("Import failed: {error:#}")),
                }
            }
        }

        let page = self.ui.page();
        if page != page_before {
            if let Some(input) = self.game.as_ref().and_then(|g| g.input.as_ref()) {
                // Nothing stays pressed in the game behind a page.
                input.release_all();
            }
            if matches!(page_before, Some(Page::Settings | Page::Servers | Page::Mods)) {
                self.settings.save(&self.user_dir);
            }
        }
        // What friends do, as notifications (unless hidden).
        for notice in glue5_runtime::friends::take_notices() {
            if self.settings.friend_notices {
                self.ui.notify(&notice);
            }
        }
        // What mods notify (`ui.notify`).
        for (module, text) in glue5_mods::take_notices() {
            self.ui.notify_mod(&module, &text);
        }
        // The name friends see follows the player's.
        if self.friend_name != self.settings.online_name {
            self.friend_name.clone_from(&self.settings.online_name);
            glue5_runtime::friends::set_name(self.friend_name.trim());
        }

        // Text input only while the game wants text: on touch devices it
        // brings up the system keyboard; on desktop it hands every key to
        // the system's input methods, whose accent menu (a held key, macOS),
        // dead keys and IME windows would otherwise get in the way of play.
        // Keys reach the game and the UI by scancode either way.
        // The system keyboard is the one the text asks for, and the field
        // stays above it (SDL moves the window's view up on iOS, pans on
        // Android).
        let text_box = glue5_runtime::input::text_box();
        self.keyboard_away &= text_box.is_some();
        let field = self.ui.text_field();
        let typing = match (field, text_box) {
            (Some((kind, _)), _) => Some(Typing::App(kind)),
            (None, Some(kind)) if !self.keyboard_away => Some(Typing::Game(kind)),
            _ => None,
        };
        if typing != self.typing {
            if self.typing.is_some() {
                self.text_input.stop(&self.window);
            }
            if let Some(typing) = typing
                && let Err(e) = self.text_input.start_with_options(&self.window, typing.options())
            {
                log::warn!("text input: {e}");
                self.text_input.start(&self.window);
            }
            self.typing = typing;
        }
        if let Some(typing) = self.typing {
            // The game draws its box in the middle of the screen.
            let size = ui::screen_size(&self.window);
            let middle = egui::Rect::from_center_size(egui::pos2(size.x * 0.5, size.y * 0.5), size * egui::vec2(0.5, 0.3));
            let rect = match (typing, field) {
                (Typing::App(_), Some((_, rect))) => rect,
                _ => middle,
            };
            if self.typing_rect != Some(rect) {
                self.typing_rect = Some(rect);
                self.text_input.set_rect(&self.window, ui::window_rect(&self.window, rect), 0);
            }
        } else {
            self.typing_rect = None;
        }
        // The start page's music keeps playing while its menus are open, dulled
        // behind them (audio::MenuMusic's low-pass); it stops with the game.
        let music_enabled = self.game.is_none() && !self.lifecycle.background.load(Ordering::Relaxed);
        let music_muffled = self.ui.page() != Some(Page::Start);
        let music_result = if let Some(music) = self.menu_music.as_mut() {
            music.update(music_enabled, music_muffled)
        } else {
            Ok(())
        };
        if let Err(error) = music_result {
            log::warn!("start-page music stopped: {error:#}");
            self.menu_music = None;
        }
        self.pointer();
        Ok(false)
    }

    /// "Find files".  On Android, the picker: it can point at a folder
    /// anywhere on the device's storage, and the game is read where it is
    /// (its 20 GB are not copied in). Elsewhere, look again at places the
    /// files may have been copied into meanwhile (Finder, Files, or adb).
    fn find_files(&mut self) {
        #[cfg(target_os = "android")]
        {
            if let Err(error) = crate::imports::pick_game() {
                let why = format!("Cannot open the Android folder picker: {error:#}");
                crate::imports::record_game_picker_issue(&self.user_dir, &why);
                // Preserve the old Find files behavior as a fallback: it can
                // still rediscover a game in the app folder or known locations.
                self.find_game(None);
                if self.game_dir.is_none() {
                    self.no_game(why);
                }
            }
            // The Android activity owns the asynchronous document picker;
            // do not fall through to the desktop folder dialog.
            return;
        }
        #[cfg(not(target_os = "android"))]
        self.find_game(None);
    }

    /// The folder the Android picker chose for the game.
    fn game_picked(&mut self, pick: crate::imports::GamePick) {
        match pick {
            crate::imports::GamePick::Found(dir) => match resolve_game_dir(&dir) {
                Some(dir) => self.chosen(Some(dir)),
                // Chosen the folder holding it, or one without the game at
                // all: say what is looked for, in this build's terms.
                None => {
                    let why = format!(
                        "{} has no game folder in it, or just below: b/{}/shaders/ and data/ are what is looked for.",
                        dir.display(),
                        glue5_runtime::GAME_BUILD
                    );
                    crate::imports::record_game_picker_issue(&self.user_dir, &why);
                    self.no_game(why);
                }
            },
            crate::imports::GamePick::Refused(why) => {
                crate::imports::record_game_picker_issue(&self.user_dir, &why);
                // Permission can be revoked by reinstall or declined in the
                // system page. Try the previous known-location scan before
                // telling the player there is no usable game folder.
                self.find_game(None);
                if self.game_dir.is_none() {
                    self.no_game(why);
                }
            },
        }
    }

    /// Why there is no game to play: a note, and the start page's card.
    fn no_game(&mut self, problem: String) {
        self.ui.note(problem.clone());
        if self.game.is_none() {
            self.problem = Some(problem);
            self.ui.repaint();
        }
    }

    /// The picker itself is owned by the Android activity.  It stages files
    /// outside the live game tree; `install_ready` above installs them after
    /// the activity returns (also after Android had to recreate the app).
    fn import_from_phone(&mut self, kind: crate::imports::Kind) {
        #[cfg(target_os = "android")]
        if let Err(error) = crate::imports::pick(kind) {
            self.ui.note(format!("Cannot open the file picker: {error:#}"));
        }
        #[cfg(not(target_os = "android"))]
        {
            let _ = kind;
            self.ui.note("Import from the phone is available in the Android app.");
        }
    }

    /// The mouse pointer, as the game has it on PC: captured in play (hidden,
    /// every motion reaching the game); free while the game shows its own
    /// cursor (the pause menu and its map, the phone's browser, ...), the
    /// system's hidden over the window then, as the game draws its own; free
    /// and shown while the window is not in front or a page of ours is
    /// open. It follows these as they change: Esc frees it with the pause
    /// menu and captures it again on leaving it, as does switching to
    /// another window and back; no click is needed.
    fn pointer(&mut self) {
        // Touch alone (phones without a mouse): nothing to do.
        if !self.mouse_used {
            return;
        }
        let Some(input) = self.game.as_ref().and_then(|g| g.input) else { return };
        let ours = self.ui.is_open() || !self.window.has_input_focus();
        let game_cursor = input.cursor_shown();
        let want = !ours && !game_cursor && self.mode != Mode::Benchmark;
        // Capturing waits a moment: back in front over a menu, the game
        // shows its cursor only a frame or two later.
        self.capture_since = want.then(|| self.capture_since.unwrap_or_else(Instant::now));
        let capture = self.capture_since.is_some_and(|t| t.elapsed() >= CAPTURE_DELAY);
        let captured = self.mouse.relative_mouse_mode(&self.window);
        if captured != capture && (capture || !want) {
            self.mouse.set_relative_mouse_mode(&self.window, capture);
            input.pointer_locked(capture);
        }
        let shown = ours || !game_cursor;
        if self.mouse.is_cursor_showing() != shown {
            self.mouse.show_cursor(shown);
        }
    }

    /// The game's back buffer follows the window and the render scale: asked
    /// of the engine when it differs. The engine waits for a size to hold for 20
    /// of its frames before rebuilding its render targets, and a window
    /// being resized by the system does not reach this loop until the end:
    /// one rebuild per resize. Phone windows never change size; only the
    /// scale moves their back buffer.
    fn resize_game(&mut self) {
        let Some(game) = &mut self.game else { return };
        let Some(input) = &game.input else { return };
        let (w, h) = self.window.size_in_pixels();
        if w == 0 || h == 0 {
            return;
        }
        let size = settings::render_size((w, h), self.settings.render_scale(), game.started.low_memory);
        if size != game.back_buffer {
            log::info!("back buffer {}x{}", size.0, size.1);
            input.resize(size.0, size.1);
            game.back_buffer = size;
        }
    }

    /// Starts the game once the pipelines known for it are prepared (the
    /// start page shows how far; called again until then); the renderer
    /// takes the screen over.
    fn play(&mut self) -> Result<()> {
        let Some(game_dir) = self.game_dir.clone() else { return Ok(()) };
        let mut gpu_settings = glue5_gpu::Settings::for_game(&game_dir);
        gpu_settings.cache_dir = Some(self.user_dir.join("cache"));
        let Some(screen) = self.screen.as_mut() else { return Ok(()) };
        self.play_requested = !screen.prepare(&gpu_settings);
        if self.play_requested {
            return Ok(());
        }
        let Some(screen) = self.screen.take() else { return Ok(()) };
        // Graphics quality is the game's own (its menu); the low-memory
        // profile only lowers it once, after which the menu has the last word.
        if self.settings.low_memory && !self.settings.low_preset_written {
            match glue5_runtime::game_settings::apply_low_preset(&self.user_dir) {
                Ok(()) => {
                    self.settings.low_preset_written = true;
                    log::info!("the game's graphics settings: low-memory preset written");
                }
                Err(e) => log::warn!("cannot write the low-memory preset: {e}"),
            }
        }
        if self.mode == Mode::Online && self.settings.online_name.trim().is_empty() {
            self.settings.online_name = settings::default_online_name();
        }
        self.settings.save(&self.user_dir);
        // A new attempt: the last one's failure is history.
        let _ = std::fs::remove_file(self.user_dir.join(FAILURE_FILE));
        log::info!("game: {}, user data: {}", game_dir.display(), self.user_dir.display());
        let s = &self.settings;
        let back_buffer = settings::render_size(self.window.size_in_pixels(), s.render_scale(), s.low_memory);
        if s.mods {
            glue5_mods::install(&paths::mods_dir(&self.user_dir));
        }
        crate::imports::sync_legacy_saves_to_default(&self.user_dir);
        let game = glue5_runtime::start(
            glue5_runtime::Config {
                game_dir: game_dir.clone(),
                stream: self.stream.clone().filter(|_| self.source == Source::Server),
                user_dir: self.user_dir.clone(),
                width: back_buffer.0,
                height: back_buffer.1,
                // The browser page's start choices: 1 the story, 2 the sandbox.
                start_mode: if self.mode == Mode::Sandbox { 2 } else { 1 },
                benchmark: self.mode == Mode::Benchmark,
                low_memory: s.low_memory,
                max_fps: s.max_fps,
                online: (self.mode == Mode::Online).then(|| glue5_runtime::Online {
                    name: s.online_name.trim().into(),
                    lobby: self.join.take().unwrap_or_default(),
                }),
                language: preferred_language(),
            },
            glue5_gpu::backend(gpu_settings, Some(screen)),
        )?;
        let audio = self._audio_subsystem.as_ref().and_then(|subsystem| match audio::start(subsystem, game.audio()) {
            Ok(audio) => Some(audio),
            Err(error) => {
                log::warn!("no game audio: {error}");
                None
            }
        });
        let started =
            Started { game_dir, source: self.source, max_fps: s.max_fps, low_memory: s.low_memory, mods: s.mods };
        self.game = Some(Running { game, input: None, started, back_buffer, _audio: audio });
        self.ui.game_started();
        Ok(())
    }

    /// The benchmark is over (the engine asked to quit): on desktop the app
    /// opens again on its start page, which shows the results; a phone app
    /// cannot, so it shows them before it ends.
    fn benchmark_done(&mut self) {
        if !MOBILE {
            relaunch(false);
            return;
        }
        let text = match benchmark::last(&self.user_dir) {
            Some(r) => format!(
                "Benchmark: {:.0} fps on average, {:.0} at least; {:.0}% of the frames in under 33 ms.",
                r.avg_fps,
                r.min_fps,
                r.smooth * 100.0
            ),
            None => "The benchmark is over.".into(),
        };
        let flag = sdl3::messagebox::MessageBoxFlag::INFORMATION;
        let _ = sdl3::messagebox::show_simple_message_box(flag, "Glue 5", &text, &self.window);
    }

    /// The game has stopped working (a thread of the engine's failed): the
    /// window shows its last frame for good. Says so, and records it for the
    /// start page, which offers the way out of a graphics setting the engine
    /// cannot run with; on desktop it opens again on that page.
    fn stopped(&mut self, why: &str) {
        let why = match glue5_mods::note_failure() {
            Some(note) => format!("{why} {note}"),
            None => why.to_string(),
        };
        let why = why.as_str();
        if let Err(e) = std::fs::write(self.user_dir.join(FAILURE_FILE), why) {
            log::warn!("cannot record the failure: {e}");
        }
        #[cfg(target_os = "android")]
        {
            let touch_events = self.touch.diagnostics();
            if let Err(error) = crate::imports::copy_diagnostics(
                &self.user_dir,
                self.source,
                self.game_dir.as_deref(),
                &self.settings,
                &touch_events,
            ) {
                log::error!("cannot save/copy support report after game failure: {error:#}");
            }
        }
        self.settings.save(&self.user_dir);
        self.mouse.set_relative_mouse_mode(&self.window, false);
        self.mouse.show_cursor(true);
        let page = if MOBILE { "when the app is opened again, its start page" } else { "the start page" };
        let text = format!(
            "The game has stopped: {why}.\n\nIf it stops again at the same point, one of its graphics settings \
             may be the cause: {page} can put them back to their defaults."
        );
        use sdl3::messagebox::{ButtonData, ClickedButton, MessageBoxButtonFlag as Button, MessageBoxFlag};
        let buttons: &[ButtonData] = if MOBILE {
            &[ButtonData { flags: Button::RETURNKEY_DEFAULT, button_id: 0, text: "OK" }]
        } else {
            &[
                ButtonData { flags: Button::ESCAPEKEY_DEFAULT, button_id: 0, text: "Quit" },
                ButtonData { flags: Button::RETURNKEY_DEFAULT, button_id: 1, text: "Start page" },
            ]
        };
        let shown =
            sdl3::messagebox::show_message_box(MessageBoxFlag::ERROR, buttons, "Glue 5", &text, &self.window, None);
        if let Ok(ClickedButton::CustomButton(ButtonData { button_id: 1, .. })) = shown {
            relaunch(false);
        }
    }

    /// The game's graphics settings back to the engine's defaults (asked for
    /// on the start page).
    fn reset_graphics(&mut self) {
        match glue5_runtime::game_settings::reset_graphics(&self.user_dir) {
            Ok(()) => {
                log::info!("the game's graphics settings: back to the defaults");
                self.graphics_reset = true;
                // The low-memory profile lowers them again at the next start.
                self.settings.low_preset_written = false;
                self.settings.save(&self.user_dir);
            }
            Err(e) => self.ui.note(format!("Cannot reset the game's graphics settings: {e}")),
        }
        self.ui.repaint();
    }

    /// Back in the foreground.
    fn came_back(&mut self) {
        // Android gave the window a new native surface (SDL has updated the
        // window's handle to it already).
        #[cfg(target_os = "android")]
        {
            self.replace_surface();
            self.asked_rate = None;
        }
        // iOS may take back a suspended app's listening sockets: the Studio
        // starts again (on the next update) on a new one. Not after it was
        // only inactive (its browsers would lose their connections).
        if self.lifecycle.suspended.swap(false, Ordering::Relaxed) && cfg!(any(target_os = "ios", target_os = "tvos")) {
            self.studio = None;
        }
        self.ui.repaint();
        // The game's files may have been copied in meanwhile (Finder, the
        // Files app, adb). Not for a server, whose copy is found once ready.
        if self.game.is_none() && self.game_dir.is_none() && self.settings.source == Source::Folder {
            self.find_game(None);
        }
    }

    /// Logs the phone's thermal state when it changes (asked once a second):
    /// hot (serious within a minute of play on an iPhone 16 Pro), iOS holds
    /// the game to its efficiency cores and frames take 20-50 ms, which
    /// tests and logs should know.
    #[cfg(target_os = "ios")]
    fn watch_thermal(&mut self) {
        if self.thermal.1.is_some_and(|at| at.elapsed() < Duration::from_secs(1)) {
            return;
        }
        let state = objc2_foundation::NSProcessInfo::processInfo().thermalState().0;
        if state != self.thermal.0 {
            const NAMES: [&str; 4] = ["nominal", "fair", "serious", "critical"];
            log::info!("thermal: {}", NAMES.get(state as usize).copied().unwrap_or("?"));
        }
        self.thermal = (state, Some(Instant::now()));
    }

    /// A surface for the window's current native surface, for the renderer
    /// to present into from its next frame.
    #[cfg(target_os = "android")]
    fn replace_surface(&mut self) {
        // The window outlives the renderer: the process exits with it.
        let surface = unsafe {
            wgpu::SurfaceTargetUnsafe::from_display_and_window(&self.window, &self.window)
                .map_err(anyhow::Error::from)
                .and_then(|target| Ok(self.instance.create_surface_unsafe(target)?))
        };
        match surface {
            Ok(surface) => self.presentation.replace_surface(surface),
            Err(e) => log::error!("no surface for the window: {e:#}"),
        }
    }

    /// The settings, the stream's index and the friends (the app ends).
    fn save(&self) {
        self.settings.save(&self.user_dir);
        if let Some(stream) = &self.stream {
            stream.flush();
        }
        glue5_runtime::friends::flush();
    }

    /// Follows the settings: the game's files from a folder or a server
    /// (the stream started, or started again for another server), this
    /// computer's folder shared or not. Not while the game runs on them,
    /// but sharing turned off stops at once.
    fn keep_sources(&mut self) {
        if self.game.is_some() {
            // Turned on, it starts with the start page: its first preparation
            // packs files for a minute, which the game would feel. Stopped in
            // the background (it says goodbye on the network).
            if !self.settings.share
                && let Some(sharing) = self.sharing.take()
            {
                std::thread::spawn(move || drop(sharing));
                self.ui.repaint();
            }
            return;
        }
        let source = self.settings.source;
        if source != self.source {
            self.source = source;
            match source {
                Source::Folder => self.find_game(None),
                Source::Server => {
                    self.game_dir = None;
                    self.problem = None;
                    // On a TV or a phone, typing an address is the last resort.
                    if self.settings.server.trim().is_empty() {
                        self.find_servers();
                    }
                }
            }
            self.ui.repaint();
        }
        // Not while the address is being typed.
        let server = self.settings.server.trim();
        let wanted = (source == Source::Server && !server.is_empty()).then(|| (server.to_string(), self.settings.download_all));
        if wanted != self.stream_for && !self.ui.wants_text() {
            self.stream_for = wanted.clone();
            self.start_stream(wanted.map(|(server, _)| server));
        }
        if let Some(stream) = &self.stream {
            let status = stream.status();
            // A complete download is a game folder: the local mode finds it too.
            if status.complete
                && self.settings.download_all
                && paths::can_download_all()
                && self.settings.game_dir.is_none()
            {
                self.settings.game_dir = Some(stream.root().to_owned());
            }
            let ready = stream.is_ready().then(|| stream.root().to_owned());
            if source == Source::Server && ready != self.game_dir {
                self.game_dir = ready;
                self.ui.repaint();
            }
            // The start page shows how far it is, while something moves.
            let moving = status.phase != glue5_stream::Phase::Ready
                || !status.connected
                || status.boot.0 < status.boot.1
                || status.filling;
            if moving {
                self.ui.repaint_in(Duration::from_millis(300));
            }
        }
        if !MOBILE {
            let share = (self.settings.share && source == Source::Folder).then(|| self.game_dir.clone()).flatten();
            if self.sharing.as_ref().map(|s| s.dir()) != share.as_deref() {
                let cache = paths::cache_dir(&self.user_dir).join("serve");
                self.sharing = share.map(|dir| crate::sharing::Sharing::start(dir, cache));
                self.ui.repaint();
            }
            if self.sharing.as_ref().is_some_and(|s| matches!(s.state(), crate::sharing::State::Preparing(_))) {
                self.ui.repaint_in(Duration::from_millis(500));
            }
        }
    }

    /// GLUE 5 Studio follows its settings: served while the game runs
    /// with mods and the setting is on, for this device or the network.
    /// The mods folder opens when mods are on (before the game too: the
    /// Mods page lists them, the Studio edits them); the Studio serves
    /// while it is on.
    fn update_studio(&mut self) {
        if self.settings.mods && glue5_mods::dir().is_none() {
            glue5_mods::open(&paths::mods_dir(&self.user_dir));
        }
        // None yet, "New" pressed, or not a code (an edited settings file).
        let code = match glue5_mods::studio::normalize(&self.settings.studio_code) {
            Some(code) => code,
            None => {
                self.settings.studio_code = glue5_mods::studio::new_code();
                self.settings.save(&self.user_dir);
                self.settings.studio_code.clone()
            }
        };
        let wanted = (self.settings.mods && glue5_mods::dir().is_some() && self.settings.studio)
            .then(|| (self.settings.studio_lan, code.clone()));
        if self.studio.as_ref().map(|(s, _)| (s.lan(), s.token().to_string())) == wanted {
            return;
        }
        if let (Some((at, tried)), Some(w)) = (&self.studio_failed, &wanted)
            && tried == w
            && at.elapsed() < Duration::from_secs(10)
        {
            return;
        }
        self.studio = None;
        if let Some((lan, code)) = wanted {
            // The usual port, else any (another app has it).
            let started = glue5_mods::studio::Studio::start(glue5_mods::studio::PORT, lan, &code)
                .or_else(|_| glue5_mods::studio::Studio::start(0, lan, &code));
            match started {
                Ok(studio) => {
                    let host = if lan {
                        glue5_stream::lan_addresses().first().map_or("127.0.0.1".into(), |ip| ip.to_string())
                    } else {
                        "127.0.0.1".into()
                    };
                    let info = ui::StudioInfo {
                        url: studio.url(&host),
                        code: studio.pairing_code(),
                        lan,
                    };
                    self.studio = Some((studio, info));
                    self.studio_failed = None;
                }
                Err(e) => {
                    log::warn!("studio: {e}");
                    self.studio_failed = Some((Instant::now(), (lan, code)));
                }
            }
        }
        self.ui.repaint();
    }

    /// (Re)starts the stream from `server` (none: stops it), its copy where
    /// the settings want it: moved there if it was elsewhere (on the same
    /// volume: a rename).
    fn start_stream(&mut self, server: Option<String>) {
        let root = self.copy_root();
        let old = self.stream.take().map(|old| {
            old.stop();
            old.root().to_owned()
        });
        let cache = paths::stream_copy(&self.user_dir);
        let earlier = [Some(cache.clone()), paths::download_dir(&self.user_dir)];
        let streaming = server.is_some();
        // Once the old client has finished (it saves its index last), before
        // the next one opens the copy.
        let to = root.clone();
        glue5_stream::after_stopped(move || {
            let root = to;
            // At start, where an earlier run left it: the download setting
            // changed while the game ran, which the stream never follows.
            let old = old.or_else(|| {
                earlier
                    .into_iter()
                    .flatten()
                    .find(|dir| *dir != root && dir.join(glue5_stream::cache::CATALOG).is_file())
            });
            // Only a partial copy moves: the whole game, downloaded, stays a game
            // folder (moved into the cache, an iPhone's system emptied most of it).
            let partial = |dir: &Path| dir.join(glue5_stream::cache::MARKER).exists();
            let old = old.filter(|old| *old != root && partial(old));
            // Folders with no file in them, left there earlier, are not in the way.
            if old.is_some() && root.exists() && holds_no_files(&root) {
                let _ = std::fs::remove_dir_all(&root);
            }
            if let Some(old) = old.filter(|_| !root.exists()) {
                if let Some(parent) = root.parent() {
                    let _ = std::fs::create_dir_all(parent);
                }
                if let Err(e) = std::fs::rename(&old, &root) {
                    log::warn!("stream: moving the copy to {}: {e}", root.display());
                }
            }
            // Beside the whole game (downloaded, or on its way), a partial
            // copy left in the cache is of no use, and takes room it needs.
            if streaming
                && root != cache
                && cache.join(glue5_stream::cache::MARKER).exists()
                && let Err(e) = glue5_stream::cache::clear(&cache)
            {
                log::warn!("stream: deleting the copy in the cache: {e}");
            }
            // Out of the backups (iOS), now that it is there: moved, or
            // made here for the stream to fill.
            if streaming {
                let _ = std::fs::create_dir_all(&root);
                paths::keep_out_of_backups(&root);
            }
        });
        self.stream = server.map(|server| {
            let client = glue5_stream::Client::start(glue5_stream::Options {
                server,
                root,
                build: glue5_runtime::GAME_BUILD.into(),
                low_memory: self.settings.low_memory,
            });
            client.set_fill(self.settings.download_all && paths::can_download_all());
            client
        });
        if self.source == Source::Server {
            self.game_dir = None;
        }
        self.ui.repaint();
    }

    /// The sign over the game while it waits for the server.
    fn stream_alert(&mut self, stream: Option<&glue5_stream::Status>) -> Option<ui::StreamAlert> {
        let stream = stream.filter(|_| self.game.is_some() && self.source == Source::Server)?;
        if stream.waiting == 0 {
            self.stream_waiting = None;
        } else {
            self.stream_waiting.get_or_insert_with(Instant::now);
        }
        // A copy of the whole game never waits for the server.
        if !stream.connected && !stream.complete {
            Some(ui::StreamAlert::Offline)
        } else if self.stream_waiting.is_some_and(|t| t.elapsed() > Duration::from_secs(1)) {
            Some(ui::StreamAlert::Waiting)
        } else {
            None
        }
    }

    /// Where the stream's copy goes: the cache folder, or the device's game
    /// folder (a whole game, or one the stream is keeping up to date there).
    fn copy_root(&self) -> PathBuf {
        copy_root(paths::download_dir(&self.user_dir), self.settings.download_all, paths::stream_copy(&self.user_dir))
    }

    /// Deletes the stream's copy (the game does not run).
    fn clear_copy(&mut self) {
        if self.game.is_some() {
            return;
        }
        let root = self.stream.as_ref().map_or_else(|| self.copy_root(), |s| s.root().to_owned());
        if let Some(stream) = self.stream.take() {
            stream.stop();
        }
        // Started again (empty) at the next turn.
        self.stream_for = None;
        if self.settings.game_dir.as_deref() == Some(root.as_path()) {
            self.settings.game_dir = None;
        }
        // In the background (gigabytes), once the client has finished, and
        // before the next one starts on the folder.
        glue5_stream::after_stopped(move || {
            if let Err(e) = glue5_stream::cache::clear(&root) {
                log::warn!("stream: the copy could not be deleted: {e}");
            }
        });
        if self.source == Source::Server {
            self.game_dir = None;
        }
    }

    /// Looks for servers on the network, in the background.
    fn find_servers(&mut self) {
        let servers = self.servers.clone();
        {
            let mut s = servers.lock().unwrap();
            if s.0 {
                return;
            }
            s.0 = true;
        }
        let spawned = std::thread::Builder::new().name("find servers".into()).spawn(move || {
            let mut found = glue5_stream::discovery::find(Duration::from_secs(3));
            // In the same order at every search (the page searches all the
            // while), shown at once with what they answered last time; each
            // is then asked again whether it serves this game.
            found.sort_by(|a, b| (&a.name, &a.address).cmp(&(&b.name, &b.address)));
            {
                let mut s = servers.lock().unwrap();
                let last = |f: &glue5_stream::discovery::Found| s.1.iter().find(|(l, _)| *l == *f).and_then(|(_, c)| c.clone());
                let shown = found.iter().map(|f| (f.clone(), last(f))).collect();
                s.1 = shown;
            }
            let answers: Vec<_> = std::thread::scope(|scope| {
                let asking: Vec<_> = found
                    .iter()
                    .map(|f| scope.spawn(|| glue5_stream::check(&f.address, glue5_runtime::GAME_BUILD)))
                    .collect();
                asking.into_iter().map(|a| a.join().ok()).collect()
            });
            *servers.lock().unwrap() = (false, found.into_iter().zip(answers).collect());
        });
        if spawned.is_err() {
            self.servers.lock().unwrap().0 = false;
        }
        self.ui.repaint_in(Duration::from_millis(300));
    }

    /// Looks for the game: the folder named on the command line or by
    /// `GLUE5_GAME_DIR`, the last one used, the platform's default places,
    /// the whole game downloaded from a server (on desktop in the user
    /// directory, not among the default places).
    fn find_game(&mut self, explicit: Option<&Path>) {
        self.game_dir = None;
        self.problem = None;
        self.ui.repaint();
        let explicit = explicit.map(PathBuf::from).or_else(|| std::env::var_os("GLUE5_GAME_DIR").map(PathBuf::from));
        if let Some(dir) = explicit {
            if is_game_dir(&dir) {
                self.game_dir = paths::canonical(&dir).ok();
                return;
            }
            self.problem = Some(format!("{} is not a game folder.", dir.display()));
        }
        let candidates = (self.settings.game_dir.iter().cloned())
            .chain(paths::default_game_dirs())
            .chain(paths::download_dir(&self.user_dir));
        if let Some(dir) = candidates.filter(|d| is_game_dir(d)).find_map(|d| paths::canonical(&d).ok()) {
            paths::keep_out_of_backups(&dir);
            self.settings.game_dir = Some(dir.clone());
            self.game_dir = Some(dir);
        }
    }

    /// The folder picker's answer.
    fn chosen(&mut self, choice: Option<PathBuf>) {
        let Some(dir) = choice else { return };
        let Ok(dir) = paths::canonical(&dir) else { return };
        if !is_game_dir(&dir) {
            let problem = format!("{} is not a game folder.", dir.display());
            self.ui.note(problem.clone());
            if self.game.is_none() {
                self.problem = Some(problem);
            }
            return;
        }
        self.settings.game_dir = Some(dir.clone());
        self.settings.save(&self.user_dir);
        if self.game.is_none() {
            self.game_dir = Some(dir);
            self.problem = None;
        }
        self.ui.repaint();
    }
}

/// The folder a picker chose, as a game folder: itself, its `game`, or the
/// one of its own folders that has the game.  A player picks the folder
/// holding it (Downloads, an SD card's root) as often as the folder itself,
/// and one level down is cheap to look at against tens of gigabytes of
/// copying to the wrong place.
fn resolve_game_dir(dir: &Path) -> Option<PathBuf> {
    if is_game_dir(dir) {
        return Some(dir.to_owned());
    }
    let inner = dir.join("game");
    if is_game_dir(&inner) {
        return Some(inner);
    }
    std::fs::read_dir(dir)
        .ok()?
        .flatten()
        .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_dir()))
        .map(|entry| entry.path())
        .find(|below| is_game_dir(below))
}

/// The whole game downloaded from a server into `dir`: a copy (it has the
/// server's catalog) that is complete (it lost the partial copy's marker).
fn is_whole_download(dir: &Path) -> bool {
    dir.join(glue5_stream::cache::CATALOG).is_file() && !dir.join(glue5_stream::cache::MARKER).exists()
}

/// Where the stream's copy goes (see `App::copy_root`): the download folder
/// when it holds the whole game, downloaded, whatever the setting says now
/// (never moved into the cache, where the system may delete it), or when the
/// download is wanted and the folder is the stream's or free; else the cache.
/// A folder with the user's own files in it (a game for another build, a
/// copy not finished) is never the stream's: its files would be cut to the
/// catalog's sizes, and Delete would take them all.
fn copy_root(download: Option<PathBuf>, download_all: bool, cache: PathBuf) -> PathBuf {
    match download {
        Some(dir) if is_whole_download(&dir) => dir,
        // A copy the stream started here stays here while it follows the
        // server's list; the files that still match are kept.
        Some(dir) if dir.join(glue5_stream::cache::CATALOG).is_file() => dir,
        Some(dir)
            if download_all
                && (!dir.exists() || holds_no_files(&dir) || dir.join(glue5_stream::cache::CATALOG).is_file()) =>
        {
            dir
        }
        _ => cache,
    }
}

/// `dir` has folders in it at most, no file at any depth.
fn holds_no_files(dir: &Path) -> bool {
    std::fs::read_dir(dir).is_ok_and(|entries| {
        entries.flatten().all(|e| e.file_type().is_ok_and(|t| t.is_dir()) && holds_no_files(&e.path()))
    })
}

/// The next event, waiting up to `timeout` for one.
#[cfg(not(any(target_os = "ios", target_os = "tvos")))]
fn wait_event(events: &mut sdl3::EventPump, timeout: Duration) -> Option<Event> {
    events.wait_event_timeout(timeout)
}

/// On iOS and tvOS SDL cannot sleep until an event: its wait runs the run loop for
/// a moment every millisecond (a sixth of a core, all the time). The run
/// loop itself sleeps until a source fires (a touch, a controller, a timer)
/// or the time is up; SDL then has the events it queued.
#[cfg(any(target_os = "ios", target_os = "tvos"))]
fn wait_event(events: &mut sdl3::EventPump, timeout: Duration) -> Option<Event> {
    use std::ffi::c_void;
    unsafe extern "C" {
        static kCFRunLoopDefaultMode: *const c_void;
        fn CFRunLoopRunInMode(mode: *const c_void, seconds: f64, return_after_source_handled: u8) -> i32;
    }
    if let Some(event) = events.poll_event() {
        return Some(event);
    }
    unsafe { CFRunLoopRunInMode(kCFRunLoopDefaultMode, timeout.as_secs_f64(), 1) };
    events.poll_event()
}

/// The refresh rate of the window's display (60 if unknown).
/// Asks Android to refresh the window's surface at the frame cap.
#[cfg(target_os = "android")]
fn ask_frame_rate(window: &sdl3::video::Window, fps: u32) {
    use sdl3::sys::properties::SDL_GetPointerProperty;
    use sdl3::sys::video::{SDL_GetWindowProperties, SDL_PROP_WINDOW_ANDROID_WINDOW_POINTER};
    let native = unsafe {
        SDL_GetPointerProperty(SDL_GetWindowProperties(window.raw()), SDL_PROP_WINDOW_ANDROID_WINDOW_POINTER, std::ptr::null_mut())
    };
    glue5_runtime::qos::ask_frame_rate(native, fps);
}

fn window_refresh_rate(window: &sdl3::video::Window) -> f32 {
    let rate = window.get_display().ok().and_then(|d| d.get_mode().ok()).map_or(0.0, |m| m.refresh_rate);
    if rate > 0.0 { rate } else { 60.0 }
}

/// The system's preferred language as a tag like `it-IT`, for the game
/// (phones have no `$LANG`).
fn preferred_language() -> Option<String> {
    use std::ffi::CStr;
    let mut count = 0;
    let locales = unsafe { sdl3::sys::locale::SDL_GetPreferredLocales(&mut count) };
    if locales.is_null() {
        return None;
    }
    let first = unsafe { (count > 0).then(|| &**locales) };
    let tag = first.filter(|l| !l.language.is_null()).map(|l| {
        let language = unsafe { CStr::from_ptr(l.language) }.to_string_lossy();
        match l.country.is_null() {
            true => language.into_owned(),
            false => format!("{language}-{}", unsafe { CStr::from_ptr(l.country) }.to_string_lossy()),
        }
    });
    unsafe { sdl3::sys::stdinc::SDL_free(locales.cast()) };
    tag
}

fn upscaler(settings: &Settings) -> Upscaler {
    settings.upscaler.upscaler()
}

/// The upscalers, as the settings offer them.
const UPSCALERS: &[Upscaling] = &[Upscaling::Bicubic, Upscaling::MetalFx, Upscaling::MetalFxTemporal, Upscaling::Fsr];

/// Keyboard and mouse into the game's input block.
fn game_input(
    input: &InputBlock,
    event: &Event,
    window: &sdl3::video::Window,
    back_buffer: (u32, u32),
) {
    let (bw, bh) = (back_buffer.0 as f32, back_buffer.1 as f32);
    match *event {
        Event::KeyDown { scancode: Some(sc), repeat, .. } => {
            if !repeat {
                key(input, sc, true);
            }
            match sc {
                Scancode::Return | Scancode::KpEnter => input.text("\r"),
                Scancode::Backspace => input.text("\u{8}"),
                _ => {}
            }
        }
        Event::KeyUp { scancode: Some(sc), .. } => key(input, sc, false),
        Event::TextInput { ref text, .. } => input.text(text),
        Event::MouseMotion { x, y, xrel, yrel, .. } => {
            let (ww, wh) = window.size();
            input.mouse_position((x * bw / ww as f32) as i32, (y * bh / wh as f32) as i32);
            input.mouse_motion(xrel.round() as i32, yrel.round() as i32);
        }
        Event::MouseButtonDown { mouse_btn, .. } => input.mouse_button(mouse_bit(mouse_btn), true),
        Event::MouseButtonUp { mouse_btn, .. } => input.mouse_button(mouse_bit(mouse_btn), false),
        Event::MouseWheel { y, .. } => input.mouse_wheel(y.round() as i32),
        Event::Window { win_event: WindowEvent::FocusGained, .. } => input.focus(true),
        Event::Window { win_event: WindowEvent::FocusLost, .. } => input.focus(false),
        _ => {}
    }
}

/// Starts the application again, straight into the game with `play`, else on
/// its start page (desktop; a phone app cannot, and the player reopens it).
fn relaunch(play: bool) {
    if MOBILE {
        return;
    }
    let args: Vec<_> = std::env::args_os().skip(1).filter(|a| a != "--play").collect();
    let started = std::env::current_exe().and_then(|exe| {
        let mut command = std::process::Command::new(exe);
        if play {
            command.arg("--play");
        }
        command.args(args).spawn()
    });
    if let Err(e) = started {
        log::warn!("cannot start again: {e}");
    }
}

fn key(input: &InputBlock, sc: sdl3::keyboard::Scancode, down: bool) {
    if let Some(vk) = keymap::vk(sc) {
        input.key(vk, down);
        if let Some(generic) = keymap::generic(vk) {
            input.key(generic, down);
        }
    }
    if let Some(alias) = keymap::numpad_alias(sc) {
        input.key(alias, down);
    }
}

fn mouse_bit(b: MouseButton) -> u32 {
    match b {
        MouseButton::Left => mouse::LEFT,
        MouseButton::Right => mouse::RIGHT,
        MouseButton::Middle => mouse::MIDDLE,
        MouseButton::X1 => mouse::X1,
        MouseButton::X2 => mouse::X2,
        _ => 0,
    }
}

/// The system's folder picker, answered asynchronously (SDL calls back on
/// the main thread while events are pumped).
struct FolderPick(Arc<Mutex<Option<Option<PathBuf>>>>);

impl FolderPick {
    fn start(window: &sdl3::video::Window) -> FolderPick {
        let picked: Arc<Mutex<Option<Option<PathBuf>>>> = Default::default();
        let slot = picked.clone();
        sdl3::dialog::show_open_folder_dialog(
            None::<&str>,
            false,
            window,
            Box::new(move |result, _| *slot.lock().unwrap() = Some(result.ok().and_then(|mut v| v.pop()))),
        );
        FolderPick(picked)
    }

    /// `Some(choice)` once answered (`Some(None)`: cancelled).
    fn poll(&self) -> Option<Option<PathBuf>> {
        self.0.lock().unwrap().take()
    }
}

#[cfg(test)]
mod tests {
    use super::{copy_root, holds_no_files, is_whole_download, resolve_game_dir};
    use glue5_stream::cache::{CATALOG, MARKER};

    /// The folder a picker chose: itself, its `game`, or the one of its own
    /// folders that has the game (a player picks the folder holding it as
    /// often as the folder itself).
    #[test]
    fn a_picked_folder_resolves_to_the_game() {
        let root = std::env::temp_dir().join(format!("glue5-app-test-pick-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let game = root.join("game");
        // This build's layout, as `is_game_dir` wants it.
        let shaders = glue5_runtime::layout::shader_dir(&game);
        std::fs::create_dir_all(&shaders).unwrap();
        std::fs::create_dir_all(game.join("data")).unwrap();
        assert_eq!(resolve_game_dir(&root), None, "no shaders index yet");
        std::fs::write(shaders.join("index.json"), b"{}").unwrap();
        assert_eq!(resolve_game_dir(&game).as_deref(), Some(game.as_path()), "the folder itself");
        assert_eq!(resolve_game_dir(&root).as_deref(), Some(game.as_path()), "the folder holding it");
        // A stream's partial copy is not a game folder, wherever it sits.
        std::fs::write(game.join(MARKER), b"").unwrap();
        assert_eq!(resolve_game_dir(&root), None, "a partial copy");
        let _ = std::fs::remove_dir_all(&root);
    }

    /// What a copy's move may delete (folders with no file), and what it
    /// never moves (the whole game, downloaded).
    #[test]
    fn empty_folders_and_whole_downloads() {
        let root = std::env::temp_dir().join(format!("glue5-app-test-{}", std::process::id()));
        let game = root.join("game");
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(game.join("data/x64")).unwrap();
        assert!(holds_no_files(&game));
        std::fs::write(game.join("data/x64/a.rpf"), b"x").unwrap();
        assert!(!holds_no_files(&game) && !is_whole_download(&game));
        std::fs::write(game.join(CATALOG), b"{}").unwrap();
        assert!(is_whole_download(&game));
        std::fs::write(game.join(MARKER), b"").unwrap();
        assert!(!is_whole_download(&game), "a partial copy");
        let _ = std::fs::remove_dir_all(&root);
    }

    /// The whole game downloads into the device's game folder only when the
    /// folder is free or already the stream's: never over the user's files.
    #[test]
    fn the_download_takes_no_folder_of_the_users() {
        let root = std::env::temp_dir().join(format!("glue5-app-test-root-{}", std::process::id()));
        let (game, cache) = (root.join("game"), root.join("cache"));
        let _ = std::fs::remove_dir_all(&root);
        let at = |all| copy_root(Some(game.clone()), all, cache.clone());
        assert_eq!(at(true), game, "no folder yet");
        assert_eq!(at(false), cache, "not wanted");
        std::fs::create_dir_all(game.join("data/x64")).unwrap();
        assert_eq!(at(true), game, "empty folders");
        // The user's game for another build (or a copy not finished): no
        // shaders for this build, so not a game folder here, but theirs.
        std::fs::write(game.join("data/x64/a.rpf"), b"x").unwrap();
        assert_eq!(at(true), cache, "the user's files");
        std::fs::write(game.join(CATALOG), b"{}").unwrap();
        std::fs::write(game.join(MARKER), b"").unwrap();
        assert_eq!(at(true), game, "the download on its way");
        assert_eq!(at(false), game, "the copy kept up to date");
        std::fs::remove_file(game.join(MARKER)).unwrap();
        assert_eq!(at(false), game, "the whole game, downloaded");
        let _ = std::fs::remove_dir_all(&root);
    }
}
