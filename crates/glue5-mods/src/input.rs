//! Mods' input: the keyboard, the mouse and the player's pad, as the game
//! gets them (`input.isDown`, `input.pressed`, `input.onKey` in Luau).
//!
//! The application reports keys and mouse buttons where it hands them to
//! the game ([`key`], [`mouse_button`], [`mouse_wheel`], [`release_all`]):
//! each is an event in a short queue, under a lock held for the push. The
//! player's pad is read from the runtime (`take_gamepad_changes`) as a mod
//! takes its turn. A mod takes what came since its last turn at the start
//! of the turn, on the engine's script update: `isDown`/`pressed` answer
//! from that, and its key handlers run as tasks there, never on the
//! application's thread.
//!
//! What the mods see is what reaches the game: nothing while one of the
//! application's pages has the keys (the settings, the mods' menus, a text
//! field), and no key presses while the game's own text box takes typed
//! text. The game sees every key too: a mod that takes over one of the
//! game's controls disables it each frame (`pad.disableControlAction`), as
//! the game's scripts do.

use std::cell::RefCell;
use std::collections::{HashMap, VecDeque};
use std::rc::Rc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};

use glue5_runtime::input::mouse;
use mlua::{Function, Lua, MultiValue, Value};

/// A key, a mouse button or a pad button: keyboard keys by Windows
/// virtual-key code (what the game's input block takes), mouse buttons
/// from [`MOUSE`], pad 0's buttons from [`PAD`] (plus the RAGE bit's index).
type Code = u16;
const MOUSE: Code = 0x100;
const WHEEL_UP: Code = MOUSE + 5;
const WHEEL_DOWN: Code = MOUSE + 6;
const PAD: Code = 0x200;
const CODES: usize = 0x300;

/// The names mods use: physical keys named as on a US keyboard (the key
/// right of Tab is "Q" whatever its label, as for the game's controls), the
/// mouse's buttons and wheel, the pad's buttons by position (SDL's names:
/// "PadSouth" is A on an Xbox pad, Cross on a PlayStation one).
#[rustfmt::skip]
const NAMES: &[(Code, &str)] = &[
    (0x41, "A"), (0x42, "B"), (0x43, "C"), (0x44, "D"), (0x45, "E"), (0x46, "F"), (0x47, "G"), (0x48, "H"),
    (0x49, "I"), (0x4A, "J"), (0x4B, "K"), (0x4C, "L"), (0x4D, "M"), (0x4E, "N"), (0x4F, "O"), (0x50, "P"),
    (0x51, "Q"), (0x52, "R"), (0x53, "S"), (0x54, "T"), (0x55, "U"), (0x56, "V"), (0x57, "W"), (0x58, "X"),
    (0x59, "Y"), (0x5A, "Z"),
    (0x30, "0"), (0x31, "1"), (0x32, "2"), (0x33, "3"), (0x34, "4"), (0x35, "5"), (0x36, "6"), (0x37, "7"),
    (0x38, "8"), (0x39, "9"),
    (0x70, "F1"), (0x71, "F2"), (0x72, "F3"), (0x73, "F4"), (0x74, "F5"), (0x75, "F6"), (0x76, "F7"),
    (0x77, "F8"), (0x78, "F9"), (0x79, "F10"), (0x7A, "F11"), (0x7B, "F12"),
    (0x0D, "Enter"), (0x1B, "Escape"), (0x08, "Backspace"), (0x09, "Tab"), (0x20, "Space"),
    (0xBD, "Minus"), (0xBB, "Equals"), (0xDB, "LeftBracket"), (0xDD, "RightBracket"), (0xDC, "Backslash"),
    (0xBA, "Semicolon"), (0xDE, "Apostrophe"), (0xC0, "Grave"), (0xBC, "Comma"), (0xBE, "Period"),
    (0xBF, "Slash"), (0xE2, "NonUsBackslash"), (0x14, "CapsLock"),
    (0x2C, "PrintScreen"), (0x91, "ScrollLock"), (0x13, "Pause"),
    (0x2D, "Insert"), (0x24, "Home"), (0x21, "PageUp"), (0x2E, "Delete"), (0x23, "End"), (0x22, "PageDown"),
    (0x26, "Up"), (0x28, "Down"), (0x25, "Left"), (0x27, "Right"),
    (0x90, "NumLock"), (0x6F, "KeypadDivide"), (0x6A, "KeypadMultiply"), (0x6D, "KeypadMinus"),
    (0x6B, "KeypadPlus"), (0x6E, "KeypadPeriod"),
    (0x60, "Keypad0"), (0x61, "Keypad1"), (0x62, "Keypad2"), (0x63, "Keypad3"), (0x64, "Keypad4"),
    (0x65, "Keypad5"), (0x66, "Keypad6"), (0x67, "Keypad7"), (0x68, "Keypad8"), (0x69, "Keypad9"),
    (0xA0, "LeftShift"), (0xA1, "RightShift"), (0xA2, "LeftCtrl"), (0xA3, "RightCtrl"),
    (0xA4, "LeftAlt"), (0xA5, "RightAlt"), (0x5B, "LeftMeta"), (0x5C, "RightMeta"), (0x5D, "Menu"),
    (MOUSE, "MouseLeft"), (MOUSE + 1, "MouseRight"), (MOUSE + 2, "MouseMiddle"), (MOUSE + 3, "MouseX1"),
    (MOUSE + 4, "MouseX2"), (WHEEL_UP, "MouseWheelUp"), (WHEEL_DOWN, "MouseWheelDown"),
    (PAD + 6, "PadSouth"), (PAD + 5, "PadEast"), (PAD + 7, "PadWest"), (PAD + 4, "PadNorth"),
    (PAD + 2, "PadLeftShoulder"), (PAD + 3, "PadRightShoulder"), (PAD, "PadLeftTrigger"), (PAD + 1, "PadRightTrigger"),
    (PAD + 9, "PadLeftStick"), (PAD + 10, "PadRightStick"), (PAD + 8, "PadBack"), (PAD + 11, "PadStart"),
    (PAD + 12, "PadUp"), (PAD + 14, "PadDown"), (PAD + 15, "PadLeft"), (PAD + 13, "PadRight"),
];

/// Events kept for the mods (a mod that waits long between its turns gets
/// the latest of them; what is held is always exact).
const EVENTS: usize = 256;

/// Modifier bits: held as the event happened (after it: a Shift's press
/// has shift).
const SHIFT: u8 = 1;
const CTRL: u8 = 2;
const ALT: u8 = 4;
const META: u8 = 8;
const MODIFIERS: &[(Code, Code, u8)] = &[(0xA0, 0xA1, SHIFT), (0xA2, 0xA3, CTRL), (0xA4, 0xA5, ALT), (0x5B, 0x5C, META)];

#[derive(Clone, Copy, Default, PartialEq, Eq, Debug)]
struct Bits([u64; CODES / 64]);

impl Bits {
    fn get(&self, c: Code) -> bool {
        self.0[c as usize / 64] & (1 << (c % 64)) != 0
    }

    fn set(&mut self, c: Code, on: bool) {
        let (word, bit) = (c as usize / 64, 1 << (c % 64));
        if on {
            self.0[word] |= bit;
        } else {
            self.0[word] &= !bit;
        }
    }

    fn codes(&self) -> impl Iterator<Item = Code> + '_ {
        (0..CODES as Code).filter(|&c| self.get(c))
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
struct Event {
    /// Increasing: what a mod has taken is below its cursor.
    seq: u64,
    code: Code,
    down: bool,
    mods: u8,
}

/// What reached the game, for the mods.
#[derive(Default)]
struct Feed {
    held: Bits,
    events: VecDeque<Event>,
    next: u64,
}

impl Feed {
    const fn new() -> Feed {
        Feed { held: Bits([0; CODES / 64]), events: VecDeque::new(), next: 0 }
    }

    /// A key or button goes down or up; a wheel's notch does both. Nothing
    /// for what is already so (a key held while a page of the app opened
    /// and closed).
    fn push(&mut self, code: Code, down: bool) {
        if matches!(code, WHEEL_UP | WHEEL_DOWN) {
            if down {
                self.event(code, true);
                self.event(code, false);
            }
            return;
        }
        if self.held.get(code) == down {
            return;
        }
        self.held.set(code, down);
        self.event(code, down);
    }

    fn event(&mut self, code: Code, down: bool) {
        let mods = MODIFIERS.iter().filter(|(l, r, _)| self.held.get(*l) || self.held.get(*r)).fold(0, |m, (_, _, b)| m | b);
        if self.events.len() == EVENTS {
            self.events.pop_front();
        }
        self.events.push_back(Event { seq: self.next, code, down, mods });
        self.next += 1;
    }

    /// Everything up (the window lost the focus, a page of the app opened).
    fn release_all(&mut self) {
        let held: Vec<Code> = self.held.codes().collect();
        for code in held {
            self.push(code, false);
        }
    }

    /// Pad 0's buttons: held now, pressed and released since the last read
    /// (a tap between two reads is a press and a release).
    fn pad(&mut self, held: u16, pressed: u16, released: u16) {
        for bit in 0..16 {
            let (code, mask) = (PAD + bit, 1u16 << bit);
            if self.held.get(code) && released & mask != 0 {
                self.push(code, false);
            }
            if pressed & mask != 0 {
                self.push(code, true);
            }
            self.push(code, held & mask != 0);
        }
    }

    /// The events from `cursor` on.
    fn since(&self, cursor: u64) -> impl Iterator<Item = &Event> {
        let new = self.events.iter().rev().take_while(|e| e.seq >= cursor).count();
        self.events.iter().skip(self.events.len() - new)
    }
}

static FEED: Mutex<Feed> = Mutex::new(Feed::new());
/// The mouse's position, fractions of the screen (x, y as f32 bits).
static POINTER: AtomicU64 = AtomicU64::new(0x3F00_0000_3F00_0000);

fn feed() -> std::sync::MutexGuard<'static, Feed> {
    crate::lock(&FEED)
}

/// A keyboard key went down or up, by Windows virtual-key code (as the
/// game's input block takes it; the generic Shift/Ctrl/Alt codes and other
/// aliases are not keys of their own). Not while the game's text box takes
/// typed text: its keys are text.
pub fn key(vk: u8, down: bool) {
    if !crate::installed() || !NAMES.iter().any(|(c, _)| *c == Code::from(vk)) {
        return;
    }
    if down && glue5_runtime::input::text_box().is_some() {
        return;
    }
    feed().push(Code::from(vk), down);
}

/// A mouse button (`glue5_runtime::input::mouse`'s bits) went down or up.
pub fn mouse_button(bit: u32, down: bool) {
    let index = [mouse::LEFT, mouse::RIGHT, mouse::MIDDLE, mouse::X1, mouse::X2].iter().position(|b| *b == bit);
    if let Some(i) = index
        && crate::installed()
    {
        feed().push(MOUSE + i as Code, down);
    }
}

/// The wheel turned this many notches (+ is up).
pub fn mouse_wheel(notches: i32) {
    if notches == 0 || !crate::installed() {
        return;
    }
    let mut feed = feed();
    let code = if notches > 0 { WHEEL_UP } else { WHEEL_DOWN };
    for _ in 0..notches.unsigned_abs().min(8) {
        feed.push(code, true);
    }
}

/// Where the mouse is, as fractions of the screen.
pub fn mouse_position(x: f32, y: f32) {
    let (x, y) = (x.clamp(0.0, 1.0), y.clamp(0.0, 1.0));
    POINTER.store(u64::from(x.to_bits()) << 32 | u64::from(y.to_bits()), Ordering::Relaxed);
}

/// Every key and button is up for the mods (the window lost the focus, a
/// page of the application has them now).
pub fn release_all() {
    feed().release_all();
}

/// The virtual-key code of a keyboard key's name (case aside), for hosts
/// that take names (the headless runner's `key F5`).
pub fn vk(name: &str) -> Option<u8> {
    NAMES.iter().find(|(c, n)| *c < MOUSE && n.eq_ignore_ascii_case(name)).map(|(c, _)| *c as u8)
}

fn code(name: &str) -> Option<Code> {
    static BY_NAME: OnceLock<HashMap<&'static str, Code>> = OnceLock::new();
    BY_NAME.get_or_init(|| NAMES.iter().map(|(c, n)| (*n, *c)).collect()).get(name).copied()
}

fn name(code: Code) -> &'static str {
    NAMES.iter().find(|(c, _)| *c == code).map_or("", |(_, n)| n)
}

/// A mod's view: its cursor in the feed, what this turn saw, its handlers.
#[derive(Clone, Default)]
pub struct Keys(Rc<RefCell<View>>);

#[derive(Default)]
struct View {
    cursor: u64,
    held: Bits,
    pressed: Bits,
    released: Bits,
    handlers: Vec<(u32, Function)>,
    next_handler: u32,
}

impl Keys {
    /// Whether a handler listens (the mod then runs every frame).
    pub fn listening(&self) -> bool {
        !self.0.borrow().handlers.is_empty()
    }

    /// The mod's turn starts: what came since its last one, and the handler
    /// calls it makes, to run as tasks.
    pub fn take(&self, lua: &Lua) -> mlua::Result<Vec<(Function, MultiValue)>> {
        let mut feed = feed();
        pull_pad(&mut feed);
        let events = self.turn(&feed);
        drop(feed);
        self.calls(lua, &events)
    }

    fn turn(&self, feed: &Feed) -> Vec<Event> {
        let mut v = self.0.borrow_mut();
        let events: Vec<Event> = feed.since(v.cursor).copied().collect();
        v.cursor = feed.next;
        v.held = feed.held;
        v.pressed = Bits::default();
        v.released = Bits::default();
        for e in &events {
            if e.down {
                v.pressed.set(e.code, true);
            } else {
                v.released.set(e.code, true);
            }
        }
        events
    }

    fn calls(&self, lua: &Lua, events: &[Event]) -> mlua::Result<Vec<(Function, MultiValue)>> {
        let handlers: Vec<Function> = self.0.borrow().handlers.iter().map(|(_, f)| f.clone()).collect();
        let mut calls = Vec::new();
        if handlers.is_empty() {
            return Ok(calls);
        }
        for e in events {
            for f in &handlers {
                let mods = lua.create_table()?;
                mods.set("shift", e.mods & SHIFT != 0)?;
                mods.set("ctrl", e.mods & CTRL != 0)?;
                mods.set("alt", e.mods & ALT != 0)?;
                mods.set("meta", e.mods & META != 0)?;
                let args = MultiValue::from_vec(vec![
                    Value::String(lua.create_string(name(e.code))?),
                    Value::Boolean(e.down),
                    Value::Table(mods),
                ]);
                calls.push((f.clone(), args));
            }
        }
        Ok(calls)
    }
}

/// Pad 0's buttons since the last read, into the feed.
fn pull_pad(feed: &mut Feed) {
    let (held, pressed, released) = glue5_runtime::input::take_gamepad_changes(0);
    feed.pad(held, pressed, released);
}

fn checked(function: &str, key: &str) -> mlua::Result<Code> {
    code(key).ok_or_else(|| {
        mlua::Error::runtime(format!(
            "input.{function}: no key \"{key}\" (keys are named as on a US keyboard: \"A\", \"F5\", \"LeftShift\", \
             \"Space\"; \"MouseLeft\", \"PadSouth\")"
        ))
    })
}

/// The `input` table; `keys` is the mod's view, which starts now (what came
/// before is not the mod's).
pub fn install(lua: &Lua, keys: &Keys) -> mlua::Result<()> {
    {
        let mut feed = feed();
        pull_pad(&mut feed);
        keys.0.borrow_mut().cursor = feed.next;
    }
    let input = lua.create_table()?;
    for (field, which) in [("isDown", 0), ("pressed", 1), ("released", 2)] {
        let k = keys.clone();
        input.set(
            field,
            lua.create_function(move |_, key: mlua::LuaString| {
                let code = checked(field, &key.to_str()?)?;
                let v = k.0.borrow();
                Ok([&v.held, &v.pressed, &v.released][which].get(code))
            })?,
        )?;
    }
    let k = keys.clone();
    input.set(
        "onKey",
        lua.create_function(move |lua, handler: Function| {
            let id = {
                let mut v = k.0.borrow_mut();
                v.next_handler += 1;
                let id = v.next_handler;
                v.handlers.push((id, handler));
                id
            };
            let connection = lua.create_table()?;
            let k = k.clone();
            connection.set(
                "remove",
                lua.create_function(move |_, _: Value| {
                    k.0.borrow_mut().handlers.retain(|(i, _)| *i != id);
                    Ok(())
                })?,
            )?;
            Ok(connection)
        })?,
    )?;
    input.set("keys", lua.create_function(|lua, ()| lua.create_sequence_from(NAMES.iter().map(|(_, n)| *n)))?)?;
    input.set(
        "mousePosition",
        lua.create_function(|_, ()| {
            let p = POINTER.load(Ordering::Relaxed);
            Ok((f32::from_bits((p >> 32) as u32), f32::from_bits(p as u32)))
        })?,
    )?;
    lua.globals().set("input", input)
}

/// The `input` table's Luau definitions.
pub fn definitions() -> String {
    let keys: Vec<String> = NAMES.iter().map(|(_, n)| format!("\"{n}\"")).collect();
    format!(
        "\
-- A keyboard key (a physical key, named as on a US keyboard), a mouse button or the wheel, a pad button
-- (by position: PadSouth is A on an Xbox pad, Cross on a PlayStation one).
type Key = {}
-- The modifiers held when a key went down or up (after it: a Shift's press has shift). meta is Cmd on a Mac.
export type KeyModifiers = {{ shift: boolean, ctrl: boolean, alt: boolean, meta: boolean }}
export type KeyHandler = {{ remove: (self: KeyHandler) -> () }}

-- The keyboard, the mouse and the player's pad as the game gets them: nothing while a page of the app
-- has them or the game's text box takes typed text. The game sees them too.
declare input: {{
\t-- Whether the key is held (as of this turn).
\tisDown: (key: Key) -> boolean,
\t-- Whether the key went down since the mod's last turn (the last frame, for a mod that waits a frame).
\tpressed: (key: Key) -> boolean,
\t-- Whether the key went up since the mod's last turn.
\treleased: (key: Key) -> boolean,
\t-- Calls `handler` for every key that goes down or up, as a task at the mod's next turn, in order;
\t-- a mod with a handler stays running. No repeats while a key is held.
\tonKey: (handler: (key: Key, down: boolean, mods: KeyModifiers) -> ()) -> KeyHandler,
\t-- Where the mouse is, as fractions of the screen (0..1, from the top left).
\tmousePosition: () -> (number, number),
\t-- Every key's name: the keyboard's, the mouse's, the pad's (a mod's key bindings offer them).
\tkeys: () -> {{ Key }},
}}

",
        keys.join(" | ")
    )
}

#[cfg(test)]
mod tests {
    use glue5_runtime::input::button;

    use super::*;

    fn names(feed: &Feed, cursor: u64) -> Vec<(&'static str, bool)> {
        feed.since(cursor).map(|e| (name(e.code), e.down)).collect()
    }

    #[test]
    fn names_are_unique_and_found() {
        let mut seen = std::collections::HashSet::new();
        for (c, n) in NAMES {
            assert!(seen.insert(*n), "{n} twice");
            assert_eq!(code(n), Some(*c));
            assert!((*c as usize) < CODES);
        }
        let codes: std::collections::HashSet<Code> = NAMES.iter().map(|(c, _)| *c).collect();
        assert_eq!(codes.len(), NAMES.len());
        assert_eq!(vk("f5"), Some(0x74));
        assert_eq!(vk("leftshift"), Some(0xA0));
        assert_eq!(vk("MouseLeft"), None);
        // The pad's names are the runtime's bits.
        assert_eq!(1 << (code("PadSouth").unwrap() - PAD), button::SOUTH);
        assert_eq!(1 << (code("PadNorth").unwrap() - PAD), button::NORTH);
        assert_eq!(1 << (code("PadBack").unwrap() - PAD), button::SELECT);
        assert_eq!(1 << (code("PadRight").unwrap() - PAD), button::RIGHT);
        assert_eq!(1 << (code("PadLeftTrigger").unwrap() - PAD), button::L2);
    }

    /// Repeated states make no events; modifiers are as after the event;
    /// a wheel's notch is a press and a release.
    #[test]
    fn events() {
        let mut f = Feed::new();
        f.push(0xA0, true);
        f.push(0x41, true);
        f.push(0x41, true);
        f.push(0x41, false);
        f.push(0x41, false);
        f.push(WHEEL_UP, true);
        assert_eq!(
            names(&f, 0),
            [("LeftShift", true), ("A", true), ("A", false), ("MouseWheelUp", true), ("MouseWheelUp", false)]
        );
        assert!(f.events.iter().all(|e| e.mods == SHIFT));
        assert!(!f.held.get(WHEEL_UP));
        f.push(0xA0, false);
        assert_eq!(f.events.back().unwrap().mods, 0);
        // From a cursor on.
        assert_eq!(names(&f, 4), [("MouseWheelUp", false), ("LeftShift", false)]);
        assert_eq!(names(&f, f.next), []);
    }

    #[test]
    fn release_all_and_bounded() {
        let mut f = Feed::new();
        f.push(0x41, true);
        f.push(MOUSE, true);
        f.release_all();
        assert_eq!(names(&f, 2), [("A", false), ("MouseLeft", false)]);
        assert_eq!(f.held, Bits::default());
        for i in 0..1000 {
            f.push(0x41, i % 2 == 0);
        }
        assert_eq!(f.events.len(), EVENTS);
        assert_eq!(f.since(0).count(), EVENTS);
        assert_eq!(f.events.back().unwrap().seq, f.next - 1);
    }

    /// A tap between two reads of the pad is a press and a release; a
    /// release and a new press, a release and a press.
    #[test]
    fn pad_taps() {
        let mut f = Feed::new();
        f.pad(0, button::SOUTH, button::SOUTH);
        assert_eq!(names(&f, 0), [("PadSouth", true), ("PadSouth", false)]);
        f.pad(button::EAST, button::EAST, 0);
        assert_eq!(names(&f, 2), [("PadEast", true)]);
        f.pad(button::EAST, button::EAST, button::EAST);
        assert_eq!(names(&f, 3), [("PadEast", false), ("PadEast", true)]);
        f.pad(0, 0, button::EAST);
        assert_eq!(names(&f, 5), [("PadEast", false)]);
        f.pad(0, 0, 0);
        assert_eq!(f.next, 6);
    }

    /// The Luau side: a turn's view, the handlers' calls in order, a
    /// handler removed, a misspelt key.
    #[test]
    fn luau() {
        let lua = Lua::new();
        let keys = Keys::default();
        install(&lua, &keys).unwrap();
        lua.load(
            "seen = {}\n\
             h = input.onKey(function(key, down, mods) table.insert(seen, `{key} {down} {mods.shift}`) end)",
        )
        .exec()
        .unwrap();
        let mut f = Feed::new();
        keys.0.borrow_mut().cursor = 0;
        f.push(0xA1, true);
        f.push(0x74, true);
        f.push(0x74, false);
        f.push(MOUSE + 1, true);
        let events = keys.turn(&f);
        for (handler, args) in keys.calls(&lua, &events).unwrap() {
            handler.call::<()>(args).unwrap();
        }
        let check = |code: &str| lua.load(code).eval::<bool>().unwrap();
        assert!(check("input.pressed('F5') and input.released('F5') and not input.isDown('F5')"));
        assert!(check("input.isDown('RightShift') and input.isDown('MouseRight') and not input.released('MouseRight')"));
        assert!(check(
            "seen[1] == 'RightShift true true' and seen[2] == 'F5 true true' and seen[3] == 'F5 false true' \
             and seen[4] == 'MouseRight true true' and #seen == 4"
        ));
        // The next turn: only what came since.
        f.push(MOUSE + 1, false);
        let events = keys.turn(&f);
        assert_eq!(events.len(), 1);
        assert!(check("not input.pressed('F5') and input.released('MouseRight')"));
        lua.load("h:remove()").exec().unwrap();
        assert!(!keys.listening());
        assert!(keys.calls(&lua, &events).unwrap().is_empty());
        let e = lua.load("input.isDown('Shift')").exec().unwrap_err().to_string();
        assert!(e.contains("no key \"Shift\""), "{e}");
        let names: Vec<String> = lua.load("input.keys()").eval().unwrap();
        assert_eq!(names.len(), NAMES.len());
        assert!(names.iter().any(|n| n == "F5") && names.iter().any(|n| n == "PadSouth"));
        let (x, y): (f32, f32) = lua.load("input.mousePosition()").eval().unwrap();
        assert!((0.0..=1.0).contains(&x) && (0.0..=1.0).contains(&y));
    }

    /// The checker knows the API: a mod that uses it is clean, a misspelt
    /// key is an error.
    #[test]
    fn typed() {
        let source = "--!strict\n\
            local h = input.onKey(function(key, down, mods)\n\
            \tif key == \"F5\" and down and mods.shift then print(input.mousePosition()) end\n\
            end)\n\
            if input.pressed(\"PadSouth\") or input.isDown(\"MouseLeft\") or input.released(\"LeftCtrl\") then h:remove() end\n\
            local x: number, y: number = input.mousePosition()\n\
            local first: string = input.keys()[1]\n\
            print(x + y, first)\n";
        let typo = "--!strict\nlocal down = input.isDown(\"Shift\")\n";
        crate::checker::Checker::with(|c| {
            c.set("input-test/main.luau", Some(source));
            let clean = c.check("input-test/main.luau");
            assert_eq!(clean, serde_json::json!([]), "{clean}");
            c.set("input-test/typo.luau", Some(typo));
            let errors = c.check("input-test/typo.luau").to_string();
            assert!(errors.contains("Shift"), "{errors}");
        })
        .unwrap();
    }
}
