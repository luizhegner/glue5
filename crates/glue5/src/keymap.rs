//! SDL scancodes to the Windows virtual-key codes the engine's input block
//! uses (by physical key position, like the page's `KeyboardEvent.code` table).

use sdl3::keyboard::Scancode as Sc;

#[allow(clippy::just_underscores_and_digits)] // SDL names the digit keys _0.._9
#[rustfmt::skip]
pub fn vk(sc: Sc) -> Option<u8> {
    use Sc::*;
    Some(match sc {
        A => b'A', B => b'B', C => b'C', D => b'D', E => b'E', F => b'F', G => b'G', H => b'H', I => b'I',
        J => b'J', K => b'K', L => b'L', M => b'M', N => b'N', O => b'O', P => b'P', Q => b'Q', R => b'R',
        S => b'S', T => b'T', U => b'U', V => b'V', W => b'W', X => b'X', Y => b'Y', Z => b'Z',
        _1 => b'1', _2 => b'2', _3 => b'3', _4 => b'4', _5 => b'5', _6 => b'6', _7 => b'7', _8 => b'8',
        _9 => b'9', _0 => b'0',
        Return => 0x0D, Escape => 0x1B, Backspace => 0x08, Tab => 0x09, Space => 0x20,
        Minus => 0xBD, Equals => 0xBB, LeftBracket => 0xDB, RightBracket => 0xDD, Backslash => 0xDC,
        Semicolon => 0xBA, Apostrophe => 0xDE, Grave => 0xC0, Comma => 0xBC, Period => 0xBE, Slash => 0xBF,
        CapsLock => 0x14, NonUsBackslash => 0xE2,
        F1 => 0x70, F2 => 0x71, F3 => 0x72, F4 => 0x73, F5 => 0x74, F6 => 0x75, F7 => 0x76, F8 => 0x77,
        F9 => 0x78, F10 => 0x79, F11 => 0x7A, F12 => 0x7B,
        PrintScreen => 0x2C, ScrollLock => 0x91, Pause => 0x13,
        Insert => 0x2D, Home => 0x24, PageUp => 0x21, Delete => 0x2E, End => 0x23, PageDown => 0x22,
        Right => 0x27, Left => 0x25, Down => 0x28, Up => 0x26,
        NumLockClear => 0x90, KpDivide => 0x6F, KpMultiply => 0x6A, KpMinus => 0x6D, KpPlus => 0x6B,
        KpEnter => 0x0D, KpPeriod => 0x6E,
        Kp0 => 0x60, Kp1 => 0x61, Kp2 => 0x62, Kp3 => 0x63, Kp4 => 0x64, Kp5 => 0x65, Kp6 => 0x66,
        Kp7 => 0x67, Kp8 => 0x68, Kp9 => 0x69,
        LCtrl => 0xA2, LShift => 0xA0, LAlt => 0xA4, LGui => 0x5B,
        RCtrl => 0xA3, RShift => 0xA1, RAlt => 0xA5, RGui => 0x5C, Application => 0x5D,
        // SDL_SCANCODE_AC_BACK (270): Android Back, sent by SDL when its
        // SDL_ANDROID_TRAP_BACK_BUTTON hint is enabled.
        _ if sc as i32 == 270 => 0x1B,
        _ => return None,
    })
}

/// Generic modifier codes the engine also checks (VK_SHIFT, VK_CONTROL, VK_MENU).
pub fn generic(vk: u8) -> Option<u8> {
    match vk {
        0xA0 | 0xA1 => Some(0x10),
        0xA2 | 0xA3 => Some(0x11),
        0xA4 | 0xA5 => Some(0x12),
        _ => None,
    }
}

/// Like the browser page: I/K/J/L/U/O also press numpad 8/2/4/6/7/9 (flight
/// controls on keyboards without a numpad).
pub fn numpad_alias(sc: Sc) -> Option<u8> {
    Some(match sc {
        Sc::I => 0x68,
        Sc::K => 0x62,
        Sc::J => 0x64,
        Sc::L => 0x66,
        Sc::U => 0x67,
        Sc::O => 0x69,
        _ => return None,
    })
}
