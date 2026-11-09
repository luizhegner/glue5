use egui::*;
use crate::settings::Source;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Page { Start, Settings, Friends, Servers, Mods, Credits }
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode { Story, Online, Sandbox, Benchmark }
#[derive(Clone, Copy)]
pub enum TextKind { Name, Code, Address }
const TEXT_KIND: &str = "text kind";
const WARN: Color32 = Color32::from_rgb(240, 180, 100);
fn clipboard_text() -> Option<String> { None }
pub enum Action { Play(Mode), FindGame, ChooseGame }
pub struct Benchmark { pub avg_fps: f32, pub min_fps: f32, pub smooth: f32 }
pub struct Status<'a> {
    pub game_dir: Option<&'a std::path::Path>,
    pub preparing: Option<f32>,
    pub started: Option<()>,
    pub mode: Mode,
    pub source: Source,
    pub problem: Option<&'a str>,
    pub stream: Option<(&'a str, &'a ())>,
    pub last_failure: Option<&'a str>,
    pub benchmark: Option<&'a Benchmark>,
}

#[path = "../../../crates/glue5/src/ui/theme.rs"]
mod theme;
#[path = "../../../crates/glue5/src/ui/widgets.rs"]
mod widgets;
#[path = "../../../crates/glue5/src/ui/shell.rs"]
mod shell;
#[path = "../../../crates/glue5/src/ui/popup.rs"]
mod popup;
#[path = "../../../crates/glue5/src/ui/menu.rs"]
mod menu;
#[path = "../../../crates/glue5/src/ui/spinner.rs"]
mod spinner;

mod pages {
    pub fn stream_state(_: &()) -> (&'static str, egui::Color32) { ("Ready", egui::Color32::WHITE) }
    pub fn rail(ui: &mut egui::Ui, value: f32) { ui.add(egui::ProgressBar::new(value)); }
}

fn status() -> Status<'static> {
    Status { game_dir: Some(std::path::Path::new("game")), preparing: None, started: None,
        mode: Mode::Story, source: Source::Server, problem: None, stream: None,
        last_failure: None, benchmark: None }
}

pub fn preview() {
    let directory = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("previews");
    std::fs::create_dir_all(&directory).unwrap();
    for (name, size) in [("landscape", vec2(844.0, 390.0)), ("portrait", vec2(390.0, 844.0)), ("compact", vec2(320.0, 480.0))] {
        for page in ["play", "settings", "confirmation"] {
            let ctx = Context::default();
            theme::install(&ctx);
            let mut raster = super::raster::Raster::new(size.x as usize, size.y as usize);
            for pass in 0..4 {
                let mut output = ctx.run_ui(RawInput { screen_rect: Some(Rect::from_min_size(Pos2::ZERO, size)), time: Some(pass as f64), ..Default::default() }, |root| {
                    shell::set_session(root.ctx(), false, false);
                    shell::begin(Some(if page == "play" { Page::Start } else { Page::Settings }));
                    if page == "play" {
                        menu::show(root.ctx(), &mut Default::default(), &status(), &mut Mode::Story, false, None, &mut vec![], &mut None);
                    } else {
                        shell::frame(root.ctx(), "SETTINGS", |ui| {
                            ComboBox::from_id_salt("category").selected_text("Category: GRAPHICS").width(ui.available_width()).show_ui(ui, |_| {});
                            widgets::row(ui, "Resolution scale", "The game is drawn at this scale and stretched to the screen.", |ui| widgets::slider(ui, &mut 0.75, 0.5..=1.0, 0.05, "%", 0));
                            widgets::row(ui, "Upscaling", "How the frame is stretched to the screen.", |ui| widgets::choice(ui, &mut 0, &[(0, "Bicubic"), (1, "FSR 2")]));
                            widgets::row(ui, "VSync", "Wait for the display between frames.", |ui| widgets::toggle(ui, &mut true));
                        });
                        if page == "confirmation" {
                            let safe = root.ctx().content_rect();
                            popup::show(root.ctx(), &mut popup::Open::new(popup::Request::clear_copy()), safe, safe, Default::default());
                        }
                    }
                });
                raster.update(&output.textures_delta);
                raster.paint(ctx.tessellate(std::mem::take(&mut output.shapes), output.pixels_per_point));
                output.textures_delta.clear();
            }
            raster.save(&directory.join(format!("{page}-{name}.png")));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(ctx: &Context, size: Vec2, events: Vec<Event>, add: impl FnOnce(&Context)) -> FullOutput {
        let mut add = Some(add);
        let mut output = ctx.run_ui(RawInput { screen_rect: Some(Rect::from_min_size(Pos2::ZERO, size)), events, ..Default::default() }, |root| {
            if let Some(add) = add.take() { add(root.ctx()); }
        });
        output.textures_delta.clear();
        output
    }

    fn pointer(pos: Pos2, pressed: bool) -> Vec<Event> {
        vec![Event::PointerMoved(pos), Event::PointerButton { pos, button: PointerButton::Primary, pressed, modifiers: Modifiers::NONE }]
    }

    fn text_rect(output: &FullOutput, wanted: &str) -> Option<Rect> {
        fn find(shape: &Shape, wanted: &str) -> Option<Rect> {
            match shape {
                Shape::Text(t) if t.galley.text() == wanted => Some(t.visual_bounding_rect()),
                Shape::Vec(shapes) => shapes.iter().find_map(|s| find(s, wanted)),
                _ => None,
            }
        }
        output.shapes.iter().find_map(|s| find(&s.shape, wanted))
    }

    #[test]
    fn keyboard_does_not_shrink_touch_targets() {
        assert_eq!(theme::layout_scale(Rect::from_min_size(Pos2::ZERO, vec2(844.0, 390.0))), theme::layout_scale(Rect::from_min_size(Pos2::ZERO, vec2(844.0, 190.0))));
    }

    #[test]
    fn labels_help_and_controls_stay_inside_rows() {
        for width in [260.0, 340.0, 680.0] {
            let ctx = Context::default();
            theme::install(&ctx);
            for _ in 0..3 {
                frame(&ctx, vec2(width + 16.0, 600.0), vec![], |ctx| {
                    Area::new(Id::new("rows")).fixed_pos(pos2(8.0, 8.0)).show(ctx, |ui| {
                        ui.set_width(width);
                        let mut child = Rect::NOTHING;
                        let row = widgets::row(ui, "A long setting label that must wrap", "Help is visible without hovering, on a phone too.", |ui| {
                            let r = widgets::choice(ui, &mut 0, &[(0, "A long selected value"), (1, "Second")]);
                            child = r.rect;
                            r
                        });
                        assert!(child.height() >= 44.0, "small touch target: {child:?}");
                        assert!(row.rect.expand(1.0).contains_rect(child), "{width}: {child:?} outside {:?}", row.rect);
                        let next = widgets::row(ui, "Next setting", "Must not overlap previous help", |ui| widgets::toggle(ui, &mut true));
                        assert!(next.rect.top() >= row.rect.bottom(), "{width}: rows overlap: {:?} and {:?}", row.rect, next.rect);
                        assert!(ui.min_rect().width() <= width + 1.0, "{width}: content expands to {:?}", ui.min_rect());
                    });
                });
            }
        }
    }

    #[test]
    fn resume_tab_returns_to_game_not_start_page() {
        let ctx = Context::default();
        theme::install(&ctx);
        let size = vec2(844.0, 390.0);
        let draw = |ctx: &Context| {
            shell::set_session(ctx, true, false);
            shell::begin(Some(Page::Settings));
            shell::frame(ctx, "SETTINGS", |_| {});
        };
        shell::take_nav();
        let mut point = Pos2::ZERO;
        for pass in 0..4 {
            let output = frame(&ctx, size, vec![], draw);
            if pass == 3 { point = text_rect(&output, "RESUME").expect("resume tab").center(); }
        }
        frame(&ctx, size, pointer(point, true), draw);
        frame(&ctx, size, pointer(point, false), draw);
        assert_eq!(shell::take_nav(), Some(None));
    }

    #[test]
    fn play_is_visible_and_launches_only_when_ready() {
        for ready in [true, false] {
            let ctx = Context::default();
            theme::install(&ctx);
            let size = vec2(844.0, 390.0);
            let mut state = status();
            if !ready { state.game_dir = None; }
            let mut settings = crate::settings::Settings::default();
            let mut selected = Mode::Story;
            let mut actions = vec![];
            let mut draw = |ctx: &Context| {
                shell::begin(Some(Page::Start));
                menu::show(ctx, &mut settings, &state, &mut selected, false, None, &mut actions, &mut None);
            };
            let mut point = Pos2::ZERO;
            for pass in 0..4 {
                let output = frame(&ctx, size, vec![], &mut draw);
                if pass == 3 {
                    let rect = text_rect(&output, "PLAY STORY MODE").expect("play action");
                    assert!(rect.bottom() < size.y - 44.0, "launch hidden below footer: {rect:?}");
                    point = rect.center();
                }
            }
            frame(&ctx, size, pointer(point, true), &mut draw);
            frame(&ctx, size, pointer(point, false), &mut draw);
            assert_eq!(actions.iter().filter(|a| matches!(a, Action::Play(Mode::Story))).count(), usize::from(ready));
        }
    }

    #[test]
    fn confirmation_actions_stay_visible_above_keyboard() {
        let ctx = Context::default();
        theme::install(&ctx);
        let screen = Rect::from_min_size(Pos2::ZERO, vec2(844.0, 390.0));
        let safe = Rect::from_min_max(pos2(40.0, 0.0), pos2(804.0, 190.0));
        let mut open = popup::Open::new(popup::Request::clear_copy());
        for pass in 0..4 {
            let output = frame(&ctx, screen.size(), vec![], |ctx| { popup::show(ctx, &mut open, screen, safe, Default::default()); });
            if pass == 3 {
                for label in ["CLEAR", "BACK"] {
                    let rect = text_rect(&output, label).expect("confirmation action");
                    assert!(safe.contains_rect(rect), "{label} outside safe area: {rect:?}");
                }
            }
        }
    }
}
