use mlua::{Lua, LuaOptions, StdLib};

use super::install;

/// A VM with the library over stand-ins for the natives: each native
/// records its call in `calls` and returns 0; `setup` changes them first.
fn stubbed(setup: &str) -> Lua {
    let lua = Lua::new_with(StdLib::ALL_SAFE, LuaOptions::default()).unwrap();
    lua.load(
        r#"
        calls = {}
        local function namespace(ns)
            return setmetatable({}, { __index = function(t, k)
                local f = function(...) table.insert(calls, { ns .. "." .. k, ... }) return 0 end
                rawset(t, k, f)
                return f
            end })
        end
        for _, ns in { "misc", "streaming", "graphics", "player", "entity", "vehicle", "ped", "object",
            "shapetest", "camera", "zone", "path", "hud", "pad" } do
            _G[ns] = namespace(ns)
        end
        local none = function() return {} end
        world = { vehicles = none, peds = none, objects = none, pickups = none }
        ui = { notify = function(text) table.insert(calls, { "app.notify", text }) end }
        wait = function() end
        task = { spawn = function(f, ...) f(...) end, wait = wait }
        mod = {}
        "#,
    )
    .exec()
    .unwrap();
    lua.load(setup).exec().unwrap();
    install(&lua).unwrap();
    lua
}

fn eval<T: mlua::FromLuaMulti>(lua: &Lua, code: &str) -> T {
    lua.load(code).eval().unwrap_or_else(|e| panic!("{code}: {e}"))
}

/// `code` fails with an error that says `error`.
fn fails(lua: &Lua, code: &str, error: &str) {
    let e = lua.load(code).exec().unwrap_err().to_string();
    assert!(e.contains(error), "{code}: {e}");
}

/// The game's own values, from game.wasm and the natives database.
#[test]
fn constants() {
    let lua = stubbed("");
    for (code, value) in [
        ("Control.JUMP", 22),
        ("Control.CONTEXT", 51),
        ("Control.VEH_ACCELERATE", 71),
        ("Bone.R_HAND", 57005),
        ("ExplosionType.GRENADE", 0),
        ("ExplosionType.CAR", 7),
        ("ExplosionType.DONTCARE", -1),
        ("PedConfigFlag.CreatedByFactory", 0),
        ("PedConfigFlag.WillFlyThroughWindscreen", 32),
        ("PedConfigFlag.NoCriticalHits", 2),
        ("HudComponent.WANTED_STARS", 1),
        ("VehicleSeat.DRIVER", -1),
    ] {
        assert_eq!(eval::<i64>(&lua, code), value, "{code}");
    }
}

#[test]
fn every() {
    let lua = stubbed("");
    fails(&lua, "task.every('soon', print)", "task.every: the interval");
    fails(&lua, "task.every(100)", "task.every: the second argument");
}

#[test]
fn streaming() {
    let lua = stubbed("");
    fails(&lua, "streaming.load('adder', 'sound')", "no kind of asset \"sound\"");
}

/// Text past a component's 99 bytes goes in pieces, cut between
/// characters, under a label of many components.
#[test]
fn long_text() {
    let lua = stubbed("");
    let text = "é".repeat(150);
    lua.globals().set("text", text.as_str()).unwrap();
    let (label, parts): (String, Vec<String>) = eval(
        &lua,
        r#"
        ui.subtitle(text)
        local parts = {}
        for _, c in calls do
            if c[1] == "hud.addTextComponentSubstringPlayerName" then table.insert(parts, c[2]) end
        end
        return calls[1][2], parts
        "#,
    );
    assert_eq!(label, "CELL_EMAIL_BCON");
    assert!(parts.iter().all(|p| p.len() <= 99), "{parts:?}");
    assert_eq!(parts.concat(), text);
    let short: String = eval(&lua, "calls = {} ui.help('Press ~INPUT_CONTEXT~') return calls[1][2]");
    assert_eq!(short, "STRING");
    fails(&lua, "ui.subtitle(12)", "the text is a string");
    fails(&lua, "ui.buttons({ { control = 'x', label = 'Jump' } })", "ui.buttons: button 1");
}

/// `ui.notify(text)` is the app's notification still; with options, the
/// game's feed.
#[test]
fn notify() {
    let lua = stubbed("");
    let first: String = eval(&lua, "ui.notify('hi') return calls[1][1]");
    assert_eq!(first, "app.notify");
    let feed: Vec<String> = eval(
        &lua,
        "calls = {} ui.notify('hi', { title = 'Lester' })
         local names = {} for _, c in calls do table.insert(names, c[1]) end return names",
    );
    assert!(feed.contains(&"hud.endTextCommandThefeedPostMessagetext".to_string()), "{feed:?}");
    assert!(feed.contains(&"hud.endTextCommandThefeedPostTicker".to_string()), "{feed:?}");
}

#[test]
fn rotations() {
    let lua = stubbed("");
    let close = |code: &str| {
        let ok: bool = eval(&lua, code);
        assert!(ok, "{code}");
    };
    close("return vector.magnitude(geo.direction(vector.create(0, 0, 90)) - vector.create(-1, 0, 0)) < 1e-6");
    close("return math.abs(geo.heading(geo.fromHeading(135)) - 135) < 1e-4");
    close("local d = geo.direction(vector.create(20, 0, 30)) return vector.magnitude(geo.direction(geo.rotation(d)) - d) < 1e-5");
    close(
        "local r = vector.create(20, 10, 30)
         return vector.magnitude(geo.fromQuaternion(geo.quaternion(r)) - r) < 1e-3",
    );
    // The quaternion turns the forward axis where the rotation points.
    close(
        "local r = vector.create(25, 0, -60)
         local x, y, z, w = geo.quaternion(r)
         -- (0, 1, 0) rotated by q: the second column of its matrix.
         local f = vector.create(2 * (x * y - w * z), 1 - 2 * (x * x + z * z), 2 * (y * z + w * x))
         return vector.magnitude(f - geo.direction(r)) < 1e-5",
    );
    close("return geo.distance(vector.create(0, 0, 0), vector.create(3, 4, 12), true) == 5");
}

/// Entities near a place (the pools' own filter); near an entity, without
/// it.
#[test]
fn nearby() {
    let lua = stubbed(
        "local at = { [1] = vector.create(10, 0, 0), [2] = vector.create(2, 0, 0), [3] = vector.create(50, 0, 0), [4] = vector.create(0, 0, 0) }
         -- As world.rs filters: within radius of near, the closest first, max of them.
         world.peds = function(f)
             local found = {}
             for e, p in at do
                 if vector.magnitude(p - f.near) <= f.radius then table.insert(found, e) end
             end
             table.sort(found, function(a, b) return vector.magnitude(at[a] - f.near) < vector.magnitude(at[b] - f.near) end)
             while f.max and #found > f.max do table.remove(found) end
             return found
         end
         entity = { getEntityCoords = function(e) return at[e] end }",
    );
    let near: Vec<i64> = eval(&lua, "return world.nearby('peds', vector.create(0, 0, 0), 20)");
    assert_eq!(near, [4, 2, 1]);
    let around: Vec<i64> = eval(&lua, "return world.nearby('peds', 4, 20)");
    assert_eq!(around, [2, 1]);
    let closest: i64 = eval(&lua, "return world.closest('peds', 4)");
    assert_eq!(closest, 2);
    let none: Option<i64> = eval(&lua, "return world.closest('peds', vector.create(100, 100, 0), 5)");
    assert!(none.is_none());
    fails(&lua, "world.nearby('cars', 4, 1)", "no kind \"cars\"");
}

/// The types: a mod using the library checks clean, and the Studio
/// completes its names.
#[test]
fn types() {
    const SOURCE: &str = r#"--!strict
local me = player.playerPedId()
local car = spawn.vehicle("adder", nil, nil, { warpIn = true })
local hit = world.raycast(entity.getEntityCoords(me, false), vector.create(0, 0, 0), { ignore = me })
if hit then print(hit.pos, hit.entity) end
local peds: { Ped } = world.nearby("peds", me, 30)
local nearest: Vehicle? = world.closest("vehicles", me)
local ok: boolean = streaming.load("prop_bench_01a", "model", 2000)
ui.notify("plain")
ui.notify("feed", { icon = "CHAR_LESTER", title = "Lester" })
ui.help("Press ~INPUT_CONTEXT~")
ui.buttons({ { control = Control.JUMP, label = "Jump" } })
local sf = scaleform.load("mp_big_message_freemode")
if sf then sf:call("SHOW_SHARD_WASTED_MP_MESSAGE", "Hi", "there", 5) sf:draw() end
local stop = task.every(1000, function() end)
mod.onStop(function() stop() end)
local x, y, z, w = geo.quaternion(vector.create(0, 0, 90))
local typed: string? = ui.prompt("FMMC_KEY_TIP8", "", 30)
local flag: number = PedConfigFlag.NoCriticalHits + ExplosionType.GRENADE + Bone.R_HAND + HudComponent.WANTED_STARS
world.raycas
"#;
    crate::checker::Checker::with(|c| {
        c.set("std/main.luau", Some(SOURCE));
        let diagnostics = c.check("std/main.luau");
        // Only the unfinished last line's.
        let errors: Vec<&serde_json::Value> =
            diagnostics.as_array().unwrap().iter().filter(|d| d["severity"] == "error").collect();
        assert!(!errors.is_empty() && errors.iter().all(|d| d["range"][0] == 19), "{diagnostics}");
        let items = c.complete("std/main.luau", 19, 12);
        let labels: Vec<&str> = items.as_array().unwrap().iter().filter_map(|i| i["label"].as_str()).collect();
        assert!(labels.contains(&"raycast") && labels.contains(&"vehicles"), "{labels:?}");
    })
    .unwrap();
}
