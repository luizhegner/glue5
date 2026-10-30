//! What the Studio and MCP both do: the mods as JSON, a file's
//! diagnostics, the natives' signatures and notes, `eval`, the guide.

use std::collections::HashMap;
use std::sync::OnceLock;
use std::time::Duration;

use serde_json::{Value, json};

use crate::checker::{self, Checker};
use crate::menus::validate::kind_name;
use crate::{Menu, MenuInput, MenuItem, MenuKind, Rgba, files, natives};

/// How the mods' API works, for people and agents (MCP's guide resource,
/// the exported CLAUDE.md).
pub const GUIDE: &str = r#"# GLUE 5 mods

A mod is a folder in the mods folder: `main.luau` runs when the mod starts,
other `.luau` files load with `require("./name")` (beside the file that
asks; `"../core/name"` from a folder up, within the mod), and `mod.json`, if there
is one, says what the mod is (the Studio shows it as the mod's page):
`{"name": "Trainer", "description": "...", "author": "...", "version":
"1.0", "autostart": false, "restartOnSave": true}`, every key optional;
`autostart` starts it with the game. Mods are Luau (typed Lua); start
files with `--!strict` to have every type checked.

## Running
- A mod runs in a script thread of the game's own, once the player is in
  the game. `main.luau` runs top to bottom: a mod that returns is done
  (one-shot); a mod that loops must call `wait()` in the loop.
- `wait()` gives the game its frame and goes on in the next one;
  `wait(ms)` waits that long. A mod that runs 2 s without waiting is
  stopped, and one frame never gets more than 2 ms of a mod.
- `print(...)` writes to the mod's log. An error stops the mod and logs the
  message with `file:line`.
- When a mod stops, what it created (vehicles, peds, objects) is released
  the way the game's own scripts' is.

## Natives
- The game's natives are functions in namespace tables, named in Luau's
  style: `CREATE_VEHICLE` is `vehicle.createVehicle`, `PLAYER_PED_ID` is
  `player.playerPedId`. Namespaces: entity, ped, vehicle, player, object,
  weapon, streaming, hud, graphics, pad, misc, camera, task, audio,
  network, ... (natives.d.luau has every signature).
- Handles have types: `Entity`, with `Ped`, `Vehicle`, `Object` under it,
  and `Player`, `Blip`, `Cam`, ... A native that returns a handle returns
  `nil` for none, so check before use (`if car then ... end`).
- Handles go stale: get the player's ped with `player.playerPedId()` when
  you need it rather than once at the start. A stale or made-up handle is
  an error, not a crash.
- Outputs come back as extra results: `local primary, secondary =
  vehicle.getVehicleColours(car)`.
- Positions are `vector`s: `vector.create(x, y, z)`, `a + b`, `v * 2`,
  `vector.magnitude(v)`, `v.x`.
- `Hash` parameters take a name too: `vehicle.createVehicle("adder", ...)`;
  `joaat("adder")` gives the hash.
- Models must be loaded before use: `streaming.requestModel(m)`, then
  `while not streaming.hasModelLoaded(m) do wait() end`.
- Controls: `pad.isControlJustPressed(0, control)`, with the game's control
  numbers (51 = E / context, 22 = jump, 21 = sprint).
- Names are typed from the installed game (glue5://world.d.luau):
  `VehicleModel` ("adder", ...), `PedModel`, `WeaponName` ("WEAPON_PISTOL",
  ...). A vehicle's model parameter takes a `VehicleModel` or a hash, so a
  misspelt name is a type error; a name held in a plain `string` needs a
  cast (`name :: VehicleModel`).

## Tasks
- `task.spawn(f, ...)` runs `f` beside the main code; `task.delay(ms, f,
  ...)` later (game time); both may `wait()`. An error in a task is logged
  and the mod goes on.

## Menus
- `local m = ui.menu("Trainer", [note])` makes a menu the app shows with
  its own widgets (touch, keyboard and controllers): the Mods page's MENUS
  (F7, or on touch screens the button beside the settings button) lists
  each mod's menus; the Studio's MENUS panel shows them too. A mod with a
  menu keeps running after its main code returns, until it is stopped.
- Builders, all `(label, value..., [opts], [callback])`; `opts` is a table
  (`note`, `enabled`, `visible` and the kind's own) or a string (the note);
  a function in its place is the callback:
  `m:button(label, {action = "Go"}, onPress)`,
  `m:toggle(label, on, opts, onChange)`,
  `m:slider(label, value, min, max, {step, unit}, onChange)` (a number in
  the opts' place is the step; unit "%" shows value x 100),
  `m:number(label, value, {min, max, step, unit}, onChange)`,
  `m:choice(label, {options}, index or text, opts, onChange(index, option))`,
  `m:multi(label, {options}, {indices or texts}, opts, onChange(indices, options))`,
  `m:text(label, value, {placeholder, max}, onChange)` (on commit),
  `m:color(label, color, {alpha}, onChange)` (a Color: 0xRRGGBBAA,
  {r, g, b, a} or '#rrggbb'; `value` is {r, g, b, a}),
  `m:label(text)`, `m:separator([title])`, `m:progress(label, 0..1)`,
  `m:submenu(label, opts)` (a menu inside, at any depth).
- Each returns a handle with properties: `value`, `label`, `note`,
  `enabled`, `visible`, `options`, `onChange`/`onPress`, `min`/`max`/
  `step`/`unit`...; assigning checks like the builder, a misspelt field is
  an error; `:set(v)`, `:remove()`. Indices are from 1. `m:clear()`,
  `m:remove()`, `m:show()` (opens the Mods page at it, once in 2 s),
  `m:onOpen(f)`/`m:onClose(f)` (a player starts/stops looking at it).
- `value` follows the player at the mod's next turn, when the callback
  runs (as a task: it may call natives and wait); the mod's own
  assignments never call back, and win over a player's value not yet
  heard.
- Limits: 4096 menus and items and 65536 options a mod, all together;
  titles 40, labels 60, notes 200, options 40 characters (cut).
- `ui.notify(text)` shows a notification over the game.

## Input
- `input.isDown(key)`; `input.pressed(key)` and `input.released(key)`:
  since the mod's last turn (the last frame in a loop that `wait()`s).
  `input.onKey(function(key, down, mods) ... end)` runs for every key that
  goes down or up, as a task, in order (`mods.shift/ctrl/alt/meta`); it
  returns a handle with `:remove()`; a mod with one keeps running.
  `input.mousePosition()`: x, y as fractions of the screen.
- Keys are physical, named as on a US keyboard: "A".."Z", "0".."9",
  "F1".."F12", "Space", "Enter", "Escape", "LeftShift", "RightCtrl",
  "LeftAlt", "LeftMeta" (Cmd), "Up", "Keypad8"...; "MouseLeft",
  "MouseRight", "MouseWheelUp"; the player's pad by position: "PadSouth"
  (A / Cross), "PadEast", "PadWest", "PadNorth", "PadLeftShoulder",
  "PadRightTrigger", "PadStart", "PadUp"... (the `Key` type lists them).
- Mods get keys only when the game does (not behind the app's pages or
  while the game's text box takes text), and the game gets them too: to
  take over a control, call `pad.disableControlAction(0, control, true)`
  every frame. Touch screens have no keyboard: their touch controls are
  the pad, and a menu is how a mod reaches every device.

## World
- `world.vehicles()`, `world.peds()`, `world.objects()`: handles of the
  entities around. `world.vehicleModels()`, `world.pedModels()`,
  `world.weapons()`: the installed game's names.
- Natives that create entities are refused when the game's pool is nearly
  full; an entity of the wrong kind (a Ped where a Vehicle is wanted) is
  an error before the native runs.
- `memory.offsets`: this game build's field offsets (not PC GTA V's) for
  `memory.read*`/`write*` from `memory.entity(handle)`:
  `memory.readFloat(memory.entity(car) + memory.offsets.vehicle.rpm)`.
  Groups: entity, vehicle, wheel, handling, ped, pedIntelligence,
  playerInfo, weaponManager, damageRecords; the hovers say each field's
  type.
- `handling.get(target, "fInitialDriveForce")`, `handling.set(target,
  field, value)`, `handling.fields(target?)`: every handling.meta field by
  name; a target is a vehicle or a handling name ("ADDER"). A model's
  vehicles share one handling; a vehicle's gearbox is set up from it when
  the vehicle is made.

## Standard library
- Constants by the game's names without their prefix: `Control.JUMP`
  (INPUT_JUMP), `Bone.R_HAND` (BONETAG_R_HAND), `ExplosionType.GRENADE`,
  `PedConfigFlag.NoCriticalHits`, `HudComponent.CASH`, `VehicleSeat.DRIVER`,
  `Font.PRICEDOWN`.
- `streaming.load(asset, [kind], [ms])` loads a model ("model", the
  default), "anim", "animSet", "clipSet", "ptfx" or "texture", waiting;
  false if it is missing or late. `spawn.vehicle(model, [pos], [heading],
  [{warpIn}])`, `spawn.ped(model, [pos], [heading])`, `spawn.prop(model,
  [pos], [{dynamic, ground}])`: loaded, made 5 m ahead of the player by
  default, released; nil if the model did not load.
- `world.nearby(kind, around, radius)` and `world.closest(kind, around,
  [radius])`: kind "vehicles", "peds", "objects" or "pickups"; around a
  vector, or an entity (left out). `world.raycast(from, to, [{flags,
  ignore}])` waits a frame or two and returns {pos, normal, entity,
  material} or nil; `world.crosshair()`, `world.groundZ(pos)`,
  `world.waypoint()`, `world.setWaypoint(pos or nil)`, `world.zoneName(pos)`,
  `world.streetName(pos)`.
- The game's own text: `ui.notify(text, {icon = "CHAR_LESTER", title,
  subtitle})` (without options: the app's notification), `ui.subtitle(text,
  [ms])`, `ui.help(text)` each frame it stays, `ui.loading(text)` and
  `ui.loading()`, `ui.hideHud([component])` this frame, `ui.prompt([label],
  [default], [max])` (waits; nil if cancelled), `ui.buttons({{control =
  Control.JUMP, label = "Jump"}})` until changed, nil to hide.
- `local sf = scaleform.load(name)` (waits), `sf:call(method, ...)`, each
  frame `sf:draw()` or `sf:draw3d(pos, rot, scale)`, `sf:release()`.
- `task.every(ms, f)` returns a function that stops it. `mod.onStop(f)`: f
  runs once when the mod stops, on its script, within 2 ms, no waiting.
- `geo.direction(rot)`, `geo.rotation(dir)`, `geo.fromHeading(h)`,
  `geo.heading(dir)`, `geo.distance(a, b, [flat])`, `geo.quaternion(rot)`,
  `geo.fromQuaternion(x, y, z, w)`: the game's rotations, in degrees.

## Drawing
- Draw every frame (a loop with `wait()`): what a turn draws shows until
  the mod's next turn. Positions are fractions of the screen (0 to 1);
  rectangles and images are centred there (as DRAW_RECT's), a text starts
  there (its top); radii, widths and text sizes are fractions of its
  height.
- Calls take positional numbers and a colour, then a style made ONCE
  outside the loop: `local s = draw.style({ z = 1, rounding = 0.01 })`.
  Colours: `0xRRGGBBAA` (fastest; `draw.rgba(r, g, b, a)`), `{r, g, b, a}`
  or a hex string (#rrggbb). Change a style's field when needed (`s.rotation = a`);
  never build `draw.style{...}` or tables in the loop.
- `draw.image(img, x, y, w, h?, color?, style?)` (no h: its proportions),
  `draw.rect(x, y, w, h, color?, style?)`, `draw.text(text, x, y, size?,
  color?, style?)`, `draw.line(x1, y1, x2, y2, width?, color?, style?)`,
  `draw.path({x1, y1, ...}, width?, color?, style?)`, `draw.circle(x, y,
  r, color?, style?)`, `draw.arc(x, y, r, from, to, width?, color?,
  style?)` (degrees clockwise from the top: gauges).
- Style fields: `z`, `rotation`, `blend = "add"`, `rounding`, `stroke` +
  `width` (outlines), `gradient` + `horizontal` (rectangles), `fill` +
  `closed` (paths, convex), `align`, `wrap`, `font` (texts), `uv = {u0,
  v0, u1, v1}` (sprite sheets; `style:setUv(...)` per frame), `nineSlice
  = {l, t, r, b}` (panels), `scale` (groups), `distanceScale`,
  `maxDistance` (the world).
- `draw.loadImage("images/x.png")` (PNG, 2048 px, 64 images / 32 MB a
  mod) and `draw.loadFont("fonts/x.ttf")` (4 a mod) wait while they load:
  call them before the loop. `draw.measureText(text, size?, style?)`
  gives width, height.
- Groups: `draw.push(x, y, style?)` ... `draw.pop()` places and scales
  what is between; `draw.clip(x, y, w, h)` ... `draw.pop()` cuts it to a
  rectangle (scrolling lists).
- The world: `draw.text3d(text, pos, size?, color?, style?)`,
  `draw.image3d`, `draw.circle3d`, or `draw.push3d(pos, style?)` ...
  `draw.pop()`: placed with the frame's own camera (labels over cars).
- Over everything the game draws, at the window's resolution. For the
  game's own sprites and text, use the natives (`graphics.drawSprite`,
  `hud.beginTextCommandDisplayText`, ...).

## Data
- `json.encode(value, pretty?)` and `json.decode(text)`: tables with keys
  1..n (or none) are arrays, others objects; JSON's null is nil.
- `storage.get(key)`, `storage.set(key, value)` (nil removes),
  `storage.keys()`, `storage.clear()`: what the mod keeps between
  sessions, JSON values, 1 MB at most. Values are copies: change a table
  and `set` it again. Written within a couple of seconds and when the mod
  stops; kept for this player, not in the mod's folder.
- `db`: the mod's SQLite database, one file for this player (256 MB).
  `db.exec(sql, ...)` runs statements (returns changes, last rowid),
  `db.query(sql, ...)` returns rows as tables by column name,
  `db.transaction(f, ...)` keeps f's work if it returns, undoes it if it
  fails. Values as `?` one by one, or a table for `:name`. Queries run on
  the mod's turn: put many writes in one transaction.
- `net.request(url or {url, method, headers, body, query})`: HTTP and
  HTTPS as Lune's @lune/net; returns {ok, statusCode, statusMessage,
  headers, body} and waits like wait() meanwhile. No answer is an error.
  `net.socket(url)`: a WebSocket (ws/wss) as Lune's: `s:send(msg, binary?)`,
  `s:next()` (waits; nil once closed), `s:close(code?)`, `s.closeCode`.
  `net.urlEncode(s, binary?)`, `net.urlDecode(s, binary?)`; request
  `options = { decompress = false }` keeps a gzip body packed.
- `mod.settings(defaults)`: the defaults with the mod folder's
  settings.json over them (written with the defaults when missing, for
  the player to edit). `mod.read(path)` (nil when missing),
  `mod.write(path, data)`, `mod.list(dir?)`: files of the mod's own folder.
- Luau's own libraries are there (math, string, table, coroutine, bit32,
  utf8, buffer, vector, os.clock/time/date); other files are not.
"#;

/// The mods with their state and files.
pub fn mods() -> Value {
    Value::Array(
        crate::mods()
            .into_iter()
            .map(|m| {
                json!({
                    "name": m.name,
                    "state": m.state.name(),
                    "error": m.error,
                    "waiting": m.waiting,
                    "cpuMs": (m.cpu_ms * 100.0).round() / 100.0,
                    "memory": m.memory,
                    "autostart": m.info.autostart,
                    "title": m.info.title,
                    "description": m.info.description,
                    "author": m.info.author,
                    "version": m.info.version,
                    "restartOnSave": m.info.restart_on_save,
                    "files": files::list(&m.name),
                })
            })
            .collect(),
    )
}

/// An item as the Studio and MCP read it: `{id, kind, label, note,
/// enabled, visible}` and its kind's fields; indices from 1, colours as
/// `#rrggbb[aa]`.
fn item_json(i: &MenuItem) -> Value {
    let mut v = json!({
        "id": i.id,
        "kind": kind_name(&i.kind),
        "label": i.label,
        "note": i.note,
        "enabled": i.enabled,
        "visible": i.visible,
    });
    let extra = match &i.kind {
        MenuKind::Button { action } => json!({ "action": action.as_deref().unwrap_or("Run") }),
        MenuKind::Toggle(on) => json!({ "value": on }),
        MenuKind::Slider { value, min, max, step, unit } => {
            json!({ "value": value, "min": min, "max": max, "step": step, "unit": unit })
        }
        MenuKind::Number { value, min, max, step, unit } => {
            json!({ "value": value, "min": min, "max": max, "step": step, "unit": unit })
        }
        MenuKind::Choice { options, index } => json!({ "value": index + 1, "options": options.as_ref() }),
        MenuKind::Multi { options, selected } => {
            json!({ "value": selected.iter().map(|i| i + 1).collect::<Vec<_>>(), "options": options.as_ref() })
        }
        MenuKind::Text { value, placeholder, max } => json!({ "value": value, "placeholder": placeholder, "max": max }),
        MenuKind::Color { value, alpha } => json!({ "value": value.hex(*alpha), "alpha": alpha }),
        MenuKind::Label | MenuKind::Separator => json!({}),
        MenuKind::Progress(p) => json!({ "value": p }),
        MenuKind::Submenu(menu) => json!({ "menu": menu }),
    };
    v.as_object_mut().unwrap().extend(extra.as_object().unwrap().clone());
    v
}

fn menu_json(m: &Menu) -> Value {
    json!({
        "mod": m.module,
        "id": m.id,
        "parent": m.parent,
        "title": m.title,
        "note": m.note,
        "search": m.search,
        "rev": m.rev,
        "items": m.items.iter().map(item_json).collect::<Vec<_>>(),
    })
}

/// The mods' menus, all of them flat (top-level and submenus, in creation
/// order): `{mod, id, parent, title, note, search, rev, items}`; the tree is
/// `parent` and the submenu rows' `menu`.
pub fn menus() -> Value {
    Value::Array(crate::menus().iter().map(menu_json).collect())
}

/// Menu `menu` of mod `module` as [`menus`] gives it, with every menu
/// under it: `{menu, submenus: [...]}` (the submenus flat, in creation
/// order).
pub fn menu_open(module: &str, menu: u32) -> Result<Value, String> {
    let menus = crate::menus();
    let Some(top) = menus.iter().find(|m| m.id == menu && m.module == module) else {
        return Err(format!("{module} has no menu {menu}"));
    };
    let mut ids = vec![menu];
    // Creation order puts a submenu after its parent.
    let submenus: Vec<Value> = menus
        .iter()
        .filter(|m| {
            let under = m.parent.is_some_and(|p| ids.contains(&p));
            if under {
                ids.push(m.id);
            }
            under
        })
        .map(menu_json)
        .collect();
    Ok(json!({ "menu": menu_json(top), "submenus": submenus }))
}

/// An option of `options` by its 1-based index or its text.
fn option(options: &[String], v: &Value) -> Result<usize, String> {
    match v {
        Value::Number(n) => match n.as_u64() {
            Some(n) if (1..=options.len() as u64).contains(&n) => Ok(n as usize - 1),
            _ => Err(format!("no option {n}: there are {}", options.len())),
        },
        Value::String(o) => options.iter().position(|x| x == o).ok_or_else(|| format!("no option {o:?}")),
        other => Err(format!("an option is its number from 1 or its text, not {other}")),
    }
}

/// Does to a menu's item what a player would. `value`: a toggle true or
/// false (none flips it), a slider or number a number, a choice an
/// option's number from 1 or its text, a multi a list of those, a text a
/// string, a colour `"#rrggbb[aa]"` or `{r, g, b, a}`, a button none.
pub fn menu_input(module: &str, item: u32, value: &Value) -> Result<(), String> {
    let menus = crate::menus();
    let found = menus.iter().filter(|m| m.module == module).flat_map(|m| m.items.iter()).find(|i| i.id == item);
    let Some(found) = found else { return Err(format!("{module} has no menu item {item}")) };
    let input = match (&found.kind, value) {
        (MenuKind::Button { .. }, Value::Null) => MenuInput::Press,
        (MenuKind::Toggle(on), Value::Null) => MenuInput::Toggle(!on),
        (MenuKind::Toggle(_), Value::Bool(b)) => MenuInput::Toggle(*b),
        (MenuKind::Slider { .. } | MenuKind::Number { .. }, Value::Number(n)) => MenuInput::Number(n.as_f64().unwrap_or(f64::NAN)),
        (MenuKind::Choice { options, .. }, v @ (Value::Number(_) | Value::String(_))) => MenuInput::Choose(option(options, v)?),
        (MenuKind::Multi { options, .. }, Value::Array(list)) => {
            let picked = list.iter().map(|v| option(options, v).map(|i| i as u16)).collect::<Result<Vec<_>, _>>()?;
            MenuInput::Select(picked)
        }
        (MenuKind::Text { .. }, Value::String(t)) => MenuInput::Text(t.clone()),
        (MenuKind::Color { .. }, Value::String(hex)) => {
            MenuInput::Color(Rgba::from_hex(hex).ok_or_else(|| format!("{hex:?} is not a colour (#rrggbb or #rrggbbaa)"))?)
        }
        (MenuKind::Color { .. }, Value::Object(c)) => {
            let part = |k: &str, default: Option<u8>| match c.get(k) {
                None if default.is_some() => Ok(default.unwrap_or(255)),
                Some(Value::Number(n)) if n.as_u64().is_some_and(|n| n <= 255) => Ok(n.as_u64().unwrap_or(0) as u8),
                _ => Err(format!("a colour's {k} is a whole number from 0 to 255")),
            };
            MenuInput::Color(Rgba { r: part("r", None)?, g: part("g", None)?, b: part("b", None)?, a: part("a", Some(255))? })
        }
        (kind @ (MenuKind::Label | MenuKind::Separator | MenuKind::Progress(_) | MenuKind::Submenu(_)), _) => {
            return Err(format!("a {} takes no input", kind_name(kind)));
        }
        (kind, v) => {
            let takes = match kind {
                MenuKind::Button { .. } => "no value",
                MenuKind::Toggle(_) => "true or false (or none: it flips)",
                MenuKind::Slider { .. } | MenuKind::Number { .. } => "a number",
                MenuKind::Choice { .. } => "an option's number from 1 or its text",
                MenuKind::Multi { .. } => "a list of options' numbers from 1 or texts",
                MenuKind::Text { .. } => "a string",
                _ => "a colour: \"#rrggbb\", \"#rrggbbaa\" or {r, g, b, a}",
            };
            return Err(format!("a {} takes {takes}, not {v}", kind_name(kind)));
        }
    };
    crate::menu_input(module, item, input)
}

/// What mod.json may hold, and of which JSON type.
const MOD_JSON_KEYS: &[(&str, &str)] = &[
    ("name", "text"),
    ("description", "text"),
    ("author", "text"),
    ("version", "text"),
    ("autostart", "true or false"),
    ("restartOnSave", "true or false"),
];

/// Diagnostics of a mod.json (`text`, or as saved): JSON, an object, the
/// keys it knows with values of their type.
fn mod_json(path: &str, text: Option<&str>) -> Value {
    let text = match text {
        Some(t) => t.to_owned(),
        None => files::read(path).unwrap_or_default(),
    };
    let diag = |line: usize, col: usize, len: usize, severity: &str, message: String| {
        json!({ "range": [line, col, line, col + len], "severity": severity, "code": "mod.json", "message": message })
    };
    // Where `"key"` is (0-based line and column).
    let at = |key: &str| {
        let quoted = format!("\"{key}\"");
        text.lines().enumerate().find_map(|(i, l)| l.find(&quoted).map(|c| (i, c, quoted.len()))).unwrap_or((0, 0, 1))
    };
    let v: Value = match serde_json::from_str(&text) {
        Ok(v) => v,
        Err(e) => {
            let (line, col) = (e.line().saturating_sub(1), e.column().saturating_sub(1));
            return json!([diag(line, col, 1, "error", format!("not JSON: {e}"))]);
        }
    };
    let Some(map) = v.as_object() else {
        return json!([diag(0, 0, 1, "error", "mod.json is an object: {\"autostart\": true}".into())]);
    };
    let mut out = Vec::new();
    for (key, value) in map {
        let (line, col, len) = at(key);
        match MOD_JSON_KEYS.iter().find(|(k, _)| k == key) {
            Some((_, "true or false")) if !value.is_boolean() => {
                out.push(diag(line, col, len, "error", format!("{key} is true or false")));
            }
            Some((_, "text")) if !value.is_string() => {
                out.push(diag(line, col, len, "error", format!("{key} is text, in quotes")));
            }
            Some(_) => {}
            None => {
                let known: Vec<&str> = MOD_JSON_KEYS.iter().map(|(k, _)| *k).collect();
                out.push(diag(line, col, len, "warning", format!("unknown key {key}: mod.json knows {}", known.join(", "))));
            }
        }
    }
    Value::Array(out)
}

/// Diagnostics of `path` as the checker sees it; `text` replaces the file
/// on disk (an editor's unsaved text). The mod's other files are read from
/// disk when the checker has not got them.
pub fn diagnostics(path: &str, text: Option<&str>) -> Value {
    let module = path.trim_start_matches('/');
    if module.ends_with("/mod.json") {
        return mod_json(module, text);
    }
    if !module.ends_with(".luau") {
        // Other files are the mod's own business.
        return json!([]);
    }
    let result = Checker::with(|c| {
        match text {
            Some(t) => c.set(module, Some(t)),
            // As saved.
            None => c.set(module, files::read(module).ok().as_deref()),
        }
        sync_mod(c, module);
        c.check(module)
    });
    let mut found =
        result.unwrap_or_else(|e| json!([{ "range": [0, 0, 0, 0], "severity": "error", "code": "checker", "message": e }]));
    if let Some(list) = found.as_array_mut() {
        for d in list {
            let message = d["message"].as_str().unwrap_or("");
            d["message"] = name_message(message).unwrap_or_else(|| crate::world::shorten(message)).into();
        }
    }
    found
}

/// A misspelt name against the catalog's union (hundreds of names in the
/// checker's message): which kind of name, and the closest one.
fn name_message(message: &str) -> Option<String> {
    let expected = message.strip_prefix("Expected this to be")?.trim_start().trim_start_matches('\'');
    if !expected.starts_with('"') {
        return None;
    }
    let got = message.rsplit("but got '\"").next()?.split('"').next()?;
    let c = crate::world::catalog()?;
    let (kind, names) = if expected.starts_with("\"WEAPON_") {
        ("a weapon", &c.weapons)
    } else if c.vehicles.first().is_some_and(|v| expected.starts_with(&format!("\"{v}\""))) {
        ("a vehicle model", &c.vehicles)
    } else if c.peds.first().is_some_and(|p| expected.starts_with(&format!("\"{p}\""))) {
        ("a ped model", &c.peds)
    } else {
        return None;
    };
    let closest = names.iter().min_by_key(|n| distance(&n.to_ascii_lowercase(), &got.to_ascii_lowercase()))?;
    Some(format!("\"{got}\" is not {kind} of the installed game; did you mean \"{closest}\"?"))
}

/// Levenshtein distance.
fn distance(a: &str, b: &str) -> usize {
    let b: Vec<char> = b.chars().collect();
    let mut row: Vec<usize> = (0..=b.len()).collect();
    for (i, ca) in a.chars().enumerate() {
        let mut prev = row[0];
        row[0] = i + 1;
        for (j, cb) in b.iter().enumerate() {
            let cur = row[j + 1];
            row[j + 1] = (prev + usize::from(ca != *cb)).min(row[j] + 1).min(cur + 1);
            prev = cur;
        }
    }
    row[b.len()]
}

/// Puts the files of `module`'s mod the checker has not got, from disk.
pub(crate) fn sync_mod(c: &mut Checker, module: &str) {
    let Some((name, _)) = module.split_once('/') else { return };
    for file in files::list(name) {
        if file.ends_with(".luau")
            && !c.has(&file)
            && let Ok(text) = files::read(&file)
        {
            c.set(&file, Some(&text));
        }
    }
}

/// Signatures by `namespace.name`, from the definitions.
fn signatures() -> &'static HashMap<String, String> {
    static SIGS: OnceLock<HashMap<String, String>> = OnceLock::new();
    SIGS.get_or_init(|| {
        let mut map = HashMap::new();
        let mut space = "";
        for line in natives::DEFINITIONS.lines() {
            if let Some(rest) = line.strip_prefix("declare ") {
                space = rest.split(':').next().unwrap_or("");
            } else if let Some(entry) = line.strip_prefix('\t')
                && let Some((name, sig)) = entry.split_once(": ")
            {
                map.insert(format!("{space}.{name}"), format!("{space}.{name}{}", sig.trim_end_matches(',')));
            }
        }
        map
    })
}

/// The natives whose names hold every word of `query` (case aside), best
/// first: name then notes.
pub fn search(query: &str, limit: usize) -> Vec<Value> {
    let words: Vec<String> = query.split_whitespace().map(|w| w.to_ascii_lowercase().replace('_', "")).collect();
    let mut hits: Vec<(u8, &natives::Native)> = Vec::new();
    for n in natives::NATIVES {
        let name = format!("{}.{}", natives::NAMESPACES[n.ns as usize], n.lua).to_ascii_lowercase();
        let doc = n.doc.to_ascii_lowercase();
        let score = if words.iter().all(|w| name.contains(w.as_str())) {
            0
        } else if words.iter().all(|w| name.contains(w.as_str()) || doc.contains(w.as_str())) {
            1
        } else {
            continue;
        };
        hits.push((score, n));
    }
    hits.sort_by_key(|(s, n)| (*s, n.lua.len()));
    hits.into_iter().take(limit).map(|(_, n)| describe(n, false)).collect()
}

/// A native by `namespace.name` or the game's `NAME`.
pub fn find(name: &str) -> Option<&'static natives::Native> {
    if let Some((space, lua)) = name.split_once('.') {
        let ns = natives::NAMESPACES.iter().position(|s| *s == space)?;
        return natives::find(ns, lua).map(|i| &natives::NATIVES[i]);
    }
    natives::NATIVES.iter().find(|n| n.name.eq_ignore_ascii_case(name))
}

/// A native's signature, game name and (with `doc`) notes.
pub fn describe(n: &natives::Native, doc: bool) -> Value {
    let key = format!("{}.{}", natives::NAMESPACES[n.ns as usize], n.lua);
    let mut v = json!({
        "name": key,
        "native": n.name,
        "signature": signatures().get(&key).cloned().unwrap_or_else(|| key.clone()),
    });
    if doc && !n.doc.is_empty() {
        v["notes"] = n.doc.into();
    }
    v
}

/// The notes on a documentation symbol, as Markdown.
pub fn documentation(symbol: &str) -> Option<String> {
    checker::documentation(symbol)
}

/// Runs `code` in the game and waits for the result (`timeout` at most).
pub fn eval(code: &str, timeout: Duration) -> Result<String, String> {
    let (tx, rx) = std::sync::mpsc::channel();
    crate::eval(code, tx);
    match rx.recv_timeout(timeout) {
        Ok(r) => r,
        Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
            Err(format!("no result within {} s (is the game paused or loading?)", timeout.as_secs()))
        }
        Err(_) => Err("the evaluation stopped (an error ended it, or it ran too long without wait())".into()),
    }
}

/// Log lines as JSON, from `since` (and only mod `module`'s).
pub fn logs(since: u64, module: Option<&str>) -> (Vec<Value>, u64) {
    let (lines, next) = crate::logs(since);
    let lines = lines
        .into_iter()
        .filter(|l| module.is_none_or(|m| m == l.module))
        .map(|l| {
            let ms = l.time.duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis() as u64);
            json!({
                "seq": l.seq,
                "time": ms,
                "mod": l.module,
                "level": if l.level == crate::LogLevel::Error { "error" } else { "info" },
                "text": l.text,
            })
        })
        .collect();
    (lines, next)
}
