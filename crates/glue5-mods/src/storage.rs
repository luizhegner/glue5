//! `storage`: what a mod keeps between sessions (settings, progress), by
//! key, as JSON. Each mod's is a file of its own in the mods folder's
//! `.glue5/storage`, not in the mod's folder: a mod given to someone else
//! does not take the player's data along. Values are copied in and out
//! (a table set and changed afterwards is not changed in the storage).
//! Written within a couple of seconds of a change, and when the mod stops.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::path::PathBuf;
use std::rc::Rc;
use std::time::{Duration, Instant};

use mlua::{Lua, Value};
use serde_json::{Map, Value as Json};

use crate::json::{self, from_json};

/// What a mod may keep, as JSON text.
const LIMIT: usize = 1 << 20;
/// Changes are written at most this often.
const EVERY: Duration = Duration::from_secs(2);

pub struct Storage {
    module: String,
    path: Option<PathBuf>,
    data: RefCell<Map<String, Json>>,
    /// Each key's size as JSON, and their sum (the limit, without writing
    /// it all out at each change).
    sizes: RefCell<HashMap<String, usize>>,
    total: Cell<usize>,
    dirty: Cell<bool>,
    written: Cell<Instant>,
}

impl Storage {
    /// Mod `module`'s storage, as last written.
    pub fn open(module: &str) -> Rc<Storage> {
        let path = crate::dir().map(|d| d.join(".glue5/storage").join(format!("{module}.json")));
        let mut data = Map::new();
        if let Some(p) = &path
            && let Ok(text) = std::fs::read_to_string(p)
        {
            match serde_json::from_str::<Map<String, Json>>(&text) {
                Ok(d) => data = d,
                Err(e) => {
                    // Kept aside, not lost.
                    let _ = std::fs::rename(p, p.with_extension("json.bad"));
                    crate::log_line(module, crate::LogLevel::Error, &format!("its storage could not be read ({e}): it starts empty"));
                }
            }
        }
        let sizes: HashMap<String, usize> = data.iter().map(|(k, v)| (k.clone(), size(k, v))).collect();
        let total = sizes.values().sum();
        Rc::new(Storage {
            module: module.to_string(),
            path,
            data: RefCell::new(data),
            sizes: RefCell::new(sizes),
            total: Cell::new(total),
            dirty: Cell::new(false),
            written: Cell::new(Instant::now()),
        })
    }

    /// Writes the changes, when there are some and the last write is a
    /// while ago (`now`: whatever the time).
    pub fn flush(&self, now: bool) {
        if !self.dirty.get() || (!now && self.written.get().elapsed() < EVERY) {
            return;
        }
        let Some(path) = &self.path else { return };
        self.dirty.set(false);
        self.written.set(Instant::now());
        let text = serde_json::to_string(&*self.data.borrow()).unwrap_or_default();
        let tmp = path.with_extension("json.tmp");
        let written = path
            .parent()
            .map_or(Ok(()), std::fs::create_dir_all)
            .and_then(|()| std::fs::write(&tmp, text))
            .and_then(|()| std::fs::rename(&tmp, path));
        if let Err(e) = written {
            crate::log_line(&self.module, crate::LogLevel::Error, &format!("its storage could not be written: {e}"));
        }
    }

    /// The `storage` table.
    pub fn install(self: &Rc<Self>, lua: &Lua) -> mlua::Result<()> {
        let t = lua.create_table()?;
        let s = self.clone();
        t.set(
            "get",
            lua.create_function(move |lua, key: String| match s.data.borrow().get(&key) {
                Some(v) => from_json(lua, v),
                None => Ok(Value::Nil),
            })?,
        )?;
        let s = self.clone();
        t.set(
            "set",
            lua.create_function(move |_, (key, value): (String, Value)| {
                let old = s.sizes.borrow().get(&key).copied().unwrap_or(0);
                if value.is_nil() {
                    s.data.borrow_mut().remove(&key);
                    s.sizes.borrow_mut().remove(&key);
                    s.total.set(s.total.get() - old);
                } else {
                    // Encoded only as far as there is room.
                    let room = (LIMIT + old).saturating_sub(s.total.get() + key.len() + 4);
                    let text = json::encode(&value, false, room).map_err(|e| match e {
                        json::Error::TooLong(_) => mlua::Error::runtime("storage.set: a mod keeps 1 MB at most"),
                        e => mlua::Error::runtime(format!("storage.set: {e}")),
                    })?;
                    let v: Json = serde_json::from_str(&text).map_err(|e| mlua::Error::runtime(format!("storage.set: {e}")))?;
                    let new = key.len() + text.len() + 4;
                    s.data.borrow_mut().insert(key.clone(), v);
                    s.sizes.borrow_mut().insert(key, new);
                    s.total.set(s.total.get() - old + new);
                }
                s.dirty.set(true);
                Ok(())
            })?,
        )?;
        let s = self.clone();
        t.set("keys", lua.create_function(move |_, ()| Ok(s.data.borrow().keys().cloned().collect::<Vec<_>>()))?)?;
        let s = self.clone();
        t.set(
            "clear",
            lua.create_function(move |_, ()| {
                s.data.borrow_mut().clear();
                s.sizes.borrow_mut().clear();
                s.total.set(0);
                s.dirty.set(true);
                Ok(())
            })?,
        )?;
        lua.globals().set("storage", t)
    }
}

impl Storage {
    /// Empties it, on disk too.
    pub fn clear_now(&self) {
        self.data.borrow_mut().clear();
        self.sizes.borrow_mut().clear();
        self.total.set(0);
        self.dirty.set(true);
        self.flush(true);
    }
}

/// A key and its value's size as JSON text.
fn size(key: &str, value: &Json) -> usize {
    key.len() + serde_json::to_string(value).map_or(0, |t| t.len()) + 4
}
