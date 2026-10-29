//! The rules a menu's values follow, the same whether the mod sets them
//! (builders, assignments) or a player does (`menu_input`).

use std::time::Duration;

use crate::{MenuInput, MenuKind, Rgba};

/// Menus and items of a run of a mod, all together. Not a limit on how
/// menus are made (any number, nested at any depth): the snapshot the app
/// reads every frame is copied on every change, and the Studio gets it as
/// JSON, so their size is bounded as the mods' other budgets are.
pub const ENTRIES: usize = 4096;
/// Options of a run of a mod's choices and multis, all together (and so
/// of one list: its indices are 16-bit), for the same reason.
pub const OPTIONS: usize = 65536;
/// Characters kept of a menu's title, an item's label, a note, an option.
pub const TITLE: usize = 40;
pub const LABEL: usize = 60;
pub const NOTE: usize = 200;
pub const OPTION: usize = 40;
/// A label item's text, a separator's title.
pub const LABEL_TEXT: usize = 200;
pub const SEPARATOR: usize = 40;
/// A text item's longest value, and its default.
pub const TEXT_MAX: u16 = 256;
pub const TEXT_DEFAULT: u16 = 64;
pub const PLACEHOLDER: usize = 60;
/// A button's action, a value's unit.
pub const ACTION: usize = 16;
pub const UNIT: usize = 8;
/// `menu:show()` is honoured once in this long per mod.
pub const SHOW_EVERY: Duration = Duration::from_secs(2);
/// Presses waiting for a mod's turn (values wait one per item).
pub const PRESSES: usize = 64;
/// A number item's side without a bound.
pub const UNBOUNDED: f64 = 1e15;

/// The first `n` characters of `s`.
pub fn cut(s: &str, n: usize) -> String {
    s.chars().take(n).collect()
}

/// A text value: control characters out, `max` characters at most.
pub fn text(s: &str, max: u16) -> String {
    s.chars().filter(|c| *c >= ' ').take(usize::from(max)).collect()
}

/// Decimals of `x` as written (0.25: 2), at most 9.
fn decimals(x: f64) -> i32 {
    let s = format!("{}", x.abs());
    s.split_once('.').map_or(0, |(_, f)| f.len().min(9) as i32)
}

/// `origin + k * step`, without the float noise (0.1 * 3 is 0.3).
fn on_grid(origin: f64, k: f64, step: f64) -> f64 {
    let v = origin + k * step;
    let scale = 10f64.powi(decimals(step).max(decimals(origin)));
    let r = (v * scale).round() / scale;
    if r.is_finite() { r } else { v }
}

/// A slider's value: within its range, on its grid from `min` (the max is
/// reachable too, on the grid or not).
pub fn slider(v: f64, min: f64, max: f64, step: f64) -> f64 {
    if v >= max {
        return max;
    }
    if v <= min {
        return min;
    }
    let s = on_grid(min, ((v - min) / step).round(), step).min(max);
    // Nearer the max than the grid's last point.
    if max - v < (v - s).abs() { max } else { s }
}

/// A number item's bounds (a side without one is ±1e15).
pub fn bounds(min: Option<f64>, max: Option<f64>) -> (f64, f64) {
    (min.unwrap_or(-UNBOUNDED), max.unwrap_or(UNBOUNDED))
}

/// A number typed by a player: rounded to the grid from `min` (or 0), then
/// within the bounds.
pub fn number(v: f64, min: Option<f64>, max: Option<f64>, step: f64) -> f64 {
    let origin = min.unwrap_or(0.0);
    let (lo, hi) = bounds(min, max);
    on_grid(origin, ((v - origin) / step).round(), step).clamp(lo, hi)
}

/// `#rrggbb` or `#rrggbbaa` (the `#` may be left out).
pub fn parse_hex(s: &str) -> Option<Rgba> {
    let h = s.strip_prefix('#').unwrap_or(s);
    if !(h.len() == 6 || h.len() == 8) || !h.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    let byte = |i: usize| u8::from_str_radix(&h[i..i + 2], 16).ok();
    Some(Rgba { r: byte(0)?, g: byte(2)?, b: byte(4)?, a: if h.len() == 8 { byte(6)? } else { 255 } })
}

/// The kind's name, as Luau and the JSON say it.
pub fn kind_name(kind: &MenuKind) -> &'static str {
    match kind {
        MenuKind::Button { .. } => "button",
        MenuKind::Toggle(_) => "toggle",
        MenuKind::Slider { .. } => "slider",
        MenuKind::Number { .. } => "number",
        MenuKind::Choice { .. } => "choice",
        MenuKind::Multi { .. } => "multi",
        MenuKind::Text { .. } => "text",
        MenuKind::Color { .. } => "color",
        MenuKind::Label => "label",
        MenuKind::Separator => "separator",
        MenuKind::Progress(_) => "progress",
        MenuKind::Submenu(_) => "submenu",
    }
}

/// A player's input checked against the item and made to fit it; `kind`
/// takes the new value. What the mod hears.
pub fn input(kind: &mut MenuKind, input: MenuInput) -> Result<MenuInput, String> {
    let wrong = |kind: &MenuKind| {
        let takes = match kind {
            MenuKind::Button { .. } => "no value (it is pressed)",
            MenuKind::Toggle(_) => "true or false",
            MenuKind::Slider { .. } | MenuKind::Number { .. } => "a number",
            MenuKind::Choice { .. } => "one option",
            MenuKind::Multi { .. } => "a list of options",
            MenuKind::Text { .. } => "text",
            MenuKind::Color { .. } => "a colour",
            _ => return format!("a {} takes no input", kind_name(kind)),
        };
        format!("a {} takes {takes}", kind_name(kind))
    };
    let finite = |v: f64| if v.is_finite() { Ok(v) } else { Err("the value must be a finite number".to_string()) };
    Ok(match (kind, input) {
        (MenuKind::Button { .. }, MenuInput::Press) => MenuInput::Press,
        (MenuKind::Toggle(on), MenuInput::Toggle(v)) => {
            *on = v;
            MenuInput::Toggle(v)
        }
        (MenuKind::Slider { value, min, max, step, .. }, MenuInput::Number(v)) => {
            *value = slider(finite(v)?, *min, *max, *step);
            MenuInput::Number(*value)
        }
        (MenuKind::Number { value, min, max, step, .. }, MenuInput::Number(v)) => {
            *value = number(finite(v)?, *min, *max, *step);
            MenuInput::Number(*value)
        }
        (MenuKind::Choice { options, index }, MenuInput::Choose(i)) => {
            if i >= options.len() {
                return Err(format!("no option {}: there are {}", i + 1, options.len()));
            }
            *index = i;
            MenuInput::Choose(i)
        }
        (MenuKind::Multi { options, selected }, MenuInput::Select(mut v)) => {
            if let Some(i) = v.iter().find(|i| usize::from(**i) >= options.len()) {
                return Err(format!("no option {}: there are {}", i + 1, options.len()));
            }
            v.sort_unstable();
            v.dedup();
            *selected = v.clone();
            MenuInput::Select(v)
        }
        (MenuKind::Text { value, max, .. }, MenuInput::Text(t)) => {
            *value = text(&t, *max);
            MenuInput::Text(value.clone())
        }
        (MenuKind::Color { value, alpha }, MenuInput::Color(mut c)) => {
            if !*alpha {
                c.a = 255;
            }
            *value = c;
            MenuInput::Color(c)
        }
        (kind, _) => return Err(wrong(kind)),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn grids() {
        assert_eq!(slider(0.31, 0.0, 1.0, 0.1), 0.3);
        assert_eq!(slider(19.4, 0.0, 23.0, 1.0), 19.0);
        // The max is reachable off the grid.
        assert_eq!(slider(9.9, 0.0, 10.0, 3.0), 10.0);
        assert_eq!(slider(50.0, 0.0, 10.0, 3.0), 10.0);
        assert_eq!(slider(-1.0, 0.0, 10.0, 3.0), 0.0);
        assert_eq!(number(1234.0, None, None, 100.0), 1200.0);
        assert_eq!(number(1234.0, Some(5.0), Some(1000.0), 100.0), 1000.0);
        assert_eq!(number(1.26, None, None, 0.25), 1.25);
        assert_eq!(number(-5e20, None, None, 1.0), -UNBOUNDED);
    }

    #[test]
    fn hex() {
        assert_eq!(parse_hex("#ff8800"), Some(Rgba { r: 255, g: 136, b: 0, a: 255 }));
        assert_eq!(parse_hex("FF880080"), Some(Rgba { r: 255, g: 136, b: 0, a: 128 }));
        assert_eq!(parse_hex("#ff880"), None);
        assert_eq!(parse_hex("#gg8800"), None);
        assert_eq!(Rgba { r: 255, g: 136, b: 0, a: 128 }.hex(true), "#ff880080");
        assert_eq!(Rgba { r: 255, g: 136, b: 0, a: 128 }.hex(false), "#ff8800");
    }

    /// Every input against every kind: what fits, what is made to fit,
    /// what is refused.
    #[test]
    fn inputs() {
        let opts: std::sync::Arc<[String]> = vec!["a".to_string(), "b".into(), "c".into()].into();
        let mut k = MenuKind::Slider { value: 0.0, min: 0.0, max: 1.0, step: 0.1, unit: None };
        assert_eq!(input(&mut k, MenuInput::Number(0.47)), Ok(MenuInput::Number(0.5)));
        assert!(input(&mut k, MenuInput::Number(f64::NAN)).unwrap_err().contains("finite"));
        assert!(input(&mut k, MenuInput::Number(f64::INFINITY)).is_err());
        assert_eq!(input(&mut k, MenuInput::Number(7.0)), Ok(MenuInput::Number(1.0)));
        assert_eq!(input(&mut k, MenuInput::Toggle(true)).unwrap_err(), "a slider takes a number");
        let mut k = MenuKind::Number { value: 0.0, min: Some(0.0), max: None, step: 100.0, unit: None };
        assert_eq!(input(&mut k, MenuInput::Number(-30.0)), Ok(MenuInput::Number(0.0)));
        assert_eq!(input(&mut k, MenuInput::Number(1e30)), Ok(MenuInput::Number(UNBOUNDED)));
        let mut k = MenuKind::Choice { options: opts.clone(), index: 0 };
        assert_eq!(input(&mut k, MenuInput::Choose(2)), Ok(MenuInput::Choose(2)));
        assert_eq!(input(&mut k, MenuInput::Choose(3)).unwrap_err(), "no option 4: there are 3");
        assert_eq!(k, MenuKind::Choice { options: opts.clone(), index: 2 });
        let mut k = MenuKind::Multi { options: opts.clone(), selected: vec![] };
        assert_eq!(input(&mut k, MenuInput::Select(vec![2, 0, 2])), Ok(MenuInput::Select(vec![0, 2])));
        assert!(input(&mut k, MenuInput::Select(vec![0, 3])).is_err());
        assert_eq!(k, MenuKind::Multi { options: opts.clone(), selected: vec![0, 2] });
        let mut k = MenuKind::Text { value: String::new(), placeholder: None, max: 4 };
        assert_eq!(input(&mut k, MenuInput::Text("a\nbcdef".into())), Ok(MenuInput::Text("abcd".into())));
        let mut k = MenuKind::Color { value: Rgba { r: 0, g: 0, b: 0, a: 255 }, alpha: false };
        let half = Rgba { r: 1, g: 2, b: 3, a: 128 };
        assert_eq!(input(&mut k, MenuInput::Color(half)), Ok(MenuInput::Color(Rgba { a: 255, ..half })));
        let mut k = MenuKind::Color { value: half, alpha: true };
        assert_eq!(input(&mut k, MenuInput::Color(half)), Ok(MenuInput::Color(half)));
        let mut k = MenuKind::Button { action: None };
        assert_eq!(input(&mut k, MenuInput::Press), Ok(MenuInput::Press));
        assert_eq!(input(&mut k, MenuInput::Toggle(true)).unwrap_err(), "a button takes no value (it is pressed)");
        let mut k = MenuKind::Toggle(false);
        assert_eq!(input(&mut k, MenuInput::Press).unwrap_err(), "a toggle takes true or false");
        for mut k in [MenuKind::Label, MenuKind::Separator, MenuKind::Progress(0.5), MenuKind::Submenu(3)] {
            let name = kind_name(&k);
            assert_eq!(input(&mut k, MenuInput::Press).unwrap_err(), format!("a {name} takes no input"));
        }
    }
}
