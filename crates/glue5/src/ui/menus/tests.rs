use std::sync::Arc;

use egui::{Event, Id, Key, Modifiers, Pos2, RawInput, Rect, SafeAreaInsets, Vec2, vec2};
use glue5_mods::{Menu, MenuInput, MenuItem, MenuKind, Rgba};

use super::super::pages::sheet;
use super::super::theme;
use super::{Level, MenusView, Out, menu_row_id, row_id};

const MODS: &[(&str, &str)] = &[("garage", "Garage"), ("trainer", "Trainer")];

fn item(id: u32, label: &str, kind: MenuKind) -> MenuItem {
    MenuItem { id, label: label.into(), note: None, enabled: true, visible: true, kind }
}

fn menu(id: u32, module: &str, parent: Option<u32>, title: &str, items: Vec<MenuItem>) -> Menu {
    Menu { module: module.into(), instance: 1, id, parent, title: title.into(), note: None, search: false, items, rev: 1 }
}

fn menus() -> Vec<Menu> {
    let cars: Arc<[String]> = ["adder", "t20", "zentorno"].map(String::from).into();
    let many: Arc<[String]> = (0..40).map(|n| format!("Option {n}")).collect();
    let mut locked = item(14, "Locked", MenuKind::Button { action: None });
    locked.enabled = false;
    let mut god = item(10, "God mode", MenuKind::Toggle(false));
    god.note = Some("Nothing hurts you".into());
    let mut trainer = menu(
        1,
        "trainer",
        None,
        "Trainer",
        vec![
            god,
            item(11, "Hour", MenuKind::Slider { value: 5.0, min: 0.0, max: 10.0, step: 1.0, unit: None }),
            item(12, "Car", MenuKind::Choice { options: cars, index: 0 }),
            item(
                13,
                "Money",
                MenuKind::Number { value: 100.0, min: Some(0.0), max: None, step: 10.0, unit: Some("$".into()) },
            ),
            locked,
            item(15, "Plate", MenuKind::Text { value: "GLUE".into(), placeholder: Some("Plate".into()), max: 8 }),
            item(16, "Vehicles", MenuKind::Submenu(2)),
            item(17, "Paint", MenuKind::Color { value: Rgba { r: 240, g: 140, b: 50, a: 255 }, alpha: true }),
            item(18, "Extras", MenuKind::Multi { options: many, selected: vec![1, 3] }),
            item(19, "Tuning", MenuKind::Separator),
            item(21, "Loaded", MenuKind::Progress(0.4)),
            item(22, "Values change as the game goes.", MenuKind::Label),
        ],
    );
    trainer.note = Some("Cheats".into());
    vec![
        trainer,
        menu(
            2,
            "trainer",
            Some(1),
            "Vehicles",
            vec![item(20, "Spawn", MenuKind::Button { action: Some("Spawn".into()) })],
        ),
        menu(3, "garage", None, "Garage", vec![item(30, "Lights", MenuKind::Toggle(true))]),
    ]
}

/// The Mods page's sheet with the menus in it, frame by frame.
struct Page {
    ctx: egui::Context,
    view: MenusView,
    menus: Vec<Menu>,
    size: Vec2,
    bottom: f32,
}

impl Page {
    fn new(size: Vec2) -> Page {
        let ctx = egui::Context::default();
        theme::install(&ctx);
        Page { ctx, view: MenusView::default(), menus: menus(), size, bottom: 16.0 }
    }

    fn frame(&mut self, events: Vec<Event>) -> Vec<Out> {
        let mut outs = Vec::new();
        let raw = RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, self.size)),
            safe_area_insets: Some(SafeAreaInsets(egui::epaint::MarginF32 {
                top: 24.0,
                bottom: self.bottom,
                left: 8.0,
                right: 8.0,
            })),
            events,
            ..Default::default()
        };
        let (view, menus) = (&mut self.view, &self.menus);
        let mut output = self.ctx.run_ui(raw, |root| {
            sheet(root.ctx(), "mods-window", "MODS", |ui| outs = view.view(ui, menus, MODS));
        });
        output.textures_delta.clear();
        outs
    }

    fn key(&mut self, key: Key) -> Vec<Out> {
        let event =
            |pressed| Event::Key { key, physical_key: None, pressed, repeat: false, modifiers: Modifiers::NONE };
        self.frame(vec![event(true), event(false)])
    }

    /// Focuses `id`, and draws once so that it has the focus.
    fn focus(&mut self, id: Id) {
        self.ctx.memory_mut(|m| m.request_focus(id));
        self.frame(Vec::new());
        self.frame(Vec::new());
        assert_eq!(self.focused(), Some(id));
    }

    fn focused(&self) -> Option<Id> {
        self.ctx.memory(|m| m.focused())
    }

    fn levels(&self) -> Vec<Level> {
        self.view.stack.iter().map(|o| o.level).collect()
    }

    fn back(&mut self) -> bool {
        let back = self.view.back(&self.ctx);
        self.frame(Vec::new());
        back
    }
}

fn input(item: u32, input: MenuInput) -> Out {
    Out::Input { module: "trainer".into(), item, input }
}

fn viewer(menu: u32, open: bool) -> Out {
    Out::Viewer { module: "trainer".into(), menu, open }
}

#[test]
fn the_list_has_the_top_level_menus_in_the_mods_order() {
    let mut page = Page::new(vec2(390.0, 844.0));
    page.frame(Vec::new());
    page.frame(Vec::new());
    let row = |id| page.ctx.read_response(menu_row_id(id)).map(|r| r.rect);
    let (garage, trainer) = (row(3).unwrap(), row(1).unwrap());
    assert!(garage.top() < trainer.top(), "Garage's menu comes first: {garage:?} {trainer:?}");
    assert!(row(2).is_none(), "a submenu is not in the list");
    assert!(page.view.viewing.is_empty());
}

#[test]
fn keys_open_menus_and_back_leaves_a_field_then_a_menu_then_the_page() {
    let mut page = Page::new(vec2(390.0, 844.0));
    page.frame(Vec::new());
    page.focus(menu_row_id(1));
    // Shown: the runtime hears it; opened by keys: its first row has the
    // focus.
    assert_eq!(page.key(Key::Enter), [viewer(1, true)]);
    assert_eq!(page.levels(), [Level::Menu(1)]);
    page.frame(Vec::new());
    assert_eq!(page.focused(), Some(row_id(10)));

    page.focus(row_id(16));
    assert_eq!(page.key(Key::Enter), [viewer(2, true)]);
    assert_eq!(page.levels(), [Level::Menu(1), Level::Menu(2)]);
    // Back: up a menu, the focus on the row that opened it.
    assert!(page.view.back(&page.ctx));
    assert_eq!(page.frame(Vec::new()), [viewer(2, false)]);
    page.frame(Vec::new());
    assert_eq!(page.levels(), [Level::Menu(1)]);
    assert_eq!(page.focused(), Some(row_id(16)));

    // A text item: Enter types into it; back lets go of the text (nothing
    // sent), then goes up, then has nothing left (the page closes).
    page.focus(row_id(15));
    page.key(Key::Enter);
    page.frame(Vec::new());
    assert!(page.ctx.text_edit_focused());
    page.frame(vec![Event::Text("X".into())]);
    assert!(page.back());
    assert!(!page.ctx.text_edit_focused());
    assert_eq!(page.frame(Vec::new()), Vec::new());
    assert_eq!(page.levels(), [Level::Menu(1)]);
    assert!(page.back());
    assert_eq!(page.levels(), []);
    assert!(!page.back());
    page.view.hidden();
}

#[test]
fn a_text_is_sent_when_committed() {
    let mut page = Page::new(vec2(390.0, 844.0));
    page.view.open_at(&page.menus.clone(), "trainer", 1);
    page.frame(Vec::new());
    page.focus(row_id(15));
    page.key(Key::Enter);
    page.frame(Vec::new());
    page.frame(vec![Event::Text("!".into())]);
    assert_eq!(page.key(Key::Enter), [input(15, MenuInput::Text("GLUE!".into()))]);
    page.frame(Vec::new());
    assert_eq!(page.focused(), Some(row_id(15)), "the row has the focus back");
}

#[test]
fn arrows_change_a_focused_slider_choice_and_number_and_move_between_rows() {
    let mut page = Page::new(vec2(390.0, 844.0));
    page.view.open_at(&page.menus.clone(), "trainer", 1);
    page.frame(Vec::new());
    page.focus(row_id(10));
    page.key(Key::ArrowDown);
    page.frame(Vec::new());
    assert_eq!(page.focused(), Some(row_id(11)));
    assert_eq!(page.key(Key::ArrowRight), [input(11, MenuInput::Number(6.0))]);
    assert_eq!(page.focused(), Some(row_id(11)), "sideways keys stay on the row");
    page.key(Key::ArrowDown);
    page.frame(Vec::new());
    assert_eq!(page.focused(), Some(row_id(12)));
    assert_eq!(page.key(Key::ArrowLeft), [input(12, MenuInput::Choose(2))], "a choice wraps around");
    page.focus(row_id(13));
    assert_eq!(page.key(Key::ArrowRight), [input(13, MenuInput::Number(110.0))]);
    assert_eq!(page.key(Key::ArrowLeft), [input(13, MenuInput::Number(90.0))]);
    page.focus(row_id(10));
    assert_eq!(page.key(Key::Enter), [input(10, MenuInput::Toggle(true))]);
}

#[test]
fn focus_skips_a_disabled_row() {
    let mut page = Page::new(vec2(390.0, 844.0));
    page.view.open_at(&page.menus.clone(), "trainer", 1);
    page.frame(Vec::new());
    page.focus(row_id(13));
    page.key(Key::ArrowDown);
    page.frame(Vec::new());
    assert_eq!(page.focused(), Some(row_id(15)), "past the disabled button");
    page.ctx.memory_mut(|m| m.request_focus(row_id(14)));
    page.frame(Vec::new());
    page.frame(Vec::new());
    assert_ne!(page.focused(), Some(row_id(14)));
}

#[test]
fn a_picker_chooses_and_goes_back_and_a_multi_ticks() {
    let mut page = Page::new(vec2(390.0, 844.0));
    page.view.open_at(&page.menus.clone(), "trainer", 1);
    page.frame(Vec::new());
    page.focus(row_id(12));
    page.key(Key::Enter);
    assert_eq!(page.levels(), [Level::Menu(1), Level::Pick(12)]);
    page.frame(Vec::new());
    page.frame(Vec::new());
    // Its chosen option has the focus.
    assert_eq!(page.focused(), Some(Id::new(("mod-option", 12u32, 0usize))));
    page.key(Key::ArrowDown);
    page.frame(Vec::new());
    assert_eq!(page.key(Key::Enter), [input(12, MenuInput::Choose(1))]);
    page.frame(Vec::new());
    assert_eq!(page.levels(), [Level::Menu(1)]);
    assert_eq!(page.focused(), Some(row_id(12)));

    page.focus(row_id(18));
    page.key(Key::Enter);
    page.frame(Vec::new());
    page.frame(Vec::new());
    assert_eq!(page.focused(), Some(Id::new(("mod-option", 18u32, 0usize))));
    assert_eq!(page.key(Key::Enter), [input(18, MenuInput::Select(vec![0, 1, 3]))]);
    assert_eq!(page.levels(), [Level::Menu(1), Level::Pick(18)], "a multi stays open");
}

#[test]
fn levels_of_removed_menus_and_items_go() {
    let mut page = Page::new(vec2(390.0, 844.0));
    page.view.open_at(&page.menus.clone(), "trainer", 2);
    assert_eq!(page.levels(), [Level::Menu(1), Level::Menu(2)]);
    assert_eq!(page.frame(Vec::new()), [viewer(1, true), viewer(2, true)]);
    page.menus.retain(|m| m.id != 2);
    page.menus[0].items.retain(|i| i.id != 16);
    assert_eq!(page.frame(Vec::new()), [viewer(2, false)]);
    assert_eq!(page.levels(), [Level::Menu(1)]);
    assert_eq!(page.view.hidden(), [viewer(1, false)]);
}

#[test]
fn every_level_fits_the_safe_area_at_all_sizes_and_above_a_keyboard() {
    for size in [vec2(1440.0, 900.0), vec2(768.0, 1024.0), vec2(390.0, 844.0), vec2(320.0, 480.0), vec2(844.0, 390.0)] {
        // A landscape phone has no room for a sheet above a 300-point
        // keyboard (theirs are lower).
        let keyboard = if size.y > size.x { 316.0 } else { 216.0 };
        for bottom in [16.0, keyboard] {
            for levels in [
                vec![],
                vec![Level::Menu(1)],
                vec![Level::Menu(1), Level::Pick(18)],
                vec![Level::Menu(1), Level::Color(17)],
            ] {
                let mut page = Page::new(size);
                page.bottom = bottom;
                for level in &levels {
                    page.view.push(*level, Id::NULL, false);
                }
                for _ in 0..4 {
                    page.frame(Vec::new());
                }
                assert_eq!(page.levels(), levels);
                let window = page.ctx.memory(|m| m.area_rect(Id::new("mods-window"))).unwrap();
                let safe = page.ctx.content_rect();
                assert!(safe.contains_rect(window), "{size:?} {bottom} {levels:?}: {window:?} outside {safe:?}");
            }
        }
    }
}

#[test]
fn a_level_starts_below_its_nav_bar() {
    for levels in [vec![Level::Menu(1)], vec![Level::Menu(1), Level::Color(17)], vec![Level::Menu(1), Level::Pick(12)]]
    {
        let mut page = Page::new(vec2(1280.0, 720.0));
        for level in &levels {
            page.view.push(*level, Id::NULL, false);
        }
        page.frame(Vec::new());
        page.frame(Vec::new());
        let bar = page.ctx.read_response(super::back_id()).unwrap().rect;
        let first = match levels.last().unwrap() {
            Level::Menu(_) => page.ctx.read_response(row_id(10)).unwrap().rect,
            Level::Color(_) => page.ctx.read_response(Id::new(("mod-swatch", 17u32, 0usize))).unwrap().rect,
            Level::Pick(_) => page.ctx.read_response(Id::new(("mod-option", 12u32, 0usize))).unwrap().rect,
        };
        assert!(first.top() >= bar.bottom(), "{levels:?}: {first:?} under {bar:?}");
    }
}

#[test]
fn a_menus_search_filters_its_rows_and_stays_while_inside() {
    let mut page = Page::new(vec2(390.0, 844.0));
    page.menus[0].search = true;
    page.view.open_at(&page.menus.clone(), "trainer", 1);
    page.frame(Vec::new());
    page.frame(Vec::new());
    let shown = |page: &Page, id| page.ctx.read_response(row_id(id)).is_some_and(|r| r.rect.is_positive());
    assert!(shown(&page, 10) && shown(&page, 16));
    // By label or note, case aside; headings go.
    page.view.stack.last_mut().unwrap().query = "HURTS".into();
    page.frame(Vec::new());
    page.frame(Vec::new());
    assert!(shown(&page, 10), "found by its note");
    assert!(!shown(&page, 16) && !shown(&page, 19) && !shown(&page, 22));
    page.view.stack.last_mut().unwrap().query = "vehi".into();
    page.frame(Vec::new());
    page.frame(Vec::new());
    assert!(shown(&page, 16) && !shown(&page, 10));
    // Into a row it found and back: the search is still there.
    page.focus(row_id(16));
    page.key(Key::Enter);
    page.frame(Vec::new());
    assert!(page.back());
    assert_eq!(page.view.stack.last().unwrap().query, "vehi");
    page.view.hidden();
}
