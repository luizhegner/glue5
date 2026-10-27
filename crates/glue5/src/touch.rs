//! Touch controls.
//!
//! - On foot and driving, controls made for touch, a layout for each: a
//!   stick where the left thumb lands (on foot, pushed past its ring, the
//!   player sprints), or, in a car, two arrows chosen in the settings, the
//!   camera turned by dragging anywhere else on the right, as a mouse turns
//!   it, and buttons named for what they do, the Enter button only next to
//!   a vehicle. Aim and the right-side Fire button turn the camera while
//!   held; Auto Fire also aims with a gun, Manual shows a left Fire button
//!   while Aim is held, and double-tapping Aim keeps aiming. The wheels
//!   (weapons, characters, radio) open while their button is held and pick by
//!   dragging from it, or on the right. The buttons sit near the thumbs, off
//!   the game's help text and radar; the camera view, the pause menu and the
//!   app's settings at the top's middle.
//! - What the game asks for besides (its instructional buttons, the inputs
//!   its help text names: [`glue5_runtime::prompts`]) is pressed by the
//!   button that does it, ringed while asked (shown for it if the layout
//!   keeps it hidden then, as Enter away from a vehicle we saw), or by a
//!   button of its own: the action's icon where the input says what it does
//!   (the camera's focus, the headlights), else the pad's symbol the game
//!   shows beside its words (the scripts' own inputs).
//! - A script's menu or scene that shows its buttons' bar (a mission's
//!   end, a minigame) has the D-pad, Select and Back, and the game's
//!   buttons; drags are the sticks. One that asks for nothing (a death, a
//!   switch, a scene) has only pause, and what it asks for when it does
//!   (its skip). With the phone out, its keys sit beside it and only the
//!   controls it leaves free stay. Cutscenes have their skip while it is
//!   taken.
//! - The menus made for the mouse (the pause menu, and the scripts' that
//!   show its cursor: shops, the interaction menu, the character creator;
//!   the screens with a pointer of their own: the browser, the hacking
//!   screens) take fingers as on a phone's own screens: a tap clicks where
//!   it lands, a drag up or down scrolls, left or right changes the value
//!   of the row picked (its arrows take no click), on the map it moves it,
//!   two fingers zoom; the D-pad, Enter and Back keys stay beside them
//!   (Back alone on a pointer's screen). Minigames keep the pad (those that
//!   load keyboard and mouse controls of their own change with them).
//!
//! They make a [`Gamepad`] (and the camera a mouse's motion): the game plays
//! as with a controller (its prompts, aim assist).

use std::collections::HashMap;
use std::time::{Duration, Instant};

use egui::{Align2, Color32, FontId, Painter, Pos2, Rect, Stroke, Vec2, pos2, vec2};
use glue5_runtime::input::{Activity, Character, Driving, Gamepad, PlayerContext, button};
use glue5_runtime::prompts::{Pad, Prompt, SKIP_CUTSCENE};

use crate::haptics;
use crate::settings::{TouchFire, TouchLayout, TouchSteer};

/// Stick travel, in points (scaled with the controls).
const STICK_RADIUS: f32 = 56.0;
/// Phones are held closer: their controls are this much smaller.
const PHONE_SCALE: f32 = 0.82;
/// Below this many points on its short side, a screen is a phone's.
const PHONE_SIDE: f32 = 500.0;

/// A position in the safe area, in points: from its left or right edge, or
/// from its middle (x), and from its top or bottom edge (y).
#[derive(Clone, Copy, Debug)]
enum At {
    LeftTop(f32, f32),
    LeftBottom(f32, f32),
    RightBottom(f32, f32),
    MidTop(f32, f32),
}

/// Where the stick rests (drawn there until a thumb takes it): right of the
/// game's radar.
const MOVE_REST: At = At::LeftBottom(240.0, 104.0);

/// A button of the touch layouts, named for what it does: it presses the pad
/// buttons the game maps that to (common/data/control/standard*.meta), or
/// the game's input itself.
#[derive(Clone, Copy, Debug)]
struct Action {
    /// What it does (in English; the tests find buttons by it).
    #[cfg_attr(not(test), allow(dead_code))]
    label: &'static str,
    /// Its icon's name, or two names joined by `+` (side by side).
    icon: &'static str,
    bits: u16,
    /// The game's input it does on foot: it presses the pad button the
    /// game maps that to now (first person and the other control schemes
    /// move some), `bits` while it is not known.
    does: Option<u32>,
    /// The game's input it presses instead of pad buttons.
    input: Option<u32>,
    at: At,
    radius: f32,
    kind: Kind,
    /// Shown only next to a vehicle the player can get into.
    near_vehicle: bool,
    /// The layout's main button: ringed in the player's colour.
    primary: bool,
    /// Kept with the phone out (it presses nothing the phone uses).
    with_phone: bool,
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Kind {
    Press,
    /// Held, the finger's drag also turns the camera.
    Turns,
    /// Held, it steers left or right by which side of the arrows' middle
    /// the finger is on, so a thumb slides from one to the other.
    Steer,
    /// Held, it opens one of the game's wheels (weapons, characters, radio
    /// stations): the finger's drag picks in it, as the right stick does.
    Wheel,
    /// Held, it opens the layout's secondary actions in a fan: the finger's
    /// drag picks one, and its finger lifts on the one pressed.
    More,
}

const fn action(label: &'static str, icon: &'static str, bits: u16, at: At, radius: f32) -> Action {
    Action {
        label,
        icon,
        bits,
        does: None,
        input: None,
        at,
        radius,
        kind: Kind::Press,
        near_vehicle: false,
        primary: false,
        with_phone: false,
    }
}

const fn does(input: u32, a: Action) -> Action {
    Action { does: Some(input), ..a }
}

const fn turns(a: Action) -> Action {
    Action { kind: Kind::Turns, ..a }
}

const fn wheel(a: Action) -> Action {
    Action { kind: Kind::Wheel, ..a }
}

const fn near_vehicle(a: Action) -> Action {
    Action { near_vehicle: true, ..a }
}

const fn primary(a: Action) -> Action {
    Action { primary: true, ..a }
}

const fn with_phone(a: Action) -> Action {
    Action { with_phone: true, ..a }
}

/// The button that opens the secondary actions of the Modern layouts.
const fn more(at: At, radius: f32) -> Action {
    Action {
        label: "More",
        icon: "more",
        bits: 0,
        does: None,
        input: None,
        at,
        radius,
        kind: Kind::More,
        near_vehicle: false,
        primary: false,
        with_phone: false,
    }
}

/// The buttons' icons, from several sets (their licenses in
/// assets/touch/README.md), white on transparent, square, all of the same
/// optical size.
macro_rules! icons {
    ($($name:literal),* $(,)?) => {
        &[$(($name, include_bytes!(concat!("../assets/touch/", $name, ".png")))),*]
    };
}
const ICON_FILES: &[(&str, &[u8])] = icons![
    "action", "aim", "back", "behind", "bell", "brake", "check", "cinematic", "cover", "crouch", "detach", "dive",
    "down", "drive-special", "duck", "enter", "exit", "fire", "gas", "handbrake", "horn", "jump", "landing-gear",
    "left", "lights", "melee", "more", "pad-a", "pad-b", "pad-lb", "pad-ls", "pad-lt", "pad-rb", "pad-rs", "pad-rt",
    "pad-x", "pad-y", "parachute", "pause", "phone", "radio", "reload", "right", "rise", "seat", "settings", "skip",
    "special", "switch", "throw", "up", "view", "weapons", "yaw-left", "yaw-right",
];

/// The icons' textures, by name.
type Icons = HashMap<&'static str, egui::TextureHandle>;

fn load_icons(ctx: &egui::Context) -> Icons {
    ICON_FILES.iter().filter_map(|(name, _)| load_icon(ctx, name).map(|t| (*name, t))).collect()
}

/// The icon `name`'s texture (the settings button's too).
pub fn load_icon(ctx: &egui::Context, name: &str) -> Option<egui::TextureHandle> {
    let (_, bytes) = ICON_FILES.iter().find(|(n, _)| *n == name)?;
    let decode = || -> Result<egui::ColorImage, png::DecodingError> {
        let mut reader = png::Decoder::new(std::io::Cursor::new(bytes)).read_info()?;
        let mut pixels = vec![0; reader.output_buffer_size().unwrap_or(0)];
        let info = reader.next_frame(&mut pixels)?;
        let size = [info.width as usize, info.height as usize];
        Ok(egui::ColorImage::from_rgba_unmultiplied(size, &pixels[..info.buffer_size()]))
    };
    match decode() {
        Ok(image) => Some(ctx.load_texture(format!("touch-{name}"), image, egui::TextureOptions::LINEAR)),
        Err(e) => {
            log::warn!("touch icon {name}: {e}");
            None
        }
    }
}

// In every layout: the camera view and pause at the top's middle (the
// app's settings button beside them, ui.rs).
const VIEW: Action = action("View", "view", button::SELECT, At::MidTop(-54.0, 30.0), 20.0);
const PAUSE: Action = action("☰", "pause", button::START, At::MidTop(0.0, 30.0), 20.0);

/// The phone, on the right below the game's HUD.
const PHONE: Action = action("Phone", "phone", button::UP, At::RightBottom(46.0, 334.0), 22.0);
/// Two columns on the left, between the game's help text and its radar.
const LEFT_1: At = At::LeftBottom(46.0, 318.0);
const LEFT_2: At = At::LeftBottom(46.0, 258.0);
const LEFT_3: At = At::LeftBottom(46.0, 202.0);
const LEFT_4: At = At::LeftBottom(104.0, 288.0);
const LEFT_5: At = At::LeftBottom(104.0, 232.0);
static STEER_LEFT: Action =
    Action { kind: Kind::Steer, ..with_phone(action("◀", "left", 0, At::LeftBottom(100.0, 96.0), 34.0)) };
static STEER_RIGHT: Action =
    Action { kind: Kind::Steer, ..with_phone(action("▶", "right", 0, At::LeftBottom(196.0, 96.0), 34.0)) };
static LEFT_FIRE: Action = does(24, action("Fire", "fire", button::R2, At::LeftBottom(240.0, 250.0), 30.0));
/// The character wheel, in every layout but a cutscene's.
const SWITCH: Action = wheel(action("Switch", "switch", button::DOWN, LEFT_4, 22.0));

/// The main buttons of the layouts in vehicles sit here: the right thumb's
/// throttle and brake, then around them.
const MAIN: At = At::RightBottom(78.0, 150.0);
const SECOND: At = At::RightBottom(184.0, 80.0);
const THIRD: At = At::RightBottom(190.0, 196.0);
const FOURTH: At = At::RightBottom(70.0, 262.0);
const FIFTH: At = At::RightBottom(280.0, 50.0);
const SIXTH: At = At::RightBottom(282.0, 128.0);

// Actions that the Modern layouts keep off the screen until More is held.
// The positions are the Classic ones (the fan places them itself).
const SPECIAL_ON_FOOT: Action = action("Special", "special", button::L3 | button::R3, LEFT_2, 22.0);
const CROUCH: Action = does(36, action("Crouch", "crouch", button::L3, LEFT_3, 22.0));
const THROW: Action = does(58, action("Throw", "throw", button::LEFT, LEFT_5, 22.0));
const HORN: Action = action("Horn", "horn", button::L3, FIFTH, 22.0);
const LIGHTS: Action = action("Lights", "lights", button::RIGHT, LEFT_2, 22.0);
const BEHIND: Action = action("Behind", "behind", button::R3, LEFT_3, 22.0);
const DRIVE_SPECIAL: Action = action("Special", "drive-special", button::L3 | button::R3, LEFT_5, 22.0);
const BELL: Action = action("Bell", "bell", button::L3, FIFTH, 22.0);

/// More sits where the left thumb can reach it without leaving the stick's
/// neighbourhood; the fan opens to the right, over the free part of the HUD.
const MORE_ON_FOOT: Action = more(LEFT_3, 22.0);
const MORE_CAR: Action = more(LEFT_2, 24.0);

/// The secondary actions of the Modern on-foot layout: rare enough to keep
/// off the screen, but one drag away.
static ON_FOOT_MORE: &[Action] = &[SPECIAL_ON_FOOT, CROUCH, THROW];
/// The secondary actions of the Modern car layout.
static CAR_MORE: &[Action] = &[HORN, LIGHTS, BEHIND, DRIVE_SPECIAL];

/// On foot (the stick sprints past its ring).
static ON_FOOT: &[Action] = &[
    does(24, primary(turns(action("Fire", "fire", button::R2, At::RightBottom(78.0, 166.0), 34.0)))),
    does(25, turns(action("Aim", "aim", button::L2, At::RightBottom(176.0, 104.0), 30.0))),
    does(22, action("Jump", "jump", button::WEST, At::RightBottom(64.0, 64.0), 28.0)),
    does(23, near_vehicle(action("Enter", "enter", button::NORTH, At::RightBottom(180.0, 214.0), 28.0))),
    action("Melee\nReload", "melee", button::EAST, At::RightBottom(268.0, 52.0), 24.0),
    does(44, action("Cover", "cover", button::R1, At::RightBottom(78.0, 262.0), 24.0)),
    does(51, action("Action", "action", button::RIGHT, At::RightBottom(274.0, 136.0), 24.0)),
    PHONE,
    does(37, wheel(action("Weapons", "weapons", button::L1, LEFT_1, 24.0))),
    action("Special", "special", button::L3 | button::R3, LEFT_2, 22.0),
    does(36, action("Crouch", "crouch", button::L3, LEFT_3, 22.0)),
    SWITCH,
    does(58, action("Throw", "throw", button::LEFT, LEFT_5, 22.0)),
    VIEW,
    PAUSE,
];

/// The Modern on-foot layout: the actions used while moving, aiming,
/// shooting or interacting stay on screen; the rare ones wait on More.
/// Weapons and the character wheel sit closer to the left thumb than in the
/// Classic layout.
static MODERN_ON_FOOT: &[Action] = &[
    does(24, primary(turns(action("Fire", "fire", button::R2, At::RightBottom(78.0, 166.0), 34.0)))),
    does(25, turns(action("Aim", "aim", button::L2, At::RightBottom(176.0, 104.0), 30.0))),
    does(22, action("Jump", "jump", button::WEST, At::RightBottom(64.0, 64.0), 28.0)),
    does(23, near_vehicle(action("Enter", "enter", button::NORTH, At::RightBottom(180.0, 214.0), 28.0))),
    action("Melee\nReload", "melee", button::EAST, At::RightBottom(268.0, 52.0), 24.0),
    does(44, action("Cover", "cover", button::R1, At::RightBottom(78.0, 262.0), 24.0)),
    does(51, action("Action", "action", button::RIGHT, At::RightBottom(274.0, 136.0), 24.0)),
    PHONE,
    does(37, wheel(action("Weapons", "weapons", button::L1, LEFT_5, 24.0))),
    SWITCH,
    MORE_ON_FOOT,
    VIEW,
    PAUSE,
];

/// Cars, motorbikes, quads, boats.
static CAR: &[Action] = &[
    primary(with_phone(action("Gas", "gas", button::R2, MAIN, 36.0))),
    with_phone(action("Brake", "brake", button::L2, SECOND, 32.0)),
    action("Exit", "exit", button::NORTH, THIRD, 26.0),
    with_phone(action("H.brake", "handbrake", button::R1, FOURTH, 26.0)),
    action("Horn", "horn", button::L3, FIFTH, 22.0),
    // Drive-by: LB aims, RB fires.
    action("Shoot", "fire", button::L1 | button::R1, SIXTH, 22.0),
    PHONE,
    wheel(action("Radio", "radio", button::LEFT, LEFT_1, 24.0)),
    action("Lights", "lights", button::RIGHT, LEFT_2, 22.0),
    action("Behind", "behind", button::R3, LEFT_3, 22.0),
    SWITCH,
    // Franklin's, at the wheel.
    DRIVE_SPECIAL,
    VIEW,
    PAUSE,
];

/// The Modern car layout: driving, getting out, drive-by fire and the radio
/// stay on screen; horn, lights, looking behind and the driving special wait
/// on More.
static MODERN_CAR: &[Action] = &[
    primary(with_phone(action("Gas", "gas", button::R2, MAIN, 36.0))),
    with_phone(action("Brake", "brake", button::L2, SECOND, 32.0)),
    action("Exit", "exit", button::NORTH, THIRD, 26.0),
    with_phone(action("H.brake", "handbrake", button::R1, FOURTH, 26.0)),
    // Drive-by: LB aims, RB fires.
    action("Shoot", "fire", button::L1 | button::R1, SIXTH, 22.0),
    PHONE,
    wheel(action("Radio", "radio", button::LEFT, LEFT_1, 24.0)),
    SWITCH,
    MORE_CAR,
    VIEW,
    PAUSE,
];

static BICYCLE: &[Action] = &[
    primary(action("Pedal", "gas", button::SOUTH, MAIN, 36.0)),
    with_phone(action("Brake", "brake", button::L2, SECOND, 32.0)),
    action("Exit", "exit", button::NORTH, THIRD, 26.0),
    BELL,
    PHONE,
    action("Behind", "behind", button::R3, LEFT_3, 22.0),
    SWITCH,
    VIEW,
    PAUSE,
];

/// Helicopters (the stick pitches and rolls).
static HELI: &[Action] = &[
    primary(with_phone(action("Up", "gas", button::R2, MAIN, 34.0))),
    with_phone(action("Down", "brake", button::L2, SECOND, 30.0)),
    action("Exit", "exit", button::NORTH, FIFTH, 24.0),
    with_phone(action("◀ Yaw", "yaw-left", button::L1, At::RightBottom(160.0, 250.0), 24.0)),
    with_phone(action("Yaw ▶", "yaw-right", button::R1, FOURTH, 24.0)),
    action("Fire", "fire", button::SOUTH, SIXTH, 24.0),
    PHONE,
    SWITCH,
    VIEW,
    PAUSE,
];

static PLANE: &[Action] = &[
    primary(with_phone(action("Throttle", "gas", button::R2, MAIN, 34.0))),
    with_phone(action("Brake", "brake", button::L2, SECOND, 30.0)),
    action("Exit", "exit", button::NORTH, FIFTH, 24.0),
    with_phone(action("◀ Yaw", "yaw-left", button::L1, At::RightBottom(160.0, 250.0), 24.0)),
    with_phone(action("Yaw ▶", "yaw-right", button::R1, FOURTH, 24.0)),
    action("Fire", "fire", button::SOUTH, SIXTH, 24.0),
    PHONE,
    action("Gear", "landing-gear", button::L3, LEFT_3, 22.0),
    SWITCH,
    VIEW,
    PAUSE,
];

static SUBMARINE: &[Action] = &[
    primary(with_phone(action("Ahead", "gas", button::R2, MAIN, 34.0))),
    with_phone(action("Back", "brake", button::L2, SECOND, 30.0)),
    action("Rise", "rise", button::WEST, SIXTH, 24.0),
    action("Dive", "dive", button::SOUTH, FIFTH, 24.0),
    action("Exit", "exit", button::NORTH, At::RightBottom(160.0, 250.0), 24.0),
    PHONE,
    SWITCH,
    VIEW,
    PAUSE,
];

/// A passenger: shooting from the window (LT aims, RT fires).
static PASSENGER: &[Action] = &[
    primary(turns(action("Shoot", "fire", button::R2, MAIN, 34.0))),
    turns(action("Aim", "aim", button::L2, SECOND, 30.0)),
    action("Exit", "exit", button::NORTH, THIRD, 26.0),
    // The driver's seat, when free.
    action("Seat", "seat", button::RIGHT, FIFTH, 22.0),
    PHONE,
    wheel(action("Radio", "radio", button::LEFT, LEFT_1, 24.0)),
    action("Behind", "behind", button::R3, LEFT_3, 22.0),
    SWITCH,
    VIEW,
    PAUSE,
];

/// Skydiving, then under the parachute (the stick steers).
static PARACHUTE: &[Action] = &[
    primary(action("Open", "parachute", button::SOUTH, MAIN, 34.0)),
    action("Detach", "detach", button::NORTH, THIRD, 26.0),
    action("◀ Brake", "yaw-left", button::L1, At::RightBottom(160.0, 250.0), 24.0),
    action("Brake ▶", "yaw-right", button::R1, FOURTH, 24.0),
    PHONE,
    SWITCH,
    VIEW,
    PAUSE,
];

/// A cutscene: its skip, the game's input itself.
static CUTSCENE: &[Action] =
    &[primary(Action { input: Some(SKIP_CUTSCENE), ..action("Skip", "skip", 0, MAIN, 30.0) }), PAUSE];

/// A screen of the engine's own (a warning: join another session? quit?;
/// a loading screen; the screen faded out): no button of play's, the ones
/// it asks for alone ([`Touch::prompt_buttons`]).
static ASKED: &[Action] = &[];

/// A warning of the engine's whose buttons are not known.
static ALERT: &[Action] = &[
    primary(action("Select", "check", button::SOUTH, At::RightBottom(70.0, 130.0), 32.0)),
    action("Back", "back", button::EAST, At::RightBottom(150.0, 50.0), 24.0),
];

/// A script's scene that asks for nothing (a death, a character's switch, a
/// scene the player watches): pause alone; what it asks for when it does
/// has buttons of its own (a skip), and the sticks where it asks for them.
static SCENE: &[Action] = &[PAUSE];

/// A script's menu or scene with its buttons' bar: the D-pad and Select
/// under the right thumb (the game's menus are on the left), Back beside
/// them; the drags are the sticks.
static SCRIPTED: &[Action] = &[
    action("▲", "up", button::UP, At::RightBottom(236.0, 190.0), 24.0),
    action("▼", "down", button::DOWN, At::RightBottom(236.0, 86.0), 24.0),
    action("◀", "left", button::LEFT, At::RightBottom(288.0, 138.0), 24.0),
    action("▶", "right", button::RIGHT, At::RightBottom(184.0, 138.0), 24.0),
    primary(action("Select", "check", button::SOUTH, At::RightBottom(70.0, 130.0), 32.0)),
    action("Back", "back", button::EAST, At::RightBottom(150.0, 50.0), 24.0),
    VIEW,
    PAUSE,
];

/// While the phone is out, its keys (the D-pad, Select, Back) beside it
/// ([`Touch::phone_keys`]); the layout's buttons the phone uses give way.
static PHONE_KEYS: [Action; 6] = [
    action("▲", "up", button::UP, MAIN, 22.0),
    action("▼", "down", button::DOWN, MAIN, 22.0),
    action("◀", "left", button::LEFT, MAIN, 22.0),
    action("▶", "right", button::RIGHT, MAIN, 22.0),
    primary(action("Select", "check", button::SOUTH, MAIN, 26.0)),
    action("Back", "back", button::EAST, MAIN, 22.0),
];

/// In the pause menu, a finger is the mouse (tap: a click, drag: the map
/// moves); two fingers turn the wheel (pinched: the map zooms, dragged
/// together: lists scroll); these buttons are keys: Back (Esc) at the top
/// left, and right of the menu the arrows (left and right change the value
/// of the row picked: its own arrows are as small as the text) and Enter.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MenuKey {
    Back,
    Left,
    Right,
    Up,
    Down,
    Enter,
}
const MENU_KEYS: [(MenuKey, Action); 6] = [
    (MenuKey::Back, action("Back", "back", 0, At::LeftTop(56.0, 46.0), 24.0)),
    (MenuKey::Left, action("◀", "left", 0, At::RightBottom(116.0, 150.0), 20.0)),
    (MenuKey::Right, action("▶", "right", 0, At::RightBottom(28.0, 150.0), 20.0)),
    (MenuKey::Up, action("▲", "up", 0, At::RightBottom(72.0, 194.0), 20.0)),
    (MenuKey::Down, action("▼", "down", 0, At::RightBottom(72.0, 106.0), 20.0)),
    (MenuKey::Enter, action("Enter", "check", 0, At::RightBottom(72.0, 46.0), 22.0)),
];
/// A pinch this many points wider or narrower turns the wheel a notch.
const PINCH_NOTCH: f32 = 40.0;
/// Two fingers dragged together this many points turn the wheel a notch (a
/// list's line, about).
const SCROLL_NOTCH: f32 = 28.0;
/// Two fingers pinch or scroll by whichever they do more of in their first
/// this many points.
const GESTURE_CLEAR: f32 = 10.0;
/// A tap's click or key lasts this long at least: the engine reads the
/// mouse and keys once a frame.
const MIN_PRESS: Duration = Duration::from_millis(60);
/// A finger on a menu that moves this many points is a drag, not a tap.
const TAP_SLOP: f32 = 10.0;
/// Two taps on Aim this close together keep it held until tapped again.
const DOUBLE_TAP: Duration = Duration::from_millis(350);
/// A sideways drag this many points long presses Left or Right once.
const SWIPE_STEP: f32 = 48.0;
/// A click starts this long after the pointer moves under the finger: the
/// menus take a press only where the pointer already was (the first tap's
/// was lost).
const CLICK_DELAY: Duration = Duration::from_millis(40);

/// A tap's second click waits this long after the first went into a
/// column (the menu takes no click on it meanwhile).
const AGAIN_DELAY: Duration = Duration::from_millis(250);

/// The buttons of the game's prompts that no layout's button does.
const PROMPT_RADIUS: f32 = 22.0;

/// An open wheel's pick: a drag this many points long points the stick.
const WHEEL_PICK: f32 = 12.0;

/// How far from the More button its fan opens, and the middle where a thumb
/// still means no action (a fan of icons is easy to open without picking one).
const RADIAL_RADIUS: f32 = 92.0;
const RADIAL_DEADZONE: f32 = 18.0;

/// The game's sprint (`INPUT_SPRINT`): A, but L3 in first person.
const SPRINT: u32 = 21;

/// Past this many stick radii from where it landed, the thumb sprints (on
/// foot); past this many, the stick follows it.
const SPRINT_REACH: f32 = 1.3;
const FOLLOW_REACH: f32 = 1.7;
/// The mouse counts a finger dragged one point turns the camera by, at
/// camera speed 1 (the game's mouse sensitivity setting applies on top).
const LOOK_GAIN: f32 = 16.0;
/// While the aim is locked on, a finger dragged this many points in a frame
/// pushes the right stick all the way (a swipe switches targets; a thumb's
/// tremor stays in the game's dead zone). A mouse's motion would break the
/// lock instead.
const LOCKED_SWIPE: f32 = 8.0;
/// A swipe on the game's phone steps its selection every this many points.
const PHONE_STEP: f32 = 30.0;

/// A button shown now: a layout's, a phone key, or one of the game's
/// prompts.
#[derive(Clone, Debug)]
struct Button {
    key: Key,
    at: Pos2,
    /// Its radius, in points.
    radius: f32,
    icon: &'static str,
    bits: u16,
    input: Option<u32>,
    kind: Kind,
    primary: bool,
    /// The game asks for it now.
    asked: bool,
}

/// What a button is, so that a finger keeps it across frames.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Key {
    Action(*const Action),
    Phone(usize),
    Prompt { bits: u16, input: Option<u32> },
}

#[derive(Clone, Copy, Debug)]
enum Finger {
    /// A stick held: where the thumb landed and where it is.
    Move { origin: Pos2, at: Pos2 },
    /// The camera, turned by the finger's drag: where it landed (or was
    /// when a wheel opened), where it was last. While a wheel is open, it
    /// picks in it instead, as the right stick.
    Camera { origin: Pos2, last: Pos2 },
    /// The right stick held (a script's scene, a prompt for it).
    Look { origin: Pos2, at: Pos2 },
    /// A button held: what it presses, where the finger landed, where it
    /// was last.
    Button { key: Key, bits: u16, input: Option<u32>, kind: Kind, origin: Pos2, last: Pos2 },
    /// The More button held: where the fan opened and where the thumb is.
    Radial { origin: Pos2, at: Pos2 },
    /// In the pause menu: the mouse (where the finger landed and is, when,
    /// what its drag does), or a second finger pinching, or a key.
    Mouse { origin: Pos2, at: Pos2, since: Instant, drag: Option<Drag> },
    Pinch(Pos2),
    MenuKey(MenuKey),
    /// On the game's phone: a tap selects, a swipe steps the selection
    /// (where the finger landed or last stepped, where it is, steps made).
    Phone { origin: Pos2, at: Pos2, steps: u32 },
}

/// What the fingers do with the mouse and keys, for the pause menu.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Mouse {
    /// Where the pointer goes, 0..1 across the screen.
    pub at: Option<Pos2>,
    pub down: bool,
    /// Wheel notches, + = up (zoom in).
    pub wheel: i32,
    /// The menu's keys held, by [`MenuKey`].
    pub keys: [bool; 6],
}

pub struct Touch {
    fingers: HashMap<u64, Finger>,
    /// Buttons pressed since the last [`Touch::gamepad`]: a tap whose finger
    /// lifts before the gamepad is read (both in one batch of events) still
    /// presses its button once.
    pressed: u16,
    /// The game's inputs tapped since the last [`Touch::take_inputs`].
    pressed_inputs: Vec<u32>,
    /// Points per unit of SDL's normalised finger coordinates.
    screen: Vec2,
    /// Where the controls go: clear of notches and rounded corners.
    area: Rect,
    scale: f32,
    /// Which set of layouts to show.
    profile: TouchLayout,
    /// How a car, bike or boat is steered.
    steer: TouchSteer,
    /// Whether Fire also aims, or Aim shows a second Fire button.
    fire: TouchFire,
    /// Aim stays held after a double tap.
    aim_sticky: bool,
    aim_tap: Option<Instant>,
    /// The only finger currently allowed to turn the camera.
    turner: Option<u64>,
    /// The gamepad read a pick in the More fan while it was held: lifting
    /// the thumb must not press it a second time.
    radial_read: bool,
    /// A tap under each button or D-pad direction pressed.
    pub haptics: bool,
    /// The camera's speed (the setting).
    pub camera_speed: f32,
    /// The player's aim is locked on to a target: drags push the right
    /// stick instead of turning the camera.
    pub aim_locked: bool,
    /// What the player is doing: the layout follows.
    context: PlayerContext,
    /// What the game asks to press, and where the phone is (points).
    prompts: Vec<Prompt>,
    /// The pad button each input of the layouts' buttons is on now
    /// ([`glue5_runtime::prompts::pad_map`]).
    pad_map: Vec<(u32, u16)>,
    phone: Option<Rect>,
    /// The camera's turn since the last [`Touch::take_look`], in mouse
    /// counts.
    look: Vec2,
    /// While locked on: the drag since the last [`Touch::gamepad`], and the
    /// right stick it made (easing off once the finger stops).
    locked_drag: Vec2,
    locked_stick: Vec2,
    /// The buttons' icons, loaded when first drawn.
    icons: std::cell::OnceCell<Icons>,
    /// The pause menu's mouse: where it is, its click from and until when
    /// at least, each key held until when at least, wheel notches not
    /// taken yet, two fingers on it.
    mouse_at: Option<Pos2>,
    click: Option<(Instant, Instant)>,
    key_until: [Option<Instant>; 6],
    wheel: f32,
    /// A sideways drag's steps not pressed yet (+ = right), and when the
    /// next may be (a key's press and release each take a frame).
    steps: f32,
    next_step: Option<Instant>,
    /// Direct touch: when the last tap clicked (it may click again, once).
    tapped: Option<Instant>,
    two: Option<TwoFingers>,
}

/// Two fingers in the pause menu: how far apart they were last and the
/// height of their middle, how much of each changed while the gesture was
/// unclear, the gesture.
#[derive(Clone, Copy, Debug)]
struct TwoFingers {
    width: f32,
    middle: f32,
    unclear: Vec2,
    gesture: Option<Gesture>,
}

/// What a mouse finger's drag does.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Drag {
    /// Turns the wheel: lists scroll.
    Scroll,
    /// Left and right, as the arrow keys: the row's value changes.
    Swipe,
    /// The mouse held: the map moves.
    Hold,
    /// A second finger came: a pinch or a scroll, not a click.
    Two,
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Gesture {
    Pinch,
    Scroll,
}

impl Touch {
    pub fn new() -> Self {
        let area = Rect::from_min_size(Pos2::ZERO, vec2(1.0, 1.0));
        Touch {
            fingers: HashMap::new(),
            pressed: 0,
            pressed_inputs: Vec::new(),
            screen: area.size(),
            area,
            scale: 1.0,
            profile: TouchLayout::Classic,
            steer: TouchSteer::Stick,
            fire: TouchFire::Auto,
            aim_sticky: false,
            aim_tap: None,
            turner: None,
            radial_read: false,
            haptics: false,
            camera_speed: 1.0,
            aim_locked: false,
            context: PlayerContext {
                activity: Activity::OnFoot,
                vehicle_near: false,
                phone: false,
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
            },
            prompts: Vec::new(),
            pad_map: Vec::new(),
            phone: None,
            look: Vec2::ZERO,
            locked_drag: Vec2::ZERO,
            locked_stick: Vec2::ZERO,
            icons: std::cell::OnceCell::new(),
            mouse_at: None,
            click: None,
            key_until: [None; 6],
            wheel: 0.0,
            steps: 0.0,
            next_step: None,
            tapped: None,
            two: None,
        }
    }

    /// The window's size and safe area in points, and the controls' size
    /// setting.
    pub fn layout(&mut self, screen: Vec2, area: Rect, scale: f32) {
        self.screen = screen;
        self.area = area;
        self.scale = control_scale(screen, scale);
    }

    /// Which set of layouts to show. Changing it lets go of what was held:
    /// a button of one layout is not a button of the other.
    pub fn set_profile(&mut self, profile: TouchLayout) {
        if self.profile != profile {
            self.profile = profile;
            self.clear();
        }
    }

    /// How the left thumb steers vehicles by touch. Changing it lets go of
    /// what was held.
    pub fn set_steer(&mut self, steer: TouchSteer) {
        if self.steer != steer {
            self.steer = steer;
            self.clear();
        }
    }

    /// How the left thumb aims and fires by touch. Changing it lets go of
    /// what was held.
    pub fn set_fire(&mut self, fire: TouchFire) {
        if self.fire != fire {
            self.fire = fire;
            self.clear();
        }
    }

    /// What the player is doing now. Fingers down keep what they hold.
    pub fn set_context(&mut self, context: PlayerContext) {
        let (menu, over, engine) = (self.menu_mode(), self.cursor_over_play(), self.engine_screen());
        self.context = context;
        if !self.aims_here() {
            self.aim_sticky = false;
            self.aim_tap = None;
        }
        // In and out of a menu, fingers start over (a finger on a button is
        // not the mouse, nor the other way round); in and out of an engine's
        // screen too (what was held, a pedal, lets go: its button is gone).
        if self.menu_mode() != menu || self.engine_screen() != engine {
            self.clear();
        } else if self.cursor_over_play() != over {
            self.fingers.retain(|_, f| !matches!(f, Finger::Mouse { .. } | Finger::Pinch(_)));
            (self.click, self.two) = (None, None);
        }
    }

    /// The screen is a menu that takes the mouse, without a touch layout:
    /// the pause menu; a script's menu showing its cursor
    /// while the player's controls are off (a shop, the character creator),
    /// or a screen with its own pointer (the browser, a hacking screen).
    fn menu_mode(&self) -> bool {
        !self.engine_screen()
            && (self.context.menu || (self.context.cursor && (self.context.scripted || self.context.pointer)))
    }

    /// A screen of the engine's own is over everything (a warning, a
    /// loading screen, the screen faded out): only what it asks for.
    fn engine_screen(&self) -> bool {
        self.context.alert || self.context.hidden
    }

    /// A script's menu shows its cursor over play (the interaction menu):
    /// fingers off the controls are its mouse.
    fn cursor_over_play(&self) -> bool {
        self.context.cursor && !self.menu_mode()
    }

    /// The menu's keys shown: the D-pad, Enter and Back beside the menu;
    /// Back alone on a screen with its own pointer (the browser: no rows).
    fn menu_keys(&self) -> &'static [(MenuKey, Action)] {
        if self.context.pointer && !self.context.menu { &MENU_KEYS[..1] } else { &MENU_KEYS }
    }

    /// The pad button each input of the layouts' buttons is on now, as the
    /// game's control layout maps it (as `(input, buttons)`).
    pub fn set_pad_map(&mut self, map: Vec<(u32, u16)>) {
        self.pad_map = map;
    }

    /// The pad buttons the game's `input` is on now, if known.
    fn mapped(&self, input: u32) -> Option<u16> {
        self.pad_map.iter().find(|(i, _)| *i == input).map(|(_, bits)| *bits)
    }

    /// The pad buttons `a` presses: where the game maps what it does now.
    fn bits_of(&self, a: &Action) -> u16 {
        a.does.and_then(|i| self.mapped(i)).unwrap_or(a.bits)
    }

    /// What the game asks to press now, and where the phone is on the
    /// screen (fractions of it: left, top, right, bottom), if it is up.
    pub fn set_game(&mut self, prompts: Vec<Prompt>, phone: Option<[f32; 4]>) {
        self.prompts = prompts;
        self.phone = phone.map(|[l, t, r, b]| {
            Rect::from_min_max(pos2(l * self.screen.x, t * self.screen.y), pos2(r * self.screen.x, b * self.screen.y))
        });
    }

    /// The touch layout for what the player is doing (kept from the last
    /// activity seen through the moments between), or none: the pause menu.
    fn touch_layout(&self) -> Option<&'static [Action]> {
        if self.engine_screen() {
            // A warning whose buttons are not known: Select and Back, as it
            // has (Continue, Yes, No...): never one with nothing to press.
            let known = self.prompts.iter().any(|p| p.pads.iter().any(|d| matches!(d, Pad::Buttons(_))));
            return Some(if self.context.alert && !known { ALERT } else { ASKED });
        }
        if self.menu_mode() {
            return None;
        }
        if self.context.cutscene {
            return Some(CUTSCENE);
        }
        // A script's menu or scene, unless the phone is out in it: the
        // D-pad and Select when it shows its buttons' bar.
        if self.context.scripted && !self.context.phone {
            return Some(if self.context.bar { SCRIPTED } else { SCENE });
        }
        let modern = self.profile == TouchLayout::Modern;
        Some(match self.context.activity {
            Activity::OnFoot => {
                if modern {
                    MODERN_ON_FOOT
                } else {
                    ON_FOOT
                }
            }
            Activity::Driving(Driving::Car) => {
                if modern {
                    MODERN_CAR
                } else {
                    CAR
                }
            }
            Activity::Driving(Driving::Bicycle) => BICYCLE,
            Activity::Driving(Driving::Heli) => HELI,
            Activity::Driving(Driving::Plane) => PLANE,
            Activity::Driving(Driving::Submarine) => SUBMARINE,
            Activity::Passenger => PASSENGER,
            Activity::Parachute => PARACHUTE,
        })
    }

    fn is(&self, layout: &'static [Action]) -> bool {
        self.touch_layout().is_some_and(|l| std::ptr::eq(l, layout))
    }

    /// The actions the More button opens now, if the layout has one.
    fn more_actions(&self) -> Option<&'static [Action]> {
        if self.is(MODERN_ON_FOOT) {
            Some(ON_FOOT_MORE)
        } else if self.is(MODERN_CAR) {
            Some(CAR_MORE)
        } else {
            None
        }
    }

    /// Where the `i`th action of a fan of `count` sits, from `origin`: a fan
    /// opening to the right, where the left thumb can drag without leaving
    /// the screen.
    fn radial_place(&self, origin: Pos2, count: usize, i: usize) -> Pos2 {
        let (start, end) = (-55.0f32.to_radians(), 55.0f32.to_radians());
        let angle = if count <= 1 { 0.0 } else { start + (end - start) * i as f32 / (count - 1) as f32 };
        origin + vec2(angle.cos(), angle.sin()) * (RADIAL_RADIUS * self.scale)
    }

    /// What a thumb dragged from `origin` to `at` picks in the open fan.
    fn radial_action(&self, origin: Pos2, at: Pos2) -> Option<&'static Action> {
        if at.distance(origin) < RADIAL_DEADZONE * self.scale {
            return None;
        }
        let actions = self.more_actions()?;
        actions
            .iter()
            .enumerate()
            .min_by(|(i, _), (j, _)| {
                let a = self.radial_place(origin, actions.len(), *i).distance(at);
                let b = self.radial_place(origin, actions.len(), *j).distance(at);
                a.partial_cmp(&b).unwrap_or(std::cmp::Ordering::Equal)
            })
            .map(|(_, a)| a)
    }

    /// The layout has a stick at rest (a script's scene has its sticks
    /// where the thumbs land, a cutscene none).
    fn has_stick(&self) -> bool {
        self.touch_layout().is_some() && !self.is(CUTSCENE) && !self.is(SCRIPTED) && !self.is(SCENE) && !self.is(ASKED) && !self.is(ALERT)
    }

    fn arrows(&self) -> bool {
        self.steer == TouchSteer::Arrows && (self.is(CAR) || self.is(MODERN_CAR) || self.is(BICYCLE))
    }

    fn stick_shown(&self) -> bool {
        self.has_stick() && !self.arrows()
    }

    fn is_fire(a: &Action) -> bool {
        a.does == Some(24)
    }

    fn is_aim(a: &Action) -> bool {
        a.does == Some(25)
    }

    fn aims_here(&self) -> bool {
        self.touch_layout().is_some_and(|layout| layout.iter().any(Self::is_aim))
    }

    fn manual_fire_shown(&self) -> bool {
        self.fire == TouchFire::Manual
            && self.aims_here()
            && self.stick_shown()
            && (self.aim_sticky
                || self.fingers.values().any(|finger| {
                    matches!(
                        finger,
                        Finger::Button { key: Key::Action(a), .. } if unsafe { Self::is_aim(&**a) }
                    )
                }))
    }

    fn aim_bits(&self) -> u16 {
        self.mapped(25).unwrap_or(button::L2)
    }

    fn press_aim(&self, pad: &mut Gamepad) {
        let bits = self.aim_bits();
        pad.buttons |= bits;
        if bits & button::L2 != 0 {
            pad.l2 = 1.0;
        }
    }

    /// The game asks for a stick (the right one, or the left).
    fn asks_stick(&self, right: bool) -> bool {
        self.prompts.iter().any(|p| p.pads.iter().any(|d| matches!(d, Pad::Stick { right: r, .. } if *r == right)))
    }

    /// Drags are the right stick, not a mouse's motion: in a script's
    /// menu or scene with its bar, and where the game asks for the right
    /// stick.
    fn drags_are_stick(&self) -> bool {
        self.is(SCRIPTED) || self.asks_stick(true)
    }

    fn on_foot(&self) -> bool {
        self.is(ON_FOOT) || self.is(MODERN_ON_FOOT)
    }

    /// The stick sprints past its ring: on foot, the phone put away, no
    /// menu taking the buttons (A picks in them: the interaction menu, a
    /// shop's, a screen's pointer).
    fn sprints(&self) -> bool {
        self.on_foot() && !self.context.phone && !self.context.bar && !self.context.cursor
    }

    /// The touch layout's buttons shown now: with the phone out, those it
    /// leaves free (and pause, the view).
    fn actions(&self) -> impl Iterator<Item = &'static Action> + '_ {
        let phone = self.context.phone && self.has_stick();
        let layout = self.touch_layout().unwrap_or_default();
        layout.iter().filter(move |a| {
            (!a.near_vehicle || self.context.vehicle_near || self.asked(a))
                && (!phone || a.with_phone || self.bits_of(a) & (button::START | button::SELECT) != 0)
        })
    }

    /// The game asks for what `a` does: its input, or its pad button.
    fn asked(&self, a: &Action) -> bool {
        self.prompts.iter().any(|p| {
            (p.input.is_some() && p.input == a.input)
                || (self.bits_of(a) != 0 && p.pads.iter().any(|d| matches!(d, Pad::Buttons(b) if *b == self.bits_of(a))))
        })
    }

    /// Every button shown now, where it is.
    fn buttons(&self) -> Vec<Button> {
        let s = self.scale;
        let round = |key, at, a: &Action| Button {
            key,
            at,
            radius: a.radius * s,
            icon: a.icon,
            bits: self.bits_of(a),
            input: a.input,
            kind: a.kind,
            primary: a.primary,
            asked: false,
        };
        let phone = self.context.phone && self.has_stick();
        let keys = self.phone_keys();
        // With the phone out, the pedals and what else stays move left of
        // its keys (the phone is where the right thumb's buttons were).
        let mut beside = [vec2(84.0, 150.0), vec2(168.0, 70.0), vec2(84.0, 250.0), vec2(168.0, 170.0)].into_iter();
        let mut out: Vec<Button> = self
            .actions()
            .map(|a| {
                // Melee and reload are one button: what B does with what is
                // in hand.
                let icon = if a.icon == "melee" && self.context.gun { "reload" } else { a.icon };
                let at = match a.at {
                    At::MidTop(..) => self.pos(a.at),
                    _ if phone => beside
                        .next()
                        .map_or(self.pos(a.at), |d| pos2(keys[4].x - d.x * s, self.area.max.y - d.y * s)),
                    _ => self.pos(a.at),
                };
                Button { icon, ..round(Key::Action(a), at, a) }
            })
            .collect();
        if phone {
            for (i, (a, at)) in PHONE_KEYS.iter().zip(keys).enumerate() {
                out.push(round(Key::Phone(i), at, a));
            }
        }
        if self.arrows() {
            out.push(round(Key::Action(&STEER_LEFT), self.pos(STEER_LEFT.at), &STEER_LEFT));
            out.push(round(Key::Action(&STEER_RIGHT), self.pos(STEER_RIGHT.at), &STEER_RIGHT));
        }
        if self.manual_fire_shown() {
            out.push(round(Key::Action(&LEFT_FIRE), self.pos(LEFT_FIRE.at), &LEFT_FIRE));
        }
        // The game's prompts: the button that does it is ringed (shown for
        // it if the layout hid it), else one of its own.
        let mut own: Vec<(u16, Option<u32>, &'static str)> = Vec::new();
        for prompt in &self.prompts {
            let buttons: Vec<u16> =
                prompt.pads.iter().filter_map(|p| if let Pad::Buttons(b) = p { Some(*b) } else { None }).collect();
            let mut shown = false;
            for b in &mut out {
                let same_input = prompt.input.is_some() && b.input == prompt.input;
                if same_input || (b.bits != 0 && buttons.contains(&b.bits)) {
                    b.asked = true;
                    shown = true;
                }
            }
            if shown {
                continue;
            }
            if buttons.is_empty() {
                // A stick's prompt: the drags are that stick; an input with
                // no pad button: pressed itself.
                if prompt.input.is_some() && prompt.pads.is_empty() {
                    own.push((0, prompt.input, prompt_icon(0, prompt.input)));
                }
                continue;
            }
            // One button for one: the action's icon if its input says what
            // it does. Several (a group): the pad's symbols the game shows.
            let input = if buttons.len() == 1 { prompt.input } else { None };
            for bits in buttons {
                // Pressed as the pad's button, which is what the game shows
                // (its input, if given, may be one of a group's).
                if !own.iter().any(|(b, ..)| *b == bits) {
                    own.push((bits, None, prompt_icon(bits, input)));
                }
            }
        }
        out.extend(self.prompt_buttons(own));
        out
    }

    /// The game's prompts that no button shows: buttons with the action's
    /// icon or the pad's symbol ([`prompt_icon`]), in a column at the right
    /// edge above Select in a script's scene with its bar, else in a row at
    /// the bottom's middle.
    fn prompt_buttons(&self, prompts: Vec<(u16, Option<u32>, &'static str)>) -> Vec<Button> {
        let s = self.scale;
        let r = PROMPT_RADIUS * s;
        let step = 2.0 * r + 10.0 * s;
        let column = self.is(SCRIPTED);
        let mut at = if column {
            pos2(self.area.max.x - 16.0 * s - r, self.area.max.y - 226.0 * s)
        } else {
            pos2(self.area.center().x - (prompts.len() as f32 - 1.0) * step / 2.0, self.area.max.y - 40.0 * s)
        };
        let mut out = Vec::new();
        for (bits, input, icon) in prompts {
            if at.y - r < self.area.min.y + 60.0 * s {
                break;
            }
            out.push(Button {
                key: Key::Prompt { bits, input },
                at,
                radius: r,
                icon,
                bits,
                input,
                kind: Kind::Press,
                primary: false,
                asked: true,
            });
            if column {
                at.y -= step;
            } else {
                at.x += step;
            }
        }
        out
    }

    /// Where the phone's keys go: the D-pad above the phone where there is
    /// room, else left of it; Select and Back left of it, near the bottom
    /// (where it is not known, as on a phone's screen).
    fn phone_keys(&self) -> [Pos2; 6] {
        let s = self.scale;
        let arm = 48.0 * s;
        let phone = self.phone.unwrap_or_else(|| {
            Rect::from_min_max(pos2(self.area.max.x - 165.0 * s, self.area.max.y - 178.0 * s), self.area.max + vec2(0.0, 40.0))
        });
        let above = phone.top() - self.area.top() > 2.0 * arm + 60.0 * s + 26.0 * s;
        let dpad = if above {
            pos2(phone.center().x.min(self.area.max.x - arm - 24.0 * s), phone.top() - arm - 26.0 * s)
        } else {
            pos2(phone.left() - arm - 26.0 * s, phone.top() + arm)
        };
        let x = (if above { phone.left() } else { dpad.x - arm }) - 12.0 * s - 26.0 * s;
        [
            dpad - vec2(0.0, arm),
            dpad + vec2(0.0, arm),
            dpad - vec2(arm, 0.0),
            dpad + vec2(arm, 0.0),
            pos2(x, self.area.max.y - 124.0 * s),
            pos2(x, self.area.max.y - 54.0 * s),
        ]
    }

    /// The camera's turn since the last call, in whole mouse counts (the
    /// fraction left waits for the next).
    pub fn take_look(&mut self) -> (i32, i32) {
        let whole = self.look.round();
        self.look -= whole;
        (whole.x as i32, whole.y as i32)
    }

    /// The game's inputs held now, and those tapped since the last call:
    /// pressed as the inputs, not by the pad.
    pub fn take_inputs(&mut self) -> Vec<u32> {
        let mut inputs = std::mem::take(&mut self.pressed_inputs);
        for f in self.fingers.values() {
            if let Finger::Button { input: Some(i), .. } = f
                && !inputs.contains(i)
            {
                inputs.push(*i);
            }
        }
        inputs
    }

    fn pos(&self, at: At) -> Pos2 {
        let (s, a) = (self.scale, self.area);
        match at {
            At::LeftTop(x, y) => pos2(a.min.x + x * s, a.min.y + y * s),
            At::LeftBottom(x, y) => pos2(a.min.x + x * s, a.max.y - y * s),
            At::RightBottom(x, y) => pos2(a.max.x - x * s, a.max.y - y * s),
            At::MidTop(x, y) => pos2(a.center().x + x * s, a.min.y + y * s),
        }
    }

    fn stick_radius(&self) -> f32 {
        STICK_RADIUS * self.scale
    }

    /// Where the controls shown now are (the stick where it rests, with its
    /// sprint ring on foot): what else is drawn keeps off them.
    pub fn occupied(&self) -> Vec<Rect> {
        let circle = |c: Pos2, r: f32| Rect::from_center_size(c, Vec2::splat(2.0 * r));
        if self.menu_mode() {
            return self.menu_keys().iter().map(|(_, a)| circle(self.pos(a.at), a.radius * self.scale)).collect();
        }
        let mut out: Vec<Rect> = self.buttons().iter().map(Button::rect).collect();
        if self.stick_shown() {
            let reach = if self.sprints() { SPRINT_REACH } else { 1.0 };
            out.push(circle(self.pos(MOVE_REST), self.stick_radius() * reach));
        }
        out
    }

    pub fn finger_down(&mut self, id: u64, x: f32, y: f32) {
        let p = pos2(x * self.screen.x, y * self.screen.y);
        let mut finger = if self.menu_mode() || (self.cursor_over_play() && self.takes_mouse(p)) {
            self.menu_finger(p)
        } else {
            self.touch_finger(p)
        };
        if let Finger::Button { kind: Kind::More, origin, .. } = finger {
            // The More button itself presses nothing: it opens its fan, and
            // the finger picks from there.
            self.radial_read = false;
            finger = Finger::Radial { origin, at: origin };
            if self.haptics {
                haptics::tap();
            }
        } else if let Finger::Button { bits, input, kind, .. } = finger {
            self.pressed |= bits;
            self.pressed_inputs.extend(input);
            if kind == Kind::Wheel {
                // The other fingers now pick in the wheel, from where they
                // are (the right thumb is often on a button: the gas, fire).
                for f in self.fingers.values_mut() {
                    if let Finger::Camera { origin, last } | Finger::Button { origin, last, .. } = f {
                        *origin = *last;
                    }
                }
            }
            if self.haptics {
                haptics::tap();
            }
        }
        self.fingers.insert(id, finger);
    }

    /// In a touch layout: what a finger landing at `p` takes.
    fn touch_finger(&self, p: Pos2) -> Finger {
        // The phone's own screen takes a finger as a phone's would.
        if self.context.phone && self.has_stick() && self.phone.is_some_and(|r| r.contains(p)) {
            return Finger::Phone { origin: p, at: p, steps: 0 };
        }
        if let Some(b) = self.buttons().iter().find(|b| b.hit(p)) {
            return Finger::Button { key: b.key, bits: b.bits, input: b.input, kind: b.kind, origin: p, last: p };
        }
        let left = p.x < self.screen.x / 2.0;
        if left && (self.stick_shown() || self.is(SCRIPTED)) {
            Finger::Move { origin: p, at: p }
        } else if self.is(SCRIPTED) {
            Finger::Look { origin: p, at: p }
        } else {
            Finger::Camera { origin: p, last: p }
        }
    }

    /// Over play, a script's menu with its cursor takes a finger off the
    /// controls (and off the stick where it rests).
    fn takes_mouse(&self, p: Pos2) -> bool {
        let stick = self.stick_shown() && p.distance(self.pos(MOVE_REST)) <= FOLLOW_REACH * self.stick_radius();
        !stick && !self.buttons().iter().any(|b| b.hit(p))
    }

    /// In a menu: a key, else the mouse, or a pinch with it.
    fn menu_finger(&mut self, p: Pos2) -> Finger {
        let keys = if self.menu_mode() { self.menu_keys() } else { &[] };
        for (i, (key, a)) in keys.iter().enumerate() {
            if self.pos(a.at).distance(p) <= a.radius * self.scale * 1.2 {
                self.key_until[i] = Some(Instant::now() + MIN_PRESS);
                if self.haptics {
                    haptics::tap();
                }
                return Finger::MenuKey(*key);
            }
        }
        let mouse = self.fingers.values_mut().find_map(|f| match f {
            Finger::Mouse { at, drag, .. } => Some((*at, drag)),
            _ => None,
        });
        match mouse {
            Some((at, drag)) => {
                // A second finger: a pinch or a scroll, not a click.
                *drag = Some(Drag::Two);
                let (width, middle) = (at.distance(p), (at.y + p.y) / 2.0);
                self.two = Some(TwoFingers { width, middle, unclear: Vec2::ZERO, gesture: None });
                self.click = None;
                Finger::Pinch(p)
            }
            None => {
                // The pointer comes under the finger; the click waits for it
                // to lift without moving. A new gesture drops what was left
                // of a step or a notch.
                self.mouse_at = Some(p);
                (self.steps, self.wheel) = (self.steps.trunc(), self.wheel.trunc());
                Finger::Mouse { origin: p, at: p, since: Instant::now(), drag: None }
            }
        }
    }

    /// The pause menu went a level deeper at `at` (its last time): when that
    /// was the last tap's click, it only went into the column or list
    /// tapped (the mouse picks in the one it is in), so it clicks again,
    /// once, where it was: a tap picks what it lands on. Not on the map
    /// (a click there marks it).
    pub fn menu_went_deeper(&mut self, at: Option<Instant>) {
        if let (Some(tap), Some(at)) = (self.tapped, at)
            && at >= tap
        {
            self.tapped = None;
            if at < tap + Duration::from_millis(500) && !self.context.map {
                let from = Instant::now() + AGAIN_DELAY;
                self.click = Some((from, from + MIN_PRESS));
            }
        }
    }

    /// Takes what the fingers do with the mouse and keys in the pause menu.
    pub fn take_mouse(&mut self) -> Mouse {
        let now = Instant::now();
        let held = |f: &Finger| matches!(f, Finger::Mouse { drag: Some(Drag::Hold), .. });
        let pinching = self.fingers.values().any(|f| matches!(f, Finger::Pinch(_)));
        let down = !pinching
            && self.click.is_some_and(|(from, until)| now >= from && (now < until || self.fingers.values().any(held)));
        // A sideways drag's steps: Left or Right pressed once each, a frame
        // apart.
        if self.steps.abs() >= 1.0 && self.next_step.is_none_or(|t| now >= t) {
            let (i, one) = if self.steps > 0.0 { (2, 1.0) } else { (1, -1.0) };
            self.key_until[i] = Some(now + MIN_PRESS);
            self.next_step = Some(now + 2 * MIN_PRESS);
            self.steps -= one;
        }
        let keys = std::array::from_fn(|i| {
            let key = MENU_KEYS[i].0;
            self.fingers.values().any(|f| matches!(f, Finger::MenuKey(k) if *k == key))
                || self.key_until[i].is_some_and(|t| now < t)
        });
        let wheel = self.wheel.trunc();
        self.wheel -= wheel;
        let at = self.mouse_at.map(|p| pos2(p.x / self.screen.x, p.y / self.screen.y));
        Mouse { at, down, wheel: wheel as i32, keys }
    }

    pub fn finger_motion(&mut self, id: u64, x: f32, y: f32) {
        let p = pos2(x * self.screen.x, y * self.screen.y);
        if let Some(Finger::Phone { origin, at, steps }) = self.fingers.get_mut(&id) {
            *at = p;
            let d = p - *origin;
            if d.x.abs().max(d.y.abs()) >= PHONE_STEP * self.scale {
                // The selection follows the finger, as on a touchpad.
                self.pressed |= if d.y.abs() >= d.x.abs() {
                    if d.y < 0.0 { button::UP } else { button::DOWN }
                } else if d.x < 0.0 {
                    button::LEFT
                } else {
                    button::RIGHT
                };
                *steps += 1;
                *origin = p;
                if self.haptics {
                    haptics::tap();
                }
            }
            return;
        }
        if matches!(self.fingers.get(&id), Some(Finger::Mouse { .. } | Finger::Pinch(_))) {
            let (map, slop) = (self.context.map, TAP_SLOP * self.scale);
            let (mut scroll, mut swipe) = (0.0, 0.0);
            match self.fingers.get_mut(&id) {
                Some(Finger::Mouse { origin, at, drag, .. }) => {
                    // Past a tap, the drag scrolls, changes a value, or
                    // moves the map with the mouse held; it counts from
                    // where the finger landed.
                    let mut from = *at;
                    if drag.is_none() && origin.distance(p) > slop {
                        let d = p - *origin;
                        from = *origin;
                        *drag = Some(if map {
                            Drag::Hold
                        } else if d.x.abs() > d.y.abs() {
                            Drag::Swipe
                        } else {
                            Drag::Scroll
                        });
                        if map {
                            let now = Instant::now();
                            self.click = Some((now, now + MIN_PRESS));
                        }
                    }
                    match drag {
                        Some(Drag::Scroll) => scroll = p.y - from.y,
                        Some(Drag::Swipe) => swipe = p.x - from.x,
                        Some(Drag::Two) => {}
                        Some(Drag::Hold) => self.mouse_at = Some(p),
                        None => {}
                    }
                    *at = p;
                }
                Some(Finger::Pinch(at)) => *at = p,
                _ => {}
            }
            // Natural scrolling: a finger moving down brings up what is
            // above.
            self.wheel += scroll / SCROLL_NOTCH;
            self.steps += swipe / SWIPE_STEP;
            let points: Vec<Pos2> = self
                .fingers
                .values()
                .filter_map(|f| match f {
                    Finger::Mouse { at, .. } | Finger::Pinch(at) => Some(*at),
                    _ => None,
                })
                .collect();
            if let (&[a, b], Some(two)) = (&points[..], self.two.as_mut()) {
                let (width, middle) = (a.distance(b), (a.y + b.y) / 2.0);
                let (wider, lower) = (width - two.width, middle - two.middle);
                (two.width, two.middle) = (width, middle);
                if two.gesture.is_none() {
                    two.unclear += vec2(wider.abs(), lower.abs());
                    let u = two.unclear;
                    if u.max_elem() > GESTURE_CLEAR {
                        two.gesture = Some(if u.x >= u.y { Gesture::Pinch } else { Gesture::Scroll });
                    }
                }
                // Natural scrolling: fingers moving down bring up what is above.
                match two.gesture {
                    Some(Gesture::Pinch) => self.wheel += wider / PINCH_NOTCH,
                    Some(Gesture::Scroll) => self.wheel += lower / SCROLL_NOTCH,
                    None => {}
                }
            }
            return;
        }
        if self.menu_mode() {
            return;
        }
        let follow = FOLLOW_REACH * self.stick_radius();
        let gain = LOOK_GAIN * self.camera_speed;
        let wheel = self.wheel_open();
        let stick = self.drags_are_stick();
        let may_turn =
            self.aim_locked || self.turner.is_none_or(|turner| turner == id || !self.fingers.contains_key(&turner));
        let mut turned = Vec2::ZERO;
        let mut look_from = None;
        let mut claims_turner = false;
        match self.fingers.get_mut(&id) {
            Some(Finger::Move { origin, at } | Finger::Look { origin, at }) => {
                *at = p;
                // Far past its reach, the stick comes along under the thumb.
                let d = p - *origin;
                if d.length() > follow {
                    *origin = p - d.normalized() * follow;
                }
            }
            Some(Finger::Camera { origin, last }) => {
                if stick && !wheel {
                    // The game asks for the right stick: the finger is it.
                    look_from = Some(*origin);
                } else if !wheel && may_turn {
                    turned = p - *last;
                    claims_turner = !self.aim_locked;
                }
                *last = p;
            }
            Some(Finger::Button { kind, last, .. }) => {
                if *kind == Kind::Turns && !wheel && may_turn {
                    turned = p - *last;
                    claims_turner = !self.aim_locked;
                }
                *last = p;
            }
            Some(Finger::Radial { at, .. }) => *at = p,
            _ => {}
        }
        if let Some(origin) = look_from {
            self.fingers.insert(id, Finger::Look { origin, at: p });
        }
        if claims_turner {
            self.turner = Some(id);
        }
        if turned != Vec2::ZERO {
            self.turn(turned, gain);
        }
    }

    /// A finger dragged by `d` turns the camera, or while locked on pushes
    /// the right stick.
    fn turn(&mut self, d: Vec2, gain: f32) {
        if self.aim_locked {
            self.locked_drag += d;
        } else {
            self.look += d * gain;
        }
    }

    pub fn finger_up(&mut self, id: u64) {
        if self.turner == Some(id) {
            self.turner = None;
        }
        match self.fingers.remove(&id) {
            Some(Finger::Pinch(_)) => self.two = None,
            Some(Finger::Phone { origin, at, steps: 0 }) if origin.distance(at) <= TAP_SLOP * self.scale => {
                self.pressed |= button::SOUTH;
            }
            Some(Finger::Button { key: Key::Action(a), origin, last, .. })
                if unsafe { Self::is_aim(&*a) } && origin.distance(last) <= TAP_SLOP * self.scale =>
            {
                let now = Instant::now();
                if self.aim_sticky {
                    self.aim_sticky = false;
                    self.aim_tap = None;
                } else if self.aim_tap.is_some_and(|tap| now.saturating_duration_since(tap) <= DOUBLE_TAP) {
                    self.aim_sticky = true;
                    self.aim_tap = None;
                } else {
                    self.aim_tap = Some(now);
                }
            }
            // A tap clicks as it lifts (once the pointer has been there a
            // moment); the map's drag lets go.
            Some(Finger::Mouse { since, drag, .. }) => match drag {
                None => {
                    let from = (since + CLICK_DELAY).max(Instant::now());
                    self.click = Some((from, from + MIN_PRESS));
                    self.tapped = Some(from);
                }
                Some(Drag::Hold) => self.click = None,
                // The pinch ends with either finger.
                Some(Drag::Two) => self.two = None,
                Some(Drag::Scroll | Drag::Swipe) => {}
            },
            // A fan pick acts once when the thumb lifts, even if the gamepad
            // was read in the same batch of events.
            Some(Finger::Radial { origin, at }) => {
                let read = self.radial_read;
                self.radial_read = false;
                if let Some(a) = self.radial_action(origin, at) {
                    if !read {
                        self.pressed |= self.bits_of(a);
                        self.pressed_inputs.extend(a.input);
                    }
                    if self.haptics {
                        haptics::tap();
                    }
                }
            }
            _ => {}
        }
    }

    /// The system took finger `id` (an edge gesture, a call, the app going
    /// away): it lets go of what it held, without the click a lift makes.
    pub fn finger_cancel(&mut self, id: u64) {
        if self.turner == Some(id) {
            self.turner = None;
        }
        match self.fingers.remove(&id) {
            Some(Finger::Pinch(_) | Finger::Mouse { drag: Some(Drag::Two), .. }) => self.two = None,
            Some(Finger::Mouse { drag: Some(Drag::Hold), .. }) => self.click = None,
            _ => {}
        }
    }

    pub fn clear(&mut self) {
        self.fingers.clear();
        self.turner = None;
        self.aim_sticky = false;
        self.aim_tap = None;
        self.pressed = 0;
        self.pressed_inputs.clear();
        self.radial_read = false;
        self.look = Vec2::ZERO;
        self.locked_drag = Vec2::ZERO;
        self.locked_stick = Vec2::ZERO;
        self.click = None;
        self.key_until = [None; 6];
        self.wheel = 0.0;
        self.steps = 0.0;
        self.two = None;
    }

    /// The gamepad the fingers make now.
    pub fn gamepad(&mut self) -> Gamepad {
        let mut pad = Gamepad { connected: true, buttons: std::mem::take(&mut self.pressed), ..Default::default() };
        if self.menu_mode() {
            return Gamepad { buttons: 0, ..pad };
        }
        let radius = self.stick_radius();
        let stick = |origin: Pos2, at: Pos2| stick_value((at - origin) / radius);
        // A wheel picks by direction: a drag a few points long pushes the
        // stick all the way there (the radio wheel takes nothing under
        // 60 %).
        let pick = WHEEL_PICK * self.scale;
        let toward = move |origin: Pos2, at: Pos2| {
            let d = at - origin;
            if d.length() < pick { [0.0, 0.0] } else { stick_value(d.normalized()) }
        };
        let mut radial_seen = false;
        let mut steer = 0.0f32;
        let mut steer_finger = false;
        for finger in self.fingers.values() {
            match *finger {
                Finger::Move { origin, at } => {
                    pad.left = stick(origin, at);
                    // On foot, a thumb pushed past the ring sprints (A held,
                    // L3 in first person); not with the phone out, where A
                    // selects.
                    if self.sprints() && (at - origin).length() > SPRINT_REACH * radius {
                        pad.buttons |= self.mapped(SPRINT).unwrap_or(button::SOUTH);
                    }
                }
                Finger::Look { origin, at } => pad.right = stick(origin, at),
                Finger::Radial { origin, at } => {
                    if let Some(a) = self.radial_action(origin, at) {
                        radial_seen = true;
                        let bits = self.bits_of(a);
                        pad.buttons |= bits;
                        if bits & button::L2 != 0 {
                            pad.l2 = 1.0;
                        }
                        if bits & button::R2 != 0 {
                            pad.r2 = 1.0;
                        }
                    }
                }
                Finger::Camera { .. }
                | Finger::Mouse { .. }
                | Finger::Pinch(_)
                | Finger::MenuKey(_)
                | Finger::Phone { .. } => {}
                Finger::Button { key, bits, kind, origin, last, .. } => {
                    if kind == Kind::Steer {
                        steer_finger = true;
                        let middle = (self.pos(STEER_LEFT.at).x + self.pos(STEER_RIGHT.at).x) / 2.0;
                        steer += if last.x < middle { -1.0 } else { 1.0 };
                    }
                    if kind == Kind::Wheel {
                        pad.right = toward(origin, last);
                    }
                    pad.buttons |= bits;
                    if bits & button::L2 != 0 {
                        pad.l2 = 1.0;
                    }
                    if bits & button::R2 != 0 {
                        pad.r2 = 1.0;
                    }
                    if self.fire == TouchFire::Auto
                        && self.context.gun
                        && self.aims_here()
                        && matches!(key, Key::Action(a) if unsafe { Self::is_fire(&*a) })
                    {
                        self.press_aim(&mut pad);
                    }
                }
            }
        }
        if self.aim_sticky && self.aims_here() {
            self.press_aim(&mut pad);
        }
        if steer_finger {
            pad.left[0] = steer.clamp(-1.0, 1.0);
        }
        self.radial_read |= radial_seen;
        // Locked on, the drags of this frame push the right stick.
        let drag = std::mem::take(&mut self.locked_drag);
        self.locked_stick = if !self.aim_locked {
            Vec2::ZERO
        } else if drag != Vec2::ZERO {
            drag / LOCKED_SWIPE
        } else {
            self.locked_stick * 0.5
        };
        if self.locked_stick.length() > 0.05 {
            pad.right = stick_value(self.locked_stick);
        }
        // An open wheel picks with the other thumb's drag too (the wheels'
        // buttons sit on the left), on the camera or from a button it holds
        // (which stays pressed).
        if self.wheel_open() {
            for finger in self.fingers.values() {
                if let Finger::Camera { origin, last }
                | Finger::Button { kind: Kind::Press | Kind::Turns, origin, last, .. } = *finger
                    && origin != last
                {
                    pad.right = toward(origin, last);
                }
            }
        }
        pad
    }

    /// A wheel's button is held: the wheel is open.
    fn wheel_open(&self) -> bool {
        self.fingers.values().any(|f| matches!(f, Finger::Button { kind: Kind::Wheel, .. }))
    }

    pub fn draw(&self, painter: &Painter, opacity: f32) {
        let look = Look::new(opacity, self.scale, self.context.character);
        if self.menu_mode() {
            for (key, a) in self.menu_keys() {
                let held = self.fingers.values().any(|f| matches!(f, Finger::MenuKey(k) if k == key));
                let b = Button {
                    key: Key::Action(a),
                    at: self.pos(a.at),
                    radius: a.radius * self.scale,
                    icon: a.icon,
                    bits: 0,
                    input: None,
                    kind: Kind::Press,
                    primary: *key == MenuKey::Enter,
                    asked: false,
                };
                self.draw_button(painter, &look, &b, held);
            }
            return;
        }
        for b in &self.buttons() {
            let held = self.fingers.values().any(|f| matches!(f, Finger::Button { key, .. } if *key == b.key))
                || (b.kind == Kind::More && self.fingers.values().any(|f| matches!(f, Finger::Radial { .. })))
                || (self.aim_sticky
                    && self.aims_here()
                    && matches!(b.key, Key::Action(a) if unsafe { Self::is_aim(&*a) }));
            self.draw_button(painter, &look, b, held);
        }
        for f in self.fingers.values() {
            if let Finger::Radial { origin, at } = *f {
                self.draw_more(painter, &look, origin, at);
            }
        }
        // The sticks held, or at rest (on foot, with its sprint ring).
        let rest = self.stick_shown().then(|| self.pos(MOVE_REST));
        let mut moved = false;
        for f in self.fingers.values() {
            match *f {
                Finger::Move { origin, at } => {
                    moved = true;
                    self.draw_stick(painter, &look, origin, at, self.sprints());
                }
                Finger::Look { origin, at } => self.draw_stick(painter, &look, origin, at, false),
                _ => {}
            }
        }
        if let (false, Some(rest)) = (moved, rest) {
            self.draw_stick(painter, &look, rest, rest, self.sprints());
        }
    }

    /// The fan of secondary actions, while More is held: each action a
    /// button in the fan, its name under it, and the one the thumb points at
    /// ringed in the player's colour.
    fn draw_more(&self, painter: &Painter, look: &Look, origin: Pos2, at: Pos2) {
        let Some(actions) = self.more_actions() else {
            return;
        };
        let picked = self.radial_action(origin, at);
        for (i, a) in actions.iter().enumerate() {
            let p = self.radial_place(origin, actions.len(), i);
            let selected = picked.is_some_and(|picked| std::ptr::eq(picked, a));
            if selected {
                painter.line_segment([origin, p], look.ring(true));
            }
            let b = Button {
                key: Key::Action(a),
                at: p,
                radius: a.radius * self.scale,
                icon: a.icon,
                bits: 0,
                input: None,
                kind: Kind::Press,
                primary: false,
                asked: false,
            };
            self.draw_button(painter, look, &b, selected);
            painter.text(
                p + vec2(0.0, b.radius * 1.2),
                Align2::CENTER_TOP,
                a.label,
                FontId::proportional(11.0 * self.scale),
                look.icon,
            );
        }
    }

    /// A stick where a thumb landed (`origin`) and is (`at`), with its sprint
    /// ring if it has one.
    fn draw_stick(&self, painter: &Painter, look: &Look, origin: Pos2, at: Pos2, ring: bool) {
        let radius = self.stick_radius();
        let d = at - origin;
        let sprint = d.length() > SPRINT_REACH * radius;
        let knob = if d.length() > radius { origin + d.normalized() * radius } else { at };
        painter.circle(origin, radius, look.fill(false), look.stroke(false));
        if ring {
            let stroke = if sprint { look.ring(true) } else { Stroke::new(look.width, look.line.gamma_multiply(0.5)) };
            painter.circle_stroke(origin, SPRINT_REACH * radius, stroke);
        }
        painter.circle(knob, radius * 0.45, look.knob, look.stroke(origin != at));
    }

    /// A button: round, with its icon; ringed in the player's colour when it
    /// is the layout's main one, the game asks for it, or it is held.
    fn draw_button(&self, painter: &Painter, look: &Look, b: &Button, held: bool) {
        let ring = if held || b.primary || b.asked { look.ring(held) } else { look.stroke(false) };
        painter.circle(b.at, b.radius, look.fill(held), ring);
        self.draw_icon(painter, look, b.at, b.radius * 1.15, b.icon);
    }

    /// The icon `name` (or two joined by `+`, side by side), `side` points.
    fn draw_icon(&self, painter: &Painter, look: &Look, c: Pos2, side: f32, name: &str) {
        if name.is_empty() {
            return;
        }
        let icons = self.icons.get_or_init(|| load_icons(painter.ctx()));
        let names: Vec<&str> = name.split('+').collect();
        let side = if names.len() > 1 { side * 0.68 } else { side };
        let uv = Rect::from_min_max(Pos2::ZERO, pos2(1.0, 1.0));
        for (i, name) in names.iter().enumerate() {
            let dx = (i as f32 - (names.len() - 1) as f32 / 2.0) * side;
            if let Some(texture) = icons.get(name) {
                painter.image(texture.id(), Rect::from_center_size(c + vec2(dx, 0.0), Vec2::splat(side)), uv, look.icon);
            }
        }
    }
}

impl Button {
    /// Where it is on the screen.
    fn rect(&self) -> Rect {
        Rect::from_center_size(self.at, Vec2::splat(2.0 * self.radius))
    }

    /// A finger at `p` takes it (a little around it).
    fn hit(&self, p: Pos2) -> bool {
        self.at.distance(p) <= self.radius * 1.2
    }
}

/// The controls' scale on a screen this big (points), at the size setting
/// `size`: smaller on phones, held closer.
pub fn control_scale(screen: Vec2, size: f32) -> f32 {
    size * if screen.x.min(screen.y) < PHONE_SIDE { PHONE_SCALE } else { 1.0 }
}

/// The icon of the pad's buttons `bits` (the game's prompt shows that
/// symbol), or of the input pressed itself.
/// A prompt's button's icon: the action's, where its input says what it
/// does; else the pad's symbol for `bits` (the scripts' own inputs: the game
/// names them by that symbol in its words).
fn prompt_icon(bits: u16, input: Option<u32>) -> &'static str {
    input.and_then(action_icon).unwrap_or_else(|| pad_icon(bits, input))
}

/// The icon of an action the game asks for by its input (`rage::InputType`).
fn action_icon(input: u32) -> Option<&'static str> {
    Some(match input {
        0 => "view",                    // NEXT_CAMERA
        18 => "skip",                   // SKIP_CUTSCENE
        19 => "switch",                 // CHARACTER_WHEEL
        22 => "jump",                   // JUMP
        23 => "enter",                  // ENTER
        24 | 69 | 92 | 114 => "fire",   // ATTACK, VEH_ATTACK, VEH_PASSENGER_ATTACK, VEH_FLY_ATTACK
        25 | 68 | 91 => "aim",          // AIM, VEH_AIM, VEH_PASSENGER_AIM
        26 | 79 => "behind",            // LOOK_BEHIND, VEH_LOOK_BEHIND
        27 => "phone",                  // PHONE
        28 | 29 => "special",           // SPECIAL_ABILITY(_SECONDARY)
        36 => "crouch",                 // DUCK
        37 | 99 => "weapons",           // SELECT_WEAPON, VEH_SELECT_NEXT_WEAPON
        44 => "cover",                  // COVER
        45 => "reload",                 // RELOAD
        51 | 52 => "action",            // CONTEXT(_SECONDARY)
        54 | 74 => "lights",            // WEAPON_SPECIAL_TWO (the flashlight), VEH_HEADLIGHT
        65 => "drive-special",          // VEH_SPECIAL
        71 | 87 => "gas",               // VEH_ACCELERATE, VEH_FLY_THROTTLE_UP
        72 | 88 => "brake",             // VEH_BRAKE, VEH_FLY_THROTTLE_DOWN
        73 => "duck",                   // VEH_DUCK
        75 => "exit",                   // VEH_EXIT
        76 => "handbrake",              // VEH_HANDBRAKE
        80 => "cinematic",              // VEH_CIN_CAM (the camera's focus on a target)
        85 => "radio",                  // VEH_RADIO_WHEEL
        86 => "horn",                   // VEH_HORN
        89 | 152 => "yaw-left",         // VEH_FLY_YAW_LEFT, PARACHUTE_BRAKE_LEFT
        90 | 153 => "yaw-right",        // VEH_FLY_YAW_RIGHT, PARACHUTE_BRAKE_RIGHT
        113 => "landing-gear",          // VEH_FLY_UNDERCARRIAGE
        131 => "rise",                  // VEH_SUB_ASCEND
        132 => "dive",                  // VEH_SUB_DESCEND
        140 => "melee",                 // MELEE_ATTACK_LIGHT
        144 => "parachute",             // PARACHUTE_DEPLOY
        145 => "detach",                // PARACHUTE_DETACH
        172 | 188 => "up",              // CELLPHONE_UP, FRONTEND_UP
        173 | 187 => "down",            // CELLPHONE_DOWN, FRONTEND_DOWN
        174 | 189 => "left",            // CELLPHONE_LEFT, FRONTEND_LEFT
        175 | 190 => "right",           // CELLPHONE_RIGHT, FRONTEND_RIGHT
        176 | 201 | 237 => "check",     // CELLPHONE_SELECT, FRONTEND_ACCEPT, CURSOR_ACCEPT
        177 | 202 | 238 => "back",      // CELLPHONE_CANCEL, FRONTEND_CANCEL, CURSOR_CANCEL
        199 | 200 => "pause",           // FRONTEND_PAUSE(_ALTERNATE)
        _ => return None,
    })
}

/// The pad's symbol for `bits`, as the game shows it.
fn pad_icon(bits: u16, input: Option<u32>) -> &'static str {
    use button::*;
    match bits {
        SOUTH => "pad-a",
        EAST => "pad-b",
        WEST => "pad-x",
        NORTH => "pad-y",
        L1 => "pad-lb",
        R1 => "pad-rb",
        L2 => "pad-lt",
        R2 => "pad-rt",
        L3 => "pad-ls",
        R3 => "pad-rs",
        START => "pause",
        SELECT => "view",
        UP => "up",
        DOWN => "down",
        LEFT => "left",
        RIGHT => "right",
        _ if input == Some(SKIP_CUTSCENE) => "skip",
        _ => "action",
    }
}

/// How the touch controls look: as the stream's sign, dark with a thin white
/// ring and a white icon, legible on the sky as in a tunnel; the main
/// buttons, those the game asks for and those held ringed in the player's
/// colour (the HUD's: Michael's blue, Franklin's green, Trevor's orange).
/// The opacity setting fades them, down to where they still read.
#[derive(Clone, Copy)]
pub struct Look {
    fill: Color32,
    pressed: Color32,
    /// The stick's knob: light, what the thumb holds.
    knob: Color32,
    line: Color32,
    accent: Color32,
    pub icon: Color32,
    width: f32,
}

impl Look {
    pub fn new(opacity: f32, scale: f32, character: Character) -> Self {
        let a = |v: f32, least: f32| ((v * opacity).max(least) * 255.0) as u8;
        let accent = match character {
            Character::Michael => Color32::from_rgb(101, 180, 212),
            Character::Franklin => Color32::from_rgb(171, 237, 171),
            Character::Trevor => Color32::from_rgb(255, 163, 87),
            Character::Other => Color32::from_rgb(120, 175, 235),
        };
        Look {
            fill: Color32::from_black_alpha(a(0.6, 0.35)),
            pressed: Color32::from_rgba_unmultiplied(72, 72, 72, a(0.75, 0.5)),
            knob: Color32::from_white_alpha(a(0.45, 0.32)),
            line: Color32::from_white_alpha(a(0.6, 0.4)),
            accent: accent.gamma_multiply(opacity.max(0.6)),
            icon: Color32::from_white_alpha(a(0.95, 0.75)),
            width: 1.5 * scale,
        }
    }

    pub fn fill(&self, held: bool) -> Color32 {
        if held { self.pressed } else { self.fill }
    }

    /// The thin white ring, whole while held.
    pub fn stroke(&self, held: bool) -> Stroke {
        if held { Stroke::new(self.width * 1.6, Color32::WHITE) } else { Stroke::new(self.width, self.line) }
    }

    /// The player's colour: a main button's ring, wider while held.
    pub fn ring(&self, held: bool) -> Stroke {
        Stroke::new(self.width * if held { 2.0 } else { 1.4 }, self.accent)
    }
}

/// A stick's value for a thumb this far from where it landed (in stick
/// radii): the circle of its reach stretched onto the square, so that the
/// axis the thumb leans along reads as far as the thumb went. A thumb sliding
/// sideways drifts up or down as well; on the circle, 30 degrees of drift
/// left the steering at 0.87 at full reach.
fn stick_value(v: Vec2) -> [f32; 2] {
    let reach = v.length().min(1.0);
    let lean = v.x.abs().max(v.y.abs());
    if lean == 0.0 {
        return [0.0, 0.0];
    }
    let v = v * (reach / lean);
    [v.x, v.y]
}

#[cfg(test)]
mod tests;
