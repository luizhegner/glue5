//! The mods' menus in the Mods page: one entry per menu, each opening what
//! its mod defines (submenus, a list's options, a colour) as a stack of
//! levels with a back button. Drawn every frame from the runtime's
//! snapshot with the app's own rows; touch, mouse, keyboard and controllers
//! alike (a controller arrives as arrows and Enter, see `Ui::nav`). The
//! view only reads the menus it is given and says what the player did
//! ([`Out`]): the Mods page sends it, tests read it.

mod color;
mod pick;
mod rows;
#[cfg(test)]
mod tests;

use egui::{Align, Id, Layout, Rect, Response, RichText, Sense, Ui, UiBuilder, pos2, vec2};
use glue5_mods::{Menu, MenuInput, MenuItem, MenuKind, Rgba};

use super::TextKind;
use super::theme::{self, MUTED, density};
use super::widgets::{focus, text_field};

/// A level of the stack: what the page shows past the list of menus.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Level {
    /// A menu, top-level or a submenu, by id.
    Menu(u32),
    /// A choice's or a multi's options, by the item's id.
    Pick(u32),
    /// A colour item's page.
    Color(u32),
}

/// What the player did, for the runtime.
#[derive(Clone, Debug, PartialEq)]
pub enum Out {
    Input {
        module: String,
        item: u32,
        input: MenuInput,
    },
    /// The page starts or stops showing a menu (its `onOpen`/`onClose`).
    Viewer {
        module: String,
        menu: u32,
        open: bool,
    },
}

/// What a row asks for.
pub(super) enum RowAction {
    Input(MenuInput),
    /// Open a level; `keys`: by a key or a controller (the row had the focus).
    Push {
        level: Level,
        keys: bool,
    },
    /// Start typing into the item (a text, a number's typed entry).
    Edit,
}

/// What a push or a pop leaves for the next drawing.
enum Pending {
    /// A level was opened: its top shows, and its first row has the focus
    /// when keys opened it.
    Opened { focus: bool },
    /// Back to a level: the row that opened the one left shows, and has
    /// the focus when keys went back.
    Row { id: Id, focus: bool },
}

/// The text typed into an item until it is committed (Enter, Done, the
/// field left) or dropped (Back).
pub(super) struct Draft {
    pub item: u32,
    pub text: String,
    /// Not drawn yet: the field takes the focus at its first drawing.
    pub fresh: bool,
}

struct Open {
    level: Level,
    /// The row that opened it.
    from: Id,
    /// What the player typed in a menu's search (`search`), kept while
    /// they go into one of the rows it found and back.
    query: String,
}

/// What the page keeps between frames: the stack, the focus to give, the
/// text being typed, a picker's search, the colour page's hue.
#[derive(Default)]
pub struct MenusView {
    stack: Vec<Open>,
    pending: Option<Pending>,
    draft: Option<Draft>,
    search: String,
    /// The colour page's item, the colour last sent and the HSV it came
    /// from (grey loses the hue: the sliders keep it).
    hsv: Option<(u32, Rgba, [f32; 3])>,
    /// The menus the runtime was told are shown.
    viewing: Vec<(String, u32)>,
}

pub(super) fn row_id(item: u32) -> Id {
    Id::new(("mod-item", item))
}

fn menu_row_id(menu: u32) -> Id {
    Id::new(("mod-menu", menu))
}

fn find_menu(menus: &[Menu], id: u32) -> Option<&Menu> {
    menus.iter().find(|m| m.id == id)
}

/// An item and the menu it is in.
fn find_item(menus: &[Menu], id: u32) -> Option<(&Menu, &MenuItem)> {
    menus.iter().find_map(|m| m.items.iter().find(|i| i.id == id).map(|i| (m, i)))
}

/// The row in `menu`'s parent that opens it.
fn submenu_row(menus: &[Menu], menu: &Menu) -> Option<(u32, bool)> {
    let parent = find_menu(menus, menu.parent?)?;
    parent.items.iter().find(|i| i.kind == MenuKind::Submenu(menu.id)).map(|i| (i.id, i.visible))
}

impl MenusView {
    /// Past the list of menus: the nav bar replaces the page's tabs.
    pub fn inside(&self) -> bool {
        !self.stack.is_empty()
    }

    /// Levels whose menu or item is gone (the mod removed it, or stopped)
    /// or can no longer be used go.
    pub fn prune(&mut self, menus: &[Menu]) {
        let usable = |level: Level| match level {
            Level::Menu(id) => find_menu(menus, id)
                .is_some_and(|m| m.parent.is_none() || submenu_row(menus, m).is_some_and(|(_, visible)| visible)),
            Level::Pick(id) => find_item(menus, id).is_some_and(|(_, i)| {
                i.enabled && i.visible && matches!(i.kind, MenuKind::Choice { .. } | MenuKind::Multi { .. })
            }),
            Level::Color(id) => find_item(menus, id)
                .is_some_and(|(_, i)| i.enabled && i.visible && matches!(i.kind, MenuKind::Color { .. })),
        };
        if let Some(n) = self.stack.iter().position(|o| !usable(o.level)) {
            self.stack.truncate(n);
            self.draft = None;
            self.pending = Some(Pending::Opened { focus: false });
        }
    }

    /// Back (B, Esc): a field being typed into lets go of the text, else a
    /// level closes. False when there was nothing to go back from (the page
    /// closes).
    pub fn back(&mut self, ctx: &egui::Context) -> bool {
        if ctx.text_edit_focused() {
            ctx.memory_mut(|m| {
                if let Some(id) = m.focused() {
                    m.surrender_focus(id);
                }
            });
            if let Some(draft) = self.draft.take() {
                self.pending = Some(Pending::Row { id: row_id(draft.item), focus: true });
            }
            return true;
        }
        if self.stack.is_empty() {
            return false;
        }
        self.pop(true);
        true
    }

    /// Opens `menu` of `module` with the levels above it (`menu:show()`).
    /// False when there is no such menu.
    pub fn open_at(&mut self, menus: &[Menu], module: &str, menu: u32) -> bool {
        let mut path = Vec::new();
        let mut at = Some(menu);
        while let Some(id) = at {
            let Some(m) = find_menu(menus, id).filter(|m| m.module == module) else { return false };
            let from = match submenu_row(menus, m) {
                Some((row, _)) => row_id(row),
                None => menu_row_id(m.id),
            };
            path.push(Open { level: Level::Menu(id), from, query: String::new() });
            at = m.parent;
        }
        path.reverse();
        self.stack = path;
        self.draft = None;
        self.pending = Some(Pending::Opened { focus: false });
        true
    }

    /// The page no longer shows the menus (closed, another tab): the
    /// runtime hears that they are not looked at. The stack stays (the
    /// page opens again where the player was).
    pub fn hidden(&mut self) -> Vec<Out> {
        self.draft = None;
        self.viewing.drain(..).map(|(module, menu)| Out::Viewer { module, menu, open: false }).collect()
    }

    fn push(&mut self, level: Level, from: Id, keys: bool) {
        self.stack.push(Open { level, from, query: String::new() });
        self.draft = None;
        if matches!(level, Level::Pick(_)) {
            self.search.clear();
        }
        self.pending = Some(Pending::Opened { focus: keys });
    }

    fn pop(&mut self, keys: bool) {
        if let Some(open) = self.stack.pop() {
            self.draft = None;
            self.pending = Some(Pending::Row { id: open.from, focus: keys });
        }
    }

    /// Draws the list of menus, or the level on top of the stack under its
    /// nav bar. `mods` are the mods' (name, title), in the order the page
    /// lists them.
    pub fn view(&mut self, ui: &mut Ui, menus: &[Menu], mods: &[(&str, &str)]) -> Vec<Out> {
        self.prune(menus);
        let pending = self.pending.take();
        let mut out = Vec::new();
        let title =
            |module: &str| mods.iter().find(|(name, _)| *name == module).map_or(module, |(_, title)| title).to_owned();
        if pending.is_some() && !self.stack.is_empty() {
            // A level opened or closed: from its top (the row to show next
            // scrolls further).
            ui.scroll_to_cursor(Some(Align::TOP));
        }
        let mut landing = Landing { pending: pending.as_ref(), done: false };
        match self.stack.last().map(|o| o.level) {
            None => self.root(ui, menus, mods, &mut landing),
            Some(Level::Menu(id)) => {
                let menu = find_menu(menus, id).expect("pruned");
                if self.nav_bar(ui, &menu.title, &title(&menu.module), |_| {}) {
                    self.pop(ui.ctx().memory(|m| m.focused() == Some(back_id())));
                } else {
                    self.menu(ui, menu, &mut landing, &mut out);
                }
            }
            Some(Level::Pick(id)) => {
                let (menu, item) = find_item(menus, id).expect("pruned");
                let filtered = pick::filter(item, &self.search);
                let mut chosen = None;
                let back = self.nav_bar(ui, &item.label, &title(&menu.module), |ui| {
                    chosen = pick::header(ui, item, &filtered);
                });
                if back {
                    self.pop(ui.ctx().memory(|m| m.focused() == Some(back_id())));
                } else {
                    if let Some(input) = chosen {
                        out.push(Out::Input { module: menu.module.clone(), item: item.id, input });
                    }
                    match pick::list(ui, item, &filtered, &mut self.search, pending.as_ref()) {
                        Some(pick::Picked::Chosen { input, keys }) => {
                            if let Some(input) = input {
                                out.push(Out::Input { module: menu.module.clone(), item: item.id, input });
                            }
                            self.pop(keys);
                        }
                        Some(pick::Picked::Toggled(input)) => {
                            out.push(Out::Input { module: menu.module.clone(), item: item.id, input });
                        }
                        None => {}
                    }
                }
            }
            Some(Level::Color(id)) => {
                let (menu, item) = find_item(menus, id).expect("pruned");
                if self.nav_bar(ui, &item.label, &title(&menu.module), |_| {}) {
                    self.pop(ui.ctx().memory(|m| m.focused() == Some(back_id())));
                } else if let Some(input) = color::page(ui, item, &mut self.hsv, &mut self.draft, &mut landing) {
                    out.push(Out::Input { module: menu.module.clone(), item: item.id, input });
                }
            }
        }
        self.viewers(menus, &mut out);
        out
    }

    /// The list of menus: one row each, the mods in the page's order.
    fn root(&mut self, ui: &mut Ui, menus: &[Menu], mods: &[(&str, &str)], landing: &mut Landing) {
        let mut top: Vec<&Menu> = menus.iter().filter(|m| m.parent.is_none()).collect();
        if top.is_empty() {
            muted(ui, "No menus. A mod makes one with ui.menu(\"Title\"); F7 opens them during the game.");
            return;
        }
        // Stable: a mod's menus stay in the order they were made.
        top.sort_by_key(|m| mods.iter().position(|(name, _)| *name == m.module).unwrap_or(usize::MAX));
        for menu in top {
            let module = mods.iter().find(|(name, _)| *name == menu.module).map_or(menu.module.as_str(), |(_, t)| t);
            let subtitle = match &menu.note {
                Some(note) => format!("{module} · {note}"),
                None => module.to_owned(),
            };
            let id = menu_row_id(menu.id);
            let row = rows::frame(ui, id, &menu.title, Some(&subtitle), 16.0 * density(ui), Sense::click(), landing);
            rows::chevron(ui, row.control.right_center() - vec2(4.0 * density(ui), 0.0), vec2(1.0, 0.0), MUTED);
            if row.response.clicked() {
                let keys = row.response.has_focus();
                self.push(Level::Menu(menu.id), id, keys);
            }
        }
    }

    /// A menu's items, under its search when it has one.
    fn menu(&mut self, ui: &mut Ui, menu: &Menu, landing: &mut Landing, out: &mut Vec<Out>) {
        let query = match self.stack.last_mut() {
            Some(open) if menu.search => {
                text_field(ui, "", TextKind::Text, &mut open.query, "Search", 40);
                open.query.trim().to_lowercase()
            }
            _ => String::new(),
        };
        let mut shown = false;
        for item in menu.items.iter().filter(|i| i.visible && found(i, &query)) {
            shown = true;
            let action = ui.add_enabled_ui(item.enabled, |ui| rows::row(ui, item, &mut self.draft, landing)).inner;
            match action {
                Some(RowAction::Input(input)) => {
                    out.push(Out::Input { module: menu.module.clone(), item: item.id, input })
                }
                Some(RowAction::Push { level, keys }) => self.push(level, row_id(item.id), keys),
                Some(RowAction::Edit) => {
                    self.draft = Some(Draft { item: item.id, text: rows::edit_text(item), fresh: true });
                }
                None => {}
            }
        }
        if !shown {
            muted(ui, if query.is_empty() { "Nothing in this menu yet." } else { "Nothing here has that." });
        }
    }

    /// The back button, the level's title and its mod's, and on the right
    /// what `right` adds. True when back was pressed.
    fn nav_bar(&mut self, ui: &mut Ui, title: &str, module: &str, right: impl FnOnce(&mut Ui)) -> bool {
        let s = density(ui);
        let height = 44.0 * s;
        let (bar, _) = ui.allocate_exact_size(vec2(ui.available_width(), height), Sense::hover());
        let back = back_button(ui, Rect::from_min_size(bar.min, vec2(height, height)));
        let rest = Rect::from_min_max(pos2(bar.left() + height + 10.0 * s, bar.top()), bar.max);
        // In the bar's own space (a scope would move the page's cursor).
        let mut end = ui.new_child(UiBuilder::new().max_rect(rest).layout(Layout::right_to_left(Align::Center)));
        end.spacing_mut().item_spacing.x = 6.0 * s;
        right(&mut end);
        let used = end.min_rect().width();
        let width = (rest.width() - if used > 0.0 { used + 10.0 * s } else { 0.0 }).max(1.0);
        let painter = ui.painter();
        let heading = rows::truncated(ui, theme::heading(&title.to_uppercase(), 22.0 * s), width);
        let x = rest.left();
        painter.galley(pos2(x, bar.center().y - heading.size().y * 0.5), heading.clone(), egui::Color32::WHITE);
        let left = width - heading.size().x - 8.0 * s;
        if left > 24.0 * s {
            let module = rows::truncated(ui, RichText::new(module).size(12.0 * s).color(MUTED), left);
            let at = pos2(x + heading.size().x + 8.0 * s, bar.center().y - module.size().y * 0.5 + 1.0 * s);
            painter.galley(at, module, MUTED);
        }
        back.clicked()
    }

    /// Tells the runtime which menus the page shows: those on the stack.
    fn viewers(&mut self, menus: &[Menu], out: &mut Vec<Out>) {
        let shown: Vec<(String, u32)> = self
            .stack
            .iter()
            .filter_map(|o| match o.level {
                Level::Menu(id) => find_menu(menus, id).map(|m| (m.module.clone(), id)),
                _ => None,
            })
            .collect();
        for (module, menu) in self.viewing.extract_if(.., |v| !shown.contains(v)) {
            out.push(Out::Viewer { module, menu, open: false });
        }
        for v in shown {
            if !self.viewing.contains(&v) {
                out.push(Out::Viewer { module: v.0.clone(), menu: v.1, open: true });
                self.viewing.push(v);
            }
        }
    }
}

/// Whether a menu's search (lowercase; empty: none) finds `item`: by its
/// label or its note. Headings and texts go while it searches.
fn found(item: &MenuItem, query: &str) -> bool {
    if query.is_empty() {
        return true;
    }
    if matches!(item.kind, MenuKind::Separator | MenuKind::Label) {
        return false;
    }
    item.label.to_lowercase().contains(query) || item.note.as_ref().is_some_and(|n| n.to_lowercase().contains(query))
}

fn back_id() -> Id {
    Id::new("mod-menus-back")
}

/// The close button's look, pointing back.
fn back_button(ui: &mut Ui, rect: Rect) -> Response {
    let s = density(ui);
    let response = ui.interact(rect, back_id(), Sense::click());
    response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, ui.is_enabled(), "Back"));
    let c = rect.center();
    ui.painter().circle_filled(c, 18.0 * s, egui::Color32::from_gray(if response.hovered() { 60 } else { 32 }));
    let stroke = egui::Stroke::new(1.6, egui::Color32::WHITE);
    let tip = c + vec2(-3.0 * s, 0.0);
    ui.painter().line_segment([tip, tip + vec2(6.0 * s, -6.0 * s)], stroke);
    ui.painter().line_segment([tip, tip + vec2(6.0 * s, 6.0 * s)], stroke);
    if response.has_focus() {
        ui.painter().circle_stroke(c, 20.0 * s, egui::Stroke::new(1.0, egui::Color32::WHITE));
    }
    focus(response).on_hover_text("Back")
}

/// Gives a level's rows what a push or a pop left: the focus, the view.
pub(super) struct Landing<'a> {
    pending: Option<&'a Pending>,
    done: bool,
}

impl Landing<'_> {
    /// A focusable row as it is drawn.
    pub(super) fn row(&mut self, response: &Response) {
        if self.done || !response.sense.is_focusable() || !response.enabled() {
            return;
        }
        match self.pending {
            Some(Pending::Opened { focus: true }) => {
                response.request_focus();
                self.done = true;
            }
            Some(Pending::Row { id, focus }) if response.id == *id => {
                if *focus {
                    response.request_focus();
                }
                response.scroll_to_me(Some(Align::Center));
                self.done = true;
            }
            _ => {}
        }
    }
}

fn muted(ui: &mut Ui, text: &str) {
    ui.label(RichText::new(text).size(12.0 * density(ui)).color(MUTED));
}
