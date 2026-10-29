--!strict
-- The vehicle the player is in: fix it, keep it whole, doors and windows, make it faster or
-- lighter, let it drive itself, and its customs (features/customs.luau).

local bind = require("../core/bind")
local game = require("../core/game")
local loop = require("../core/loop")
local customs = require("./customs")
local data = require("../data/vehicles")

local M = {}

-- `f` with the player's vehicle, or the message that there is none.
local function withVehicle(f: (car: Vehicle) -> string?): () -> string?
	return function()
		local car = game.vehicle()
		if not car then
			return "You are not in a vehicle"
		end
		return f(car)
	end
end

-- A feature that does `apply` to the vehicle the player is in, every frame, and `release` to
-- it once they leave it (or the feature goes off): it is as the game made it again.
local function perVehicle(apply: (car: Vehicle) -> (), release: ((car: Vehicle) -> ())?): bind.Feature
	local last: Vehicle? = nil
	local function leave()
		if last and release and entity.doesEntityExist(last) then
			release(last)
		end
		last = nil
	end
	return {
		tick = function()
			local car = game.vehicle()
			if car ~= last then
				leave()
				last = car
			end
			if car then
				apply(car)
			end
		end,
		off = leave,
	}
end

local function fix(car: Vehicle)
	vehicle.setVehicleFixed(car)
	vehicle.setVehicleDeformationFixed(car)
	vehicle.setVehicleDirtLevel(car, 0)
	vehicle.setVehicleEngineHealth(car, 1000)
end

local function actions(menu: Menu)
	bind.command(menu, "vehicle.repair", "Repair", { note = "Body, engine, tyres and dirt", action = "Repair" }, withVehicle(function(car: Vehicle): string?
		fix(car)
		return "Repaired"
	end))
	bind.command(menu, "vehicle.flip", "Flip upright", { action = "Flip" }, withVehicle(function(car: Vehicle): string?
		local rot = entity.getEntityRotation(car, 2)
		entity.setEntityRotation(car, vector.create(0, 0, rot.z), 2, true)
		vehicle.setVehicleOnGroundProperly(car, 5)
		return nil
	end))
	bind.command(menu, "vehicle.engine", "Engine on or off", { action = "Switch" }, withVehicle(function(car: Vehicle): string?
		vehicle.setVehicleEngineOn(car, not vehicle.getIsVehicleEngineRunning(car), true, true)
		return nil
	end))
	bind.command(menu, "vehicle.last", "Back in your last vehicle", { note = "Wherever it is", action = "Get in" }, function(): string?
		local car = player.getPlayersLastVehicle()
		if not car or not entity.doesEntityExist(car) then
			return "No last vehicle"
		end
		ped.setPedIntoVehicle(game.me(), car, VehicleSeat.DRIVER)
		return nil
	end)
	bind.command(menu, "vehicle.delete", "Delete", { note = "The vehicle goes; you stay", action = "Delete" }, withVehicle(function(car: Vehicle): string?
		local me = game.me()
		entity.setEntityCoordsNoOffset(me, entity.getEntityCoords(car, true) + vector.create(0, 0, 1), false, false, true)
		entity.setEntityAsMissionEntity(car, true, true)
		vehicle.deleteVehicle(car)
		return nil
	end))
	bind.command(menu, "vehicle.keep", "Keep it", { note = "It stays where you leave it, never taken away", action = "Keep" }, withVehicle(function(car: Vehicle): string?
		entity.setEntityAsMissionEntity(car, true, true)
		vehicle.setVehicleHasBeenOwnedByPlayer(car, true)
		return "It stays"
	end))
	local seat = menu:choice("Seat", { "Driver" }, 1, "Move to another seat", bind.safe("Seat", function(i: number)
		local car = game.vehicle()
		if not car then
			return
		end
		-- Seats count from -1, the driver's.
		local index = i - 2
		if vehicle.isVehicleSeatFree(car, index, false) then
			ped.setPedIntoVehicle(game.me(), car, index)
		else
			game.notify("That seat is taken")
		end
	end))
	-- The seats of the vehicle the player is in when the menu opens.
	menu:onOpen(bind.safe("Vehicle", function()
		local car = game.vehicle()
		local options = { "Driver" }
		if car then
			for n = 1, vehicle.getVehicleMaxNumberOfPassengers(car) do
				table.insert(options, `Passenger {n}`)
			end
		end
		seat.options = options
		seat.enabled = car ~= nil
	end))
end

local function doors(root: Menu)
	local menu = root:submenu("Doors and windows")
	bind.command(menu, "vehicle.doorsOpen", "Open every door", { action = "Open" }, withVehicle(function(car: Vehicle): string?
		for door = 0, 5 do
			vehicle.setVehicleDoorOpen(car, door, false, false)
		end
		return nil
	end))
	bind.command(menu, "vehicle.doorsShut", "Shut every door", { action = "Shut" }, withVehicle(function(car: Vehicle): string?
		vehicle.setVehicleDoorsShut(car, false)
		return nil
	end))
	menu:separator("One door")
	-- The doors, from 0 (front left) to 5 (trunk).
	for door = 0, 5 do
		local name = data.doors[door]
		local open = false
		bind.action(menu, name, { action = "Open / shut" }, withVehicle(function(car: Vehicle): string?
			open = not open
			if open then
				vehicle.setVehicleDoorOpen(car, door, false, false)
			else
				vehicle.setVehicleDoorShut(car, door, false)
			end
			return nil
		end))
	end
	menu:separator("Windows and locks")
	bind.action(menu, "Windows down", { action = "Down" }, withVehicle(function(car: Vehicle): string?
		vehicle.rollDownWindows(car)
		return nil
	end))
	bind.action(menu, "Windows up", { action = "Up" }, withVehicle(function(car: Vehicle): string?
		for window = 0, 3 do
			vehicle.rollUpWindow(car, window)
		end
		return nil
	end))
	bind.feature(menu, "vehicle.locked", "Locked", "Nobody gets in", perVehicle(function(car)
		vehicle.setVehicleDoorsLocked(car, 2)
	end, function(car)
		vehicle.setVehicleDoorsLocked(car, 1)
	end))
end

local function protection(menu: Menu)
	menu:separator("Protection")
	bind.feature(menu, "vehicle.god", "Indestructible", "The vehicle you are in takes no damage", perVehicle(function(car)
		entity.setEntityInvincible(car, true)
		vehicle.setVehicleCanBeVisiblyDamaged(car, false)
		vehicle.setVehicleTyresCanBurst(car, false)
	end, function(car)
		entity.setEntityInvincible(car, false)
		vehicle.setVehicleCanBeVisiblyDamaged(car, true)
		vehicle.setVehicleTyresCanBurst(car, true)
	end))
	bind.feature(menu, "vehicle.autoRepair", "Repair itself", "Fixed as soon as it is damaged", perVehicle(function(car)
		if vehicle.getVehicleBodyHealth(car) < 1000 or vehicle.getVehicleEngineHealth(car) < 1000 then
			fix(car)
		end
	end))
	bind.feature(menu, "vehicle.clean", "Always clean", nil, perVehicle(function(car)
		vehicle.setVehicleDirtLevel(car, 0)
	end))
	bind.feature(menu, "vehicle.autoFlip", "Lands on its wheels", "Turned over: back upright", perVehicle(function(car)
		if math.abs(entity.getEntityRoll(car)) > 90 and not entity.isEntityInAir(car) then
			vehicle.setVehicleOnGroundProperly(car, 5)
		end
	end))
	bind.feature(menu, "vehicle.seatbelt", "Seatbelt", "Never thrown through the windscreen, never knocked off a bike", {
		tick = function()
			-- 32: PED_FLAG_CAN_FLY_THRU_WINDSCREEN.
			ped.setPedConfigFlag(game.me(), 32, false)
			ped.setPedCanBeKnockedOffVehicle(game.me(), 1)
		end,
		off = function()
			ped.setPedConfigFlag(game.me(), 32, true)
			ped.setPedCanBeKnockedOffVehicle(game.me(), 0)
		end,
	})
	bind.feature(menu, "vehicle.noDragOut", "Nobody pulls you out", nil, {
		tick = function()
			ped.setPedCanBeDraggedOut(game.me(), false)
		end,
		off = function()
			ped.setPedCanBeDraggedOut(game.me(), true)
		end,
	})
end

local function driving(menu: Menu)
	menu:separator("Driving")
	bind.feature(menu, "vehicle.hornBoost", "Horn boost", "The horn pushes you forward", perVehicle(function(car)
		if pad.isControlPressed(0, Control.VEH_HORN) then
			vehicle.setVehicleForwardSpeed(car, entity.getEntitySpeed(car) + 3)
		end
	end))
	bind.feature(menu, "vehicle.instantBrake", "Instant brake", "The handbrake stops it dead", perVehicle(function(car)
		if pad.isControlPressed(0, Control.VEH_HANDBRAKE) then
			entity.setEntityVelocity(car, vector.create(0, 0, 0))
		end
	end))
	bind.command(menu, "vehicle.jump", "Jump", { note = "Up in the air (give it a key)", action = "Jump" }, withVehicle(function(car: Vehicle): string?
		entity.applyForceToEntityCenterOfMass(car, 1, 0, 0, 12, true, true, true, true)
		return nil
	end))
	-- Extra power and top speed, given every frame to the vehicle the player is in.
	local power, top = 0, 0
	local function boost()
		if power > 0 or top > 0 then
			loop.add("vehicle.power", function()
				local car = game.vehicle()
				if car then
					vehicle.setVehicleCheatPowerIncrease(car, 1 + power)
					vehicle.modifyVehicleTopSpeed(car, top * 100)
				end
			end)
		else
			loop.remove("vehicle.power")
			local car = game.vehicle()
			if car then
				vehicle.setVehicleCheatPowerIncrease(car, 1)
				vehicle.modifyVehicleTopSpeed(car, 0)
			end
		end
	end
	bind.slider(menu, "vehicle.power", "Engine power", 0, 0, 5, { step = 0.1, unit = "%", note = "Extra power over the engine's" }, function(v)
		power = v
		boost()
	end)
	bind.slider(menu, "vehicle.topSpeed", "Top speed", 0, 0, 5, { step = 0.1, unit = "%", note = "Faster than it was built for" }, function(v)
		top = v
		boost()
	end)
	bind.feature(menu, "vehicle.drift", "Drift tyres", "Little grip, long slides", perVehicle(function(car)
		vehicle.setVehicleReduceGrip(car, true)
	end, function(car)
		vehicle.setVehicleReduceGrip(car, false)
	end))
	bind.feature(menu, "vehicle.noGravity", "No gravity", "Drive off a cliff and keep going", perVehicle(function(car)
		vehicle.setVehicleGravity(car, false)
	end, function(car)
		vehicle.setVehicleGravity(car, true)
	end))
	bind.feature(menu, "vehicle.invisible", "Invisible", "Others do not see it; you still do", perVehicle(function(car)
		entity.setEntityVisible(car, false, false)
		network.setEntityLocallyVisible(car)
	end, function(car)
		entity.setEntityVisible(car, true, false)
	end))
	-- Through the colour wheel, one turn every 6 seconds.
	bind.feature(menu, "vehicle.rainbow", "Rainbow paint", nil, perVehicle(function(car)
		local h = (os.clock() / 6) % 1
		local function channel(n: number): number
			local k = (n + h * 6) % 6
			return math.floor(255 * (1 - math.max(0, math.min(k, 4 - k, 1))) + 0.5)
		end
		local r, g, b = channel(5), channel(3), channel(1)
		vehicle.setVehicleCustomPrimaryColour(car, r, g, b)
		vehicle.setVehicleCustomSecondaryColour(car, r, g, b)
	end))
end

local function lights(root: Menu)
	local menu = root:submenu("Lights and siren")
	-- SET_VEHICLE_LIGHTS: 0 as the game wants, 1 always off, 2 always on.
	menu:choice("Headlights", { "Automatic", "Off", "On" }, 1, nil, bind.safe("Headlights", function(i: number)
		local car = game.vehicle()
		if car then
			vehicle.setVehicleLights(car, i - 1)
		end
	end))
	bind.feature(menu, "vehicle.fullBeam", "Full beam", nil, perVehicle(function(car)
		vehicle.setVehicleFullbeam(car, true)
	end, function(car)
		vehicle.setVehicleFullbeam(car, false)
	end))
	-- SET_VEHICLE_INDICATOR_LIGHTS' side: 1 left, 0 right.
	menu:choice("Indicators", { "Off", "Left", "Right", "Hazards" }, 1, nil, bind.safe("Indicators", function(i: number)
		local car = game.vehicle()
		if car then
			vehicle.setVehicleIndicatorLights(car, true, i == 2 or i == 4)
			vehicle.setVehicleIndicatorLights(car, false, i == 3 or i == 4)
		end
	end))
	bind.feature(menu, "vehicle.siren", "Siren", "Emergency vehicles' lights and sound", perVehicle(function(car)
		vehicle.setVehicleSiren(car, true)
	end, function(car)
		vehicle.setVehicleSiren(car, false)
	end))
	bind.feature(menu, "vehicle.quietSiren", "Lights without the sound", "The siren's lights only", perVehicle(function(car)
		vehicle.setVehicleHasMutedSirens(car, true)
	end, function(car)
		vehicle.setVehicleHasMutedSirens(car, false)
	end))
end

-- Stunts, and a few touches: flips, a blip, frozen in place, tyres, the lights' brightness, a
-- light under it.
local function tricks(root: Menu)
	local menu = root:submenu("Tricks", "Flips, hops, a blip on it, frozen, under-glow")
	-- A push at a point of the vehicle (its own axes), up and around.
	local function flip(push: vector, at: vector): () -> string?
		return withVehicle(function(car: Vehicle): string?
			entity.applyForceToEntity(car, 1, push, at, 0, true, true, true, false, true)
			return nil
		end)
	end
	bind.command(menu, "vehicle.hop", "Bunny hop", { action = "Hop" }, flip(vector.create(0, 0, 7), vector.zero))
	bind.command(menu, "vehicle.kickflip", "Kickflip", { action = "Flip" }, flip(vector.create(0, 0, 10.5), vector.create(2, 0, 0)))
	bind.command(menu, "vehicle.frontFlip", "Front flip", { action = "Flip" }, flip(vector.create(0, 0, 12), vector.create(0, -3.5, 0)))
	bind.command(menu, "vehicle.backFlip", "Back flip", { action = "Flip" }, flip(vector.create(0, 0, 12), vector.create(0, 3.5, 0)))
	local blip: Blip? = nil
	bind.feature(menu, "vehicle.blip", "On the map", "A blip on the vehicle you are in", perVehicle(function(car)
		if not blip then
			blip = hud.addBlipForEntity(car)
		end
	end, function()
		if blip and hud.doesBlipExist(blip) then
			hud.removeBlip(blip)
		end
		blip = nil
	end))
	bind.feature(menu, "vehicle.freeze", "Frozen in place", "It does not move, whatever pushes it", perVehicle(function(car)
		entity.freezeEntityPosition(car, true)
	end, function(car)
		entity.freezeEntityPosition(car, false)
	end))
	bind.command(menu, "vehicle.tyres", "Fix the tyres", { action = "Fix" }, withVehicle(function(car: Vehicle): string?
		for tyre = 0, 7 do
			vehicle.setVehicleTyreFixed(car, tyre)
		end
		return nil
	end))
	bind.slider(menu, "vehicle.lightPower", "Headlight brightness", 1, 1, 20, { step = 1, unit = "×" }, function(v)
		local car = game.vehicle()
		if car then
			vehicle.setVehicleLightMultiplier(car, v)
		end
	end)
	local glow = menu:color("Under-glow colour", { r = 0, g = 160, b = 255 }, nil)
	bind.feature(menu, "vehicle.underGlow", "Under-glow", "A light on the road under the vehicle", perVehicle(function(car)
		local c = glow.value
		graphics.drawLightWithRange(entity.getEntityCoords(car, true) - vector.create(0, 0, 0.5), c.r, c.g, c.b, 4, 4)
	end))
end

-- Its weapons: shot from the front with the vehicle attack control (a choice of the game's).
local GUNS = {
	{ "Rockets", "WEAPON_VEHICLE_ROCKET" },
	{ "Jet rockets", "VEHICLE_WEAPON_PLANE_ROCKET" },
	{ "Cannon", "VEHICLE_WEAPON_PLAYER_BULLET" },
	{ "Tank shells", "VEHICLE_WEAPON_TANK" },
	{ "RPG", "WEAPON_RPG" },
	{ "Fireworks", "WEAPON_FIREWORK" },
}

local function guns(menu: Menu)
	menu:separator("Weapons")
	local names = {}
	for _, g in GUNS do
		table.insert(names, g[1])
	end
	local kind = bind.choice(menu, "vehicle.gunKind", "Its weapon", names, names[1], "What Vehicle weapons shoots")
	local next = 0
	bind.feature(menu, "vehicle.guns", "Vehicle weapons", "Shoot from the front with the vehicle attack control", perVehicle(function(car)
		if not pad.isControlPressed(0, Control.VEH_ATTACK) or os.clock() < next then
			return
		end
		local hash = joaat(GUNS[kind.value][2])
		if not weapon.isWeaponValid(hash) then
			return
		end
		weapon.requestWeaponAsset(hash, 31, 0)
		if not weapon.hasWeaponAssetLoaded(hash) then
			return
		end
		next = os.clock() + 0.25
		local low, high = misc.getModelDimensions(entity.getEntityModel(car))
		for _, side in { low.x, high.x } do
			local from = entity.getOffsetFromEntityInWorldCoords(car, vector.create(side, high.y + 1, 0.3))
			local to = entity.getOffsetFromEntityInWorldCoords(car, vector.create(side, high.y + 300, 0.3))
			misc.shootSingleBulletBetweenCoords(from, to, 250, true, hash, game.me(), true, false, -1)
		end
	end))
end

-- Driving with a little help: cruise control, flight, a road over the water.
local function help(menu: Menu)
	menu:separator("Help at the wheel")
	-- Holds the speed the accelerator was let go at, until the brake.
	local hold: number? = nil
	bind.feature(menu, "vehicle.cruise", "Cruise control", "Keeps the speed you let go of the accelerator at; brake to stop it", perVehicle(function(car)
		local speed = entity.getEntitySpeed(car)
		if pad.isControlPressed(0, Control.VEH_BRAKE) or pad.isControlPressed(0, Control.VEH_HANDBRAKE) then
			hold = nil
		elseif pad.isControlPressed(0, Control.VEH_ACCELERATE) then
			hold = speed
		elseif hold and speed < hold and not entity.isEntityInAir(car) then
			vehicle.setVehicleForwardSpeed(car, hold)
		end
	end, function()
		hold = nil
	end))
	bind.feature(menu, "vehicle.fly", "Flying car", "Accelerate to fly where you look, brake to stop in the air", perVehicle(function(car)
		vehicle.setVehicleGravity(car, false)
		if pad.isControlPressed(0, Control.VEH_ACCELERATE) then
			local rot = camera.getGameplayCamRot(2)
			entity.setEntityRotation(car, rot, 2, true)
			entity.setEntityVelocity(car, geo.direction(rot) * 45)
		elseif pad.isControlPressed(0, Control.VEH_BRAKE) then
			entity.setEntityVelocity(car, vector.zero)
		else
			entity.setEntityVelocity(car, entity.getEntityVelocity(car) * 0.98)
		end
	end, function(car)
		vehicle.setVehicleGravity(car, true)
	end))
	-- An invisible platform under the vehicle at the sea's level, wherever it is over water.
	local platform: Object? = nil
	local PLATFORM = "prop_container_ld_pu"
	local function drop()
		if platform and entity.doesEntityExist(platform) then
			entity.setEntityAsMissionEntity(platform, true, true)
			object.deleteObject(platform)
		end
		platform = nil
	end
	bind.feature(menu, "vehicle.onWater", "Drive on water", "The sea is a road", {
		tick = function()
			local car = game.vehicle()
			if not car then
				drop()
				return
			end
			local at = entity.getEntityCoords(car, true)
			local wet, level = water.getWaterHeightNoWaves(at)
			if not wet or at.z < level - 2 then
				drop()
				return
			end
			if not platform or not entity.doesEntityExist(platform) then
				if not streaming.load(PLATFORM) then
					return
				end
				platform = object.createObject(PLATFORM, at, false, false, false)
				streaming.setModelAsNoLongerNeeded(PLATFORM)
				if not platform then
					return
				end
				entity.setEntityVisible(platform, false, false)
				entity.freezeEntityPosition(platform, true)
			end
			local _, high = misc.getModelDimensions(PLATFORM)
			entity.setEntityCoordsNoOffset(platform, vector.create(at.x, at.y, level + 0.3 - high.z), false, false, false)
			entity.setEntityHeading(platform, entity.getEntityHeading(car))
		end,
		off = drop,
	})
end

-- The model's handling, field by field (handling.meta's), for every vehicle of the model
-- until the game restarts. Kept as multiples of the values the model came with.
local TUNING = {
	{ "Mass", "fMass" },
	{ "Engine power", "fInitialDriveForce", "Vehicles made after the change" },
	{ "Top speed", "fInitialDriveMaxFlatVel", "Vehicles made after the change" },
	{ "Brakes", "fBrakeForce" },
	{ "Grip", "fTractionCurveMax" },
	{ "Grip when sliding", "fTractionCurveMin" },
	{ "Suspension", "fSuspensionForce" },
	{ "Air drag", "fInitialDragCoeff" },
}

local function tuning(root: Menu)
	local menu = root:submenu("Handling", "Mass, power, grip, brakes, suspension of the model")
	-- The values each model came with, by model, read the first time it is tuned.
	local original: { [number]: { [string]: number } } = {}
	local function build()
		menu:clear()
		local car = game.vehicle()
		if not car then
			menu:label("Get in a vehicle first.")
			return
		end
		local model = entity.getEntityModel(car)
		local known = {}
		for _, f in handling.fields(car) do
			known[f.name] = true
		end
		original[model] = original[model] or {}
		local base = original[model]
		menu:label("Every vehicle of this model changes too, until the game restarts.")
		for _, t in TUNING do
			local label, field, note = t[1], t[2], t[3]
			if known[field] then
				local was = base[field]
				if was == nil then
					was = handling.get(car, field)
					base[field] = was
				end
				local now = handling.get(car, field)
				local times = if was ~= 0 then now / was else 1
				menu:slider(label, times, 0.1, 5, { step = 0.1, unit = "×", note = note }, bind.safe(label, function(v)
					local target = game.vehicle()
					if target then
						handling.set(target, field, was * v)
					end
				end))
			end
		end
		bind.action(menu, "As it was built", { action = "Reset" }, function(): string?
			local target = game.vehicle()
			if not target then
				return "Get in a vehicle first"
			end
			local was = original[entity.getEntityModel(target)]
			if was then
				for field, value in was do
					handling.set(target, field, value)
				end
			end
			build()
			return nil
		end)
	end
	menu:onOpen(bind.safe("Handling", build))
end

-- The vehicle drives itself: to the waypoint, or around.
local function autopilot(root: Menu)
	local menu = root:submenu("Autopilot", "The vehicle drives you")
	local styles = {}
	for _, s in data.drivingStyles do
		table.insert(styles, s.name)
	end
	local style = bind.choice(menu, "vehicle.drivingStyle", "Driving", styles, styles[1])
	local speed = bind.slider(menu, "vehicle.autopilotSpeed", "Speed", 25, 5, 60, { step = 5, unit = "m/s" })
	local function drive(to: vector?): string?
		local car = game.vehicle()
		if not car then
			return "You are not in a vehicle"
		end
		local me = game.me()
		if vehicle.getPedInVehicleSeat(car, VehicleSeat.DRIVER, false) ~= me then
			return "Take the wheel first"
		end
		local mode = data.drivingStyles[style.value].flags
		if to then
			task.taskVehicleDriveToCoordLongrange(me, car, to, speed.value, mode, 10)
		else
			task.taskVehicleDriveWander(me, car, speed.value, mode)
		end
		return nil
	end
	bind.command(menu, "vehicle.toWaypoint", "Drive to the waypoint", { action = "Go" }, function(): string?
		local at = world.waypoint()
		if not at then
			return "No waypoint on the map"
		end
		return drive(at)
	end)
	bind.command(menu, "vehicle.wander", "Drive around", { action = "Go" }, function(): string?
		return drive(nil)
	end)
	bind.command(menu, "vehicle.stopDriving", "Stop", { note = "The wheel is yours again", action = "Stop" }, function(): string?
		task.clearPedTasks(game.me())
		return nil
	end)
end

function M.build(root: Menu)
	local menu = root:submenu("Vehicle", "The one you are in: repair, protect, tune, handling, lights, autopilot, weapons")
	actions(menu)
	customs.build(menu)
	tuning(menu)
	doors(menu)
	lights(menu)
	tricks(menu)
	autopilot(menu)
	protection(menu)
	driving(menu)
	help(menu)
	guns(menu)
end

return M
