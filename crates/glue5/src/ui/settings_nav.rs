//! Settings navigation: persistent sidebar on wide panels, category index on
//! narrow panels. Content and navigation scroll independently, without effects.
use super::{theme, widgets};
use egui::{Align, Id, Layout, ScrollArea, Ui, vec2};

pub(super) fn show<T: Copy + PartialEq>(
    ui: &mut Ui,
    selected: &mut T,
    categories: &[(T, &str)],
    mut content: impl FnMut(&mut Ui, T),
) {
    let scale = theme::density(ui);
    let width = ui.available_width();
    let height = (ui.clip_rect().bottom() - ui.cursor().top()).max(44.0 * scale);
    let name = categories.iter().find(|(value, _)| value == selected).map_or("SETTINGS", |(_, name)| *name);
    if width >= 620.0 * scale {
        let side = 156.0 * scale;
        let gap = 16.0 * scale;
        ui.horizontal_top(|ui| {
            ui.spacing_mut().item_spacing.x = gap;
            ui.allocate_ui_with_layout(vec2(side, height), Layout::top_down(Align::Min), |ui| {
                ui.set_width(side);
                ScrollArea::vertical().id_salt("settings-sections").max_height(height).show(ui, |ui| {
                    ui.set_width(side);
                    for &(value, label) in categories {
                        if widgets::tab_button(ui, label, *selected == value, vec2(side, 44.0 * scale)).clicked() {
                            *selected = value;
                        }
                    }
                });
            });
            let name = categories.iter().find(|(value, _)| value == selected).map_or("SETTINGS", |(_, name)| *name);
            ui.allocate_ui_with_layout(vec2((width - side - gap).max(1.0), height), Layout::top_down(Align::Min), |ui| {
                ui.set_width((width - side - gap).max(1.0));
                ScrollArea::vertical().id_salt(("settings-content", name)).max_height(height).show(ui, |ui| {
                    widgets::section(ui, name);
                    content(ui, *selected);
                });
            });
        });
    } else {
        let id = Id::new("settings-category-index");
        let overview = ui.ctx().data(|d| d.get_temp::<bool>(id)).unwrap_or(false);
        if overview {
            widgets::section(ui, "ALL SETTINGS");
            for &(value, label) in categories {
                if widgets::tab_button(ui, label, *selected == value, vec2(width, 44.0 * scale)).clicked() {
                    *selected = value;
                    ui.ctx().data_mut(|d| d.insert_temp(id, false));
                }
            }
        } else {
            if widgets::tab_button(ui, "‹ ALL SETTINGS", false, vec2(width, 44.0 * scale)).clicked() {
                ui.ctx().data_mut(|d| d.insert_temp(id, true));
            }
            ScrollArea::vertical().id_salt(("settings-content", name))
                .max_height((height - 44.0 * scale).max(1.0)).show(ui, |ui| {
                    widgets::section(ui, name);
                    content(ui, *selected);
                });
        }
    }
}
