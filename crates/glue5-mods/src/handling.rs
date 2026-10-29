//! Vehicles' handling (`CHandlingData`: handling.meta's values in memory)
//! for mods, every field by handling.meta's name: `handling.get(car,
//! "fInitialDriveForce")`. The names, offsets and types are the game's own:
//! the parser's description of each structure (`parStructureStaticData`,
//! what the game reads handling.meta with), filled by the game's static
//! initialisers, so no field is listed here by hand. The sub-handlings
//! (`CCarHandlingData`, `CBikeHandlingData`...) a handling holds are
//! covered the same way.
//!
//! Vehicles of a model share its handling: a change reaches all of them,
//! and stays until the game restarts. Values are as the game keeps them
//! after `ConvertToGameUnits` (handling.meta's km/h as m/s, degrees as
//! radians...), and a few are copied into a vehicle when it is made (its
//! gearbox: gears, drive force, top speed).

use std::cell::OnceCell;

use glue5_runtime::memory::Mem;
use mlua::{Lua, Value};

use crate::natives;

/// `CHandlingDataMgr`'s handlings: an atArray of `CHandlingData*` (its u16
/// count at +8).
const HANDLINGS: u64 = 25978928;
/// A vehicle's `CHandlingData*` (`CVehicle::GetBrakeForce`).
const VEHICLE_HANDLING: u64 = 2392;
/// A handling's name hash (handling.meta's `handlingName`).
const HANDLING_NAME: u64 = 8;
/// A handling's sub-handlings: an atArray of `CBaseSubHandlingData*` (u16
/// count at +8; `CHandlingData::GetHandlingData`).
const SUB_HANDLINGS: u64 = 344;
/// What `CEntity` + 40 holds for a vehicle.
const VEHICLE_TYPE: u8 = 3;

/// The structures: the class, its `parStructureStaticData` (name hash +0,
/// class name +8, members +24: null-ended pointers, a member's type u8 at
/// +16; offsets +32: u32 each; member names +40) and, for the sub-handlings,
/// the vtable an object of the class starts with (`atPtrCreator<...>`).
const STRUCTURES: &[(&str, u64, u64)] = &[
    ("CHandlingData", 25977848, 0),
    ("CCarHandlingData", 25978544, 6082688),
    ("CBikeHandlingData", 25978352, 6082296),
    ("CFlyingHandlingData", 25978184, 6082352),
    ("CBoatHandlingData", 25978072, 6082464),
    ("CSeaPlaneHandlingData", 25978128, 6082408),
    ("CSubmarineHandlingData", 25978408, 6082576),
    ("CTrailerHandlingData", 25978464, 6082632),
    ("CSpecialFlightHandlingData", 25978600, 6082744),
    ("CVehicleWeaponHandlingData", 25978240, 6082520),
];

/// The most members a structure is read with (the largest has 57).
const MAX_MEMBERS: u64 = 512;

/// A member's type, as `rage::parMemberType` numbers them.
#[derive(Clone, Copy, PartialEq, Debug)]
enum Kind {
    Bool,
    I8,
    U8,
    I16,
    U16,
    I32,
    U32,
    Float,
    /// handling.meta's text (names, flags) hashed, 32 bits.
    Hash,
    /// A `Vec3V` (16 bytes; x, y, z first).
    Vector,
    Enum,
    /// Arrays and anything else: listed, not read (memory.* at the address).
    Other,
}

impl Kind {
    fn of(t: u8) -> Kind {
        match t {
            0 => Kind::Bool,
            1 => Kind::I8,
            2 => Kind::U8,
            3 => Kind::I16,
            4 => Kind::U16,
            5 => Kind::I32,
            6 => Kind::U32,
            7 => Kind::Float,
            11 => Kind::Hash,
            14 => Kind::Enum,
            20 => Kind::Vector,
            _ => Kind::Other,
        }
    }

    fn name(self) -> &'static str {
        match self {
            Kind::Bool => "bool",
            Kind::I8 => "i8",
            Kind::U8 => "u8",
            Kind::I16 => "i16",
            Kind::U16 => "u16",
            Kind::I32 => "i32",
            Kind::U32 => "u32",
            Kind::Float => "float",
            Kind::Hash => "hash",
            Kind::Vector => "vector",
            Kind::Enum => "enum",
            Kind::Other => "other",
        }
    }

    /// The bytes read and written, none for what is not read.
    fn size(self) -> Option<u64> {
        match self {
            Kind::Bool | Kind::I8 | Kind::U8 => Some(1),
            Kind::I16 | Kind::U16 => Some(2),
            Kind::I32 | Kind::U32 | Kind::Float | Kind::Hash | Kind::Enum => Some(4),
            Kind::Vector => Some(12),
            Kind::Other => None,
        }
    }
}

struct Member {
    name: String,
    offset: u64,
    kind: Kind,
}

struct Structure {
    class: &'static str,
    vtable: u64,
    members: Vec<Member>,
}

thread_local! {
    /// The structures, read once (the static initialisers ran before any mod).
    static READ: OnceCell<Vec<Structure>> = const { OnceCell::new() };
}

/// Reads the structures from the parser's static data, an error if they
/// are not what this build has there.
fn read(mem: Mem) -> Result<Vec<Structure>, String> {
    STRUCTURES
        .iter()
        .map(|&(class, at, vtable)| {
            if mem.u64(at + 8) == 0 || mem.str(mem.u64(at + 8)) != class {
                return Err(format!("the game's description of {class} is not where this build has it"));
            }
            let (members, offsets, names) = (mem.u64(at + 24), mem.u64(at + 32), mem.u64(at + 40));
            let mut list = Vec::new();
            for i in (0..MAX_MEMBERS).take_while(|&i| members != 0 && mem.u64(members + 8 * i) != 0) {
                let member = mem.u64(members + 8 * i);
                let name = match names {
                    0 => format!("0x{:08x}", mem.u32(member)),
                    n => mem.str(mem.u64(n + 8 * i)),
                };
                list.push(Member { name, offset: u64::from(mem.u32(offsets + 4 * i)), kind: Kind::of(mem.u8(member + 16)) });
            }
            Ok(Structure { class, vtable, members: list })
        })
        .collect()
}

/// Runs `f` with the structures.
fn with<R>(mem: Mem, f: impl FnOnce(&[Structure]) -> R) -> Result<R, String> {
    READ.with(|cell| {
        if cell.get().is_none() {
            let _ = cell.set(read(mem)?);
        }
        Ok(f(cell.get().expect("set above")))
    })
}

/// The handling a target names: a vehicle (its handle), or a handling by
/// handling.meta's name (any case).
fn handling(mem: Mem, target: &Value) -> Result<u64, String> {
    match target {
        Value::Integer(_) | Value::Number(_) => {
            let handle = match *target {
                Value::Integer(i) => i32::try_from(i).ok(),
                Value::Number(n) if n.fract() == 0.0 && n.abs() < 2e9 => Some(n as i32),
                _ => None,
            };
            let entity = handle.and_then(|h| natives::entity_address(mem, h)).ok_or("not an existing vehicle")?;
            if mem.u8(entity + 40) != VEHICLE_TYPE {
                return Err("not a vehicle".into());
            }
            match mem.u64(entity + VEHICLE_HANDLING) {
                0 => Err("the vehicle has no handling".into()),
                h => Ok(h),
            }
        }
        Value::String(name) => {
            let name = name.to_str().map_err(|e| e.to_string())?;
            let hash = natives::joaat(&name);
            let (list, count) = (mem.u64(HANDLINGS), mem.u16(HANDLINGS + 8));
            (0..u64::from(count))
                .map(|i| mem.u64(list + 8 * i))
                .find(|&h| h != 0 && mem.u32(h + HANDLING_NAME) == hash)
                .ok_or_else(|| format!("no handling named {name:?}"))
        }
        _ => Err("a vehicle or a handling's name".into()),
    }
}

/// The objects of a handling with their structures: the handling, then
/// each sub-handling of a known class.
fn objects(mem: Mem, structures: &[Structure], handling: u64) -> Vec<(u64, usize)> {
    let mut list = vec![(handling, 0)];
    let (subs, count) = (mem.u64(handling + SUB_HANDLINGS), mem.u16(handling + SUB_HANDLINGS + 8));
    for i in 0..u64::from(count) {
        let sub = mem.u64(subs + 8 * i);
        if sub == 0 {
            continue;
        }
        let vtable = mem.u64(sub);
        if let Some(s) = structures.iter().position(|s| s.vtable != 0 && s.vtable == vtable) {
            list.push((sub, s));
        }
    }
    list
}

/// The address and type of field `name` of a handling (its sub-handlings'
/// too).
fn field(mem: Mem, structures: &[Structure], handling: u64, name: &str) -> Option<(u64, Kind)> {
    objects(mem, structures, handling).into_iter().find_map(|(object, s)| {
        structures[s].members.iter().find(|m| m.name.eq_ignore_ascii_case(name)).map(|m| (object + m.offset, m.kind))
    })
}

/// Whether `bytes` bytes at `at` are in memory of `mem_size` bytes.
fn inside(mem_size: u64, at: u64, bytes: u64) -> bool {
    at >= 1024 && at.saturating_add(bytes) <= mem_size
}

/// Installs `handling`.
pub fn install(lua: &Lua) -> mlua::Result<()> {
    let fail = |what: &str, e: String| mlua::Error::runtime(format!("handling.{what}: {e}"));
    let context = move |what: &'static str| -> mlua::Result<(Mem, u64)> {
        let env = natives::env().ok_or_else(|| fail(what, "only in a mod's script".into()))?;
        Ok((unsafe { Mem::of(env) }, unsafe { (*(*env).memory).size }))
    };
    let t = lua.create_table()?;
    t.set(
        "address",
        lua.create_function(move |_, target: Value| {
            let (mem, _) = context("address")?;
            Ok(handling(mem, &target).ok())
        })?,
    )?;
    t.set(
        "fields",
        lua.create_function(move |lua, target: Option<Value>| {
            let (mem, _) = context("fields")?;
            let at = match &target {
                Some(t) => Some(handling(mem, t).map_err(|e| fail("fields", e))?),
                None => None,
            };
            let list = lua.create_table()?;
            with(mem, |structures| -> mlua::Result<()> {
                let objects = at.map_or_else(|| vec![(0, 0)], |h| objects(mem, structures, h));
                for (_, s) in objects {
                    for m in &structures[s].members {
                        let f = lua.create_table()?;
                        f.set("name", m.name.as_str())?;
                        f.set("kind", m.kind.name())?;
                        f.set("offset", m.offset)?;
                        f.set("struct", structures[s].class)?;
                        list.push(f)?;
                    }
                }
                Ok(())
            })
            .map_err(|e| fail("fields", e))??;
            Ok(list)
        })?,
    )?;
    t.set(
        "get",
        lua.create_function(move |_, (target, name): (Value, String)| {
            let (mem, size) = context("get")?;
            let at = handling(mem, &target).map_err(|e| fail("get", e))?;
            let (a, kind) = with(mem, |s| field(mem, s, at, &name))
                .map_err(|e| fail("get", e))?
                .ok_or_else(|| fail("get", format!("no field {name:?} (handling.fields lists them)")))?;
            let bytes = kind.size().ok_or_else(|| fail("get", format!("{name} is not a value (read it at handling.address() + its offset)")))?;
            if !inside(size, a, bytes) {
                return Err(fail("get", format!("{name} is outside the game's memory")));
            }
            Ok(match kind {
                Kind::Bool => Value::Boolean(mem.u8(a) != 0),
                Kind::I8 => Value::Integer(i64::from(mem.u8(a) as i8)),
                Kind::U8 => Value::Integer(i64::from(mem.u8(a))),
                Kind::I16 => Value::Integer(i64::from(mem.u16(a) as i16)),
                Kind::U16 => Value::Integer(i64::from(mem.u16(a))),
                Kind::I32 | Kind::Enum => Value::Integer(i64::from(mem.i32(a))),
                Kind::U32 | Kind::Hash => Value::Integer(i64::from(mem.u32(a))),
                Kind::Float => Value::Number(f64::from(f32::from_bits(mem.u32(a)))),
                Kind::Vector => {
                    let f = |o: u64| f32::from_bits(mem.u32(a + o));
                    Value::Vector(mlua::Vector::new(f(0), f(4), f(8)))
                }
                Kind::Other => unreachable!("no size"),
            })
        })?,
    )?;
    t.set(
        "set",
        lua.create_function(move |_, (target, name, value): (Value, String, Value)| {
            let (mem, size) = context("set")?;
            let at = handling(mem, &target).map_err(|e| fail("set", e))?;
            let (a, kind) = with(mem, |s| field(mem, s, at, &name))
                .map_err(|e| fail("set", e))?
                .ok_or_else(|| fail("set", format!("no field {name:?} (handling.fields lists them)")))?;
            let bytes = kind.size().ok_or_else(|| fail("set", format!("{name} is not a value (write it at handling.address() + its offset)")))?;
            if !inside(size, a, bytes) {
                return Err(fail("set", format!("{name} is outside the game's memory")));
            }
            let number = || match value {
                Value::Integer(i) => Ok(i as f64),
                Value::Number(n) => Ok(n),
                _ => Err(fail("set", format!("{name} takes a number"))),
            };
            match kind {
                Kind::Bool => match value {
                    Value::Boolean(b) => mem.set_u8(a, u8::from(b)),
                    _ => return Err(fail("set", format!("{name} takes a boolean"))),
                },
                Kind::I8 | Kind::U8 => mem.set_u8(a, number()? as i64 as u8),
                Kind::I16 | Kind::U16 => mem.set_u16(a, number()? as i64 as u16),
                Kind::I32 | Kind::Enum | Kind::U32 | Kind::Hash => mem.set_u32(a, number()? as i64 as u32),
                Kind::Float => mem.set_u32(a, (number()? as f32).to_bits()),
                Kind::Vector => match value {
                    Value::Vector(v) => {
                        mem.set_u32(a, v.x().to_bits());
                        mem.set_u32(a + 4, v.y().to_bits());
                        mem.set_u32(a + 8, v.z().to_bits());
                    }
                    _ => return Err(fail("set", format!("{name} takes a vector"))),
                },
                Kind::Other => unreachable!("no size"),
            }
            Ok(())
        })?,
    )?;
    lua.globals().set("handling", t)
}

/// The checker's definitions of `handling`.
pub const DEFINITIONS: &str = "
-- A field of a handling (handling.fields): handling.meta's name, how it is kept, where (bytes from its
-- structure's address: the handling's, or a sub-handling's), and which structure holds it.
export type HandlingField = {
\tname: string,
\tkind: \"bool\" | \"i8\" | \"u8\" | \"i16\" | \"u16\" | \"i32\" | \"u32\" | \"float\" | \"hash\" | \"vector\" | \"enum\" | \"other\",
\toffset: number,
\tstruct: string,
}

-- Vehicles' handling (handling.meta's values, as the game keeps them), every field by handling.meta's
-- name, as the game's own parser describes it: CHandlingData's and the sub-handlings' (CCarHandlingData,
-- CBikeHandlingData, CFlyingHandlingData...). A target is a vehicle or a handling's name (\"ADDER\").
-- Vehicles of a model share its handling: a change reaches all of them and lasts until the game restarts.
-- Values are in the game's units (m/s, radians: handling.meta's after conversion); a vehicle's gearbox
-- (gears, drive force, top speed) is set up from them when it is made.
declare handling: {
\t-- The value of a field. A `hash` field is handling.meta's text hashed (joaat): the flags as bits
\t-- are at memory.offsets.handling.
\tget: (target: Vehicle | string, field: string) -> any,
\tset: (target: Vehicle | string, field: string, value: number | boolean | vector) -> (),
\t-- The fields: CHandlingData's, and with a target those of its sub-handlings too.
\tfields: (target: (Vehicle | string)?) -> { HandlingField },
\t-- The handling's address (for memory.*), nil if there is none.
\taddress: (target: Vehicle | string) -> number?,
}
";

#[cfg(test)]
mod tests {
    use super::Kind;

    #[test]
    fn kinds() {
        assert_eq!(Kind::of(7), Kind::Float);
        assert_eq!(Kind::of(20).size(), Some(12));
        assert_eq!(Kind::of(13), Kind::Other);
        assert_eq!(Kind::of(13).size(), None);
        assert_eq!(Kind::of(11).name(), "hash");
    }
}
