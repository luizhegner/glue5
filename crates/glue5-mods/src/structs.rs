//! Structures and arrays the natives fill, as tables: a thin layer in Luau
//! (structs.luau) over `native.invoke` and `memory`, for the few natives
//! where a table is much nicer than raw memory: the events' data
//! (`events.data`), the DLC vehicles', weapons' and weapon components'
//! data (`extrametadata.getDlcVehicleData`...), the peds near a ped
//! (`ped.getPedNearbyPeds`...). Each structure's layout is the engine's:
//! the fields its command or its event's RetrieveData writes, one 8-byte
//! script slot each, so the scratch memory has exactly the size the
//! engine checks or fills; it is freed after the call.

use glue5_runtime::memory::Mem;
use mlua::Lua;

/// The engine's names of the event types (`eEventType`): a `char*` each,
/// by type, NUM_EVENTTYPE of them.
const EVENT_NAMES: u64 = 6963856;
const EVENT_TYPES: u32 = 255;

/// The types of what structs.luau adds, for the checker.
pub const DEFINITIONS: &str = include_str!("structs.d.luau");

/// Adds the layer to a mod's VM, after the natives and `memory`.
pub fn install(lua: &Lua) -> mlua::Result<()> {
    let name = lua.create_function(|lua, t: u32| {
        let env = crate::natives::env().ok_or_else(|| mlua::Error::runtime("events: only in a mod's script"))?;
        if t >= EVENT_TYPES {
            return Ok(None);
        }
        let mem = unsafe { Mem::of(env) };
        match mem.u64(EVENT_NAMES + 8 * u64::from(t)) {
            0 => Ok(None),
            p => Ok(Some(lua.create_string(mem.str(p))?)),
        }
    })?;
    lua.load(include_str!("structs.luau")).set_name("=structs").call::<()>(name)
}

#[cfg(test)]
mod tests {
    use mlua::{Lua, Table};

    /// The layer over a pretend game: `memory` on a byte array, natives
    /// that write what the engine would.
    fn vm(natives: &str) -> Lua {
        let lua = Lua::new();
        lua.load(
            r#"
            local mem = buffer.create(4096)
            local next = 8
            memory = {
                alloc = function(n) local at = next; next += n; return at end,
                free = function() end,
                readInt = function(a) return buffer.readi32(mem, a) end,
                readU32 = function(a) return buffer.readu32(mem, a) end,
                readFloat = function(a) return buffer.readf32(mem, a) end,
                readString = function(a)
                    local s = buffer.readstring(mem, a, 64)
                    return string.split(s, "\0")[1]
                end,
                writeInt = function(a, v) buffer.writei32(mem, a, v) end,
            }
            mem_ = mem
            extrametadata, ped = {}, {}
            "#,
        )
        .exec()
        .unwrap();
        lua.load(natives).exec().unwrap();
        let name = lua.create_function(|_, t: u32| Ok(format!("EVENT_{t}"))).unwrap();
        lua.load(include_str!("structs.luau")).call::<()>(name).unwrap();
        lua
    }

    #[test]
    fn dlc_vehicle_data() {
        let lua = vm(r#"
            native = { invoke = function(name, i, at)
                assert(name == "GET_DLC_VEHICLE_DATA")
                if i ~= 3 then return false end
                buffer.writeu32(mem_, at, 0xDEADBEEF)
                buffer.writeu32(mem_, at + 8, 0xB779A091)
                buffer.writei32(mem_, at + 16, 1500)
                return true
            end }
        "#);
        let data: Table = lua.load("return extrametadata.getDlcVehicleData(3)").eval().unwrap();
        assert_eq!(data.get::<u32>("lockHash").unwrap(), 0xDEADBEEF);
        assert_eq!(data.get::<u32>("model").unwrap(), 0xB779A091);
        assert_eq!(data.get::<i32>("cost").unwrap(), 1500);
        assert!(lua.load("return extrametadata.getDlcVehicleData(4)").eval::<Option<Table>>().unwrap().is_none());
    }

    #[test]
    fn weapon_labels() {
        let lua = vm(r#"
            native = { invoke = function(name, i, at)
                buffer.writeu32(mem_, at + 8, 42)
                buffer.writestring(mem_, at + 56, "WT_PIST\0")
                buffer.writestring(mem_, at + 248, "WTU_PIST\0")
                return true
            end }
        "#);
        let data: Table = lua.load("return extrametadata.getDlcWeaponData(0)").eval().unwrap();
        assert_eq!(data.get::<u32>("weapon").unwrap(), 42);
        assert_eq!(data.get::<String>("nameLabel").unwrap(), "WT_PIST");
        assert_eq!(data.get::<String>("descLabel").unwrap(), "");
        assert_eq!(data.get::<String>("upperNameLabel").unwrap(), "WTU_PIST");
    }

    #[test]
    fn nearby_arrays() {
        let lua = vm(r#"
            native = { invoke = function(name, p, at, ignore)
                local room = buffer.readi32(mem_, at)
                seen = { name = name, room = room, ignore = ignore }
                for i = 1, math.min(room, 3) do buffer.writei32(mem_, at + 8 * i, 100 + i) end
                return math.min(room, 3)
            end }
        "#);
        let list: Vec<i32> = lua.load("return ped.getPedNearbyPeds(1, nil, 2)").eval().unwrap();
        assert_eq!(list, [101, 102]);
        assert_eq!(lua.load("return seen.ignore, seen.room").eval::<(i32, i32)>().unwrap(), (-1, 2));
        let list: Vec<i32> = lua.load("return ped.getPedNearbyVehicles(1)").eval().unwrap();
        assert_eq!(list, [101, 102, 103]);
        assert_eq!(lua.load("return seen.room").eval::<i32>().unwrap(), 16);
        lua.load("ped.getPedNearbyVehicles(1, 1000)").exec().unwrap();
        assert_eq!(lua.load("return seen.room").eval::<i32>().unwrap(), 64);
    }

    #[test]
    fn events() {
        let lua = vm(r#"
            native = { invoke = function(name, g, i, at, size)
                if name == "GET_NUMBER_OF_EVENTS" then return 2 end
                if name == "GET_EVENT_AT_INDEX" then return ({ [0] = 186, [1] = 3 })[i] or -1 end
                assert(name == "GET_EVENT_DATA" and g == 1)
                if size ~= 13 then return false end
                buffer.writei32(mem_, at, 514)
                buffer.writef32(mem_, at + 16, 25.5)
                buffer.writei32(mem_, at + 40, 1)
                buffer.writeu32(mem_, at + 48, 0x1B06D571)
                return true
            end }
        "#);
        assert_eq!(lua.load("return events.count('network')").eval::<i32>().unwrap(), 2);
        assert_eq!(lua.load("return events.at('network', 0)").eval::<(String, i32)>().unwrap(), ("EVENT_186".into(), 186));
        let d: Table = lua.load("return events.data('network', 0)").eval().unwrap();
        assert_eq!(d.get::<String>("type").unwrap(), "EVENT_186");
        assert_eq!(d.get::<i32>("victim").unwrap(), 514);
        assert_eq!(d.get::<Option<i32>>("damager").unwrap(), None);
        assert_eq!(d.get::<f32>("damage").unwrap(), 25.5);
        assert!(d.get::<bool>("victimDestroyed").unwrap());
        assert!(!d.get::<bool>("headshot").unwrap());
        assert_eq!(d.get::<u32>("weapon").unwrap(), 0x1B06D571);
        let (none, why): (Option<Table>, String) = lua.load("return events.data('network', 1)").eval().unwrap();
        assert!(none.is_none() && why.contains("EVENT_3 has no data"), "{why}");
        assert!(lua.load("return events.at('network', 5)").eval::<Option<String>>().unwrap().is_none());
        let e = lua.load("events.count('net')").exec().unwrap_err().to_string();
        assert!(e.contains("\"ai\", \"network\" or \"errors\""), "{e}");
    }

    /// The checker knows the layer: the events' types narrow on `type`.
    #[test]
    fn types() {
        const SOURCE: &str = r#"--!strict
local me = player.playerPedId()
for i = 0, events.count("network") - 1 do
    local d = events.data("network", i)
    if d and d.type == "EVENT_NETWORK_DAMAGE_ENTITY" then
        local hit: number = d.damage
        local by: Entity? = d.damager
        print(hit, by, d.headshot)
    end
end
local v = extrametadata.getDlcVehicleData(0)
local cost: number = if v then v.cost else 0
local w = extrametadata.getDlcWeaponComponentData(0, 1)
local label: string = if w then w.nameLabel else ""
local cars: { Vehicle } = ped.getPedNearbyVehicles(me, 8)
local peds: { Ped } = ped.getPedNearbyPeds(me, -1)
print(cost, label, #cars, #peds, events.at("ai", 0))
local bad = events.count("everything")
"#;
        crate::checker::Checker::with(|c| {
            c.set("structs/main.luau", Some(SOURCE));
            let diagnostics = c.check("structs/main.luau");
            let errors: Vec<&serde_json::Value> =
                diagnostics.as_array().unwrap().iter().filter(|d| d["severity"] == "error").collect();
            // Only the group that is none.
            assert!(errors.len() == 1 && errors[0]["range"][0] == 17, "{diagnostics}");
        })
        .unwrap();
    }
}
