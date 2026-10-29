--!strict
-- Going places: the waypoint (now or whenever one is set), the mission's objective, the map's
-- icons, the places of data/places.luau, interiors (data/interiors.luau), the player's own
-- saved spots, coordinates, the nearest vehicle.

local bind = require("../core/bind")
local game = require("../core/game")
local places = require("../data/places")
local interiors = require("../data/interiors")
local teleports = require("../data/teleports")
local blips = require("../data/blips")

type Spot = { name: string, x: number, y: number, z: number, heading: number }

local M = {}

local function toWaypoint(): string?
	local at = world.waypoint()
	if not at then
		return "No waypoint on the map"
	end
	game.teleport(at, true)
	return nil
end

-- Where the player is, named after the street and the area.
local function here(name: string?): Spot
	local pos = game.position()
	if not name or name == "" then
		local street = world.streetName(pos)
		local area = world.zoneName(pos)
		name = if street ~= "" then `{street}, {area}` else area
	end
	return { name = name :: string, x = pos.x, y = pos.y, z = pos.z, heading = entity.getEntityHeading(game.mover()) }
end

-- The player's spots, kept in the mod's storage.
local function saved(): { Spot }
	return storage.get("spots") or {}
end

local function spots(root: Menu)
	local menu = root:submenu("Saved spots", "Yours: save where you are, come back later")
	local function build()
		menu:clear()
		local list = saved()
		bind.action(menu, "Save here", { note = "Named after the street", action = "Save" }, function(): string?
			local s = saved()
			table.insert(s, here())
			storage.set("spots", s)
			build()
			return `Saved: {s[#s].name}`
		end)
		menu:text("Save here as", "", { placeholder = "A name, then Enter", max = 40 }, bind.safe("Save here as", function(name)
			if name == "" then
				return
			end
			local s = saved()
			table.insert(s, here(name))
			storage.set("spots", s)
			build()
			game.notify(`Saved: {name}`)
		end))
		if #list == 0 then
			menu:label("Nothing saved yet.")
			return
		end
		menu:separator("Go")
		for _, spot in list do
			bind.action(menu, spot.name, { note = world.zoneName(vector.create(spot.x, spot.y, spot.z)), action = "Go" }, function(): string?
				game.teleport(vector.create(spot.x, spot.y, spot.z), false, spot.heading)
				return nil
			end)
		end
		local forget = menu:submenu("Delete a spot")
		for i, spot in list do
			bind.action(forget, spot.name, { action = "Delete" }, function(): string?
				local s = saved()
				table.remove(s, i)
				storage.set("spots", s)
				build()
				return `Deleted: {spot.name}`
			end)
		end
	end
	build()
end

-- Mission objectives are standard blips (sprite 1) in the game's yellows.
local OBJECTIVE_SPRITE = 1
local OBJECTIVE_COLOURS = { 5, 46, 60, 66 }

-- Where the mission wants the player: a yellow blip, else any standard one.
local function objective(): vector?
	local any: vector? = nil
	local blip = hud.getFirstBlipInfoId(OBJECTIVE_SPRITE)
	while blip and hud.doesBlipExist(blip) do
		local at = hud.getBlipInfoIdCoord(blip)
		if table.find(OBJECTIVE_COLOURS, hud.getBlipColour(blip)) then
			return at
		end
		any = any or at
		blip = hud.getNextBlipInfoId(OBJECTIVE_SPRITE)
	end
	return any
end

local function coordinates(root: Menu)
	local menu = root:submenu("Coordinates", "Anywhere, by its numbers")
	local x = menu:number("X", 0, { step = 10 })
	local y = menu:number("Y", 0, { step = 10 })
	local z = menu:number("Z", 0, { step = 5, note = "0: on the ground" })
	bind.action(menu, "Go there", { action = "Go" }, function(): string?
		game.teleport(vector.create(x.value, y.value, z.value), z.value == 0)
		return nil
	end)
	bind.action(menu, "Where you are", { note = "These numbers, from where you stand", action = "Fill" }, function(): string?
		local pos = game.position()
		x.value = math.floor(pos.x * 10 + 0.5) / 10
		y.value = math.floor(pos.y * 10 + 0.5) / 10
		z.value = math.floor(pos.z * 10 + 0.5) / 10
		return nil
	end)
end

-- To a place of the lists: its IPLs loaded (or removed) and its interior enabled first.
local function go(place: teleports.Place): string?
	local o = place.options
	if o and o.online and not network.networkIsSessionStarted() then
		return "That place is on GTA Online's map only"
	end
	local unload = o and o.unload
	if unload then
		for _, ipl in unload do
			if streaming.isIplActive(ipl) then
				streaming.removeIpl(ipl)
			end
		end
	end
	local load = o and o.load
	if load then
		for _, ipl in load do
			if not streaming.isIplActive(ipl) then
				streaming.requestIpl(ipl)
			end
		end
	end
	local at = vector.create(place.x, place.y, place.z)
	if o and o.interior then
		local room = interior.getInteriorAtCoords(at)
		if room and interior.isValidInterior(room) and interior.isInteriorDisabled(room) then
			interior.pinInteriorInMemory(room)
			streaming.setInteriorActive(room :: any, true)
			interior.disableInterior(room, false)
		end
	end
	game.teleport(at, false, place.heading)
	return nil
end

-- To an interior: what it needs loaded first, then there.
local function interiorsMenu(root: Menu)
	local menu = root:submenu("Interiors", "Rooms and places the game keeps hidden until a mission")
	for _, room in interiors do
		bind.action(menu, room.name, { action = "Go" }, function(): string?
			for _, ipl in room.ipls do
				if not streaming.isIplActive(ipl) then
					streaming.requestIpl(ipl)
				end
			end
			game.teleport(vector.create(room.x, room.y, room.z), false, room.heading)
			return nil
		end)
	end
end

-- The map's icons there are now, by kind (sprite), to the nearest of each kind.
local function icons(root: Menu)
	local menu = root:submenu("Map icons", "The nearest of any icon on the map")
	local function build()
		menu:clear()
		local found = 0
		for sprite = 1, 600 do
			local first = hud.getFirstBlipInfoId(sprite)
			if first and hud.doesBlipExist(first) then
				local count = 0
				local b: Blip? = first
				while b and hud.doesBlipExist(b) do
					count += 1
					b = hud.getNextBlipInfoId(sprite)
				end
				found += 1
				local name = blips[sprite] or `Icon {sprite}`
				bind.action(menu, name, { note = `{count} on the map`, action = "Go" }, function(): string?
					local nearest = hud.getClosestBlipInfoId(sprite)
					if not nearest or not hud.doesBlipExist(nearest) then
						return "It is no longer on the map"
					end
					game.teleport(hud.getBlipInfoIdCoord(nearest), true)
					return nil
				end)
			end
		end
		if found == 0 then
			menu:label("No icons on the map now.")
		end
	end
	menu:onOpen(bind.safe("Map icons", build))
end

function M.build(root: Menu)
	local menu = root:submenu("Teleport", "Waypoint, objective, map icons, places, interiors, your saved spots")
	bind.command(menu, "teleport.waypoint", "To the waypoint", { note = "The map's marker", action = "Go" }, toWaypoint)
	bind.command(menu, "teleport.objective", "To the objective", { note = "Where the mission wants you", action = "Go" }, function(): string?
		local at = objective()
		if not at then
			return "No objective on the map"
		end
		game.teleport(at, true)
		return nil
	end)
	-- A new waypoint takes you there (a teleport waits for the ground: a task of its own).
	local seen: vector? = nil
	bind.feature(menu, "teleport.auto", "Go to every new waypoint", "Mark the map, you are there", {
		on = function()
			seen = world.waypoint()
		end,
		tick = function()
			local at = world.waypoint()
			if at and (not seen or vector.magnitude(at - (seen :: vector)) > 1) then
				task.spawn(game.teleport, at, true)
			end
			seen = at
		end,
	})
	bind.command(menu, "teleport.forward", "Forward", { note = "10 m ahead, through walls", action = "Go" }, function(): string?
		local e = game.mover()
		local ahead = entity.getEntityCoords(e, true) + entity.getEntityForwardVector(e) * 10
		game.teleport(ahead, false)
		return nil
	end)
	bind.command(menu, "teleport.skydive", "Skydive to the waypoint", { note = "High above it, with a parachute", action = "Jump" }, function(): string?
		local at = world.waypoint()
		if not at then
			return "No waypoint on the map"
		end
		weapon.giveWeaponToPed(game.me(), joaat("GADGET_PARACHUTE"), 1, false, false)
		game.teleport(vector.create(at.x, at.y, 800), false)
		return nil
	end)
	local steps: { { name: string, by: vector } } = {
		{ name = "Up", by = vector.create(0, 0, 10) },
		{ name = "Down", by = vector.create(0, 0, -10) },
		{ name = "Back", by = vector.create(0, -10, 0) },
	}
	for _, step in steps do
		bind.command(menu, `teleport.{string.lower(step.name)}`, step.name, { note = "10 m, through walls", action = "Go" }, function(): string?
			game.teleportNow(entity.getOffsetFromEntityInWorldCoords(game.mover(), step.by), false)
			return nil
		end)
	end
	bind.command(menu, "teleport.nearestVehicle", "Into the nearest vehicle", { note = "A free seat within 50 m", action = "Get in" }, function(): string?
		local me = game.me()
		for _, car in world.nearby("vehicles", me, 50) do
			for seat = -1, vehicle.getVehicleMaxNumberOfPassengers(car) - 1 do
				if vehicle.isVehicleSeatFree(car, seat, false) then
					ped.setPedIntoVehicle(me, car, seat)
					return nil
				end
			end
		end
		return "No free seat nearby"
	end)
	spots(menu)
	coordinates(menu)
	icons(menu)
	interiorsMenu(menu)
	menu:separator("Places")
	-- The trainer's own places first, then the published lists' (data/teleports.luau), the
	-- kinds both have together.
	local kinds: { { name: string, places: { teleports.Place } } } = {}
	local byName: { [string]: { teleports.Place } } = {}
	local function add(name: string, list: { teleports.Place })
		if not byName[name] then
			byName[name] = {}
			table.insert(kinds, { name = name, places = byName[name] })
		end
		for _, place in list do
			table.insert(byName[name], place)
		end
	end
	for _, kind in places do
		add(kind.name, kind.places :: any)
	end
	for _, kind in teleports do
		add(kind.name, kind.places)
	end
	for _, kind in kinds do
		local sub = menu:submenu(kind.name, { note = `{#kind.places} places`, search = #kind.places > 20 })
		for _, place in kind.places do
			bind.action(sub, place.name, { action = "Go" }, function(): string?
				return go(place)
			end)
		end
	end
end

return M
