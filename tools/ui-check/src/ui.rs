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
#[path = "../../../crates/glue5/src/ui/woff.rs"]
mod woff;
#[path = "../../../crates/glue5/src/ui/title.rs"]
mod title;
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
#[path = "../../../crates/glue5/src/ui/bootstrap.rs"]
mod bootstrap;
#[path = "../../../crates/glue5/src/ui/settings_nav.rs"]
mod settings_nav;
#[path = "../../../crates/glue5/src/ui/capture.rs"]
mod capture;
#[path = "../../../crates/glue5/src/ui/overlay_layout.rs"]
mod overlay_layout;
#[path = "../../../crates/glue5/src/touch/geometry.rs"]
mod touch_geometry;
#[path = "../../../crates/glue5-runtime/src/radar.rs"]
mod radar;
#[path = "../../../crates/glue5-runtime/src/prompts/taxi.rs"]
mod taxi_prompt;
#[path = "../../../crates/glue5-runtime/src/diagnostic_control/session.rs"]
mod diagnostic_session;
#[path = "../../../crates/glue5-runtime/src/diagnostic_control/spatial.rs"]
mod diagnostic_spatial;
#[path = "../../../crates/glue5/src/ui/diagnostic_bar.rs"]
mod diagnostic_bar;
#[path = "../../../crates/glue5/src/ui/diagnostic_setup.rs"]
mod diagnostic_setup;
#[path = "../../../crates/glue5/src/ui/diagnostic_connection.rs"]
mod diagnostic_connection;

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
        for page in ["play", "settings", "confirmation", "ai-control", "ai-setup", "preparation", "setup"] {
            let ctx = Context::default();
            theme::install(&ctx);
            let mut raster = super::raster::Raster::new(size.x as usize, size.y as usize);
            for pass in 0..4 {
                let mut output = ctx.run_ui(RawInput { screen_rect: Some(Rect::from_min_size(Pos2::ZERO, size)), time: Some(pass as f64), ..Default::default() }, |root| {
                    shell::set_session(root.ctx(), false, false);
                    shell::begin(Some(if page == "play" { Page::Start } else { Page::Settings }));
                    if page == "setup" {
                        bootstrap::show(root.ctx(),&mut Default::default(),bootstrap::View {
                            resources:Default::default(),address:"192.168.0.2:47100",detail:"",problem:None,progress:None,servers:&[],
                        },None);
                    } else if page == "preparation" {
                        menu::preparation(root.ctx(),root.max_rect(),None,Some(0.54));
                    } else if page == "ai-control" {
                        diagnostic_bar::draw(root.ctx(),root.max_rect());
                    } else if page == "ai-setup" {
                        shell::frame(root.ctx(), "DIAGNOSTICS", |ui| {
                            egui::ScrollArea::vertical().show(ui, |ui| {
                                diagnostic_setup::show(ui, &diagnostic_setup::Setup {
                                    enabled: true, mods_enabled: true, studio_enabled: true,
                                    studio_running: true, lan: true, paired: 1, game_running: true, mods_running: true,
                                    address: Some("http://192.0.2.1:47110/mcp"), code: None,
                                });
                            });
                        });
                    } else if page == "play" {
                        menu::show(root.ctx(), &mut Default::default(), &status(), &mut Mode::Story, false, None, &mut vec![], &mut None);
                    } else {
                        shell::frame(root.ctx(), "SETTINGS", |ui| {
                            settings_nav::show(ui, &mut 0, &[(0, "GRAPHICS"), (1, "DISPLAY"), (2, "AUDIO"), (3, "CONTROLS"), (4, "GAMEPLAY"), (5, "FILES"), (6, "NETWORK"), (7, "ADVANCED"), (8, "DIAGNOSTICS")], |ui, _| {
                            widgets::row(ui, "Resolution scale", "The game is drawn at this scale and stretched to the screen.", |ui| widgets::slider(ui, &mut 0.75, 0.5..=1.0, 0.05, "%", 0));
                            widgets::row(ui, "Upscaling", "How the frame is stretched to the screen.", |ui| widgets::choice(ui, &mut 0, &[(0, "Bicubic"), (1, "FSR 2")]));
                            widgets::row(ui, "VSync", "Wait for the display between frames.", |ui| widgets::toggle(ui, &mut true));
                            });
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
    fn required_setup_has_no_play_or_skip_and_routes_explicit_choices() {
        for (label,expected) in [("CONNECT",bootstrap::Action::Server("192.0.2.1:47100".into())),("CHOOSE BUILD FOLDER",bootstrap::Action::Folder)] {
            let ctx=Context::default(); theme::install(&ctx);
            let mut setup=bootstrap::Setup::default();
            let mut actions=vec![];
            let mut draw=|ctx:&Context| {
                actions.extend(bootstrap::show(ctx,&mut setup,bootstrap::View {
                    resources:Default::default(),address:"192.0.2.1:47100",detail:"",problem:None,progress:None,servers:&[],
                },None));
            };
            let size=vec2(844.0,500.0);
            let mut output=frame(&ctx,size,vec![],&mut draw);
            for _ in 0..3 { output=frame(&ctx,size,vec![],&mut draw); }
            assert!(text_rect(&output,"PLAY").is_none());
            assert!(text_rect(&output,"SKIP").is_none());
            let at=text_rect(&output,label).expect("setup choice").center();
            frame(&ctx,size,pointer(at,true),&mut draw);
            frame(&ctx,size,pointer(at,false),&mut draw);
            assert_eq!(actions,vec![expected]);
        }
    }

    #[test]
    fn play_modes_and_launch_are_one_contiguous_column() {
        let ctx=Context::default(); theme::install(&ctx);
        let draw=|ctx:&Context| {
            shell::set_session(ctx,false,false); shell::begin(Some(Page::Start));
            menu::show(ctx,&mut Default::default(),&status(),&mut Mode::Story,false,None,&mut vec![],&mut None);
        };
        let mut output=frame(&ctx,vec2(844.0,390.0),vec![],draw);
        for _ in 0..3 { output=frame(&ctx,vec2(844.0,390.0),vec![],draw); }
        let mut previous=None::<Rect>;
        for label in ["Story mode","GTA Online","Sandbox","Benchmark","PLAY STORY MODE"] {
            let text=text_rect(&output,label).expect("mode or launch");
            // Text visual bounds include glyph-specific side bearings. Test
            // the painted button, not an optical ink box that varies by name.
            let rect=output.shapes.iter().find_map(|s| match &s.shape {
                Shape::Rect(r) if r.rect.contains(text.center()) && r.rect.width()>200.0 && r.rect.height()<50.0 => Some(r.rect),
                _=>None,
            }).expect("mode button bounds");
            if let Some(previous)=previous {
                assert!((rect.left()-previous.left()).abs()<0.1 && (rect.right()-previous.right()).abs()<0.1,"{label}: {rect:?} follows {previous:?}");
                assert!((44.0..=48.1).contains(&(rect.center().y-previous.center().y)),"{label}: {rect:?} follows {previous:?}");
            }
            previous=Some(rect);
        }
        let help=text_rect(&output,"The story of Los Santos: three lives, one city").expect("selected mode help");
        let name=text_rect(&output,"Story mode").unwrap();
        assert!(help.top()>name.bottom() && help.bottom()<140.0,"help must remain inside its selected row: {help:?}");
    }

    #[test]
    fn diagnostic_opt_in_responds_to_label_and_switch_without_double_toggle() {
        for (enabled, target) in [(false, "Permitir solicitações de IA"), (false, "OFF"), (true, "Permitir solicitações de IA"), (true, "ON")] {
            let ctx = Context::default();
            theme::install(&ctx);
            let size = vec2(390.0, 844.0);
            let mut actions = Vec::new();
            let mut draw = |ctx: &Context| {
                egui::Area::new(Id::new("diagnostic-setup-test")).fixed_pos(Pos2::ZERO).show(ctx, |ui| {
                    ui.set_width(size.x);
                    actions.extend(diagnostic_setup::show(ui, &diagnostic_setup::Setup { enabled, ..Default::default() }));
                });
            };
            let mut output = frame(&ctx, size, vec![], &mut draw);
            for _ in 0..3 { output = frame(&ctx, size, vec![], &mut draw); }
            let point = text_rect(&output, target).expect("diagnostic opt-in target").center();
            frame(&ctx, size, pointer(point, true), &mut draw);
            frame(&ctx, size, pointer(point, false), &mut draw);
            drop(draw);
            assert_eq!(actions, vec![diagnostic_setup::Action::Enable(!enabled)], "tap {target}");
        }
    }

    #[test]
    fn diagnostic_credential_creation_requires_an_explicit_tap() {
        let ctx = Context::default();
        theme::install(&ctx);
        let size = vec2(844.0, 844.0);
        let mut actions = Vec::new();
        let mut draw = |ctx: &Context| {
            egui::Area::new(Id::new("diagnostic-copy-test")).fixed_pos(Pos2::ZERO).show(ctx, |ui| {
                ui.set_width(size.x);
                actions.extend(diagnostic_setup::show(ui, &diagnostic_setup::Setup {
                    enabled: true, mods_enabled: true, mods_running: true, game_running: true,
                    studio_enabled: true, studio_running: true, lan: true, paired: 0,
                    address: Some("http://192.0.2.1:47110/mcp"), code: None,
                }));
            });
        };
        let mut output = frame(&ctx, size, vec![], &mut draw);
        for _ in 0..3 { output = frame(&ctx, size, vec![], &mut draw); }
        let point = text_rect(&output, "CRIAR E COPIAR").expect("create connection action").center();
        frame(&ctx, size, pointer(point, true), &mut draw);
        frame(&ctx, size, pointer(point, false), &mut draw);
        drop(draw);
        // Neither painting nor press-down alone must create an access token.
        assert_eq!(actions, vec![diagnostic_setup::Action::CopyConnection(diagnostic_connection::Format::Codex)]);
    }

    #[test]
    fn shell_and_back_fit_above_a_keyboard_without_shrinking_controls() {
        for size in [vec2(320.0,480.0), vec2(390.0,844.0), vec2(844.0,390.0)] {
            for scale in [0.85, 1.0, 1.3] {
                let ctx = Context::default();
                theme::install(&ctx);
                theme::set_ui_scale(&ctx, scale);
                let mut output = FullOutput::default();
                for _ in 0..4 {
                    output = ctx.run_ui(RawInput {
                        screen_rect: Some(Rect::from_min_size(Pos2::ZERO,size)),
                        safe_area_insets: Some(SafeAreaInsets(egui::epaint::MarginF32 {
                            top:24.0, bottom:if size.y>size.x {316.0} else {216.0}, left:8.0, right:8.0,
                        })), ..Default::default()
                    }, |root| {
                        shell::set_keyboard_height(root.ctx(),if size.y>size.x {316.0} else {216.0});
                        shell::begin(Some(Page::Mods));
                        shell::set_session(root.ctx(),false,false);
                        shell::frame(root.ctx(), "MODS", |ui| {
                            for _ in 0..30 { widgets::row(ui,"Menu item","Help",|ui|widgets::action_value(ui,"OPEN")); }
                        });
                    });
                    output.textures_delta.clear();
                }
                // Android's gesture/top inset no longer shrinks the entire
                // shell. An actual keyboard still limits its bottom edge.
                let safe = Rect::from_min_max(pos2(8.0,0.0),pos2(size.x-8.0,size.y-if size.y>size.x {316.0}else{216.0}));
                let area = ctx.memory(|m|m.area_rect(Id::new(("shell","MODS")))).unwrap();
                assert!(safe.contains_rect(area),"{size:?}, scale {scale}: {area:?} outside {safe:?}");
                assert!(safe.contains_rect(text_rect(&output,"BACK").expect("Back above keyboard")));
            }
        }
    }

    #[test]
    fn session_footer_stays_inside_screen_at_every_interface_scale() {
        for size in [vec2(320.0, 480.0), vec2(390.0, 844.0), vec2(844.0, 390.0)] {
            for scale in [0.85, 1.0, 1.30] {
                let ctx = Context::default();
                theme::install(&ctx);
                theme::set_ui_scale(&ctx, scale);
                let draw = |ctx: &Context| {
                    shell::set_session(ctx, true, false);
                    shell::begin(Some(Page::Settings));
                    shell::frame(ctx, "SETTINGS", |_| {});
                };
                let mut output = frame(&ctx, size, vec![], draw);
                for _ in 0..3 { output = frame(&ctx, size, vec![], draw); }
                let mut previous: Option<Rect> = None;
                for label in ["EXIT", "HOME", "RESUME"] {
                    // RESUME also appears in the top navigation. Inspect the
                    // footer instance rather than mistaking that tab for it.
                    let rect = output.shapes.iter().find_map(|shape| match &shape.shape {
                        Shape::Text(text) if text.galley.text() == label => {
                            let rect = text.visual_bounding_rect();
                            (rect.center().y > size.y - 80.0).then_some(rect)
                        }
                        _ => None,
                    }).expect("session footer action");
                    assert!(ctx.content_rect().contains_rect(rect), "{label}: {size:?} at {scale}");
                    if let Some(previous) = previous { assert!(rect.left() >= previous.right()); }
                    previous = Some(rect);
                }
                let point = text_rect(&output, "HOME").unwrap().center();
                shell::take_nav();
                frame(&ctx, size, pointer(point, true), draw);
                frame(&ctx, size, pointer(point, false), draw);
                assert_eq!(shell::take_nav(), Some(Some(Page::Start)));
            }
        }
    }

    #[test]
    fn home_resumes_the_existing_session_without_launching_another() {
        let ctx = Context::default();
        theme::install(&ctx);
        let size = vec2(390.0, 844.0);
        let mut state = status();
        state.started = Some(());
        let mut settings = crate::settings::Settings::default();
        let mut actions = vec![];
        let mut go = None;
        let mut selected = Mode::Story;
        let mut draw = |ctx: &Context| {
            shell::set_session(ctx, true, false);
            shell::begin(Some(Page::Start));
            menu::show(ctx, &mut settings, &state, &mut selected, false, None, &mut actions, &mut go);
        };
        let mut output = frame(&ctx, size, vec![], &mut draw);
        for _ in 0..3 { output = frame(&ctx, size, vec![], &mut draw); }
        let point = text_rect(&output, "RESUME GAME").expect("resume existing session").center();
        frame(&ctx, size, pointer(point, true), &mut draw);
        frame(&ctx, size, pointer(point, false), &mut draw);
        assert_eq!(go, Some(None));
        assert!(actions.is_empty());
    }

    #[test]
    fn session_menu_exposes_exit_without_immediately_quitting() {
        let ctx = Context::default();
        theme::install(&ctx);
        let size = vec2(390.0, 844.0);
        let draw = |ctx: &Context| {
            shell::set_session(ctx, true, false);
            shell::begin(Some(Page::Settings));
            shell::frame(ctx, "SETTINGS", |_| {});
        };
        popup::take();
        let mut point = Pos2::ZERO;
        for pass in 0..4 {
            let output = frame(&ctx, size, vec![], draw);
            if pass == 3 {
                let exit = text_rect(&output, "EXIT").expect("visible exit action");
                assert!(ctx.content_rect().contains_rect(exit));
                point = exit.center();
            }
        }
        frame(&ctx, size, pointer(point, true), draw);
        frame(&ctx, size, pointer(point, false), draw);
        assert!(popup::take().is_some(), "exit opens confirmation rather than immediately quitting");
    }

    #[test]
    fn settings_categories_can_be_changed_without_a_dropdown() {
        for size in [vec2(844.0, 390.0), vec2(390.0, 844.0)] {
            let ctx = Context::default();
            theme::install(&ctx);
            let mut selected = 0;
            let mut draw = |ctx: &Context| {
                shell::set_session(ctx, false, false);
                shell::begin(Some(Page::Settings));
                shell::frame(ctx, "SETTINGS", |ui| {
                    settings_nav::show(ui, &mut selected, &[(0, "GRAPHICS"), (1, "AUDIO"), (2, "CONTROLS")], |ui, _| { ui.label("Section content"); });
                });
            };
            let mut output = frame(&ctx, size, vec![], &mut draw);
            for _ in 0..3 { output = frame(&ctx, size, vec![], &mut draw); }
            if size.x >= 620.0 {
                let graphics = text_rect(&output, "GRAPHICS").expect("graphics section");
                let audio = text_rect(&output, "AUDIO").expect("audio section");
                let controls = text_rect(&output, "CONTROLS").expect("controls section");
                assert!(audio.top() >= graphics.bottom(), "sidebar categories must stack, not run horizontally");
                assert!(controls.top() >= audio.bottom());
                assert!(ctx.content_rect().contains_rect(controls));
                let content = text_rect(&output, "Section content").expect("settings content");
                assert!(ctx.content_rect().contains_rect(content), "content must not overflow the panel");
            }
            if size.x < 620.0 {
                let point = text_rect(&output, "ALL SETTINGS").expect("category index route").center();
                frame(&ctx, size, pointer(point, true), &mut draw);
                frame(&ctx, size, pointer(point, false), &mut draw);
                for _ in 0..3 { output = frame(&ctx, size, vec![], &mut draw); }
            }
            let point = text_rect(&output, "AUDIO").expect("visible category").center();
            frame(&ctx, size, pointer(point, true), &mut draw);
            frame(&ctx, size, pointer(point, false), &mut draw);
            assert_eq!(selected, 1);
        }
    }

    #[test]
    fn game_files_action_follows_selected_source() {
        for source in [Source::Folder, Source::Server] {
            let ctx = Context::default();
            theme::install(&ctx);
            let size = vec2(390.0, 1600.0);
            let mut settings = crate::settings::Settings::default();
            settings.source = source;
            let mut state = status();
            state.source = source;
            state.game_dir = None;
            let mut actions = vec![];
            let mut go = None;
            let mut selected = Mode::Story;
            let label = if source == Source::Server { "OPEN GLUE SERVER" } else { "FIND FILES" };
            let mut draw = |ctx: &Context| {
                shell::set_session(ctx, false, false);
                shell::begin(Some(Page::Start));
                menu::show(ctx, &mut settings, &state, &mut selected, false, None, &mut actions, &mut go);
            };
            let mut point = Pos2::ZERO;
            for pass in 0..4 {
                let output = frame(&ctx, size, vec![], &mut draw);
                if pass == 3 { point = text_rect(&output, label).expect("source-specific file action").center(); }
            }
            frame(&ctx, size, pointer(point, true), &mut draw);
            frame(&ctx, size, pointer(point, false), &mut draw);
            if source == Source::Server {
                assert_eq!(go, Some(Some(Page::Servers)));
                assert!(actions.is_empty(), "server action must not open a local folder picker");
            } else {
                assert!(actions.iter().any(|a| matches!(a, Action::FindGame | Action::ChooseGame)));
                assert_eq!(go, None);
            }
        }
    }

    #[test]
    fn segmented_options_reflow_without_squeezing_touch_targets() {
        for width in [100.0, 220.0, 340.0, 640.0] {
            let ctx = Context::default();
            theme::install(&ctx);
            for _ in 0..3 {
                frame(&ctx, vec2(width + 32.0, 800.0), vec![], |ctx| {
                    Area::new(Id::new("segments")).fixed_pos(pos2(8.0, 8.0)).show(ctx, |ui| {
                        ui.set_width(width);
                        widgets::segmented(ui, &mut 0, &[(0, "First"), (1, "Second"), (2, "Third"), (3, "Fourth"), (4, "Fifth")], 44.0);
                        assert!(ui.min_rect().width() <= width + 1.0);
                        if width <= 100.0 {
                            assert!(ui.min_rect().height() >= 5.0 * 44.0, "narrow options must stack");
                        }
                    });
                });
            }
        }
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

    #[test]
    fn narrow_navigation_exposes_every_destination() {
        for size in [vec2(320.0, 480.0), vec2(390.0, 844.0)] {
            let ctx = Context::default();
            theme::install(&ctx);
            for pass in 0..4 {
                let output = frame(&ctx, size, vec![], |ctx| {
                    shell::begin(Some(Page::Start));
                    shell::frame(ctx, "PLAY", |_| {});
                });
                if pass == 3 {
                    for (_, name) in shell::TABS {
                        let rect = text_rect(&output, name).expect("navigation destination");
                        assert!(ctx.content_rect().contains_rect(rect), "{name} clipped at {size:?}: {rect:?}");
                    }
                }
            }
        }
    }

    #[test]
    fn modal_blocks_underlying_settings() {
        let ctx = Context::default();
        theme::install(&ctx);
        let size = vec2(844.0, 390.0);
        let mut on = false;
        let mut control = Rect::NOTHING;
        for pass in 0..6 {
            let events = match pass {
                4 => pointer(control.center(), true),
                5 => pointer(control.center(), false),
                _ => vec![],
            };
            frame(&ctx, size, events, |ctx| {
                shell::set_blocked(ctx, true);
                shell::begin(Some(Page::Settings));
                shell::frame(ctx, "SETTINGS", |ui| {
                    widgets::row(ui, "VSync", "Wait for the display", |ui| {
                        let response = widgets::toggle(ui, &mut on);
                        control = response.rect;
                        response
                    });
                });
            });
        }
        assert!(!on, "a click changed a setting behind the confirmation");
    }
}
