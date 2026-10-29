//! The game world for mods, read from the engine's own tables:
//!
//! - the entities in the ped, vehicle, object and pickup pools and the
//!   projectiles in flight, as script handles (`world.vehicles()`...);
//! - how much room the pools have left (natives that create entities are
//!   refused before the engine would fail);
//! - the catalog of the installed game's names: vehicle and ped models,
//!   weapons. The engine keeps the names it hashed
//!   (`atHashStringNamespaceSupport`), so they come from the assets the
//!   game loaded, whatever their version: types for the checker
//!   (`world.d.luau`), lists for mods.

use std::sync::Mutex;

use glue5_game::{Env, orig};
use glue5_runtime::memory::Mem;

/// fwBasePool* of the peds, of the objects and of the pickups (objects
/// too: `CPickupManager::CreatePickup`, `RemoveAllPickups`).
const PED_POOL: u64 = 20159656;
const OBJECT_POOL: u64 = 19670000;
const PICKUP_POOL: u64 = 20306344;
/// The projectiles in flight (`CProjectileManager`): an atFixedArray of 50
/// pointers, its i32 count right after them.
const PROJECTILES: u64 = 28497840;
const MAX_PROJECTILES: u64 = 50;
/// The vehicles' pool allocator: u64 at u64 here; base +0, size +8, free
/// +16, element size +24, used bits +56 (a pointer per slot).
const VEHICLE_POOL: u64 = 26005240;
/// The script handles' pool (fwBasePool*).
const GUID_POOL: u64 = 12318120;
/// The archetype store (atPoolBase): storage +0, size +8, element size
/// +24, used bits +56; archetype: name hash +24, model type +157 (& 31:
/// 5 vehicle, 6 ped).
const ARCHETYPES: u64 = 10872112;
/// CWeaponInfoManager's sorted item infos: u64 array, u16 count at +8;
/// an info's name hash at +16.
const ITEM_INFOS: u64 = 28496576;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Pool {
    Peds,
    Vehicles,
    Objects,
    Pickups,
    Projectiles,
}

/// The entities alive in an fwBasePool (slot flags: bit 7 = free).
fn base_pool(mem: Mem, at: u64) -> Vec<u64> {
    let pool = mem.u64(at);
    if pool == 0 {
        return Vec::new();
    }
    let (storage, flags) = (mem.u64(pool + 16), mem.u64(pool + 24));
    let (size, item) = (mem.i32(pool + 32).max(0) as u64, mem.i32(pool + 36).max(0) as u64);
    (0..size).filter(|&i| (mem.u8(flags + i) as i8) >= 0).map(|i| storage + item * i).collect()
}

/// How many more an fwBasePool takes.
fn base_free(mem: Mem, at: u64) -> u64 {
    let pool = mem.u64(at);
    if pool == 0 {
        return 0;
    }
    let size = mem.i32(pool + 32).max(0) as u64;
    let used = (((mem.u32(pool + 48) << 2) as i32) >> 2).max(0) as u64;
    size.saturating_sub(used)
}

fn vehicle_allocator(mem: Mem) -> u64 {
    match mem.u64(VEHICLE_POOL) {
        0 => 0,
        p => mem.u64(p),
    }
}

/// The vehicles (the allocator's used slots hold pointers to them).
fn vehicles(mem: Mem) -> Vec<u64> {
    let p = vehicle_allocator(mem);
    if p == 0 {
        return Vec::new();
    }
    let (base, size, elem, bits) = (mem.u64(p), mem.u64(p + 8), mem.u64(p + 24), mem.u64(p + 56));
    (0..size)
        .filter(|&i| bits != 0 && (mem.u32(bits + 4 * (i >> 5)) >> (i & 31)) & 1 == 1)
        .map(|i| mem.u64(base + elem * i))
        .filter(|&v| v != 0)
        .collect()
}

/// The projectiles in flight.
fn projectiles(mem: Mem) -> Vec<u64> {
    let count = (mem.i32(PROJECTILES + 8 * MAX_PROJECTILES).max(0) as u64).min(MAX_PROJECTILES);
    (0..count).map(|i| mem.u64(PROJECTILES + 8 * i)).filter(|&p| p != 0).collect()
}

/// How many more entities of a kind the game takes.
pub fn free(mem: Mem, pool: Pool) -> u64 {
    match pool {
        Pool::Peds => base_free(mem, PED_POOL),
        Pool::Objects => base_free(mem, OBJECT_POOL),
        Pool::Pickups => base_free(mem, PICKUP_POOL),
        Pool::Projectiles => MAX_PROJECTILES - projectiles(mem).len() as u64,
        Pool::Vehicles => match vehicle_allocator(mem) {
            0 => 0,
            p => mem.u64(p + 16),
        },
    }
}

/// Which entities of a pool a mod asks for.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Filter {
    pub max: usize,
    /// Only those within `radius` of this place, the closest first.
    pub near: Option<([f32; 3], f32)>,
    /// Only those of this model (its name's hash).
    pub model: Option<u32>,
}

impl Default for Filter {
    fn default() -> Self {
        Filter { max: 1024, near: None, model: None }
    }
}

/// An entity's model (`CEntity` +32: its archetype, the name's hash at +24)
/// and position (its matrix's last row, +144), as GET_ENTITY_MODEL and
/// GET_ENTITY_COORDS read them.
fn model(mem: Mem, entity: u64) -> u32 {
    match mem.u64(entity + 32) {
        0 => 0,
        archetype => mem.u32(archetype + 24),
    }
}

fn position(mem: Mem, entity: u64) -> [f32; 3] {
    [0, 4, 8].map(|o| f32::from_bits(mem.u32(entity + 144 + o)))
}

/// Script handles for the entities of a pool that pass `filter`. Handles
/// are made only for those, while the handles' pool keeps room (the engine
/// fails when it fills).
///
/// # Safety
/// `env` is the engine thread's, inside a script's update.
pub unsafe fn handles(env: *mut Env, pool: Pool, filter: &Filter) -> Vec<i32> {
    let mem = unsafe { Mem::of(env) };
    let mut entities = match pool {
        Pool::Peds => base_pool(mem, PED_POOL),
        Pool::Objects => base_pool(mem, OBJECT_POOL),
        Pool::Pickups => base_pool(mem, PICKUP_POOL),
        Pool::Projectiles => projectiles(mem),
        Pool::Vehicles => vehicles(mem),
    };
    if let Some(m) = filter.model {
        entities.retain(|&e| model(mem, e) == m);
    }
    if let Some((at, radius)) = filter.near {
        let distance = |e: u64| {
            let p = position(mem, e);
            (0..3).map(|i| (p[i] - at[i]).powi(2)).sum::<f32>()
        };
        let mut near: Vec<(f32, u64)> =
            entities.into_iter().map(|e| (distance(e), e)).filter(|&(d, _)| d <= radius * radius).collect();
        near.sort_by(|a, b| a.0.total_cmp(&b.0));
        entities = near.into_iter().map(|(_, e)| e).collect();
    }
    let mut out = Vec::new();
    for e in entities {
        if out.len() >= filter.max || base_free(mem, GUID_POOL) < 64 {
            break;
        }
        match unsafe { orig::script_guid(env, e) } {
            0 => {}
            h => out.push(h as i32),
        }
    }
    out
}

/// A filter as a mod writes it: nothing, a count, or `{ max, near,
/// radius, model }`.
fn filter_of(value: mlua::Value) -> Result<Filter, String> {
    let mut f = Filter::default();
    let count = |v: &mlua::Value, what: &str| match *v {
        mlua::Value::Integer(n) if n >= 0 => Ok(n as usize),
        mlua::Value::Number(n) if n >= 0.0 && n.fract() == 0.0 => Ok(n as usize),
        _ => Err(format!("{what} must be a count")),
    };
    match value {
        mlua::Value::Nil => {}
        mlua::Value::Table(t) => {
            let get = |k: &str| t.raw_get::<mlua::Value>(k).unwrap_or(mlua::Value::Nil);
            if !get("max").is_nil() {
                f.max = count(&get("max"), "max")?;
            }
            match (get("near"), get("radius")) {
                (mlua::Value::Nil, mlua::Value::Nil) => {}
                (mlua::Value::Vector(v), r) => {
                    let radius = match r {
                        mlua::Value::Nil => f32::INFINITY,
                        mlua::Value::Integer(n) if n >= 0 => n as f32,
                        mlua::Value::Number(n) if n >= 0.0 => n as f32,
                        _ => return Err("radius must be a distance".into()),
                    };
                    f.near = Some(([v.x(), v.y(), v.z()], radius));
                }
                (mlua::Value::Nil, _) => return Err("radius needs near, the place to measure from".into()),
                _ => return Err("near must be a vector".into()),
            }
            f.model = match get("model") {
                mlua::Value::Nil => None,
                mlua::Value::String(s) => Some(crate::natives::joaat(&s.to_string_lossy())),
                mlua::Value::Integer(n) => Some(n as u32),
                mlua::Value::Number(n) if n.fract() == 0.0 => Some(n as i64 as u32),
                _ => return Err("model must be a name or a hash".into()),
            };
        }
        ref v => f.max = count(v, "the argument")?,
    }
    f.max = f.max.min(4096);
    Ok(f)
}

/// `world`: the pools (`world.vehicles(filter)`...) and the catalog's lists.
pub fn install(lua: &mlua::Lua) -> mlua::Result<()> {
    let world = lua.create_table()?;
    for (key, pool) in [
        ("vehicles", Pool::Vehicles),
        ("peds", Pool::Peds),
        ("objects", Pool::Objects),
        ("pickups", Pool::Pickups),
        ("projectiles", Pool::Projectiles),
    ] {
        world.set(
            key,
            lua.create_function(move |_, filter: mlua::Value| {
                let fail = |e: String| mlua::Error::runtime(format!("world.{key}: {e}"));
                let filter = filter_of(filter).map_err(fail)?;
                let env = crate::natives::env().ok_or_else(|| fail("only in a mod's script".into()))?;
                Ok(unsafe { handles(env, pool, &filter) })
            })?,
        )?;
    }
    world.set("vehicleModels", lua.create_function(|_, ()| Ok(catalog().map(|c| c.vehicles).unwrap_or_default()))?)?;
    world.set("pedModels", lua.create_function(|_, ()| Ok(catalog().map(|c| c.peds).unwrap_or_default()))?)?;
    world.set("weapons", lua.create_function(|_, ()| Ok(catalog().map(|c| c.weapons).unwrap_or_default()))?)?;
    lua.globals().set("world", world)
}

/// The installed game's names, lowercase models and uppercase weapons.
#[derive(Clone, Default, PartialEq)]
pub struct Catalog {
    pub vehicles: Vec<String>,
    pub peds: Vec<String>,
    pub weapons: Vec<String>,
}

static CATALOG: Mutex<Option<Catalog>> = Mutex::new(None);

/// The catalog, once read (this session, or the last one's, saved).
pub fn catalog() -> Option<Catalog> {
    crate::lock(&CATALOG).clone()
}

/// Reads the catalog from the engine's tables (after loading).
///
/// # Safety
/// `env` is the engine thread's.
pub unsafe fn read_catalog(env: *mut Env) -> Catalog {
    let mem = unsafe { Mem::of(env) };
    let name = |hash: u32| match unsafe { orig::hash_name(env, 0, hash) } {
        0 => None,
        p => Some(mem.str(p)),
    };
    let mut c = Catalog::default();
    let (storage, size, elem, bits) =
        (mem.u64(ARCHETYPES), mem.u64(ARCHETYPES + 8), mem.u64(ARCHETYPES + 24), mem.u64(ARCHETYPES + 56));
    if storage != 0 && bits != 0 {
        for i in 0..size.min(1 << 20) {
            if (mem.u32(bits + 4 * (i >> 5)) >> (i & 31)) & 1 == 0 {
                continue;
            }
            let arch = mem.u64(storage + elem * i);
            if arch == 0 {
                continue;
            }
            let list = match mem.u8(arch + 157) & 31 {
                5 => &mut c.vehicles,
                6 => &mut c.peds,
                _ => continue,
            };
            if let Some(n) = name(mem.u32(arch + 24)).filter(|n| valid(n)) {
                list.push(n.to_ascii_lowercase());
            }
        }
    }
    let (infos, count) = (mem.u64(ITEM_INFOS), mem.u16(ITEM_INFOS + 8));
    if infos != 0 {
        for i in 0..u64::from(count) {
            let info = mem.u64(infos + 8 * i);
            if info == 0 {
                continue;
            }
            if let Some(n) = name(mem.u32(info + 16)).filter(|n| n.starts_with("WEAPON_") && valid(n)) {
                c.weapons.push(n.to_ascii_uppercase());
            }
        }
    }
    for list in [&mut c.vehicles, &mut c.peds, &mut c.weapons] {
        list.sort();
        list.dedup();
    }
    c
}

/// A name a Luau string type can hold as it is.
fn valid(n: &str) -> bool {
    !n.is_empty() && n.len() <= 64 && n.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}

/// Takes a catalog read now: kept, saved for the next sessions (the
/// checker has the types before the game loads), and the checker made
/// again when it changed.
pub fn set_catalog(c: Catalog) {
    let mut slot = crate::lock(&CATALOG);
    if slot.as_ref() == Some(&c) {
        return;
    }
    log::info!("mods: catalog of {} vehicles, {} peds, {} weapons", c.vehicles.len(), c.peds.len(), c.weapons.len());
    let text = definitions_of(Some(&c));
    *slot = Some(c);
    drop(slot);
    *crate::lock(&ALIASES) = None;
    // The rest off the caller's thread (the engine's): a file to write, and
    // the Studio's type checker to wait for (busy for a while at times).
    let dir = crate::dir().map(std::path::Path::to_path_buf);
    let _ = std::thread::Builder::new().name("mods catalog".into()).spawn(move || {
        if let Some(dir) = dir {
            let _ = std::fs::create_dir_all(dir.join(".glue5"));
            let _ = std::fs::write(dir.join(".glue5/world.d.luau"), text);
        }
        crate::checker::Checker::release();
    });
}

/// The catalog saved by an earlier session.
pub fn load_saved(dir: &std::path::Path) {
    let Ok(text) = std::fs::read_to_string(dir.join(".glue5/world.d.luau")) else { return };
    let list = |name: &str| -> Vec<String> {
        text.lines()
            .find_map(|l| l.strip_prefix(&format!("type {name} = ")))
            .map(|rest| rest.split(" | ").map(|s| s.trim_matches('"').to_string()).filter(|s| valid(s)).collect())
            .unwrap_or_default()
    };
    let c = Catalog { vehicles: list("VehicleModel"), peds: list("PedModel"), weapons: list("WeaponName") };
    if !c.vehicles.is_empty() {
        *crate::lock(&CATALOG) = Some(c);
        *crate::lock(&ALIASES) = None;
    }
}

/// The catalog's types as sets of names, for [`shorten`] (the largest
/// first: a model is a vehicle's or a ped's).
static ALIASES: Mutex<Option<Vec<(&'static str, std::collections::HashSet<String>)>>> = Mutex::new(None);

fn aliases() -> Vec<(&'static str, std::collections::HashSet<String>)> {
    let mut slot = crate::lock(&ALIASES);
    if slot.is_none() {
        let c = catalog().unwrap_or_default();
        let set = |lists: &[&Vec<String>]| lists.iter().flat_map(|l| l.iter().cloned()).collect();
        *slot = Some(vec![
            ("Model", set(&[&c.vehicles, &c.peds])),
            ("VehicleModel", set(&[&c.vehicles])),
            ("PedModel", set(&[&c.peds])),
            ("WeaponName", set(&[&c.weapons])),
        ]);
    }
    slot.clone().unwrap()
}

/// `text` (a type or a message of the checker's) with the catalog's
/// unions of names written as their types (`VehicleModel`...) and other
/// long unions of strings cut short: the checker spells them out, a few
/// thousand characters for a vehicle's model.
pub fn shorten(text: &str) -> String {
    if !text.contains("\" | \"") {
        return text.to_owned();
    }
    let aliases = aliases();
    let mut out = String::with_capacity(text.len().min(4096));
    let mut rest = text;
    while let Some(start) = rest.find('"') {
        out.push_str(&rest[..start]);
        // A run of string literals joined by " | ".
        let mut names = Vec::new();
        let mut at = start;
        loop {
            let Some(len) = rest[at + 1..].find('"') else { break };
            names.push(&rest[at + 1..at + 1 + len]);
            let after = at + len + 2;
            if rest[after..].starts_with(" | \"") {
                at = after + 3;
            } else {
                at = after;
                break;
            }
        }
        if names.is_empty() {
            out.push_str(&rest[start..]);
            return out;
        }
        if names.len() < 6 {
            out.push_str(&rest[start..at]);
        } else {
            let mut left: std::collections::HashSet<&str> = names.iter().copied().collect();
            let mut parts: Vec<String> = Vec::new();
            for (alias, set) in &aliases {
                if !set.is_empty() && set.iter().all(|n| left.contains(n.as_str())) {
                    left.retain(|n| !set.contains(*n));
                    parts.push((*alias).to_owned());
                }
            }
            let mut rest_names: Vec<&str> = names.iter().copied().filter(|n| left.contains(n)).collect();
            rest_names.dedup();
            let more = rest_names.len().saturating_sub(3);
            if more > 1 {
                rest_names.truncate(3);
            }
            parts.extend(rest_names.iter().map(|n| format!("\"{n}\"")));
            if more > 1 {
                parts.push(format!("… {more} more"));
            }
            out.push_str(&parts.join(" | "));
        }
        rest = &rest[at..];
    }
    out.push_str(rest);
    out
}

/// The world's types for the checker: the catalog's names as string
/// unions, or plain strings before there is a catalog.
pub fn definitions() -> String {
    definitions_of(crate::lock(&CATALOG).as_ref())
}

fn definitions_of(c: Option<&Catalog>) -> String {
    let union = |names: &[String]| {
        if names.is_empty() {
            "string".to_string()
        } else {
            names.iter().map(|n| format!("\"{n}\"")).collect::<Vec<_>>().join(" | ")
        }
    };
    let empty = Vec::new();
    let (v, p, w) = match c {
        Some(c) => (&c.vehicles, &c.peds, &c.weapons),
        None => (&empty, &empty, &empty),
    };
    format!(
        "-- The installed game's names, read by GLUE 5 from what the game loaded.\n\
         type VehicleModel = {}\ntype PedModel = {}\ntype WeaponName = {}\ntype Model = VehicleModel | PedModel\n\n",
        union(v),
        union(p),
        union(w)
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn filters() {
        let lua = mlua::Lua::new();
        let f = |code: &str| filter_of(lua.load(code).eval().unwrap());
        assert_eq!(f("nil"), Ok(Filter::default()));
        assert_eq!(f("10"), Ok(Filter { max: 10, ..Filter::default() }));
        assert_eq!(f("1e9").unwrap().max, 4096);
        let near = f("{ near = vector.create(1, 2, 3), radius = 50, model = 'adder', max = 3 }").unwrap();
        assert_eq!(near, Filter { max: 3, near: Some(([1.0, 2.0, 3.0], 50.0)), model: Some(crate::natives::joaat("adder")) });
        assert_eq!(f("{ near = vector.create(0, 0, 0) }").unwrap().near, Some(([0.0; 3], f32::INFINITY)));
        assert_eq!(f("{ model = 0x3D8FA25C }").unwrap().model, Some(0x3D8FA25C));
        assert!(f("{ radius = 5 }").unwrap_err().contains("needs near"));
        assert!(f("{ near = 5 }").unwrap_err().contains("vector"));
        assert!(f("-1").is_err());
        assert!(f("'x'").is_err());
    }

    #[test]
    fn long_unions_get_their_names() {
        let names = |p: &str, n: usize| (0..n).map(|i| format!("{p}{i}")).collect::<Vec<_>>();
        let c = Catalog { vehicles: names("car", 8), peds: names("ped", 7), weapons: names("WEAPON_", 6) };
        *crate::lock(&CATALOG) = Some(c.clone());
        *crate::lock(&ALIASES) = None;
        let union = |l: &[String]| l.iter().map(|n| format!("\"{n}\"")).collect::<Vec<_>>().join(" | ");
        let both: Vec<String> = c.vehicles.iter().chain(&c.peds).cloned().collect();
        assert_eq!(
            shorten(&format!("(modelHash: {}, x: number) -> Vehicle?", union(&c.vehicles))),
            "(modelHash: VehicleModel, x: number) -> Vehicle?"
        );
        assert_eq!(shorten(&format!("(m: ({})?)", union(&both))), "(m: (Model)?)");
        assert_eq!(shorten(&union(&names("x", 9))), "\"x0\" | \"x1\" | \"x2\" | … 6 more");
        assert_eq!(shorten("(a: \"one\" | \"two\")"), "(a: \"one\" | \"two\")");
    }
}
