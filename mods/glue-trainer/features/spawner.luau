--!strict
-- Vehicles of the game, by class or by search, delivered with the options the player keeps;
-- and the garage: vehicles saved with their parts and colours, delivered again as they were.
-- In GTA Online they are networked: every player sees them (see spawnAt).

local bind = require("../core/bind")
local game = require("../core/game")
local catalog = require("../core/catalog")
local lists = require("../core/lists")
local customs = require("./customs")

local M = {}

-- The options, read when spawning.
type Options = { warpIn: Toggle, replace: Toggle, maxed: Toggle, god: Toggle, plate: Text }

type Saved = { name: string, look: customs.Look }

local options: Options

-- The vehicle the trainer spawned last (replaced by the next, when asked).
local last: Vehicle? = nil

-- A vehicle of model `hash` at `pos`, facing `heading`, engine running; nil and why if it could
-- not. In GTA Online it is networked (mods run as the session's script there), so every player
-- sees it, and the player in it.
function M.spawnAt(hash: number, pos: vector, heading: number?): (Vehicle?, string?)
	if not streaming.load(hash) then
		return nil, "That vehicle did not load"
	end
	local online = network.networkIsSessionStarted()
	local car = vehicle.createVehicle(hash, pos, heading or 0, online, false, false)
	streaming.setModelAsNoLongerNeeded(hash)
	if not car then
		return nil, if online then "The game made none (in a job, only its own vehicles)" else "The game made none"
	end
	if online then
		network.setNetworkIdExistsOnAllMachines(network.vehToNet(car), true)
	end
	if not (vehicle.isThisModelAHeli(hash) or vehicle.isThisModelAPlane(hash)) then
		vehicle.setVehicleOnGroundProperly(car, 5)
	end
	vehicle.setVehicleEngineOn(car, true, true, false)
	return car, nil
end

-- A vehicle of model `hash` where the player is, with the options.
function M.spawn(hash: number, name: string): (Vehicle?, string?)
	local me = game.me()
	if options.replace.value and last and entity.doesEntityExist(last) then
		local old = last :: Vehicle
		if ped.isPedInVehicle(me, old, false) then
			-- Out of it first: the new one takes its place.
			entity.setEntityCoordsNoOffset(me, entity.getEntityCoords(old, true), false, false, true)
		end
		entity.setEntityAsMissionEntity(old, true, true)
		vehicle.deleteVehicle(old)
	end
	-- Where the player stands when they get in (ahead of a vehicle they are still in), else
	-- ahead of them; aircraft they get in, in the air.
	local from = game.mover()
	local inside = options.warpIn.value
	local pos = entity.getEntityCoords(from, true)
	if not inside or game.vehicle() then
		pos += entity.getEntityForwardVector(from) * 8
	end
	local flying = vehicle.isThisModelAHeli(hash) or vehicle.isThisModelAPlane(hash)
	if flying and inside then
		pos += vector.create(0, 0, 100)
	end
	local car, why = M.spawnAt(hash, pos, entity.getEntityHeading(from))
	if not car then
		return nil, `{name}: {why}`
	end
	last = car
	if inside then
		ped.setPedIntoVehicle(me, car, VehicleSeat.DRIVER)
		if flying then
			vehicle.setHeliBladesFullSpeed(car)
			vehicle.setVehicleForwardSpeed(car, 40)
		end
	end
	if options.maxed.value then
		customs.maxUpgrade(car)
	end
	if options.god.value then
		entity.setEntityInvincible(car, true)
		vehicle.setVehicleCanBeVisiblyDamaged(car, false)
	end
	if options.plate.value ~= "" then
		vehicle.setVehicleNumberPlateText(car, options.plate.value)
	end
	return car, nil
end

local function garage(root: Menu)
	local menu = root:submenu("Your garage", "Vehicles saved with their parts and colours")
	local function saved(): { Saved }
		return storage.get("garage") or {}
	end
	local function build()
		menu:clear()
		bind.action(menu, "Save the vehicle you are in", { note = "Parts, colours, plate", action = "Save" }, function(): string?
			local car = game.vehicle()
			if not car then
				return "You are not in a vehicle"
			end
			local look = customs.look(car)
			local name = game.label(vehicle.getDisplayNameFromVehicleModel(look.model))
			local list = saved()
			table.insert(list, { name = if look.plate ~= "" then `{name} ({look.plate})` else name, look = look })
			storage.set("garage", list)
			build()
			return `Saved: {name}`
		end)
		local list = saved()
		if #list == 0 then
			menu:label("Nothing saved yet.")
			return
		end
		menu:separator("Deliver")
		for _, s in list do
			bind.action(menu, s.name, { action = "Deliver" }, function(): string?
				local car, why = M.spawn(s.look.model, s.name)
				if car then
					customs.apply(car, s.look)
				end
				return why
			end)
		end
		local forget = menu:submenu("Delete a vehicle")
		for i, s in list do
			bind.action(forget, s.name, { action = "Delete" }, function(): string?
				local l = saved()
				table.remove(l, i)
				storage.set("garage", l)
				build()
				return `Deleted: {s.name}`
			end)
		end
	end
	build()
end

-- Every vehicle of the game in `menu`, by search and by the game's classes (added once the
-- catalog is read); choosing one runs `pick` (its message said, as an action's).
function M.picker(menu: Menu, verb: string, pick: (entry: catalog.VehicleEntry) -> string?)
	local function fill(target: Menu, entries: { catalog.VehicleEntry }): boolean
		for _, entry in entries do
			bind.action(target, entry.name, { note = entry.model, action = verb }, function(): string?
				return pick(entry)
			end)
		end
		return #entries > 0
	end
	lists.lazy(menu, "Search all", "Every vehicle, by name", function(target)
		return fill(target, catalog.vehicles())
	end)
	local listed = false
	menu:onOpen(bind.safe(menu.title, function()
		if listed then
			return
		end
		local classes = catalog.vehicleClasses()
		for _, class in classes do
			lists.lazy(menu, class.name, `{#class.vehicles} vehicles`, function(target)
				return fill(target, class.vehicles)
			end)
		end
		listed = #classes > 0
	end))
end

function M.build(root: Menu)
	local menu = root:submenu("Spawn a vehicle", "Every vehicle of the game, by class or by name; your garage")
	local optionsMenu = menu:submenu("Options", "How vehicles are delivered")
	options = {
		warpIn = (bind.feature(optionsMenu, "spawn.warpIn", "Get in", "In the driver's seat; aircraft in the air", {})),
		replace = (bind.feature(optionsMenu, "spawn.replace", "Replace the last one", "The vehicle spawned before goes", {})),
		maxed = (bind.feature(optionsMenu, "spawn.maxed", "Fully upgraded", "Every upgrade the vehicle has", {})),
		god = (bind.feature(optionsMenu, "spawn.god", "Indestructible", "No damage, no wear", {})),
		plate = optionsMenu:text("Plate", "", { placeholder = "As the game gives it", max = 8 }),
	}
	garage(menu)
	menu:separator("Vehicles")
	M.picker(menu, "Spawn", function(entry: catalog.VehicleEntry): string?
		local car, why = M.spawn(entry.hash, entry.name)
		return if car then `{entry.name} delivered` else why
	end)
end

return M
