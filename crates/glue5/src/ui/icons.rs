//! Small vector icons for Settings: consistent across platforms and independent
//! of emoji or an installed icon font.
use egui::{Align2, Color32, FontId, Rect, Stroke, StrokeKind, Ui, Vec2, pos2, vec2};

#[derive(Clone, Copy)]
pub(super) enum Icon {
    Display,
    Touch,
    Game,
    Logs,
    Adjust,
    Aim,
    Storage,
    Community,
    Shield,
    Settings,
}

pub(super) fn tile(ui: &Ui, rect: Rect, icon: Icon, tint: Color32, fill: Color32) {
    let scale = (rect.width().min(rect.height()) / 40.0).clamp(0.65, 1.3);
    let painter = ui.painter();
    painter.rect_filled(rect, 8, fill);
    let center = rect.center();
    let stroke = Stroke::new(1.8 * scale, tint);
    match icon {
        Icon::Display => {
            let screen = Rect::from_center_size(center + vec2(0.0, -2.0 * scale), vec2(22.0, 15.0) * scale);
            painter.rect_stroke(screen, 3, stroke, StrokeKind::Inside);
            painter.line_segment([center + vec2(0.0, 5.0) * scale, center + vec2(0.0, 8.0) * scale], stroke);
            painter.line_segment([center + vec2(-5.0, 9.0) * scale, center + vec2(5.0, 9.0) * scale], stroke);
        }
        Icon::Touch => {
            let phone = Rect::from_center_size(center, vec2(15.0, 25.0) * scale);
            painter.rect_stroke(phone, 5, stroke, StrokeKind::Inside);
            painter.circle_filled(center + vec2(0.0, 7.5) * scale, 1.0 * scale, tint);
            painter.line_segment([center + vec2(-3.0, -5.0) * scale, center + vec2(3.0, -5.0) * scale], stroke);
        }
        Icon::Game => {
            let body = Rect::from_center_size(center, vec2(25.0, 15.0) * scale);
            painter.rect_stroke(body, 6, stroke, StrokeKind::Inside);
            painter.line_segment([center + vec2(-8.0, 0.0) * scale, center + vec2(-2.0, 0.0) * scale], stroke);
            painter.line_segment([center + vec2(-5.0, -3.0) * scale, center + vec2(-5.0, 3.0) * scale], stroke);
            painter.circle_filled(center + vec2(5.0, -2.0) * scale, 1.3 * scale, tint);
            painter.circle_filled(center + vec2(9.0, 2.0) * scale, 1.3 * scale, tint);
        }
        Icon::Logs => {
            let box_rect = Rect::from_center_size(center, vec2(24.0, 20.0) * scale);
            painter.rect_stroke(box_rect, 4, stroke, StrokeKind::Inside);
            let points = [(-8.0, 1.0), (-4.0, 1.0), (-1.0, -5.0), (2.0, 5.0), (5.0, -2.0), (8.0, -2.0)];
            painter.line(points.iter().map(|(x, y)| center + vec2(*x, *y) * scale).collect::<Vec<_>>(), stroke);
        }
        Icon::Adjust => {
            for (y, x) in [(-6.0, 4.0), (0.0, -4.0), (6.0, 2.0)] {
                painter.line_segment([center + vec2(-9.0, y) * scale, center + vec2(9.0, y) * scale], stroke);
                painter.circle_filled(center + vec2(x, y) * scale, 2.2 * scale, fill);
                painter.circle_stroke(center + vec2(x, y) * scale, 2.2 * scale, stroke);
            }
        }
        Icon::Aim => {
            painter.circle_stroke(center, 10.0 * scale, stroke);
            painter.circle_stroke(center, 4.0 * scale, stroke);
            painter.circle_filled(center, 1.4 * scale, tint);
        }
        Icon::Storage => {
            let box_rect = Rect::from_center_size(center, vec2(22.0, 20.0) * scale);
            painter.rect_stroke(box_rect, 4, stroke, StrokeKind::Inside);
            painter.line_segment([center + vec2(-10.0, -3.0) * scale, center + vec2(10.0, -3.0) * scale], stroke);
            painter.circle_filled(center + vec2(6.0, 3.0) * scale, 1.0 * scale, tint);
        }
        Icon::Community => {
            painter.circle_stroke(center + vec2(0.0, -5.0) * scale, 4.0 * scale, stroke);
            painter.circle_stroke(center + vec2(-9.0, -2.0) * scale, 3.0 * scale, stroke);
            painter.circle_stroke(center + vec2(9.0, -2.0) * scale, 3.0 * scale, stroke);
            painter.line_segment([center + vec2(-7.0, 8.0) * scale, center + vec2(-5.0, 3.0) * scale], stroke);
            painter.line_segment([center + vec2(-5.0, 3.0) * scale, center + vec2(5.0, 3.0) * scale], stroke);
            painter.line_segment([center + vec2(5.0, 3.0) * scale, center + vec2(7.0, 8.0) * scale], stroke);
            painter.line_segment([center + vec2(-12.0, 7.0) * scale, center + vec2(-10.0, 3.0) * scale], stroke);
            painter.line_segment([center + vec2(10.0, 3.0) * scale, center + vec2(12.0, 7.0) * scale], stroke);
        }
        Icon::Shield => {
            let points = [(-8.0, -8.0), (8.0, -8.0), (7.0, 3.0), (0.0, 10.0), (-7.0, 3.0), (-8.0, -8.0)];
            painter.line(points.iter().map(|(x, y)| center + vec2(*x, *y) * scale).collect::<Vec<_>>(), stroke);
        }
        Icon::Settings => {
            painter.circle_stroke(center, 7.0 * scale, stroke);
            painter.circle_filled(center, 2.2 * scale, tint);
            for tooth in 0..8 {
                let angle = tooth as f32 * std::f32::consts::TAU / 8.0;
                let direction = vec2(angle.cos(), angle.sin());
                painter.line_segment([center + direction * (8.5 * scale), center + direction * (11.0 * scale)], stroke);
            }
        }
    }
}

pub(super) fn heading(ui: &mut Ui, icon: Icon, title: &str, subtitle: &str, tint: Color32, scale: f32) {
    let size = 42.0 * scale;
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 58.0 * scale), egui::Sense::hover());
    let tile_rect = Rect::from_center_size(pos2(rect.left() + size * 0.5, rect.center().y), Vec2::splat(size));
    tile(ui, tile_rect, icon, tint, Color32::from_rgba_unmultiplied(tint.r(), tint.g(), tint.b(), 35));
    let x = rect.left() + size + 12.0 * scale;
    ui.painter().text(
        pos2(x, rect.center().y - 7.0 * scale),
        Align2::LEFT_CENTER,
        title,
        FontId::new(18.0 * scale, egui::FontFamily::Name("condensed".into())),
        Color32::WHITE,
    );
    ui.painter().text(
        pos2(x, rect.center().y + 11.0 * scale),
        Align2::LEFT_CENTER,
        subtitle,
        FontId::proportional(11.5 * scale),
        super::theme::expressive::ON_SURFACE_VARIANT,
    );
}

pub(super) fn section(ui: &mut Ui, icon: Icon, title: &str, tint: Color32) {
    let scale = super::theme::density(ui);
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 28.0 * scale), egui::Sense::hover());
    let tile_rect = Rect::from_center_size(pos2(rect.left() + 10.0 * scale, rect.center().y), Vec2::splat(20.0 * scale));
    tile(ui, tile_rect, icon, tint, Color32::TRANSPARENT);
    ui.painter().text(
        pos2(rect.left() + 24.0 * scale, rect.center().y),
        Align2::LEFT_CENTER,
        title,
        FontId::new(12.0 * scale, egui::FontFamily::Name("condensed".into())),
        super::theme::expressive::ON_SURFACE_VARIANT,
    );
}
