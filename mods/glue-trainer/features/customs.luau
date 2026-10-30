--!strict
-- Los Santos Customs for the vehicle the player is in: every part the game has for it (with
-- the game's own names), wheels, colours, neon, tint, plate, livery, extras. The menu is built
-- for the vehicle each time it opens. Also what a vehicle looks like as data (the garage
-- keeps it), and put back on a vehicle.

local bind = require("../core/bind")
local game = require("../core/game")
local data = require("../data/vehicles")

local M = {}

-- The game's mod slots (eVehicleModType), and its extras (1-14).
local SLOTS = 49
local EXTRAS = 14

-- A vehicle's looks and parts, as the garage keeps them.
export type Look = {
	model: number,
	wheelType: number,
	mods: { [string]: number },
	toggles: { [string]: boolean },
	colours: { number },
	extraColours: { number },
	primary: { number }?,
	secondary: { number }?,
	neon: { number }?,
	smoke: { number },
	tint: number,
	plate: string,
	plateStyle: number,
	livery: number,
	extras: { [string]: boolean },
}

local function slotName(car: Vehicle, slot: number): string
	return game.label(vehicle.getModSlotName(car, slot), data.modSlots[slot] or `Slot {slot}`)
end

-- Every upgrade the vehicle has, at its best.
function M.maxUpgrade(car: Vehicle)
	vehicle.setVehicleModKit(car, 0)
	for slot = 0, SLOTS - 1 do
		if data.toggles[slot] then
			vehicle.toggleVehicleMod(car, slot, true)
		else
			local n = vehicle.getNumVehicleMods(car, slot)
			if n > 0 then
				vehicle.setVehicleMod(car, slot, n - 1, false)
			end
		end
	end
	vehicle.setVehicleWindowTint(car, 1)
end

-- Any part, any colour.
function M.randomize(car: Vehicle)
	vehicle.setVehicleModKit(car, 0)
	for slot = 0, SLOTS - 1 do
		local n = vehicle.getNumVehicleMods(car, slot)
		if n > 0 and not data.toggles[slot] then
			vehicle.setVehicleMod(car, slot, math.random(-1, n - 1), false)
		end
	end
	vehicle.setVehicleCustomPrimaryColour(car, math.random(0, 255), math.random(0, 255), math.random(0, 255))
	vehicle.setVehicleCustomSecondaryColour(car, math.random(0, 255), math.random(0, 255), math.random(0, 255))
end

-- What `car` looks like and has.
function M.look(car: Vehicle): Look
	local l: Look = {
		model = entity.getEntityModel(car),
		wheelType = vehicle.getVehicleWheelType(car),
		mods = {},
		toggles = {},
		colours = { vehicle.getVehicleColours(car) },
		extraColours = { vehicle.getVehicleExtraColours(car) },
		smoke = { vehicle.getVehicleTyreSmokeColor(car) },
		tint = vehicle.getVehicleWindowTint(car),
		plate = (string.gsub(vehicle.getVehicleNumberPlateText(car), "%s+$", "")),
		plateStyle = vehicle.getVehicleNumberPlateTextIndex(car),
		livery = vehicle.getVehicleLivery(car),
		extras = {},
	}
	for slot = 0, SLOTS - 1 do
		if data.toggles[slot] then
			l.toggles[tostring(slot)] = vehicle.isToggleModOn(car, slot)
		elseif vehicle.getNumVehicleMods(car, slot) > 0 then
			l.mods[tostring(slot)] = vehicle.getVehicleMod(car, slot)
		end
	end
	if vehicle.getIsVehiclePrimaryColourCustom(car) then
		l.primary = { vehicle.getVehicleCustomPrimaryColour(car) }
	end
	if vehicle.getIsVehicleSecondaryColourCustom(car) then
		l.secondary = { vehicle.getVehicleCustomSecondaryColour(car) }
	end
	if vehicle.getVehicleNeonEnabled(car, 0) then
		l.neon = { vehicle.getVehicleNeonColour(car) }
	end
	for id = 1, EXTRAS do
		if vehicle.doesExtraExist(car, id) then
			l.extras[tostring(id)] = vehicle.isVehicleExtraTurnedOn(car, id)
		end
	end
	return l
end

-- `l` put on `car` (of its model).
function M.apply(car: Vehicle, l: Look)
	vehicle.setVehicleModKit(car, 0)
	vehicle.setVehicleWheelType(car, l.wheelType)
	vehicle.setVehicleColours(car, l.colours[1], l.colours[2])
	vehicle.setVehicleExtraColours(car, l.extraColours[1], l.extraColours[2])
	for slot, index in l.mods do
		vehicle.setVehicleMod(car, tonumber(slot) :: number, index, false)
	end
	for slot, on in l.toggles do
		vehicle.toggleVehicleMod(car, tonumber(slot) :: number, on)
	end
	if l.primary then
		vehicle.setVehicleCustomPrimaryColour(car, l.primary[1], l.primary[2], l.primary[3])
	end
	if l.secondary then
		vehicle.setVehicleCustomSecondaryColour(car, l.secondary[1], l.secondary[2], l.secondary[3])
	end
	if l.neon then
		for side = 0, 3 do
			vehicle.setVehicleNeonEnabled(car, side, true)
		end
		vehicle.setVehicleNeonColour(car, l.neon[1], l.neon[2], l.neon[3])
	end
	vehicle.setVehicleTyreSmokeColor(car, l.smoke[1], l.smoke[2], l.smoke[3])
	vehicle.setVehicleWindowTint(car, l.tint)
	vehicle.setVehicleNumberPlateTextIndex(car, l.plateStyle)
	if l.plate ~= "" then
		vehicle.setVehicleNumberPlateText(car, l.plate)
	end
	if l.livery >= 0 then
		vehicle.setVehicleLivery(car, l.livery)
	end
	for id, on in l.extras do
		-- The native's flag is "disable".
		vehicle.setVehicleExtra(car, tonumber(id) :: number, not on)
	end
end

local function rgb(r: number, g: number, b: number): Rgba
	return { r = r, g = g, b = b, a = 255 }
end

-- A choice over `count` options named `name(i)` (from 0), the current one `current`.
local function indexed(menu: Menu, label: string, count: number, current: number, name: (number) -> string, apply: (number) -> ())
	if count <= 0 then
		return
	end
	local options = {}
	for i = 0, count - 1 do
		table.insert(options, name(i))
	end
	menu:choice(label, options, math.clamp(current + 1, 1, count), bind.safe(label, function(i: number)
		apply(i - 1)
	end))
end

local function colours(menu: Menu, car: Vehicle)
	menu:separator("Colours")
	menu:color("Primary", rgb(vehicle.getVehicleCustomPrimaryColour(car)), bind.safe("Primary", function(c)
		vehicle.setVehicleCustomPrimaryColour(car, c.r, c.g, c.b)
	end))
	menu:color("Secondary", rgb(vehicle.getVehicleCustomSecondaryColour(car)), bind.safe("Secondary", function(c)
		vehicle.setVehicleCustomSecondaryColour(car, c.r, c.g, c.b)
	end))
	local neon = menu:toggle("Neon", vehicle.getVehicleNeonEnabled(car, 0), "Under the four sides", bind.safe("Neon", function(on)
		for side = 0, 3 do
			vehicle.setVehicleNeonEnabled(car, side, on)
		end
	end))
	menu:color("Neon colour", rgb(vehicle.getVehicleNeonColour(car)), bind.safe("Neon colour", function(c)
		vehicle.setVehicleNeonColour(car, c.r, c.g, c.b)
		if not neon.value then
			for side = 0, 3 do
				vehicle.setVehicleNeonEnabled(car, side, true)
			end
			neon.value = true
		end
	end))
	menu:color("Tyre smoke", rgb(vehicle.getVehicleTyreSmokeColor(car)), bind.safe("Tyre smoke", function(c)
		vehicle.toggleVehicleMod(car, 20, true)
		vehicle.setVehicleTyreSmokeColor(car, c.r, c.g, c.b)
	end))
end

local function looks(menu: Menu, car: Vehicle)
	menu:separator("Looks")
	indexed(menu, "Window tint", vehicle.getNumVehicleWindowTints(), vehicle.getVehicleWindowTint(car), function(i)
		return data.windowTints[i] or `Tint {i}`
	end, function(i)
		vehicle.setVehicleWindowTint(car, i)
	end)
	indexed(menu, "Plate style", vehicle.getNumberOfVehicleNumberPlates(), vehicle.getVehicleNumberPlateTextIndex(car), function(i)
		return data.plates[i] or `Style {i}`
	end, function(i)
		vehicle.setVehicleNumberPlateTextIndex(car, i)
	end)
	menu:text("Plate", string.gsub(vehicle.getVehicleNumberPlateText(car), "%s+$", ""), { max = 8 }, bind.safe("Plate", function(text)
		vehicle.setVehicleNumberPlateText(car, text)
	end))
	indexed(menu, "Livery", vehicle.getVehicleLiveryCount(car), vehicle.getVehicleLivery(car), function(i)
		return `Livery {i + 1}`
	end, function(i)
		vehicle.setVehicleLivery(car, i)
	end)
	for id = 1, EXTRAS do
		if vehicle.doesExtraExist(car, id) then
			menu:toggle(`Extra {id}`, vehicle.isVehicleExtraTurnedOn(car, id), bind.safe("Extra", function(on)
				vehicle.setVehicleExtra(car, id, not on)
			end))
		end
	end
end

local function build(menu: Menu, car: Vehicle)
	vehicle.setVehicleModKit(car, 0)
	local function again()
		menu:clear()
		build(menu, car)
	end
	bind.action(menu, "Upgrade everything", { note = "Engine, brakes, armour, turbo, looks", action = "Upgrade" }, function(): string?
		M.maxUpgrade(car)
		again()
		return "Fully upgraded"
	end)
	bind.action(menu, "Surprise me", { note = "Random parts and colours", action = "Roll" }, function(): string?
		M.randomize(car)
		again()
		return nil
	end)
	colours(menu, car)
	looks(menu, car)

	menu:separator("Wheels")
	-- The wheels of slot 23 are the type's: the menu is built again for a new type.
	local types = {}
	local kind = 0
	while data.wheelTypes[kind] do
		table.insert(types, data.wheelTypes[kind])
		kind += 1
	end
	menu:choice("Wheel type", types, math.clamp(vehicle.getVehicleWheelType(car) + 1, 1, #types), bind.safe("Wheel type", function(i: number)
		vehicle.setVehicleWheelType(car, i - 1)
		again()
	end))

	menu:separator("Performance and parts")
	for slot = 0, SLOTS - 1 do
		local name = slotName(car, slot)
		if data.toggles[slot] then
			-- Turbo and xenon lights are on or off (the others of these slots do nothing).
			if slot == 18 or slot == 22 then
				menu:toggle(name, vehicle.isToggleModOn(car, slot), bind.safe(name, function(on)
					vehicle.toggleVehicleMod(car, slot, on)
				end))
			end
		else
			local n = vehicle.getNumVehicleMods(car, slot)
			if n > 0 then
				local options = { "Stock" }
				for i = 0, n - 1 do
					table.insert(options, game.label(vehicle.getModTextLabel(car, slot, i), `{name} {i + 1}`))
				end
				menu:choice(name, options, vehicle.getVehicleMod(car, slot) + 2, bind.safe(name, function(i: number)
					if i == 1 then
						vehicle.removeVehicleMod(car, slot)
					else
						vehicle.setVehicleMod(car, slot, i - 2, false)
					end
				end))
			end
		end
	end
end

-- The customs submenu of `root`: for the vehicle the player is in when it opens.
function M.build(root: Menu)
	local menu = root:submenu("Customs", "Parts, wheels, colours, neon, plate, extras")
	menu:onOpen(bind.safe("Customs", function()
		menu:clear()
		local car = game.vehicle()
		if not car then
			menu:label("Get in a vehicle first.")
			return
		end
		build(menu, car)
	end))
end

return M
