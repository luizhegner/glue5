//! The mod's side of its menus: `ui.menu(title)`, the menus' and items'
//! handles. A handle reads and writes properties (`god.value = true`,
//! `row.enabled = false`) checked as the builders check them; the mod's
//! copy of each item lives here, and players' inputs reach it only at the
//! mod's turn ([`take_calls`]), together with their callbacks.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Arc;

use mlua::{AnyUserData, Function, Lua, MetaMethod, MultiValue, Table, UserData, UserDataMethods, Value};

use super::Event;
use super::validate::{self, cut};
use crate::{Menu, MenuInput, MenuItem, MenuKind, Rgba, SHARED};

/// A run of a mod's menus and items, as the mod sees them.
pub type Handlers = Rc<RefCell<Local>>;

#[derive(Default)]
pub struct Local {
    pub(super) items: HashMap<u32, LocalItem>,
    pub(super) menus: HashMap<u32, LocalMenu>,
}

pub(super) struct LocalItem {
    menu: u32,
    item: MenuItem,
    /// `onChange` / `onPress`.
    callback: Option<Function>,
}

pub(super) struct LocalMenu {
    /// The menu whose row opens this one, and that row.
    parent: Option<u32>,
    row: Option<u32>,
    title: String,
    note: Option<String>,
    /// Players filter its rows by what they type.
    search: bool,
    /// A top-level menu's (a submenu's are its row's).
    enabled: bool,
    visible: bool,
    items: Vec<u32>,
    /// Run when the first viewer opens it, the last one closes it.
    on_open: Option<Function>,
    on_close: Option<Function>,
}

/// What every handle of a run of a mod shares.
struct Ctx {
    module: String,
    instance: u64,
    local: Handlers,
}

type Result<T> = mlua::Result<T>;

fn error(text: String) -> mlua::Error {
    mlua::Error::runtime(text)
}

/// A value's type as Luau names it.
fn type_of(v: &Value) -> &'static str {
    match v {
        Value::Integer(_) | Value::Number(_) => "number",
        other => other.type_name(),
    }
}

/// The handle's type, as the definitions name it.
fn type_name(kind: &MenuKind) -> &'static str {
    match kind {
        MenuKind::Button { .. } => "Button",
        MenuKind::Toggle(_) => "Toggle",
        MenuKind::Slider { .. } => "Slider",
        MenuKind::Number { .. } => "Number",
        MenuKind::Choice { .. } => "Choice",
        MenuKind::Multi { .. } => "Multi",
        MenuKind::Text { .. } => "Text",
        MenuKind::Color { .. } => "Color",
        MenuKind::Label => "Label",
        MenuKind::Separator => "Separator",
        MenuKind::Progress(_) => "Progress",
        MenuKind::Submenu(_) => "Menu",
    }
}

fn boolean(what: &str, v: &Value) -> Result<bool> {
    match v {
        Value::Boolean(b) => Ok(*b),
        other => Err(error(format!("{what}: a boolean, not {}", type_of(other)))),
    }
}

fn number(what: &str, v: &Value) -> Result<f64> {
    let n = match *v {
        Value::Integer(i) => i as f64,
        Value::Number(n) => n,
        ref other => return Err(error(format!("{what}: a number, not {}", type_of(other)))),
    };
    if n.is_finite() { Ok(n) } else { Err(error(format!("{what}: a finite number, not {n}"))) }
}

fn optional_number(what: &str, v: &Value) -> Result<Option<f64>> {
    if v.is_nil() { Ok(None) } else { number(what, v).map(Some) }
}

fn string(what: &str, v: &Value) -> Result<String> {
    match v {
        Value::String(s) => Ok(s.to_string_lossy()),
        other => Err(error(format!("{what}: a string, not {}", type_of(other)))),
    }
}

fn optional_string(what: &str, v: &Value, max: usize) -> Result<Option<String>> {
    if v.is_nil() { Ok(None) } else { string(what, v).map(|s| Some(cut(&s, max))) }
}

fn callback(what: &str, v: Value) -> Result<Option<Function>> {
    match v {
        Value::Nil => Ok(None),
        Value::Function(f) => Ok(Some(f)),
        other => Err(error(format!("{what}: a function, not {}", type_of(&other)))),
    }
}

/// A list of options: 1 to 65536 strings, each cut to 40 characters.
fn options(what: &str, v: &Value) -> Result<Arc<[String]>> {
    let Value::Table(t) = v else { return Err(error(format!("{what}: a list of strings, not {}", type_of(v)))) };
    let n = t.raw_len();
    if n == 0 {
        return Err(error(format!("{what}: give it options")));
    }
    if n > validate::OPTIONS {
        return Err(error(format!("{what}: {} options at most", validate::OPTIONS)));
    }
    let mut out = Vec::with_capacity(n);
    for i in 1..=n {
        let o = string(&format!("{what}[{i}]"), &t.raw_get::<Value>(i)?)?;
        if o.is_empty() {
            return Err(error(format!("{what}: option {i} is empty")));
        }
        out.push(cut(&o, validate::OPTION));
    }
    Ok(out.into())
}

/// One option, by its index from 1 or its text (the first match).
fn option(what: &str, options: &[String], v: &Value) -> Result<usize> {
    match v {
        Value::String(s) => {
            let s = s.to_string_lossy();
            options.iter().position(|o| *o == s).ok_or_else(|| error(format!("{what}: no option \"{s}\"")))
        }
        _ => {
            let n = number(what, v)?;
            if n.fract() != 0.0 || n < 1.0 || n > options.len() as f64 {
                return Err(error(format!("{what}: no option {n}: there are {}", options.len())));
            }
            Ok(n as usize - 1)
        }
    }
}

/// Options by index or text: ascending, each once.
fn selection(what: &str, options: &[String], v: &Value) -> Result<Vec<u16>> {
    let Value::Table(t) = v else { return Err(error(format!("{what}: a list of options, not {}", type_of(v)))) };
    let mut out = Vec::new();
    for i in 1..=t.raw_len() {
        out.push(option(what, options, &t.raw_get::<Value>(i)?)? as u16);
    }
    out.sort_unstable();
    out.dedup();
    Ok(out)
}

/// A `Color`: `0xRRGGBBAA`, `{r, g, b, a}` (0-255, `a` 255 when left out)
/// or `"#rrggbb[aa]"`; `a` is 255 without `alpha`.
fn color(what: &str, v: &Value, alpha: bool) -> Result<Rgba> {
    let mut c = match v {
        Value::String(s) => {
            let s = s.to_string_lossy();
            validate::parse_hex(&s).ok_or_else(|| error(format!("{what}: \"{s}\" is not #rrggbb or #rrggbbaa")))?
        }
        Value::Table(t) => {
            let part = |k: &str| -> Result<u8> {
                let v: Value = t.raw_get(k)?;
                if k == "a" && v.is_nil() {
                    return Ok(255);
                }
                let n = number(&format!("{what}.{k}"), &v)?;
                if n.fract() != 0.0 || !(0.0..=255.0).contains(&n) {
                    return Err(error(format!("{what}.{k}: a whole number from 0 to 255, not {n}")));
                }
                Ok(n as u8)
            };
            Rgba { r: part("r")?, g: part("g")?, b: part("b")?, a: part("a")? }
        }
        // 0xRRGGBBAA, as draw.rgba makes it.
        Value::Integer(_) | Value::Number(_) => {
            let n = number(what, v)?;
            if n.fract() != 0.0 || !(0.0..=f64::from(u32::MAX)).contains(&n) {
                return Err(error(format!("{what}: 0xRRGGBBAA is a whole number from 0 to 0xFFFFFFFF, not {n}")));
            }
            let [r, g, b, a] = (n as u32).to_be_bytes();
            Rgba { r, g, b, a }
        }
        other => return Err(error(format!("{what}: a Color (0xRRGGBBAA, {{r, g, b, a}} or \"#rrggbb\"), not {}", type_of(other)))),
    };
    if !alpha {
        c.a = 255;
    }
    Ok(c)
}

fn color_table(lua: &Lua, c: Rgba) -> Result<Table> {
    let t = lua.create_table()?;
    t.raw_set("r", c.r)?;
    t.raw_set("g", c.g)?;
    t.raw_set("b", c.b)?;
    t.raw_set("a", c.a)?;
    Ok(t)
}

fn strings(lua: &Lua, items: impl IntoIterator<Item = String>) -> Result<Table> {
    lua.create_sequence_from(items)
}

/// What a label of an item may hold.
fn label_room(kind: &MenuKind) -> usize {
    match kind {
        MenuKind::Label => validate::LABEL_TEXT,
        MenuKind::Separator => validate::SEPARATOR,
        _ => validate::LABEL,
    }
}

/// A slider's range: finite, `min` below `max`, a step above 0.
fn check_range(what: &str, min: f64, max: f64, step: f64) -> Result<()> {
    if min >= max {
        return Err(error(format!("{what}: min must be below max")));
    }
    if step <= 0.0 {
        return Err(error(format!("{what}: step must be above 0")));
    }
    Ok(())
}

/// The options an item's builder was given: `note`, `enabled`, `visible`
/// and the kind's own (`keys`), from a table or a note string.
#[derive(Default)]
struct Opts {
    note: Option<String>,
    enabled: Option<bool>,
    visible: Option<bool>,
    table: Option<Table>,
}

impl Opts {
    /// `v` in the options' place: a string (the note), a table, a function
    /// (the callback, returned), nil.
    fn parse(what: &str, v: Value, keys: &[&str]) -> Result<(Opts, Option<Function>)> {
        match v {
            Value::Nil => Ok((Opts::default(), None)),
            Value::String(s) => Ok((Opts { note: Some(cut(&s.to_string_lossy(), validate::NOTE)), ..Opts::default() }, None)),
            Value::Function(f) => Ok((Opts::default(), Some(f))),
            Value::Table(t) => {
                for pair in t.pairs::<Value, Value>() {
                    let (k, _) = pair?;
                    let known = matches!(&k, Value::String(s) if {
                        let s = s.to_string_lossy();
                        ["note", "enabled", "visible"].contains(&s.as_str()) || keys.contains(&s.as_str())
                    });
                    if !known {
                        let k = match &k {
                            Value::String(s) => s.to_string_lossy(),
                            other => type_of(other).to_string(),
                        };
                        return Err(error(format!("{what}: no option '{k}'")));
                    }
                }
                let get = |k: &str| t.raw_get::<Value>(k);
                let flag = |k: &str| -> Result<Option<bool>> {
                    let v = get(k)?;
                    if v.is_nil() { Ok(None) } else { boolean(&format!("{what}: {k}"), &v).map(Some) }
                };
                let opts = Opts {
                    note: optional_string(&format!("{what}: note"), &get("note")?, validate::NOTE)?,
                    enabled: flag("enabled")?,
                    visible: flag("visible")?,
                    table: Some(t.clone()),
                };
                Ok((opts, None))
            }
            other => Err(error(format!("{what}: options are a table or a note, not {}", type_of(&other)))),
        }
    }

    fn get(&self, key: &str) -> Result<Value> {
        self.table.as_ref().map_or(Ok(Value::Nil), |t| t.raw_get(key))
    }

    /// A menu's `search` (false when left out).
    fn search(&self, what: &str) -> Result<bool> {
        match self.get("search")? {
            Value::Nil => Ok(false),
            v => boolean(&format!("{what}: search"), &v),
        }
    }
}

/// An item's handle.
struct ItemRef {
    id: u32,
    /// Its type's name ("Toggle"), also once the item is gone.
    type_name: &'static str,
    ctx: Rc<Ctx>,
}

impl ItemRef {
    fn removed(&self, what: &str) -> mlua::Error {
        error(format!("{}{what}: the item was removed", self.type_name))
    }

    fn get(&self, lua: &Lua, key: &str) -> Result<Value> {
        let local = self.ctx.local.borrow();
        let Some(li) = local.items.get(&self.id) else { return Err(self.removed(&format!(".{key}"))) };
        let item = &li.item;
        let cb = || li.callback.clone().map_or(Value::Nil, Value::Function);
        let text = |s: &str| lua.create_string(s).map(Value::String);
        let opt_text = |s: &Option<String>| s.as_deref().map_or(Ok(Value::Nil), text);
        let opt_number = |n: &Option<f64>| n.map_or(Value::Nil, Value::Number);
        Ok(match (&item.kind, key) {
            (_, "id") => Value::Integer(self.id.into()),
            (kind, "kind") => text(validate::kind_name(kind))?,
            (_, "label") => text(&item.label)?,
            (_, "note") => opt_text(&item.note)?,
            (_, "enabled") => Value::Boolean(item.enabled),
            (_, "visible") => Value::Boolean(item.visible),
            (MenuKind::Button { action }, "action") => text(action.as_deref().unwrap_or("Run"))?,
            (MenuKind::Button { .. }, "onPress") => cb(),
            (
                MenuKind::Toggle(_)
                | MenuKind::Slider { .. }
                | MenuKind::Number { .. }
                | MenuKind::Choice { .. }
                | MenuKind::Multi { .. }
                | MenuKind::Text { .. }
                | MenuKind::Color { .. },
                "onChange",
            ) => cb(),
            (MenuKind::Toggle(on), "value") => Value::Boolean(*on),
            (MenuKind::Slider { value, .. } | MenuKind::Number { value, .. }, "value") => Value::Number(*value),
            (MenuKind::Slider { min, .. }, "min") => Value::Number(*min),
            (MenuKind::Slider { max, .. }, "max") => Value::Number(*max),
            (MenuKind::Number { min, .. }, "min") => opt_number(min),
            (MenuKind::Number { max, .. }, "max") => opt_number(max),
            (MenuKind::Slider { step, .. } | MenuKind::Number { step, .. }, "step") => Value::Number(*step),
            (MenuKind::Slider { unit, .. } | MenuKind::Number { unit, .. }, "unit") => opt_text(unit)?,
            (MenuKind::Choice { index, .. }, "value") => Value::Integer(*index as i64 + 1),
            (MenuKind::Choice { options, index }, "option") => text(&options[*index])?,
            (MenuKind::Choice { options, .. } | MenuKind::Multi { options, .. }, "options") => {
                Value::Table(strings(lua, options.iter().cloned())?)
            }
            (MenuKind::Multi { selected, .. }, "value") => {
                Value::Table(lua.create_sequence_from(selected.iter().map(|i| i64::from(*i) + 1))?)
            }
            (MenuKind::Text { value, .. }, "value") => text(value)?,
            (MenuKind::Text { placeholder, .. }, "placeholder") => opt_text(placeholder)?,
            (MenuKind::Text { max, .. }, "max") => Value::Integer((*max).into()),
            (MenuKind::Color { value, .. }, "value") => Value::Table(color_table(lua, *value)?),
            (MenuKind::Color { alpha, .. }, "alpha") => Value::Boolean(*alpha),
            (MenuKind::Progress(p), "value") => Value::Number(f64::from(*p)),
            _ => return Err(error(format!("{} has no field '{key}'", self.type_name))),
        })
    }

    fn assign(&self, key: &str, v: Value) -> Result<()> {
        let what = format!("{}.{key}", self.type_name);
        let mut local = self.ctx.local.borrow_mut();
        // New options, within the mod's budget.
        let lists = matches!(self.type_name, "Choice" | "Multi");
        let new_options = if key == "options" && lists && local.items.contains_key(&self.id) {
            let o = options(&what, &v)?;
            local.room_for_options(&what, Some(self.id), o.len())?;
            Some(o)
        } else {
            None
        };
        let Some(li) = local.items.get_mut(&self.id) else { return Err(self.removed(&format!(".{key}"))) };
        match key {
            "id" | "kind" | "option" => return Err(error(format!("{what} is read-only"))),
            "label" => {
                let label = cut(&string(&what, &v)?, label_room(&li.item.kind));
                li.item.label = label.clone();
                super::edit_item(self.id, |i| i.label = label);
                return Ok(());
            }
            "note" => {
                let note = optional_string(&what, &v, validate::NOTE)?;
                li.item.note = note.clone();
                super::edit_item(self.id, |i| i.note = note);
                return Ok(());
            }
            "enabled" | "visible" => {
                let on = boolean(&what, &v)?;
                let enabled = key == "enabled";
                if enabled { li.item.enabled = on } else { li.item.visible = on }
                super::edit_item(self.id, |i| if enabled { i.enabled = on } else { i.visible = on });
                return Ok(());
            }
            _ => {}
        }
        let is_callback = match &li.item.kind {
            MenuKind::Button { .. } => key == "onPress",
            MenuKind::Label | MenuKind::Separator | MenuKind::Progress(_) | MenuKind::Submenu(_) => false,
            _ => key == "onChange",
        };
        if is_callback {
            li.callback = callback(&what, v)?;
            return Ok(());
        }
        let mut kind = li.item.kind.clone();
        match (&mut kind, key) {
            (MenuKind::Button { action }, "action") => *action = optional_string(&what, &v, validate::ACTION)?,
            (MenuKind::Toggle(on), "value") => *on = boolean(&what, &v)?,
            (MenuKind::Slider { value, min, max, step, .. }, "value" | "min" | "max" | "step") => {
                let n = number(&what, &v)?;
                match key {
                    "value" => *value = n,
                    "min" => *min = n,
                    "max" => *max = n,
                    _ => *step = n,
                }
                check_range(&what, *min, *max, *step)?;
                *value = validate::slider(*value, *min, *max, *step);
            }
            (MenuKind::Number { value, min, max, step, .. }, "value" | "min" | "max" | "step") => {
                match key {
                    "value" => *value = number(&what, &v)?,
                    "min" => *min = optional_number(&what, &v)?,
                    "max" => *max = optional_number(&what, &v)?,
                    _ => {
                        *step = number(&what, &v)?;
                        if *step <= 0.0 {
                            return Err(error(format!("{what}: step must be above 0")));
                        }
                    }
                }
                let (lo, hi) = validate::bounds(*min, *max);
                if lo > hi {
                    return Err(error(format!("{what}: min must not be above max")));
                }
                *value = value.clamp(lo, hi);
            }
            (MenuKind::Slider { unit, .. } | MenuKind::Number { unit, .. }, "unit") => {
                *unit = optional_string(&what, &v, validate::UNIT)?;
            }
            (MenuKind::Choice { options, index }, "value") => *index = option(&what, options, &v)?,
            (MenuKind::Choice { options, index }, "options") => {
                *options = new_options.clone().unwrap_or_default();
                if *index >= options.len() {
                    *index = 0;
                }
            }
            (MenuKind::Multi { options, selected }, "value") => *selected = selection(&what, options, &v)?,
            (MenuKind::Multi { options, selected }, "options") => {
                *options = new_options.clone().unwrap_or_default();
                selected.retain(|i| usize::from(*i) < options.len());
            }
            (MenuKind::Text { value, max, .. }, "value") => *value = validate::text(&string(&what, &v)?, *max),
            (MenuKind::Text { placeholder, .. }, "placeholder") => *placeholder = optional_string(&what, &v, validate::PLACEHOLDER)?,
            (MenuKind::Text { value, max, .. }, "max") => {
                *max = text_max(&what, &v)?;
                *value = validate::text(value, *max);
            }
            (MenuKind::Color { value, alpha }, "value") => *value = color(&what, &v, *alpha)?,
            (MenuKind::Color { value, alpha }, "alpha") => {
                *alpha = boolean(&what, &v)?;
                if !*alpha {
                    value.a = 255;
                }
            }
            (MenuKind::Progress(p), "value") => *p = number(&what, &v)?.clamp(0.0, 1.0) as f32,
            _ => return Err(error(format!("{} has no field '{key}'", self.type_name))),
        }
        li.item.kind = kind.clone();
        super::set_kind(self.id, kind);
        Ok(())
    }

    /// `:set(v)`: the value, or a button's or label's text.
    fn set(&self, v: Value) -> Result<()> {
        match self.type_name {
            "Button" | "Label" => self.assign("label", v),
            "Separator" => Err(error("Separator has no field 'set'".into())),
            _ => self.assign("value", v),
        }
    }

    fn remove(&self) -> Result<()> {
        let mut local = self.ctx.local.borrow_mut();
        let Some(li) = local.items.remove(&self.id) else { return Err(self.removed(":remove")) };
        if let Some(m) = local.menus.get_mut(&li.menu) {
            m.items.retain(|i| *i != self.id);
        }
        super::remove(&[self.id], &[]);
        Ok(())
    }
}

fn text_max(what: &str, v: &Value) -> Result<u16> {
    let n = number(what, v)?;
    if n.fract() != 0.0 || n < 1.0 || n > f64::from(validate::TEXT_MAX) {
        return Err(error(format!("{what}: max is a whole number from 1 to {}", validate::TEXT_MAX)));
    }
    Ok(n as u16)
}

impl UserData for ItemRef {
    fn add_methods<M: UserDataMethods<Self>>(methods: &mut M) {
        methods.add_method("set", |_, this, v: Value| this.set(v));
        methods.add_method("remove", |_, this, ()| this.remove());
        methods.add_meta_method(MetaMethod::Index, |lua, this, key: String| this.get(lua, &key));
        methods.add_meta_method(MetaMethod::NewIndex, |_, this, (key, v): (String, Value)| this.assign(&key, v));
    }
}

/// A menu's handle.
struct MenuRef {
    id: u32,
    ctx: Rc<Ctx>,
}

impl MenuRef {
    fn removed(&self, what: &str) -> mlua::Error {
        error(format!("{what}: the menu was removed"))
    }

    fn get(&self, lua: &Lua, key: &str) -> Result<Value> {
        let local = self.ctx.local.borrow();
        let Some(m) = local.menus.get(&self.id) else { return Err(self.removed(&format!("Menu.{key}"))) };
        Ok(match key {
            "id" => Value::Integer(self.id.into()),
            "title" => Value::String(lua.create_string(&m.title)?),
            "note" => m.note.as_deref().map_or(Ok(Value::Nil), |n| lua.create_string(n).map(Value::String))?,
            "search" => Value::Boolean(m.search),
            "enabled" | "visible" => {
                // A submenu's are its row's.
                let (enabled, visible) = match m.row.and_then(|r| local.items.get(&r)) {
                    Some(row) => (row.item.enabled, row.item.visible),
                    None => (m.enabled, m.visible),
                };
                Value::Boolean(if key == "enabled" { enabled } else { visible })
            }
            _ => return Err(error(format!("Menu has no field '{key}'"))),
        })
    }

    fn assign(&self, key: &str, v: Value) -> Result<()> {
        let what = format!("Menu.{key}");
        let mut local = self.ctx.local.borrow_mut();
        let local = &mut *local;
        let Some(m) = local.menus.get_mut(&self.id) else { return Err(self.removed(&what)) };
        // A submenu's row says its title and note too, and is what its
        // enabled and visible change.
        let row = m.row.and_then(|r| local.items.get_mut(&r));
        match key {
            "id" => return Err(error(format!("{what} is read-only"))),
            "title" => {
                let title = cut(&string(&what, &v)?, validate::TITLE);
                m.title = title.clone();
                if let Some(row) = row {
                    row.item.label = title.clone();
                    let title = title.clone();
                    super::edit_item(row.item.id, |i| i.label = title);
                }
                super::edit_menu(self.id, super::MenuField::Title(title));
            }
            "note" => {
                let note = optional_string(&what, &v, validate::NOTE)?;
                m.note = note.clone();
                if let Some(row) = row {
                    row.item.note = note.clone();
                    let note = note.clone();
                    super::edit_item(row.item.id, |i| i.note = note);
                }
                super::edit_menu(self.id, super::MenuField::Note(note));
            }
            "search" => {
                let on = boolean(&what, &v)?;
                m.search = on;
                super::edit_menu(self.id, super::MenuField::Search(on));
            }
            "enabled" | "visible" => {
                let on = boolean(&what, &v)?;
                let enabled = key == "enabled";
                match row {
                    Some(row) => {
                        if enabled { row.item.enabled = on } else { row.item.visible = on }
                        super::edit_item(row.item.id, |i| if enabled { i.enabled = on } else { i.visible = on });
                    }
                    None if enabled => m.enabled = on,
                    None => m.visible = on,
                }
            }
            _ => return Err(error(format!("Menu has no field '{key}'"))),
        }
        Ok(())
    }

    /// Adds an item; `opts` already parsed.
    fn add(&self, lua: &Lua, what: &str, label: String, opts: Opts, kind: MenuKind, cb: Option<Function>) -> Result<AnyUserData> {
        let mut local = self.ctx.local.borrow_mut();
        local.room(what, 1)?;
        if let MenuKind::Choice { options, .. } | MenuKind::Multi { options, .. } = &kind {
            local.room_for_options(what, None, options.len())?;
        }
        let Some(menu) = local.menus.get_mut(&self.id) else { return Err(self.removed(what)) };
        let id = super::next_id();
        let item = MenuItem {
            id,
            label: cut(&label, label_room(&kind)),
            note: opts.note,
            enabled: opts.enabled.unwrap_or(true),
            visible: opts.visible.unwrap_or(true),
            kind,
        };
        let type_name = type_name(&item.kind);
        if !super::add_item(self.id, item.clone()) {
            return Err(self.removed(what));
        }
        menu.items.push(id);
        local.items.insert(id, LocalItem { menu: self.id, item, callback: cb });
        lua.create_userdata(ItemRef { id, type_name, ctx: self.ctx.clone() })
    }

    /// `menu:onOpen(f)` / `menu:onClose(f)`; nil clears.
    fn on_view(&self, open: bool, f: Value) -> Result<()> {
        let what = if open { "menu:onOpen" } else { "menu:onClose" };
        let f = callback(what, f)?;
        let mut local = self.ctx.local.borrow_mut();
        let Some(m) = local.menus.get_mut(&self.id) else { return Err(self.removed(what)) };
        if open { m.on_open = f } else { m.on_close = f }
        Ok(())
    }

    /// `menu:submenu(label, opts)`: a menu, and its row here.
    fn submenu(&self, lua: &Lua, label: Value, opts: Value) -> Result<AnyUserData> {
        let what = "menu:submenu";
        let title = cut(&string(&format!("{what}: the label"), &label)?, validate::TITLE);
        let (opts, _) = match opts {
            Value::Function(_) => return Err(error(format!("{what}: options are a table or a note, not function"))),
            o => Opts::parse(what, o, &["search"])?,
        };
        let search = opts.search(what)?;
        let mut local = self.ctx.local.borrow_mut();
        if !local.menus.contains_key(&self.id) {
            return Err(self.removed(what));
        }
        // The menu and its row.
        local.room(what, 2)?;
        let (id, row) = (super::next_id(), super::next_id());
        // The menu before the row that opens it, for readers in between.
        super::add_menu(Menu {
            module: self.ctx.module.clone(),
            instance: self.ctx.instance,
            id,
            parent: Some(self.id),
            title: title.clone(),
            note: opts.note.clone(),
            search,
            items: Vec::new(),
            rev: 0,
        });
        let item = MenuItem {
            id: row,
            label: title.clone(),
            note: opts.note.clone(),
            enabled: opts.enabled.unwrap_or(true),
            visible: opts.visible.unwrap_or(true),
            kind: MenuKind::Submenu(id),
        };
        if !super::add_item(self.id, item.clone()) {
            return Err(self.removed(what));
        }
        local.menus.get_mut(&self.id).expect("checked above").items.push(row);
        local.items.insert(row, LocalItem { menu: self.id, item, callback: None });
        local.menus.insert(
            id,
            LocalMenu { parent: Some(self.id), row: Some(row), title, note: opts.note, search, enabled: true, visible: true, items: Vec::new(), on_open: None, on_close: None },
        );
        lua.create_userdata(MenuRef { id, ctx: self.ctx.clone() })
    }

    /// The items of this menu go, with the submenus they open, here and
    /// for players.
    fn clear(&self) -> Result<()> {
        let mut local = self.ctx.local.borrow_mut();
        let Some(m) = local.menus.get_mut(&self.id) else { return Err(self.removed("menu:clear")) };
        let rows = std::mem::take(&mut m.items);
        let (mut items, mut menus) = (Vec::new(), Vec::new());
        for row in rows {
            under(&local, row, &mut items, &mut menus);
        }
        forget(&mut local, &items, &menus);
        super::remove(&items, &menus);
        Ok(())
    }

    /// The menu goes, with its submenus and its row in its parent.
    fn remove(&self) -> Result<()> {
        let mut local = self.ctx.local.borrow_mut();
        let Some(m) = local.menus.get(&self.id) else { return Err(self.removed("menu:remove")) };
        let (parent, row) = (m.parent, m.row);
        let (mut items, mut menus) = (Vec::new(), Vec::new());
        match row {
            Some(row) => under(&local, row, &mut items, &mut menus),
            None => subtree(&local, self.id, &mut items, &mut menus),
        }
        if let (Some(parent), Some(row)) = (parent, row)
            && let Some(p) = local.menus.get_mut(&parent)
        {
            p.items.retain(|i| *i != row);
        }
        forget(&mut local, &items, &menus);
        super::remove(&items, &menus);
        Ok(())
    }
}

/// Item `item` and, for a submenu's row, what is under it.
fn under(local: &Local, item: u32, items: &mut Vec<u32>, menus: &mut Vec<u32>) {
    items.push(item);
    if let Some(MenuKind::Submenu(m)) = local.items.get(&item).map(|i| &i.item.kind) {
        subtree(local, *m, items, menus);
    }
}

/// Menu `menu`, its items and the submenus they open, at any depth (a
/// worklist, not recursion).
fn subtree(local: &Local, menu: u32, items: &mut Vec<u32>, menus: &mut Vec<u32>) {
    let mut todo = vec![menu];
    while let Some(menu) = todo.pop() {
        // Each menu once, whatever its rows say.
        if menus.contains(&menu) {
            continue;
        }
        menus.push(menu);
        for &i in local.menus.get(&menu).map(|m| m.items.as_slice()).unwrap_or_default() {
            items.push(i);
            if let Some(MenuKind::Submenu(m)) = local.items.get(&i).map(|i| &i.item.kind) {
                todo.push(*m);
            }
        }
    }
}

fn forget(local: &mut Local, items: &[u32], menus: &[u32]) {
    for i in items {
        local.items.remove(i);
    }
    for m in menus {
        local.menus.remove(m);
    }
}

/// The builders' arguments, one after the other.
struct Args {
    what: &'static str,
    args: std::vec::IntoIter<Value>,
}

impl Args {
    fn next(&mut self) -> Value {
        self.args.next().unwrap_or(Value::Nil)
    }

    fn label(&mut self) -> Result<String> {
        string(&format!("{}: the label", self.what), &self.next())
    }

    /// The options (with the kind's `keys`), then the callback, which may
    /// stand in the options' place.
    fn opts(&mut self, keys: &[&str]) -> Result<(Opts, Option<Function>)> {
        let (opts, cb) = Opts::parse(self.what, self.next(), keys)?;
        let after = callback(&format!("{}: the callback", self.what), self.next())?;
        Ok((opts, cb.or(after)))
    }
}

impl UserData for MenuRef {
    fn add_methods<M: UserDataMethods<Self>>(methods: &mut M) {
        fn builder<M: UserDataMethods<MenuRef>>(
            methods: &mut M,
            name: &'static str,
            f: fn(&Lua, &MenuRef, &mut Args) -> Result<AnyUserData>,
        ) {
            methods.add_method(name, move |lua, this, args: MultiValue| {
                let what = match name {
                    "button" => "menu:button",
                    "toggle" => "menu:toggle",
                    "slider" => "menu:slider",
                    "number" => "menu:number",
                    "choice" => "menu:choice",
                    "multi" => "menu:multi",
                    "text" => "menu:text",
                    "color" => "menu:color",
                    "label" => "menu:label",
                    "separator" => "menu:separator",
                    _ => "menu:progress",
                };
                f(lua, this, &mut Args { what, args: args.into_vec().into_iter() })
            });
        }
        builder(methods, "button", |lua, m, a| {
            let label = a.label()?;
            let (opts, cb) = a.opts(&["action"])?;
            let action = optional_string(&format!("{}: action", a.what), &opts.get("action")?, validate::ACTION)?;
            m.add(lua, a.what, label, opts, MenuKind::Button { action }, cb)
        });
        builder(methods, "toggle", |lua, m, a| {
            let label = a.label()?;
            let v = a.next();
            let on = if v.is_nil() { false } else { boolean(&format!("{}: the value", a.what), &v)? };
            let (opts, cb) = a.opts(&[])?;
            m.add(lua, a.what, label, opts, MenuKind::Toggle(on), cb)
        });
        builder(methods, "slider", |lua, m, a| {
            let what = a.what;
            let label = a.label()?;
            let value = number(&format!("{what}: the value"), &a.next())?;
            let min = number(&format!("{what}: min"), &a.next())?;
            let max = number(&format!("{what}: max"), &a.next())?;
            if min >= max {
                return Err(error(format!("{what}: min must be below max")));
            }
            // A number in the options' place is the step.
            let o = a.next();
            let (opts, cb, step) = match o {
                Value::Integer(_) | Value::Number(_) => {
                    let step = number(&format!("{what}: step"), &o)?;
                    (Opts::default(), callback(&format!("{what}: the callback"), a.next())?, Some(step))
                }
                o => {
                    let (opts, cb) = Opts::parse(what, o, &["step", "unit"])?;
                    let cb = cb.or(callback(&format!("{what}: the callback"), a.next())?);
                    let step = optional_number(&format!("{what}: step"), &opts.get("step")?)?;
                    (opts, cb, step)
                }
            };
            let step = step.unwrap_or((max - min) / 100.0);
            check_range(what, min, max, step)?;
            let unit = optional_string(&format!("{what}: unit"), &opts.get("unit")?, validate::UNIT)?;
            let value = validate::slider(value, min, max, step);
            m.add(lua, what, label, opts, MenuKind::Slider { value, min, max, step, unit }, cb)
        });
        builder(methods, "number", |lua, m, a| {
            let what = a.what;
            let label = a.label()?;
            let value = number(&format!("{what}: the value"), &a.next())?;
            let (opts, cb) = a.opts(&["min", "max", "step", "unit"])?;
            let min = optional_number(&format!("{what}: min"), &opts.get("min")?)?;
            let max = optional_number(&format!("{what}: max"), &opts.get("max")?)?;
            let step = optional_number(&format!("{what}: step"), &opts.get("step")?)?.unwrap_or(1.0);
            if step <= 0.0 {
                return Err(error(format!("{what}: step must be above 0")));
            }
            let (lo, hi) = validate::bounds(min, max);
            if lo > hi {
                return Err(error(format!("{what}: min must not be above max")));
            }
            let unit = optional_string(&format!("{what}: unit"), &opts.get("unit")?, validate::UNIT)?;
            let value = value.clamp(lo, hi);
            m.add(lua, what, label, opts, MenuKind::Number { value, min, max, step, unit }, cb)
        });
        builder(methods, "choice", |lua, m, a| {
            let what = a.what;
            let label = a.label()?;
            let options = options(&format!("{what}: options"), &a.next())?;
            let v = a.next();
            let index = if v.is_nil() { 0 } else { option(what, &options, &v)? };
            let (opts, cb) = a.opts(&[])?;
            m.add(lua, what, label, opts, MenuKind::Choice { options, index }, cb)
        });
        builder(methods, "multi", |lua, m, a| {
            let what = a.what;
            let label = a.label()?;
            let options = options(&format!("{what}: options"), &a.next())?;
            let v = a.next();
            let selected = if v.is_nil() { Vec::new() } else { selection(what, &options, &v)? };
            let (opts, cb) = a.opts(&[])?;
            m.add(lua, what, label, opts, MenuKind::Multi { options, selected }, cb)
        });
        builder(methods, "text", |lua, m, a| {
            let what = a.what;
            let label = a.label()?;
            let v = a.next();
            let value = if v.is_nil() { String::new() } else { string(&format!("{what}: the value"), &v)? };
            let (opts, cb) = a.opts(&["placeholder", "max"])?;
            let max = opts.get("max")?;
            let max = if max.is_nil() { validate::TEXT_DEFAULT } else { text_max(&format!("{what}: max"), &max)? };
            let placeholder = optional_string(&format!("{what}: placeholder"), &opts.get("placeholder")?, validate::PLACEHOLDER)?;
            m.add(lua, what, label, opts, MenuKind::Text { value: validate::text(&value, max), placeholder, max }, cb)
        });
        builder(methods, "color", |lua, m, a| {
            let what = a.what;
            let label = a.label()?;
            let v = a.next();
            let (opts, cb) = a.opts(&["alpha"])?;
            let alpha = opts.get("alpha")?;
            let alpha = !alpha.is_nil() && boolean(&format!("{what}: alpha"), &alpha)?;
            let white = Rgba { r: 255, g: 255, b: 255, a: 255 };
            let value = if v.is_nil() { white } else { color(&format!("{what}: the value"), &v, alpha)? };
            m.add(lua, what, label, opts, MenuKind::Color { value, alpha }, cb)
        });
        builder(methods, "label", |lua, m, a| {
            let text = string(&format!("{}: the text", a.what), &a.next())?;
            let (opts, _) = Opts::parse(a.what, a.next(), &[])?;
            m.add(lua, a.what, text, opts, MenuKind::Label, None)
        });
        builder(methods, "separator", |lua, m, a| {
            let v = a.next();
            let title = if v.is_nil() { String::new() } else { string(&format!("{}: the title", a.what), &v)? };
            m.add(lua, a.what, title, Opts::default(), MenuKind::Separator, None)
        });
        builder(methods, "progress", |lua, m, a| {
            let label = a.label()?;
            let p = number(&format!("{}: the value", a.what), &a.next())?.clamp(0.0, 1.0) as f32;
            let (opts, _) = Opts::parse(a.what, a.next(), &[])?;
            m.add(lua, a.what, label, opts, MenuKind::Progress(p), None)
        });
        methods.add_method("submenu", |lua, this, (label, opts): (Value, Value)| this.submenu(lua, label, opts));
        methods.add_method("clear", |_, this, ()| this.clear());
        methods.add_method("onOpen", |_, this, f: Value| this.on_view(true, f));
        methods.add_method("onClose", |_, this, f: Value| this.on_view(false, f));
        methods.add_method("show", |_, this, ()| {
            if !this.ctx.local.borrow().menus.contains_key(&this.id) {
                return Err(this.removed("menu:show"));
            }
            // More than once in two seconds: ignored.
            super::request_show(&this.ctx.module, this.id);
            Ok(())
        });
        methods.add_method("remove", |_, this, ()| this.remove());
        methods.add_meta_method(MetaMethod::Index, |lua, this, key: String| this.get(lua, &key));
        methods.add_meta_method(MetaMethod::NewIndex, |_, this, (key, v): (String, Value)| this.assign(&key, v));
    }
}

/// The `ui` table: `ui.menu(title, [opts])`, `ui.notify(text)`.
pub fn install(lua: &Lua, module: &str, instance: u64, handlers: Handlers) -> Result<()> {
    let ui = lua.create_table()?;
    let m = module.to_string();
    ui.set(
        "notify",
        lua.create_function(move |_, text: String| {
            let text: String = text.chars().take(200).collect();
            let mut s = crate::lock(&SHARED.state);
            if s.notices.len() < 16 {
                s.notices.push_back((m.clone(), text));
            }
            Ok(())
        })?,
    )?;
    let ctx = Rc::new(Ctx { module: module.to_string(), instance, local: handlers });
    ui.set(
        "menu",
        lua.create_function(move |lua, (title, opts): (Value, Value)| {
            let what = "ui.menu";
            let title = cut(&string(&format!("{what}: the title"), &title)?, validate::TITLE);
            let (opts, _) = match opts {
                Value::Function(_) => return Err(error(format!("{what}: options are a table or a note, not function"))),
                o => Opts::parse(what, o, &["search"])?,
            };
            let search = opts.search(what)?;
            let mut local = ctx.local.borrow_mut();
            local.room(what, 1)?;
            let id = super::next_id();
            super::add_menu(Menu {
                module: ctx.module.clone(),
                instance: ctx.instance,
                id,
                parent: None,
                title: title.clone(),
                note: opts.note.clone(),
                search,
                items: Vec::new(),
                rev: 0,
            });
            local.menus.insert(
                id,
                LocalMenu {
                    parent: None,
                    row: None,
                    title,
                    note: opts.note,
                    search,
                    enabled: opts.enabled.unwrap_or(true),
                    visible: opts.visible.unwrap_or(true),
                    items: Vec::new(),
                    on_open: None,
                    on_close: None,
                },
            );
            lua.create_userdata(MenuRef { id, ctx: ctx.clone() })
        })?,
    )?;
    lua.globals().set("ui", ui)
}

impl Local {
    /// Whether `n` more menus or items fit the mod's budget.
    fn room(&self, what: &str, n: usize) -> Result<()> {
        if self.menus.len() + self.items.len() + n > validate::ENTRIES {
            return Err(error(format!("{what}: a mod's menus hold {} menus and items at most, all together", validate::ENTRIES)));
        }
        Ok(())
    }

    /// Whether a list of `n` options fits the mod's budget (in place of
    /// item `replacing`'s).
    fn room_for_options(&self, what: &str, replacing: Option<u32>, n: usize) -> Result<()> {
        let used: usize = self
            .items
            .values()
            .filter(|i| Some(i.item.id) != replacing)
            .map(|i| match &i.item.kind {
                MenuKind::Choice { options, .. } | MenuKind::Multi { options, .. } => options.len(),
                _ => 0,
            })
            .sum();
        if used + n > validate::OPTIONS {
            return Err(error(format!("{what}: a mod's choices and multis hold {} options at most, all together", validate::OPTIONS)));
        }
        Ok(())
    }
}

/// Applies a player's input to the mod's copy of the item; what its
/// callback gets.
fn apply(lua: &Lua, kind: &mut MenuKind, input: MenuInput) -> Result<MultiValue> {
    let one = |v: Value| MultiValue::from_vec(vec![v]);
    Ok(match (kind, input) {
        (MenuKind::Button { .. }, MenuInput::Press) => MultiValue::new(),
        (MenuKind::Toggle(on), MenuInput::Toggle(v)) => {
            *on = v;
            one(Value::Boolean(v))
        }
        (MenuKind::Slider { value, .. } | MenuKind::Number { value, .. }, MenuInput::Number(v)) => {
            *value = v;
            one(Value::Number(v))
        }
        (MenuKind::Choice { options, index }, MenuInput::Choose(i)) if i < options.len() => {
            *index = i;
            MultiValue::from_vec(vec![Value::Integer(i as i64 + 1), Value::String(lua.create_string(&options[i])?)])
        }
        (MenuKind::Multi { options, selected }, MenuInput::Select(v)) => {
            *selected = v.into_iter().filter(|i| usize::from(*i) < options.len()).collect();
            let indices = lua.create_sequence_from(selected.iter().map(|i| i64::from(*i) + 1))?;
            let texts = strings(lua, selected.iter().map(|i| options[usize::from(*i)].clone()))?;
            MultiValue::from_vec(vec![Value::Table(indices), Value::Table(texts)])
        }
        (MenuKind::Text { value, .. }, MenuInput::Text(t)) => {
            *value = t;
            one(Value::String(lua.create_string(value.as_str())?))
        }
        (MenuKind::Color { value, .. }, MenuInput::Color(c)) => {
            *value = c;
            one(Value::Table(color_table(lua, c)?))
        }
        // Checked when it came: an item the mod changed since.
        _ => return Err(error("a menu input that no longer fits its item".into())),
    })
}

/// What waits for this run of the mod, as callbacks to run as tasks: the
/// items take the players' values first, so `value` and the callbacks
/// agree.
pub fn take_calls(lua: &Lua, handlers: &Handlers, instance: u64) -> Result<Vec<(Function, MultiValue)>> {
    let mut calls = Vec::new();
    for event in super::take(instance) {
        let mut local = handlers.borrow_mut();
        match event {
            Event::Input(item, input) => {
                let Some(li) = local.items.get_mut(&item) else { continue };
                let args = apply(lua, &mut li.item.kind, input)?;
                if let Some(f) = li.callback.clone() {
                    calls.push((f, args));
                }
            }
            Event::Viewer(menu, open) => {
                let f = local.menus.get(&menu).and_then(|m| if open { m.on_open.clone() } else { m.on_close.clone() });
                if let Some(f) = f {
                    calls.push((f, MultiValue::new()));
                }
            }
        }
    }
    Ok(calls)
}
