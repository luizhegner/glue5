//! Required source selection and presentation preparation, before any tabs.
//! No skip path and no synthetic success for a missing game resource.
use egui::*;
use super::{loading, shell, spinner, theme::{self, fonts, palette}, widgets};

#[derive(Clone, Copy, Default)]
pub(super) struct Resources {
    pub confirmed: bool,
    pub game: bool,
    pub font: bool,
    pub spinner: bool,
    pub artwork: bool,
    pub working: bool,
}

impl Resources {
    pub fn ready(self) -> bool {
        self.confirmed && self.game && self.font && self.spinner && self.artwork && !self.working
    }

    pub fn missing(self) -> String {
        [(self.font,"Chalet font"),(self.spinner,"loading spinner"),(self.artwork,"loading artwork")]
            .into_iter().filter_map(|(ready,name)| (!ready).then_some(name)).collect::<Vec<_>>().join(", ")
    }
}

#[derive(Default)]
pub(super) struct Setup {
    address: Option<String>,
    loading: loading::Feedback,
}

#[derive(Debug, PartialEq)]
pub(super) enum Action { Folder, Server(String), Search }

pub(super) struct View<'a> {
    pub resources: Resources,
    pub address: &'a str,
    pub detail: &'a str,
    pub problem: Option<&'a str>,
    pub progress: Option<f32>,
    pub reduce_motion: bool,
    pub servers: &'a [(&'a str, &'a str)],
}

/// A full-output setup screen; only its text and controls respect cutouts.
/// Recovery is always available, including with an older/missing server.
pub(super) fn show(ctx: &Context, setup: &mut Setup, view: View<'_>, texture: Option<&TextureHandle>) -> Vec<Action> {
    let output = shell::output_rect(ctx);
    ctx.layer_painter(LayerId::background()).with_clip_rect(output).rect_filled(output,0,palette::INK_950);
    let safe = ctx.content_rect();
    let scale = theme::menu_scale(ctx,safe);
    let pad = 24.0 * scale;
    let width = (safe.width()-pad*2.0).clamp(1.0,680.0*scale);
    let bounds = Rect::from_min_size(pos2(safe.center().x-width*0.5,safe.top()+12.0*scale),
        vec2(width,(safe.height()-24.0*scale).max(1.0)));
    let indicator = setup.loading.observe(
        view.resources.working && view.problem.is_none(),
        view.detail,
        view.progress,
        std::time::Instant::now(),
    );
    let address = setup.address.get_or_insert_with(|| view.address.to_owned());
    let mut actions = vec![];
    Area::new(Id::new("required-game-setup")).fixed_pos(bounds.min).constrain(false).show(ctx,|ui| {
        ui.set_width(bounds.width()); ui.set_height(bounds.height()); ui.set_clip_rect(bounds);
        theme::apply_density(ui,scale);
        widgets::scroll_vertical().id_salt("required-game-setup-list").auto_shrink([false,false]).max_height(bounds.height()).show(ui,|ui| {
            ui.set_width(bounds.width());
            ui.label(RichText::new(if view.resources.confirmed { "Preparing GLUE 5" } else { "Choose your game build" })
                .font(FontId::new(24.0*scale,fonts::regular())));
            ui.label(theme::note("Required setup · Connect to a GLUE server or choose your WebAssembly build folder",12.0*scale));
            ui.add_space(8.0*scale);
            if view.resources.confirmed {
                ui.label(theme::note(view.detail,12.0*scale));
                // This rail appears only when the stream reports measured
                // bytes for the interface resources. Unknown is indeterminate.
                if view.resources.working && let Some(progress) = view.progress.filter(|p| p.is_finite()) {
                    super::pages::rail(ui,progress.clamp(0.0,1.0));
                }
                if view.resources.working && view.problem.is_none() {
                    if indicator.visible && let Some(texture) = texture {
                        let (rect,_) = ui.allocate_exact_size(Vec2::splat(24.0*scale),Sense::hover());
                        let time = ctx.input(|i| i.time) as f32;
                        let angle = if view.reduce_motion { 0.0 } else { -(time * 10.0).floor() * 0.1 * std::f32::consts::PI };
                        spinner::paint(ui.painter(),texture,rect,angle);
                    }
                    if indicator.quiet {
                        ui.label(theme::note("No new progress reported. Preparation may still be active.",11.0*scale));
                    } else if indicator.details && view.progress.is_none() {
                        ui.label(theme::note("Waiting for a status update from this stage.",11.0*scale));
                    }
                    ctx.request_repaint_after(std::time::Duration::from_millis(if view.reduce_motion { 200 } else { 100 }));
                } else if view.resources.game && !view.resources.ready() && view.problem.is_none() {
                    ui.label(RichText::new(format!("Missing or unreadable: {}. Check this build or select another source.",view.resources.missing()))
                        .size(12.0*scale).color(super::WARN));
                }
            }
            if let Some(problem) = view.problem { ui.label(RichText::new(problem).size(12.0*scale).color(super::WARN)); }
            ui.add_space(8.0*scale);
            widgets::section(ui,"GLUE SERVER");
            let edit = widgets::text_field(ui,"",super::TextKind::Address,address,"IP:port · for example 192.168.0.2:47100",120);
            let enter = edit.lost_focus() && ui.input(|i|i.key_pressed(Key::Enter));
            let connect = ui.add_enabled_ui(!address.trim().is_empty(),|ui| {
                widgets::tab_button(ui,if view.problem.is_some() { "RETRY CONNECTION" } else { "CONNECT" },true,vec2(ui.available_width(),44.0*scale))
            }).inner.clicked();
            if !address.trim().is_empty() && (connect || enter) {
                ui.memory_mut(|m|m.surrender_focus(edit.id));
                actions.push(Action::Server(address.trim().to_owned()));
            }
            for &(name,address) in view.servers {
                if widgets::tab_button(ui,&format!("{name} · {address}"),false,vec2(ui.available_width(),44.0*scale)).clicked() {
                    ui.memory_mut(|m|m.surrender_focus(edit.id));
                    actions.push(Action::Server(address.to_owned()));
                }
            }
            if widgets::tab_button(ui,"FIND SERVERS",false,vec2(ui.available_width(),44.0*scale)).clicked() { actions.push(Action::Search); }
            if !crate::paths::TV {
                ui.add_space(8.0*scale);
                if widgets::tab_button(ui,"CHOOSE BUILD FOLDER",false,vec2(ui.available_width(),44.0*scale)).clicked() { actions.push(Action::Folder); }
            }
            ui.label(theme::note("Uses resources from your selected game build or GLUE server.",11.0*scale));
        });
    });
    actions
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_required_resource_and_explicit_selection_gate_the_panel() {
        for mask in 0..64 {
            let resources=Resources { confirmed:mask&1!=0,game:mask&2!=0,font:mask&4!=0,spinner:mask&8!=0,artwork:mask&16!=0,working:mask&32!=0 };
            assert_eq!(resources.ready(),mask==31,"gate mask {mask}");
        }
    }

    #[test]
    fn missing_resources_are_named_without_claiming_audio_support() {
        let resources=Resources{font:true,..Default::default()};
        assert_eq!(resources.missing(),"loading spinner, loading artwork");
    }
}
