//! A colour item's page: the colour large, a palette, hue, saturation and
//! brightness (and opacity) sliders, its hex code. Every change goes to the
//! mod at once.

use egui::*;
use glue5_mods::{MenuInput, MenuItem, MenuKind, Rgba};

use super::super::TextKind;
use super::super::theme::{INK, density};
use super::super::widgets::{Control, focus, setting, slider, text_field};
use super::rows::{check, paint_swatch};
use super::{Draft, Landing};

const PALETTE: [u32; 16] = [
    0xffffff, 0xc0c0c0, 0x808080, 0x202020, 0xe03c3c, 0xf08c32, 0xf0d232, 0x78cd91, 0x2fa85a, 0x3cc8dc, 0x3c78f0,
    0x283ca0, 0x9650dc, 0xe650a0, 0x8c5a3c, 0xf0c8a0,
];

/// The page of `item`; `hsv` keeps the sliders' values between frames,
/// `draft` the hex code being typed.
pub(super) fn page(
    ui: &mut Ui,
    item: &MenuItem,
    hsv: &mut Option<(u32, Rgba, [f32; 3])>,
    draft: &mut Option<Draft>,
    landing: &mut Landing,
) -> Option<MenuInput> {
    let MenuKind::Color { value, alpha } = &item.kind else { return None };
    let (value, alpha) = (*value, *alpha);
    let s = density(ui);
    let mut sent = None;

    // The colour, its code on it.
    let (preview, _) = ui.allocate_exact_size(vec2(ui.available_width(), 56.0 * s), Sense::hover());
    paint_swatch(ui, preview, value, 14.0 * s);
    let ink = if light(value) { INK } else { Color32::WHITE };
    ui.painter().text(preview.center(), Align2::CENTER_CENTER, value.hex(alpha), FontId::proportional(15.0 * s), ink);

    // The palette, wrapping.
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing = vec2(8.0 * s, 8.0 * s);
        for (n, rgb) in PALETTE.into_iter().enumerate() {
            let color = Rgba { r: (rgb >> 16) as u8, g: (rgb >> 8) as u8, b: rgb as u8, a: value.a };
            let (rect, _) = ui.allocate_exact_size(vec2(44.0 * s, 44.0 * s), Sense::hover());
            let response = ui.interact(rect, Id::new(("mod-swatch", item.id, n)), Sense::click());
            response.widget_info(|| WidgetInfo::labeled(WidgetType::Button, ui.is_enabled(), color.hex(false)));
            let radius = 12.0 * s;
            paint_swatch(ui, rect, Rgba { a: 255, ..color }, radius);
            if (color.r, color.g, color.b) == (value.r, value.g, value.b) {
                check(ui, rect.center(), if light(color) { INK } else { Color32::WHITE });
            }
            if response.has_focus() {
                ui.painter().rect_stroke(
                    rect.expand(2.0 * s),
                    radius + 2.0 * s,
                    Stroke::new(1.5, Color32::WHITE),
                    StrokeKind::Outside,
                );
            }
            landing.row(&response);
            if focus(response).clicked() {
                sent = Some(color);
            }
        }
    });

    // Hue, saturation, brightness: a view of the colour; the hue (and the
    // saturation) stay put through grey and black.
    let mut current = match *hsv {
        Some((id, last, kept)) if id == item.id && last == value => kept,
        Some((id, _, kept)) if id == item.id => {
            let mut new = to_hsv(value);
            if new[1] == 0.0 || new[2] == 0.0 {
                new[0] = kept[0];
            }
            if new[2] == 0.0 {
                new[1] = kept[1];
            }
            new
        }
        _ => to_hsv(value),
    };
    let before = current;
    setting(ui, "Hue", Control::Slider, None, |ui| slider(ui, &mut current[0], 0.0..=360.0, 1.0, "°", 0));
    setting(ui, "Saturation", Control::Slider, None, |ui| slider(ui, &mut current[1], 0.0..=1.0, 0.01, "%", 0));
    setting(ui, "Brightness", Control::Slider, None, |ui| slider(ui, &mut current[2], 0.0..=1.0, 0.01, "%", 0));
    let mut opacity = f32::from(value.a) / 255.0;
    if alpha
        && setting(ui, "Opacity", Control::Slider, None, |ui| slider(ui, &mut opacity, 0.0..=1.0, 0.01, "%", 0))
            .changed()
    {
        sent = Some(Rgba { a: (opacity * 255.0).round() as u8, ..value });
    }
    if current != before {
        let color = from_hsv(current, value.a);
        *hsv = Some((item.id, color, current));
        sent = Some(color);
    } else if hsv.is_none_or(|(id, last, _)| id != item.id || last != value) {
        *hsv = Some((item.id, value, current));
    }

    // The code, typed (committed with Enter or by leaving the field).
    let mut text = match draft.as_ref().filter(|d| d.item == item.id) {
        Some(d) => d.text.clone(),
        None => value.hex(alpha),
    };
    let field = text_field(ui, "Hex", TextKind::Address, &mut text, "#rrggbb", if alpha { 9 } else { 7 });
    if field.has_focus() {
        *draft = Some(Draft { item: item.id, text, fresh: false });
    } else if field.lost_focus()
        && let Some(d) = draft.take().filter(|d| d.item == item.id)
        && let Some(color) = Rgba::from_hex(d.text.trim())
    {
        sent = Some(if alpha { color } else { Rgba { a: 255, ..color } });
    }
    sent.filter(|c| *c != value).map(MenuInput::Color)
}

/// Dark text reads on it.
fn light(c: Rgba) -> bool {
    let luma = 0.2126 * f32::from(c.r) + 0.7152 * f32::from(c.g) + 0.0722 * f32::from(c.b);
    luma * f32::from(c.a) / 255.0 > 150.0
}

/// Hue (0–360), saturation and brightness (0–1).
fn to_hsv(c: Rgba) -> [f32; 3] {
    let [r, g, b] = [c.r, c.g, c.b].map(|v| f32::from(v) / 255.0);
    let max = r.max(g).max(b);
    let range = max - r.min(g).min(b);
    let hue = if range == 0.0 {
        0.0
    } else if max == r {
        60.0 * ((g - b) / range).rem_euclid(6.0)
    } else if max == g {
        60.0 * ((b - r) / range + 2.0)
    } else {
        60.0 * ((r - g) / range + 4.0)
    };
    [hue, if max == 0.0 { 0.0 } else { range / max }, max]
}

fn from_hsv([h, s, v]: [f32; 3], a: u8) -> Rgba {
    let c = v * s;
    let h = (h % 360.0) / 60.0;
    let x = c * (1.0 - (h % 2.0 - 1.0).abs());
    let (r, g, b) = match h as u32 {
        0 => (c, x, 0.0),
        1 => (x, c, 0.0),
        2 => (0.0, c, x),
        3 => (0.0, x, c),
        4 => (x, 0.0, c),
        _ => (c, 0.0, x),
    };
    let m = v - c;
    let byte = |f: f32| ((f + m) * 255.0).round().clamp(0.0, 255.0) as u8;
    Rgba { r: byte(r), g: byte(g), b: byte(b), a }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hsv_round_trips() {
        for rgb in PALETTE {
            let c = Rgba { r: (rgb >> 16) as u8, g: (rgb >> 8) as u8, b: rgb as u8, a: 200 };
            assert_eq!(from_hsv(to_hsv(c), 200), c, "{}", c.hex(true));
        }
    }
}
