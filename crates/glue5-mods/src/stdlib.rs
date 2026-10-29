//! The mods' standard library: what SHVDN's classes give C# mods, in Luau
//! over the natives (stdlib/*.luau), run in every mod's VM before it is
//! sandboxed (which makes its tables read-only); and the named constants
//! (`Control.JUMP`), which the build script reads from game.wasm and the
//! natives database (build/constants.rs).
//!
//! Its functions join the tables they belong to (`world.raycast`,
//! `streaming.load`) or make tables of their own (`spawn`, `geo`). Those
//! that wait for the game (loading, a raycast) yield, as `wait` does: from
//! the mod's code and its tasks.

use mlua::Lua;

mod generated {
    include!(concat!(env!("OUT_DIR"), "/constants.rs"));
}

/// The library's files, run in this order.
const SOURCES: &[(&str, &str)] = &[
    ("constants", include_str!("stdlib/constants.luau")),
    ("geo", include_str!("stdlib/geo.luau")),
    ("task", include_str!("stdlib/task.luau")),
    ("streaming", include_str!("stdlib/streaming.luau")),
    ("world", include_str!("stdlib/world.luau")),
    ("scaleform", include_str!("stdlib/scaleform.luau")),
    ("ui", include_str!("stdlib/ui.luau")),
];

/// The library's types, for the checker: after every other definition (it
/// adds to tables declared there).
pub const DEFINITIONS: &str =
    concat!(include_str!(concat!(env!("OUT_DIR"), "/constants.d.luau")), include_str!("stdlib/stdlib.d.luau"));

/// Adds the library to a mod's VM (its natives and globals installed, not
/// sandboxed yet: the sandbox makes the tables read-only).
pub fn install(lua: &Lua) -> mlua::Result<()> {
    let globals = lua.globals();
    for (name, entries) in generated::CONSTANTS {
        let table = lua.create_table_with_capacity(0, entries.len())?;
        for &(key, value) in *entries {
            table.raw_set(key, value)?;
        }
        globals.raw_set(*name, table)?;
    }
    for (name, source) in SOURCES {
        lua.load(*source).set_name(format!("=std/{name}")).exec()?;
    }
    Ok(())
}

#[cfg(test)]
mod tests;
