--!strict
-- The world: the clock, the weather and the sky, gravity and time's pace, the screen's
-- filters, the streets and the police, the people around, bodyguards.
--
-- In a GTA Online session the clock and the weather are the session's: the trainer changes
-- them for you only (the game's own override), as every player sees their own.

local bind = require("../core/bind")
local game = require("../core/game")
local loop = require("../core/loop")
local catalog = require("../core/catalog")
local data = require("../data/world")

local M = {}

local function online(): boolean
	return network.networkIsSessionStarted()
end

local function setTime(h: number, m: number)
	if online() then
		network.networkOverrideClockTime(h, m, 0)
	else
		clock.setClockTime(h, m, 0)
	end
end

local function time(menu: Menu)
	menu:separator("Time")
	local hour = menu:slider("Hour", clock.getClockHours(), 0, 23, 1, bind.safe("Hour", function(h)
		setTime(h, clock.getClockMinutes())
	end))
	local minute = menu:number("Minute", clock.getClockMinutes(), { min = 0, max = 59, step = 5 }, bind.safe("Minute", function(m)
		setTime(clock.getClockHours(), m)
	end))
	-- The game's clock runs on: shown as it is whenever the menu opens.
	menu:onOpen(bind.safe("World", function()
		hour.value = clock.getClockHours()
		minute.value = clock.getClockMinutes()
	end))
	bind.command(menu, "world.gameTime", "Back to the game's time", { note = "In GTA Online, the session's", action = "Reset" }, function(): string?
		network.networkClearClockTimeOverride()
		return nil
	end)
	bind.feature(menu, "world.freezeClock", "Freeze time", "The clock stops where it is", {
		on = function()
			clock.pauseClock(true)
		end,
		off = function()
			clock.pauseClock(false)
		end,
	})
	bind.feature(menu, "world.realTime", "Real time", "The clock follows this device's", {
		tick = function()
			local now = os.date("*t")
			setTime(now.hour, now.min)
		end,
	})
	bind.slider(menu, "world.timeScale", "Game speed", 1, 0.1, 1, { step = 0.1, unit = "%", note = "Slow motion below 100 %" }, function(v)
		misc.setTimeScale(v)
	end)
end

local function weather(menu: Menu)
	menu:separator("Weather and sea")
	local names = {}
	for _, w in data.weathers do
		table.insert(names, w[2])
	end
	menu:choice("Weather", names, 1, "Changes at once", bind.safe("Weather", function(i: number)
		local name = data.weathers[i][1]
		if online() then
			misc.setOverrideWeather(name)
		else
			misc.setWeatherTypeNowPersist(name)
		end
	end))
	bind.command(menu, "world.gameWeather", "Back to the game's weather", { action = "Reset" }, function(): string?
		misc.clearOverrideWeather()
		misc.clearWeatherTypePersist()
		return nil
	end)
	bind.slider(menu, "world.wind", "Wind", 0, 0, 12, { step = 1, unit = "m/s" }, function(v)
		misc.setWindSpeed(v)
	end)
	bind.slider(menu, "world.waves", "Waves", 1, 0, 5, { step = 0.5, unit = "×", note = "How high the sea's waves are" }, function(v)
		water.setDeepOceanScaler(v)
	end)
	bind.feature(menu, "world.blackout", "Blackout", "Every light of the city off", {
		on = function()
			graphics.setArtificialLightsState(true)
		end,
		off = function()
			graphics.setArtificialLightsState(false)
		end,
	})
	bind.choice(menu, "world.gravity", "Gravity", data.gravity, data.gravity[1], nil, function(i)
		misc.setGravityLevel(i - 1)
	end)
end

local function filters(root: Menu)
	local menu = root:submenu("Screen filters", { note = "The game's own looks", search = true })
	local strength = bind.slider(menu, "world.filterStrength", "Strength", 1, 0, 1, { step = 0.05, unit = "%" }, function(v)
		graphics.setTimecycleModifierStrength(v)
	end)
	bind.command(menu, "world.noFilter", "None", { action = "Clear" }, function(): string?
		graphics.clearTimecycleModifier()
		return nil
	end)
	menu:separator()
	for _, f in data.filters do
		bind.action(menu, f[1], { action = "Use" }, function(): string?
			graphics.setTimecycleModifier(f[2])
			graphics.setTimecycleModifierStrength(strength.value)
			return nil
		end)
	end
end

local function streets(menu: Menu)
	menu:separator("Streets and police")
	local traffic, people = 1, 1
	local function busy()
		vehicle.setVehicleDensityMultiplierThisFrame(traffic)
		vehicle.setRandomVehicleDensityMultiplierThisFrame(traffic)
		vehicle.setParkedVehicleDensityMultiplierThisFrame(traffic)
		ped.setPedDensityMultiplierThisFrame(people)
		ped.setScenarioPedDensityMultiplierThisFrame(people, people)
	end
	-- The game asks again every frame: the loop runs only while one is not 100 %.
	local function apply()
		if traffic < 1 or people < 1 then
			loop.add("world.density", busy)
		else
			loop.remove("world.density")
		end
	end
	bind.slider(menu, "world.traffic", "Traffic", 1, 0, 1, { step = 0.1, unit = "%" }, function(v)
		traffic = v
		apply()
	end)
	bind.slider(menu, "world.people", "People", 1, 0, 1, { step = 0.1, unit = "%" }, function(v)
		people = v
		apply()
	end)
	bind.feature(menu, "world.noTrains", "No trains or boats", "The city's own, not yours", {
		on = function()
			vehicle.setRandomTrains(false)
			vehicle.setRandomBoats(false)
		end,
		off = function()
			vehicle.setRandomTrains(true)
			vehicle.setRandomBoats(true)
		end,
	})
	bind.feature(menu, "world.noDispatch", "No police dispatch", "No patrols, helicopters, roadblocks or SWAT sent after you", {
		on = function()
			for service = 1, data.dispatchServices do
				misc.enableDispatchService(service, false)
			end
			ped.setCreateRandomCops(false)
		end,
		off = function()
			for service = 1, data.dispatchServices do
				misc.enableDispatchService(service, true)
			end
			ped.setCreateRandomCops(true)
		end,
	})
	-- The scripts start again when the player comes back: ended again every few seconds.
	local since = 0
	bind.feature(menu, "world.restricted", "Go anywhere", "The army base, the prison, the airport let you in", {
		tick = function()
			since += misc.getFrameTime()
			if since < 3 then
				return
			end
			since = 0
			for _, name in data.restrictedAreas do
				misc.terminateAllScriptsWithThisName(name)
			end
		end,
	})
	bind.feature(menu, "world.riot", "Riot", "Everyone fights everyone", {
		on = function()
			misc.setRiotModeEnabled(true)
		end,
		off = function()
			misc.setRiotModeEnabled(false)
		end,
	})
end

local function around(menu: Menu)
	menu:separator("Around you")
	bind.command(menu, "world.clear", "Clear the area", { note = "Vehicles, people, objects and fires within 100 m", action = "Clear" }, function(): string?
		misc.clearArea(game.position(), 100, true, false, false, false)
		return nil
	end)
	bind.command(menu, "world.knockOut", "Knock out everyone", { note = "People within 100 m, not players", action = "Run" }, function(): string?
		local count = 0
		for _, p in world.nearby("peds", game.me(), 100) do
			if not ped.isPedAPlayer(p) and not ped.isPedDeadOrDying(p, true) then
				entity.setEntityHealth(p, 0, nil)
				count += 1
			end
		end
		return `{count} people`
	end)
	bind.command(menu, "world.disarm", "Disarm everyone", { note = "People within 100 m, not players", action = "Run" }, function(): string?
		for _, p in world.nearby("peds", game.me(), 100) do
			if not ped.isPedAPlayer(p) then
				weapon.removeAllPedWeapons(p, true)
			end
		end
		return nil
	end)
	bind.command(menu, "world.explode", "Blow up the vehicles", { note = "Within 100 m, not yours", action = "Boom" }, function(): string?
		local mine = game.vehicle()
		local count = 0
		for _, car in world.nearby("vehicles", game.me(), 100) do
			if car ~= mine then
				vehicle.explodeVehicle(car, true, false)
				count += 1
			end
		end
		return `{count} vehicles`
	end)
	bind.command(menu, "world.launch", "Launch the vehicles", { note = "Within 100 m, up in the air", action = "Launch" }, function(): string?
		local mine = game.vehicle()
		for _, car in world.nearby("vehicles", game.me(), 100) do
			if car ~= mine then
				entity.applyForceToEntityCenterOfMass(car, 1, 0, 0, 30, true, false, true, false)
			end
		end
		return nil
	end)
end

-- The sky's clouds (clouds.xml's cloud hats), a light around you, the ocean's waves, the
-- interior you are in redrawn.
local function sky(menu: Menu)
	menu:separator("Sky and light")
	local hats = { "Clear sky", "Cloudy 01", "Rain", "Horizon", "Horizonband1", "Snowy 01", "Stormy 01", "Overcast", "Shower", "Stripey", "Wispy", "Altostratus", "Cirrus", "Cirrocumulus", "Nimbus", "Puffs", "Contrails" }
	menu:choice("Clouds", hats, 1, "The sky's clouds now", bind.safe("Clouds", function(i: number, name: string)
		misc.unloadAllCloudHats()
		if i > 1 then
			misc.loadCloudHat(name, 2)
		end
	end))
	bind.feature(menu, "world.torch", "Light around you", "A lamp that follows you at night", {
		tick = function()
			graphics.drawLightWithRange(game.position() + vector.create(0, 0, 1.5), 255, 240, 220, 12, 2)
		end,
	})
	bind.slider(menu, "world.ocean", "Ocean waves", 1, 0, 3, { step = 0.1, unit = "×", note = "The open sea's" }, function(v)
		water.setDeepOceanScaler(v)
	end)
	bind.command(menu, "world.refreshInterior", "Redraw this interior", { note = "Fixes one that loaded wrong", action = "Redraw" }, function(): string?
		local room = interior.getInteriorFromEntity(game.me())
		if not room then
			return "You are not inside"
		end
		interior.refreshInterior(room)
		return nil
	end)
end

-- Trains: get on one, set its speed.
local function trains(menu: Menu)
	menu:separator("Trains")
	bind.command(menu, "world.train", "Get on the nearest train", { note = "Within 500 m, in the driver's cab", action = "Get on" }, function(): string?
		for _, car in world.nearby("vehicles", game.me(), 500) do
			if vehicle.isThisModelATrain(entity.getEntityModel(car)) and vehicle.isVehicleSeatFree(car, VehicleSeat.DRIVER, false) then
				ped.setPedIntoVehicle(game.me(), car, VehicleSeat.DRIVER)
				return nil
			end
		end
		return "No train nearby"
	end)
	bind.slider(menu, "world.trainSpeed", "Train speed", 15, 0, 60, { step = 5, unit = "m/s", note = "The train you are on" }, function(v)
		local me = game.me()
		if ped.isPedInAnyTrain(me) then
			local train = ped.getVehiclePedIsIn(me, false)
			vehicle.setTrainCruiseSpeed(train, v)
			vehicle.setTrainSpeed(train, v)
		end
	end)
end

-- What falls from the sky: rocks that burst into flames, fireworks, vehicles, people; and a
-- black hole that swallows what is near it. All here, on this game only.
local function fun(root: Menu)
	local menu = root:submenu("Mayhem", "Meteors, fireworks, a black hole, vehicles and people from the sky")
	-- A random point within `r` of the player, `up` metres high.
	local function above(r: number, up: number): vector
		local at = game.position()
		return vector.create(at.x + math.random(-r, r), at.y + math.random(-r, r), at.z + up)
	end
	-- Meteors: big burning rocks coming down at a slant from high up to the ground around the
	-- player, bursting there. Moved along their path every frame (an object made far up has no
	-- physics yet: a velocity would not move it).
	type Meteor = { rock: Object, from: vector, to: vector, born: number, takes: number, fire: number }
	local falling: { Meteor } = {}
	-- The largest rock the game makes as an object (the bigger ones are the map's only).
	local ROCK = "prop_rock_4_big2"
	local nextRock = 0
	local function remove(m: Meteor)
		if m.fire ~= 0 then
			graphics.stopParticleFxLooped(m.fire, false)
		end
		if entity.doesEntityExist(m.rock) then
			entity.setEntityAsMissionEntity(m.rock, true, true)
			object.deleteObject(m.rock)
		end
	end
	bind.feature(menu, "world.meteors", "Meteor shower", "Burning rocks fall from the sky around you", bind.storyOnly({
		tick = function()
			local now = os.clock()
			for i = #falling, 1, -1 do
				local m = falling[i]
				local t = (now - m.born) / m.takes
				if t >= 1 or not entity.doesEntityExist(m.rock) then
					fire.addExplosion(m.to, ExplosionType.PLANE, 1, true, false, 1, false)
					remove(m)
					table.remove(falling, i)
				else
					entity.setEntityCoordsNoOffset(m.rock, m.from + (m.to - m.from) * t, false, false, false)
					entity.setEntityRotation(m.rock, vector.create(now * 200 % 360, now * 130 % 360, 0), 2, true)
				end
			end
			if now < nextRock or #falling >= 10 then
				return
			end
			nextRock = now + 0.8
			task.spawn(function()
				if not streaming.load(ROCK) or not streaming.load("core", "ptfx") then
					return
				end
				-- Where it lands, on the ground around the player; it comes from far up, to a side.
				local spot = above(80, 50)
				local ground = world.groundZ(spot) or game.position().z
				local to = vector.create(spot.x, spot.y, ground)
				local side = math.random() * math.pi * 2
				local from = to + vector.create(math.cos(side) * 150, math.sin(side) * 150, 220)
				local rock = object.createObject(ROCK, from, false, false, false)
				streaming.setModelAsNoLongerNeeded(ROCK)
				if not rock then
					return
				end
				entity.setEntityCollision(rock, false, false)
				graphics.useParticleFxAsset("core")
				local flames = graphics.startParticleFxLoopedOnEntity("fire_wrecked_plane_cockpit", rock, 0, 0, 0, 0, 0, 0, 4, false, false, false)
				-- About 60 m/s.
				local takes = vector.magnitude(to - from) / 60
				table.insert(falling, { rock = rock, from = from, to = to, born = os.clock(), takes = takes, fire = flames })
			end)
		end,
		off = function()
			for _, m in falling do
				remove(m)
			end
			falling = {}
		end,
	}))
	local nextFirework = 0
	bind.feature(menu, "world.fireworks", "Fireworks", "Over your head", {
		tick = function()
			if os.clock() < nextFirework then
				return
			end
			nextFirework = os.clock() + 0.7
			local asset = "scr_indep_fireworks"
			streaming.requestNamedPtfxAsset(asset)
			if not streaming.hasNamedPtfxAssetLoaded(asset) then
				return
			end
			graphics.useParticleFxAsset(asset)
			local burst = if math.random() < 0.5 then "scr_indep_firework_starburst" else "scr_indep_firework_trailburst"
			-- In GTA Online, for everyone to see.
			if game.online() then
				graphics.startNetworkedParticleFxNonLoopedAtCoord(burst, above(40, 50), vector.zero, 2, false, false, false, false)
			else
				graphics.startParticleFxNonLoopedAtCoord(burst, above(40, 50), vector.zero, 2, false, false, false)
			end
		end,
	})
	-- Pulls what is within 80 m to a point 40 m ahead of where it was made, and swallows it.
	local hole: vector? = nil
	bind.feature(menu, "world.blackHole", "Black hole", "Everything near it is pulled in", bind.storyOnly({
		on = function()
			local me = game.me()
			hole = entity.getEntityCoords(me, true) + entity.getEntityForwardVector(me) * 40 + vector.create(0, 0, 15)
		end,
		tick = function()
			local at = hole
			if not at then
				return
			end
			local mine = game.vehicle()
			local function pull(e: Entity)
				local to = at - entity.getEntityCoords(e, true)
				local d = vector.magnitude(to)
				if d < 3 then
					entity.setEntityVelocity(e, vector.zero)
				else
					entity.setEntityVelocity(e, vector.normalize(to) * math.min(40, 400 / d + 8))
				end
			end
			for _, v in world.nearby("vehicles", at, 80) do
				if v ~= mine then
					pull(v)
				end
			end
			for _, p in world.nearby("peds", at, 80) do
				if not ped.isPedAPlayer(p) and not ped.isPedInAnyVehicle(p, false) then
					pull(p)
				end
			end
		end,
		off = function()
			hole = nil
		end,
	}))
	-- Vehicles or people from the sky, a few a second, the oldest let go after 30.
	local rain = bind.choice(menu, "world.rainOf", "Rain of", { "Vehicles", "People" }, "Vehicles", "What Raining down drops")
	local dropped: { Entity } = {}
	local nextDrop = 0
	bind.feature(menu, "world.rain", "Raining down", "From the sky, around you", bind.storyOnly({
		tick = function()
			if os.clock() < nextDrop then
				return
			end
			nextDrop = os.clock() + 0.4
			local cars = rain.value == 1
			local hash
			if cars then
				local all = catalog.vehicles()
				if #all == 0 then
					return
				end
				hash = all[math.random(1, #all)].hash
			else
				local all = catalog.peds()
				if #all == 0 then
					return
				end
				hash = joaat(all[math.random(1, #all)])
			end
			task.spawn(function()
				if not streaming.load(hash) then
					return
				end
				local at = above(40, 40)
				local e: Entity? = if cars then vehicle.createVehicle(hash, at, math.random(0, 359), false, false, false) else ped.createPed(26, hash, at, 0, false, false)
				streaming.setModelAsNoLongerNeeded(hash)
				if not e then
					return
				end
				table.insert(dropped, e)
				if #dropped > 30 then
					entity.setEntityAsNoLongerNeeded(table.remove(dropped, 1) :: Entity)
				end
			end)
		end,
		off = function()
			for _, e in dropped do
				if entity.doesEntityExist(e) then
					entity.setEntityAsNoLongerNeeded(e)
				end
			end
			dropped = {}
		end,
	}))
end

-- People who follow and protect you: any ped of the game, with any weapon.
local function bodyguards(root: Menu)
	local menu = root:submenu("Bodyguards", "Any ped of the game, armed, by your side")
	local guards: { Ped } = {}
	local built = false
	local function dismiss()
		for _, g in guards do
			if entity.doesEntityExist(g) then
				ped.removePedFromGroup(g)
				entity.setEntityAsNoLongerNeeded(g)
			end
		end
		guards = {}
	end
	mod.onStop(dismiss)
	menu:onOpen(bind.safe("Bodyguards", function()
		if built then
			return
		end
		local peds = catalog.peds()
		local weapons = { "Unarmed" }
		for _, w in catalog.weapons() do
			table.insert(weapons, w.model)
		end
		if #peds == 0 then
			return
		end
		built = true
		local modelItem = menu:choice("Model", peds, if table.find(peds, "s_m_y_swat_01") then "s_m_y_swat_01" else 1)
		local weaponItem = menu:choice("Weapon", weapons, if table.find(weapons, "WEAPON_CARBINERIFLE") then "WEAPON_CARBINERIFLE" else 1)
		bind.action(menu, "Call one", { action = "Call" }, function(): string?
			local hash = joaat(modelItem.option)
			if not streaming.load(hash) then
				return "That model did not load"
			end
			local me = game.me()
			local at = entity.getOffsetFromEntityInWorldCoords(me, vector.create(math.random(-3, 3), -2, 0))
			-- Networked in GTA Online: everyone sees them.
			local g = ped.createPed(26, hash, at, entity.getEntityHeading(me), game.online(), false)
			streaming.setModelAsNoLongerNeeded(hash)
			if not g then
				return "The game made none"
			end
			ped.setPedAsGroupMember(g, player.getPlayerGroup(game.player()))
			ped.setPedNeverLeavesGroup(g, true)
			ped.setPedCombatAbility(g, 2)
			entity.setEntityInvincible(g, false)
			if weaponItem.value > 1 then
				weapon.giveWeaponToPed(g, joaat(weaponItem.option), 9999, false, true)
			end
			table.insert(guards, g)
			return `{#guards} by your side`
		end)
		bind.action(menu, "Dismiss them", { action = "Dismiss" }, function(): string?
			dismiss()
			return nil
		end)
	end))
end

function M.build(root: Menu)
	local menu = root:submenu("World", "Time, weather, filters, traffic, police, the area around, trains, bodyguards, mayhem")
	time(menu)
	weather(menu)
	filters(menu)
	bodyguards(menu)
	fun(menu)
	streets(menu)
	around(menu)
	sky(menu)
	trains(menu)
end

return M
