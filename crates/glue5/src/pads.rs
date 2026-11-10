//! Physical gamepads (SDL) to the engine's pads.

use std::time::{Duration, Instant};

use glue5_runtime::input::{self, Gamepad, MAX_PADS, button};
use sdl3::gamepad::{Axis, Button};
use sdl3::joystick::JoystickId;

use crate::paths::TV;
use crate::ui::Nav;

/// SDL buttons to RAGE pad buttons (SDL names buttons by position, so the
/// layout is right for Xbox, PlayStation and Nintendo-style pads alike).
const BUTTONS: &[(Button, u16)] = &[
    (Button::South, button::SOUTH),
    (Button::East, button::EAST),
    (Button::West, button::WEST),
    (Button::North, button::NORTH),
    (Button::LeftShoulder, button::L1),
    (Button::RightShoulder, button::R1),
    (Button::LeftStick, button::L3),
    (Button::RightStick, button::R3),
    (Button::Back, button::SELECT),
    (Button::Start, button::START),
    (Button::DPadUp, button::UP),
    (Button::DPadDown, button::DOWN),
    (Button::DPadLeft, button::LEFT),
    (Button::DPadRight, button::RIGHT),
];

/// Triggers pressed past this also set the digital L2/R2 bits.
const TRIGGER_THRESHOLD: f32 = 0.5;

/// Holding Back + Start (on one controller) this long opens the
/// application's settings.
const MENU_HOLD: Duration = Duration::from_millis(700);
/// A rumble is sent for a second and renewed after this while it lasts.
const RUMBLE_RENEW: Duration = Duration::from_millis(500);

/// A direction held in the application's pages repeats after this, then
/// at this interval.
const REPEAT_AFTER: Duration = Duration::from_millis(400);
const REPEAT_EVERY: Duration = Duration::from_millis(110);
/// The left stick moves in the pages past this.
const STICK_NAV: f32 = 0.6;

pub struct Pads {
    subsystem: sdl3::GamepadSubsystem,
    slots: [Option<Slot>; MAX_PADS],
    /// The Apple TV's remote: it moves around the pages and never plays
    /// (else it takes pad 0 from a controller, and a brush on its touch
    /// surface moves the player).
    remotes: Vec<Slot>,
    /// Buttons down at the previous update, on any pad (for the pages).
    previous: u16,
    /// The direction held in a page, and when it repeats next.
    repeat: Option<(u16, Instant)>,
    /// The loop reads the controllers itself (see [`Pads::read_by_loop`]).
    by_loop: bool,
    wait_for_neutral: bool,
}

/// What the pads did for the application this update.
#[derive(Default)]
pub struct Report {
    /// Back + Start held: open the settings.
    pub menu: bool,
    /// Back + RB held, as it opened Social Club's overlay: the friends.
    pub friends: bool,
    /// Navigation in the application's page (only while one is open).
    pub nav: Vec<Nav>,
}

/// Emergency input bypasses the release-to-neutral gate. Never combine
/// buttons from different controllers or generate normal UI navigation.
fn diagnostic_report(buttons: impl Iterator<Item = u16>) -> Report {
    let combo = button::SELECT | button::START;
    Report { menu: buttons.into_iter().any(|down| down & combo == combo), ..Default::default() }
}

struct Slot {
    pad: sdl3::gamepad::Gamepad,
    id: JoystickId,
    /// The rumble last sent, and when (SDL stops it when its duration ends).
    rumble: (u16, u16),
    rumble_at: Instant,
    /// Since when Back + Start (or Back + RB) are held, and whether that
    /// hold has fired.
    combo: Option<(Instant, bool)>,
    friends_combo: Option<(Instant, bool)>,
}

impl Pads {
    pub fn new(subsystem: sdl3::GamepadSubsystem) -> Self {
        Pads { subsystem, slots: Default::default(), remotes: Vec::new(), previous: 0, repeat: None, by_loop: false, wait_for_neutral:false }
    }

    /// A controller is open.
    pub fn any(&self) -> bool {
        self.slots.iter().any(Option::is_some) || !self.remotes.is_empty()
    }

    /// Whether the application's loop reads the controllers itself (then
    /// call [`Pads::read`] after each wait): while one is open, SDL reads it
    /// every millisecond of the loop's waits otherwise, a twentieth of a
    /// core on a page that does nothing.
    pub fn read_by_loop(&mut self, by_loop: bool) {
        if by_loop != self.by_loop {
            self.by_loop = by_loop;
            sdl3::hint::set("SDL_AUTO_UPDATE_JOYSTICKS", if by_loop { "0" } else { "1" });
        }
    }

    /// Reads the controllers, when the loop does (their events follow).
    pub fn read(&self) {
        if self.by_loop {
            self.subsystem.update();
        }
    }

    pub fn added(&mut self, id: JoystickId) {
        if self.slots.iter().flatten().chain(&self.remotes).any(|s| s.id == id) {
            return;
        }
        let Some(free) = self.slots.iter().position(Option::is_none) else {
            return;
        };
        let pad = match self.subsystem.open(id) {
            Ok(pad) => pad,
            Err(e) => return log::warn!("cannot open gamepad: {e}"),
        };
        let slot = Slot { pad, id, rumble: (0, 0), rumble_at: Instant::now(), combo: None, friends_combo: None };
        // SDL makes the Siri Remote a gamepad with one stick (its touch
        // surface) and a few buttons; controllers have two sticks.
        if TV && !slot.pad.has_axis(Axis::RightX) {
            log::info!("remote: {}", slot.pad.name().unwrap_or_default());
            self.remotes.push(slot);
            return;
        }
        log::info!("gamepad {}: {}", free, slot.pad.name().unwrap_or_default());
        self.slots[free] = Some(slot);
    }

    pub fn removed(&mut self, id: JoystickId) {
        self.remotes.retain(|s| s.id != id);
        for (i, slot) in self.slots.iter_mut().enumerate() {
            if slot.as_ref().is_some_and(|s| s.id == id) {
                log::info!("gamepad {i} removed");
                *slot = None;
                input::set_gamepad(i, Gamepad::default());
            }
        }
    }

    /// Publishes every pad's state and applies the engine's rumble; while a
    /// page of the application's is open the game gets idle pads and the
    /// buttons navigate it.
    pub fn update(&mut self, page_open: bool, touch_is_pad0: bool) -> Report {
        if glue5_runtime::diagnostic_control::active() {
            self.wait_for_neutral = true;
            return diagnostic_report(self.slots.iter().flatten().chain(&self.remotes).map(|s| read(&s.pad).buttons));
        }
        if self.wait_for_neutral {
            let neutral=self.slots.iter().flatten().chain(&self.remotes).all(|s| {
                let p=read(&s.pad);
                p.buttons==0 && p.left.into_iter().chain(p.right).all(|v|v.abs()<0.15) && p.l2<0.1 && p.r2<0.1
            });
            self.wait_for_neutral=!neutral;
            return Report::default();
        }
        let mut report = Report::default();
        let mut all = 0u16;
        // The left stick pushed far, as the D-pad (for the pages).
        let mut stick = 0u16;
        for remote in &self.remotes {
            let state = read(&remote.pad);
            all |= state.buttons;
            stick |= stick_nav(&state);
        }
        for (i, slot) in self.slots.iter_mut().enumerate() {
            let Some(slot) = slot else { continue };
            let state = read(&slot.pad);
            all |= state.buttons;
            stick |= stick_nav(&state);
            if i == 0 && touch_is_pad0 {
                // The touch controls have pad 0.
            } else if page_open {
                input::set_gamepad(i, Gamepad { connected: true, ..Default::default() });
            } else {
                input::set_gamepad(i, state);
            }

            for (combo, held, fire) in [
                (button::SELECT | button::START, &mut slot.combo, &mut report.menu),
                (button::SELECT | button::R1, &mut slot.friends_combo, &mut report.friends),
            ] {
                if state.buttons & combo == combo {
                    let (since, fired) = held.get_or_insert_with(|| (Instant::now(), false));
                    if !*fired && since.elapsed() >= MENU_HOLD {
                        *fired = true;
                        *fire = true;
                    }
                } else {
                    *held = None;
                }
            }

            // Pad 0's rumble is the touch player's while the touch controls
            // have it (the phone's haptics then).
            let rumble = if i == 0 && touch_is_pad0 { (0, 0) } else { input::rumble(i) };
            // Sent on change, and renewed while it lasts: a request ends
            // after its duration however long the game holds it.
            if rumble != slot.rumble || (rumble != (0, 0) && slot.rumble_at.elapsed() >= RUMBLE_RENEW) {
                slot.rumble = rumble;
                slot.rumble_at = Instant::now();
                let _ = slot.pad.set_rumble(rumble.0, rumble.1, if rumble == (0, 0) { 0 } else { 1000 });
            }
        }
        let all = all | stick;
        if page_open {
            report.nav = self.navigate(all);
        } else {
            self.repeat = None;
        }
        self.previous = all;
        report
    }

    /// Require release before local controller input resumes after ownership changes.
    pub fn reset_after_control(&mut self) {
        self.previous=0;self.repeat=None;self.wait_for_neutral=true;
        for slot in self.slots.iter_mut().flatten().chain(&mut self.remotes) {
            slot.combo=None;slot.friends_combo=None;
            let _=slot.pad.set_rumble(0,0,0);slot.rumble=(0,0);
        }
    }

    /// Page navigation: D-pad/stick repeats, A activates, B goes back.
    fn navigate(&mut self, down: u16) -> Vec<Nav> {
        const DIRECTIONS: [(u16, Nav); 4] =
            [(button::UP, Nav::Up), (button::DOWN, Nav::Down), (button::LEFT, Nav::Left), (button::RIGHT, Nav::Right)];
        let pressed = down & !self.previous;
        let mut nav = Vec::new();
        let now = Instant::now();
        for (bit, n) in DIRECTIONS {
            if pressed & bit != 0 {
                nav.push(n);
                self.repeat = Some((bit, now + REPEAT_AFTER));
            }
        }
        match self.repeat {
            Some((bit, _)) if down & bit == 0 => self.repeat = None,
            Some((bit, at)) if now >= at => {
                nav.extend(DIRECTIONS.iter().filter(|d| d.0 == bit).map(|d| d.1));
                self.repeat = Some((bit, now + REPEAT_EVERY));
            }
            _ => {}
        }
        if pressed & button::SOUTH != 0 {
            nav.push(Nav::Activate);
        }
        if pressed & button::EAST != 0 {
            nav.push(Nav::Back);
        }
        nav
    }
}

/// A pad's state, as the engine reads pads.
fn read(pad: &sdl3::gamepad::Gamepad) -> Gamepad {
    let axis = |a| pad.axis(a) as f32 / 32767.0;
    let mut state = Gamepad {
        connected: true,
        left: [axis(Axis::LeftX), axis(Axis::LeftY)],
        right: [axis(Axis::RightX), axis(Axis::RightY)],
        l2: axis(Axis::TriggerLeft).max(0.0),
        r2: axis(Axis::TriggerRight).max(0.0),
        buttons: 0,
    };
    for &(b, bit) in BUTTONS {
        if pad.button(b) {
            state.buttons |= bit;
        }
    }
    if state.l2 > TRIGGER_THRESHOLD {
        state.buttons |= button::L2;
    }
    if state.r2 > TRIGGER_THRESHOLD {
        state.buttons |= button::R2;
    }
    state
}

/// The left stick pushed far, as the D-pad (for the pages).
fn stick_nav(state: &Gamepad) -> u16 {
    let mut stick = 0;
    for (v, minus, plus) in [(state.left[0], button::LEFT, button::RIGHT), (state.left[1], button::UP, button::DOWN)] {
        stick |= if v < -STICK_NAV {
            minus
        } else if v > STICK_NAV {
            plus
        } else {
            0
        };
    }
    stick
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn diagnostic_emergency_works_with_other_buttons_held() {
        let report = diagnostic_report([button::SELECT | button::START | button::R2 | button::SOUTH].into_iter());
        assert!(report.menu);
        assert!(!report.friends);
        assert!(report.nav.is_empty());
    }

    #[test]
    fn diagnostic_emergency_requires_both_buttons_on_one_controller() {
        let report = diagnostic_report([button::SELECT, button::START].into_iter());
        assert!(!report.menu);
        assert!(report.nav.is_empty());
        assert!(!diagnostic_report(std::iter::empty()).menu);
    }
}
