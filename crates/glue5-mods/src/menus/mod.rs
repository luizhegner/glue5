//! Mods' menus: `ui.menu(...)` in Luau makes one; the application draws it
//! with its own widgets (touch and controllers included), the Studio and
//! MCP too, and send back what the player does (`menu_input`). The shared
//! value changes at once; the mod hears it at its next turn: the item's
//! `value` changes and its callback runs as a task.
//!
//! The menus live here, behind one lock, as one `Arc` that readers share
//! until the next change; the mod's side (lua.rs) keeps its own copy of
//! each item, which inputs reach only at the mod's turn.

mod lua;
pub mod validate;
#[cfg(test)]
mod tests;

use std::collections::HashMap;
use std::sync::atomic::Ordering;
use std::sync::{Arc, LazyLock, Mutex};
use std::time::Instant;

pub use lua::{Handlers, install, take_calls};

/// The Luau definitions of `ui` and the menus' types, for the type
/// checker, the Studio and MCP.
pub const DEFINITIONS: &str = include_str!("menus.d.luau");

use crate::{Menu, MenuInput, MenuItem, MenuKind, SHARED};

/// What waits for a mod's turn.
#[derive(Clone, Debug, PartialEq)]
pub enum Event {
    /// A player's input on an item (checked, made to fit).
    Input(u32, MenuInput),
    /// A menu got its first viewer (true) or lost its last (false).
    Viewer(u32, bool),
}

struct Pending {
    instance: u64,
    event: Event,
}

#[derive(Default)]
struct Store {
    /// Every menu, submenus included, in creation order; shared with the
    /// readers of [`snapshot`] until the next change.
    menus: Arc<Vec<Menu>>,
    /// In the order they came; values coalesced per item.
    pending: Vec<Pending>,
    /// Viewers per menu.
    viewers: HashMap<u32, u32>,
    /// A mod asked to be shown: (module, menu).
    show: Option<(String, u32)>,
    /// When each mod was last shown.
    shown: HashMap<String, Instant>,
    next_id: u32,
}

static STORE: LazyLock<Mutex<Store>> = LazyLock::new(|| Mutex::new(Store { next_id: 1, ..Store::default() }));

fn store() -> std::sync::MutexGuard<'static, Store> {
    crate::lock(&STORE)
}

fn changed() {
    SHARED.generation.fetch_add(1, Ordering::Relaxed);
}

impl Store {
    fn menu_index(&self, id: u32) -> Option<usize> {
        self.menus.iter().position(|m| m.id == id)
    }

    /// The menu and position of item `id`.
    fn item_index(&self, id: u32) -> Option<(usize, usize)> {
        self.menus.iter().enumerate().find_map(|(m, menu)| menu.items.iter().position(|i| i.id == id).map(|i| (m, i)))
    }

    /// Changes menu number `index` (a copy if readers hold the snapshot).
    fn edit(&mut self, index: usize, f: impl FnOnce(&mut Menu)) {
        let menu = &mut Arc::make_mut(&mut self.menus)[index];
        f(menu);
        menu.rev += 1;
        changed();
    }

    /// Whether every submenu row on the way to menu `id` is shown.
    fn chain_visible(&self, mut id: u32) -> bool {
        // Up the parents: a menu's parent is made before it, so this ends
        // at a top-level menu; bounded anyway by the number of menus.
        for _ in 0..=self.menus.len() {
            let Some(menu) = self.menus.iter().find(|m| m.id == id) else { return false };
            let Some(parent) = menu.parent else { return true };
            let row = self.menus.iter().find(|m| m.id == parent).and_then(|p| p.items.iter().find(|i| i.kind == MenuKind::Submenu(id)));
            match row {
                Some(row) if row.visible => id = parent,
                _ => return false,
            }
        }
        false
    }

    /// Value inputs waiting for item `id` go (the mod set it since).
    fn drop_values(&mut self, id: u32) {
        self.pending.retain(|p| !matches!(&p.event, Event::Input(i, input) if *i == id && *input != MenuInput::Press));
    }
}

/// A new id for a menu or an item (unique across mods and runs).
pub(crate) fn next_id() -> u32 {
    let mut s = store();
    let id = s.next_id;
    s.next_id += 1;
    id
}

/// The menus, as one snapshot shared until the next change.
pub(crate) fn snapshot() -> Arc<Vec<Menu>> {
    store().menus.clone()
}

/// Whether any mod has a menu.
pub(crate) fn any() -> bool {
    !store().menus.is_empty()
}

/// Whether a run of a mod has a menu (then it stays, waiting for players).
pub fn has_menus(instance: u64) -> bool {
    store().menus.iter().any(|m| m.instance == instance)
}

/// A number for each run of a mod.
pub fn next_instance() -> u64 {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
    NEXT.fetch_add(1, Ordering::Relaxed)
}

/// A mod made a menu.
pub(crate) fn add_menu(menu: Menu) {
    Arc::make_mut(&mut store().menus).push(menu);
    changed();
}

/// A mod added an item to menu `menu`; false when the menu is gone.
pub(crate) fn add_item(menu: u32, item: MenuItem) -> bool {
    let mut s = store();
    let Some(m) = s.menu_index(menu) else { return false };
    s.edit(m, |m| m.items.push(item));
    true
}

// A mod may set its items every frame (to show the game's state): what
// does not change leaves the snapshot, its revision and the readers alone.

/// What a mod changes of a menu itself.
pub(crate) enum MenuField {
    Title(String),
    Note(Option<String>),
    Search(bool),
}

/// A mod changed menu `id`'s title, note or search.
pub(crate) fn edit_menu(id: u32, field: MenuField) {
    let mut s = store();
    let Some(m) = s.menu_index(id) else { return };
    let menu = &s.menus[m];
    let same = match &field {
        MenuField::Title(t) => menu.title == *t,
        MenuField::Note(n) => menu.note == *n,
        MenuField::Search(on) => menu.search == *on,
    };
    if !same {
        s.edit(m, |m| match field {
            MenuField::Title(t) => m.title = t,
            MenuField::Note(n) => m.note = n,
            MenuField::Search(on) => m.search = on,
        });
    }
}

/// A mod changed item `id`'s label, note, enabled or visible: values
/// waiting for the mod still reach it.
pub(crate) fn edit_item(id: u32, f: impl FnOnce(&mut MenuItem)) {
    let mut s = store();
    if let Some((m, i)) = s.item_index(id) {
        let mut item = s.menus[m].items[i].clone();
        f(&mut item);
        if item != s.menus[m].items[i] {
            s.edit(m, |m| m.items[i] = item);
        }
    }
}

/// A mod set item `id`'s value (or what it means: its options, range):
/// the last write wins, so a player's value waiting for the mod goes.
pub(crate) fn set_kind(id: u32, kind: MenuKind) {
    let mut s = store();
    if let Some((m, i)) = s.item_index(id) {
        s.drop_values(id);
        if s.menus[m].items[i].kind != kind {
            s.edit(m, |m| m.items[i].kind = kind);
        }
    }
}

/// A mod removed items, and the menus they opened (with their rows, when
/// a submenu itself was removed): all of it goes, with what waits for it.
pub(crate) fn remove(items: &[u32], menus: &[u32]) {
    let mut s = store();
    let touched: Vec<usize> = s
        .menus
        .iter()
        .enumerate()
        .filter(|(_, m)| !menus.contains(&m.id) && m.items.iter().any(|i| items.contains(&i.id)))
        .map(|(n, _)| n)
        .collect();
    for n in touched {
        s.edit(n, |m| m.items.retain(|i| !items.contains(&i.id)));
    }
    if s.menus.iter().any(|m| menus.contains(&m.id)) {
        Arc::make_mut(&mut s.menus).retain(|m| !menus.contains(&m.id));
        changed();
    }
    s.pending.retain(|p| match &p.event {
        Event::Input(i, _) => !items.contains(i),
        Event::Viewer(m, _) => !menus.contains(m),
    });
    s.viewers.retain(|m, _| !menus.contains(m));
    if s.show.as_ref().is_some_and(|(_, m)| menus.contains(m)) {
        s.show = None;
    }
}

/// The menus of a run of a mod go (it stopped); not those of the mod's
/// next run, which may be there already (a restart).
pub fn remove_instance(instance: u64) {
    let mut s = store();
    let gone: Vec<u32> = s.menus.iter().filter(|m| m.instance == instance).map(|m| m.id).collect();
    if !gone.is_empty() {
        Arc::make_mut(&mut s.menus).retain(|m| m.instance != instance);
        changed();
    }
    s.pending.retain(|p| p.instance != instance);
    s.viewers.retain(|m, _| !gone.contains(m));
    if s.show.as_ref().is_some_and(|(_, m)| gone.contains(m)) {
        s.show = None;
    }
}

/// What waits for a run of a mod, in order; it is the mod's now.
pub(crate) fn take(instance: u64) -> Vec<Event> {
    let mut s = store();
    if !s.pending.iter().any(|p| p.instance == instance) {
        return Vec::new();
    }
    let mut out = Vec::new();
    s.pending.retain_mut(|p| {
        if p.instance == instance {
            out.push(std::mem::replace(&mut p.event, Event::Viewer(0, false)));
            false
        } else {
            true
        }
    });
    out
}

/// A player's input (see [`crate::menu_input`]).
pub(crate) fn input(module: &str, item: u32, input: MenuInput) -> Result<(), String> {
    let mut s = store();
    let Some((m, i)) = s.item_index(item).filter(|(m, _)| s.menus[*m].module == module) else {
        return Err(format!("{module} has no menu item {item}"));
    };
    let menu = &s.menus[m];
    let it = &menu.items[i];
    if !it.visible {
        return Err(format!("\"{}\" is hidden", it.label));
    }
    if !it.enabled {
        return Err(format!("\"{}\" is disabled", it.label));
    }
    if !s.chain_visible(menu.id) {
        return Err(format!("menu \"{}\" is hidden", menu.title));
    }
    let instance = menu.instance;
    let mut kind = it.kind.clone();
    let input = validate::input(&mut kind, input)?;
    if kind != s.menus[m].items[i].kind {
        s.edit(m, |menu| menu.items[i].kind = kind);
    }
    if input == MenuInput::Press {
        let press = |p: &Pending| p.instance == instance && matches!(p.event, Event::Input(_, MenuInput::Press));
        // The oldest press goes past the cap.
        if s.pending.iter().filter(|p| press(p)).count() >= validate::PRESSES
            && let Some(oldest) = s.pending.iter().position(press)
        {
            s.pending.remove(oldest);
        }
        s.pending.push(Pending { instance, event: Event::Input(item, input) });
    } else {
        // A newer value replaces the one waiting: a dragged slider is one
        // callback per turn, with the latest value.
        let waiting = s.pending.iter_mut().find(|p| matches!(&p.event, Event::Input(i, x) if *i == item && *x != MenuInput::Press));
        match waiting {
            Some(p) => p.event = Event::Input(item, input),
            None => s.pending.push(Pending { instance, event: Event::Input(item, input) }),
        }
    }
    Ok(())
}

/// A viewer starts or stops looking at a menu (see [`crate::menu_viewer`]).
pub(crate) fn viewer(module: &str, menu: u32, open: bool) {
    let mut s = store();
    let Some(instance) = s.menus.iter().find(|m| m.id == menu && m.module == module).map(|m| m.instance) else { return };
    let count = s.viewers.entry(menu).or_insert(0);
    let before = *count;
    *count = if open { before + 1 } else { before.saturating_sub(1) };
    let after = *count;
    if after == 0 {
        s.viewers.remove(&menu);
    }
    if (before == 0) == (after == 0) {
        return;
    }
    // Opened and closed again before the mod's turn: it hears neither.
    let last = s.pending.iter().rposition(|p| matches!(p.event, Event::Viewer(m, _) if m == menu));
    match last {
        Some(n) if s.pending[n].event == Event::Viewer(menu, !open) => {
            s.pending.remove(n);
        }
        _ => s.pending.push(Pending { instance, event: Event::Viewer(menu, open) }),
    }
}

/// A mod asked to show menu `menu`: recorded unless it was shown less than
/// two seconds ago. Whether it was.
pub(crate) fn request_show(module: &str, menu: u32) -> bool {
    let mut s = store();
    let now = Instant::now();
    if s.shown.get(module).is_some_and(|t| now.duration_since(*t) < validate::SHOW_EVERY) {
        return false;
    }
    s.shown.insert(module.to_string(), now);
    s.show = Some((module.to_string(), menu));
    true
}

/// See [`crate::take_menu_show`].
pub(crate) fn take_show() -> Option<(String, u32)> {
    store().show.take()
}
