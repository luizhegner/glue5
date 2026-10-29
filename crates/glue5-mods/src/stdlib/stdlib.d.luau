-- The mods' standard library (stdlib/*.luau): its additions to the tables declared before.

-- Seats for ped.setPedIntoVehicle and the vehicle natives.
declare VehicleSeat: { ANY_FREE: number, DRIVER: number, PASSENGER: number, LEFT_REAR: number, RIGHT_REAR: number }
-- Fonts for hud.setTextFont.
declare Font: { CHALET_LONDON: number, HOUSE_SCRIPT: number, MONOSPACE: number, CHALET_COMPRIME_COLOGNE: number, PRICEDOWN: number }

-- Rotations as the game has them, in degrees (x pitch, y roll, z yaw). Luau's `vector` library has
-- lerp, angle, dot, cross, normalize.
declare geo: {
	-- Where a rotation points (a unit vector).
	direction: (rot: vector) -> vector,
	-- The rotation (no roll) that points along `dir`.
	rotation: (dir: vector) -> vector,
	-- The direction of a heading, on the ground; and the heading of a direction (0 north, 90 west).
	fromHeading: (heading: number) -> vector,
	heading: (dir: vector) -> number,
	-- The distance between two places (on the ground with `flat`).
	distance: (a: vector, b: vector, flat: boolean?) -> number,
	-- A rotation's quaternion, x, y, z, w (entity.setEntityQuaternion); and back.
	quaternion: (rot: vector) -> (number, number, number, number),
	fromQuaternion: (x: number, y: number, z: number, w: number) -> vector,
}

declare task: typeof(task) & {
	-- Runs `f` every `ms` milliseconds of game time, as a task; the function returned stops it.
	every: <A...>(ms: number, f: (A...) -> ...any, A...) -> () -> (),
}

declare mod: typeof(mod) & {
	-- Runs `f` when the mod stops (stopped, restarted, failed or done), on its script, in one last turn
	-- of 2 ms at most: put back what it changed, delete what it made. It cannot wait.
	onStop: (f: () -> ()) -> (),
}

export type AssetKind = "model" | "anim" | "animSet" | "clipSet" | "ptfx" | "texture"

declare streaming: typeof(streaming) & {
	-- Loads an asset (a model by default: its name or hash), waiting until it is (10 s at most):
	-- false if it is not in the game or did not load in time.
	load: (asset: string | number, kind: AssetKind?, timeoutMs: number?) -> boolean,
}

-- Entities made where the player is (5 m ahead, facing as they do) or at `pos`; their model is
-- loaded (waiting) and released. nil if the model did not load.
declare spawn: {
	vehicle: (model: VehicleModel | Hash, pos: vector?, heading: number?, options: { warpIn: boolean? }?) -> Vehicle?,
	ped: (model: PedModel | Hash, pos: vector?, heading: number?) -> Ped?,
	-- `dynamic`: physics moves it; `ground`: placed on the ground.
	prop: (model: string | Hash, pos: vector?, options: { dynamic: boolean?, ground: boolean? }?) -> Object?,
}

-- What a ray hit.
export type RaycastHit = { pos: vector, normal: vector, entity: Entity?, material: Hash }
export type RaycastOptions = {
	-- What the ray stops at (shape test flags: 1 the world, 2 vehicles, 4 peds, 16 objects...); all by default.
	flags: number?,
	-- An entity the ray goes through (a ped, a vehicle...).
	ignore: any,
}

declare world: typeof(world) & {
	-- The entities of a kind within `radius` of a place or an entity (left out), nearest first.
	nearby: ((kind: "vehicles", around: vector | Entity, radius: number) -> { Vehicle })
		& ((kind: "peds", around: vector | Entity, radius: number) -> { Ped })
		& ((kind: "objects" | "pickups", around: vector | Entity, radius: number) -> { Object }),
	-- The nearest one, within `radius` if given.
	closest: ((kind: "vehicles", around: vector | Entity, radius: number?) -> Vehicle?)
		& ((kind: "peds", around: vector | Entity, radius: number?) -> Ped?)
		& ((kind: "objects" | "pickups", around: vector | Entity, radius: number?) -> Object?),
	-- What a ray from `from` to `to` hits first, nil if nothing: waits for the game (a frame or two).
	raycast: (from: vector, to: vector, options: RaycastOptions?) -> RaycastHit?,
	-- What the camera looks at, `distance` (1000 m) at most, through the player's ped: waits as raycast.
	crosshair: (options: { distance: number?, flags: number?, ignore: any }?) -> RaycastHit?,
	-- The height of the ground below `pos` (where the game has it loaded), nil if none.
	groundZ: (pos: vector) -> number?,
	-- The map's waypoint (its height may be 0), nil if none; setWaypoint(nil) removes it.
	waypoint: () -> vector?,
	setWaypoint: (pos: vector?) -> (),
	-- The zone's name, as the game shows it, and its code ("AIRP").
	zoneName: (pos: vector) -> (string, string),
	-- The street's name, and the crossing's if at one.
	streetName: (pos: vector) -> (string, string?),
}

-- A scaleform movie (scaleform.load).
export type Scaleform = {
	handle: number,
	name: string,
	-- Calls a method of the movie with numbers (whole ones as ints), booleans and strings.
	call: (self: Scaleform, method: string, ...(number | boolean | string)) -> (),
	-- Draws it this frame: over the whole screen, or centred at x, y (fractions of the screen) in width × height.
	draw: (self: Scaleform, x: number?, y: number?, width: number?, height: number?) -> (),
	-- Draws it this frame in the world.
	draw3d: (self: Scaleform, pos: vector, rot: vector, scale: vector?) -> (),
	release: (self: Scaleform) -> (),
}

declare scaleform: {
	-- The movie, once loaded (waits; nil if it does not load in `timeoutMs`, 10 s).
	load: (name: string, timeoutMs: number?) -> Scaleform?,
}

declare ui: typeof(ui) & {
	-- With options, a message in the game's feed: `icon` a picture's texture dictionary ("CHAR_LESTER"),
	-- `title` and `subtitle` beside it, `blink`.
	notify: (text: string, options: { icon: string?, title: string?, subtitle: string?, blink: boolean? }?) -> (),
	-- A subtitle at the bottom, for `ms` milliseconds (2.5 s).
	subtitle: (text: string, ms: number?) -> (),
	-- The help box (`~INPUT_CONTEXT~` shows a button); called each frame it stays; nil clears it.
	help: (text: string?, beep: boolean?) -> (),
	-- The spinner at the bottom right, with `text`; nil or false stops it.
	loading: (text: (string | false)?) -> (),
	-- Hides a HUD component (HudComponent) this frame; nil: the HUD and the radar.
	hideHud: (component: number?) -> (),
	-- The game's keyboard: what the player typed, nil if cancelled (waits). `title` is a text label
	-- of the game's ("FMMC_KEY_TIP8"), `max` bytes (60).
	prompt: (title: string?, default: string?, max: number?) -> string?,
	-- Instructional buttons at the bottom right until changed: { { control = Control.JUMP, label = "Jump" }, ... }; nil hides them.
	buttons: (buttons: { { control: number, label: string } }?) -> (),
}
