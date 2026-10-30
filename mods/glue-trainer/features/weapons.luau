--!strict
-- Weapons: every weapon of the game (all at once or one by one, by group), the one in your
-- hands (attachments, tint), saved loadouts, ammo that never runs out, special rounds, damage,
-- and trick guns.

local bind = require("../core/bind")
local game = require("../core/game")
local catalog = require("../core/catalog")
local lists = require("../core/lists")
local data = require("../data/weapons")

local M = {}

type Loadout = { name: string, weapons: { { hash: number, tint: number, components: { number } } } }

local function give(w: catalog.WeaponEntry)
	local me = game.me()
	local _, max = weapon.getMaxAmmo(me, w.hash)
	weapon.giveWeaponToPed(me, w.hash, if max > 0 then max else 9999, false, false)
end

-- The weapon in the player's hands, nil when unarmed.
local function held(): catalog.WeaponEntry?
	local hash = weapon.getSelectedPedWeapon(game.me())
	for _, w in catalog.weapons() do
		if w.hash == hash then
			return w
		end
	end
	return nil
end

local function arsenal(menu: Menu)
	bind.command(menu, "weapons.giveAll", "Give every weapon", { note = "With full ammo", action = "Give" }, function(): string?
		local all = catalog.weapons()
		for _, w in all do
			give(w)
		end
		return `{#all} weapons`
	end)
	bind.command(menu, "weapons.fill", "Fill ammo", { note = "Every weapon you have", action = "Fill" }, function(): string?
		local me = game.me()
		for _, w in catalog.weapons() do
			if weapon.hasPedGotWeapon(me, w.hash, 0) then
				local _, max = weapon.getMaxAmmo(me, w.hash)
				weapon.setPedAmmo(me, w.hash, max, false)
			end
		end
		return nil
	end)
	bind.command(menu, "weapons.removeAll", "Remove every weapon", { action = "Remove" }, function(): string?
		weapon.removeAllPedWeapons(game.me(), true)
		return nil
	end)
	local function fill(target: Menu, weapons: { catalog.WeaponEntry }): boolean
		for _, w in weapons do
			bind.action(target, w.name, { note = w.model, action = "Give" }, function(): string?
				give(w)
				weapon.setCurrentPedWeapon(game.me(), w.hash, true)
				return nil
			end)
		end
		return #weapons > 0
	end
	local pick = menu:submenu("Give one", "By group, or search them all")
	lists.lazy(pick, "Search all", "Every weapon, by name", function(target)
		return fill(target, catalog.weapons())
	end)
	pick:separator("Groups")
	local listed = false
	pick:onOpen(bind.safe("Weapons", function()
		if listed then
			return
		end
		local groups = catalog.weaponGroups()
		for _, group in groups do
			lists.lazy(pick, group.name, `{#group.weapons} weapons`, function(target)
				return fill(target, group.weapons)
			end)
		end
		listed = #groups > 0
	end))
end

-- The weapon in your hands: its tint and attachments, as the game has them for it.
local function inHand(root: Menu)
	local menu = root:submenu("In your hands", "Attachments and tint of the weapon you hold")
	menu:onOpen(bind.safe("In your hands", function()
		menu:clear()
		local w = held()
		if not w then
			menu:label("Take a weapon in your hands first.")
			return
		end
		local gun = w :: catalog.WeaponEntry
		menu.note = gun.name
		menu:label(gun.name)
		local me = game.me()
		local tints = weapon.getWeaponTintCount(gun.hash)
		if tints > 1 then
			local names = {}
			for i = 0, tints - 1 do
				table.insert(names, data.tints[i] or `Tint {i + 1}`)
			end
			menu:choice("Tint", names, weapon.getPedWeaponTintIndex(me, gun.hash) + 1, bind.safe("Tint", function(i: number)
				weapon.setPedWeaponTintIndex(game.me(), gun.hash, i - 1)
			end))
		end
		local parts = catalog.components(gun)
		if #parts == 0 then
			menu:label("It takes no attachments.")
			return
		end
		bind.action(menu, "Every attachment", { action = "Fit" }, function(): string?
			for _, c in parts do
				weapon.giveWeaponComponentToPed(game.me(), gun.hash, c.hash)
			end
			return nil
		end)
		menu:separator("Attachments")
		for _, c in parts do
			menu:toggle(c.name, weapon.hasPedGotWeaponComponent(me, gun.hash, c.hash), bind.safe(c.name, function(on)
				if on then
					weapon.giveWeaponComponentToPed(game.me(), gun.hash, c.hash)
				else
					weapon.removeWeaponComponentFromPed(game.me(), gun.hash, c.hash)
				end
			end))
		end
	end))
end

-- Every weapon you have now, with its tint and attachments, to have again.
local function loadouts(root: Menu)
	local menu = root:submenu("Loadouts", "Save your weapons, get them back")
	local function saved(): { Loadout }
		return storage.get("loadouts") or {}
	end
	local function current(name: string): Loadout
		local me = game.me()
		local l: Loadout = { name = name, weapons = {} }
		for _, w in catalog.weapons() do
			if weapon.hasPedGotWeapon(me, w.hash, 0) then
				local components = {}
				for _, c in catalog.components(w) do
					if weapon.hasPedGotWeaponComponent(me, w.hash, c.hash) then
						table.insert(components, c.hash)
					end
				end
				table.insert(l.weapons, { hash = w.hash, tint = weapon.getPedWeaponTintIndex(me, w.hash), components = components })
			end
		end
		return l
	end
	local function equip(l: Loadout)
		local me = game.me()
		weapon.removeAllPedWeapons(me, true)
		for _, w in l.weapons do
			local _, max = weapon.getMaxAmmo(me, w.hash)
			weapon.giveWeaponToPed(me, w.hash, if max > 0 then max else 9999, false, false)
			weapon.setPedWeaponTintIndex(me, w.hash, w.tint)
			for _, c in w.components do
				weapon.giveWeaponComponentToPed(me, w.hash, c)
			end
		end
	end
	local function build()
		menu:clear()
		menu:text("Save as", "", { placeholder = "A name, then Enter", max = 40 }, bind.safe("Save as", function(name)
			if name == "" then
				return
			end
			local list = saved()
			table.insert(list, current(name))
			storage.set("loadouts", list)
			build()
			game.notify(`Saved: {name}`)
		end))
		local list = saved()
		if #list == 0 then
			menu:label("Nothing saved yet.")
			return
		end
		menu:separator("Equip")
		for _, l in list do
			bind.action(menu, l.name, { note = `{#l.weapons} weapons`, action = "Equip" }, function(): string?
				equip(l)
				return nil
			end)
		end
		local forget = menu:submenu("Delete a loadout")
		for i, l in list do
			bind.action(forget, l.name, { action = "Delete" }, function(): string?
				local all = saved()
				table.remove(all, i)
				storage.set("loadouts", all)
				build()
				return `Deleted: {l.name}`
			end)
		end
	end
	build()
end

local function ammo(menu: Menu)
	menu:separator("Ammo")
	bind.feature(menu, "weapons.infiniteAmmo", "Infinite ammo", "Never runs out", {
		tick = function()
			local me = game.me()
			weapon.setPedInfiniteAmmo(me, true, weapon.getSelectedPedWeapon(me))
		end,
		off = function()
			local me = game.me()
			for _, w in catalog.weapons() do
				weapon.setPedInfiniteAmmo(me, false, w.hash)
			end
		end,
	})
	bind.feature(menu, "weapons.noReload", "No reloading", "The clip never empties", {
		tick = function()
			weapon.setPedInfiniteAmmoClip(game.me(), true)
		end,
		off = function()
			weapon.setPedInfiniteAmmoClip(game.me(), false)
		end,
	})
	bind.feature(menu, "weapons.explosive", "Explosive rounds", nil, {
		tick = function()
			misc.setExplosiveAmmoThisFrame(game.player())
		end,
	})
	bind.feature(menu, "weapons.fire", "Fire rounds", nil, {
		tick = function()
			misc.setFireAmmoThisFrame(game.player())
		end,
	})
	bind.feature(menu, "weapons.explosiveMelee", "Explosive punches", nil, {
		tick = function()
			misc.setExplosiveMeleeThisFrame(game.player())
		end,
	})
end

local function damage(menu: Menu)
	menu:separator("Damage")
	bind.slider(menu, "weapons.damage", "Weapon damage", 1, 0.1, 10, { step = 0.1, unit = "×" }, function(v)
		player.setPlayerWeaponDamageModifier(game.player(), v)
	end)
	bind.slider(menu, "weapons.melee", "Melee damage", 1, 0.1, 10, { step = 0.1, unit = "×" }, function(v)
		player.setPlayerMeleeWeaponDamageModifier(game.player(), v, true)
	end)
	bind.slider(menu, "weapons.defense", "Damage taken", 1, 0.1, 2, { step = 0.1, unit = "×", note = "From other people's weapons" }, function(v)
		player.setPlayerWeaponDefenseModifier(game.player(), v)
	end)
end

-- What the player aims at, if anything.
local function aimed(): Entity?
	local aiming, target = player.getEntityPlayerIsFreeAimingAt(game.player())
	return if aiming then target else nil
end

-- Guns with a trick: where the shot lands, or what it aims at.
local function guns(menu: Menu)
	menu:separator("Trick guns")
	local me = function(): Ped
		return game.me()
	end
	bind.feature(menu, "weapons.teleportGun", "Teleport gun", "You go where your shots land", {
		tick = function()
			local hit, at = weapon.getPedLastWeaponImpactCoord(me())
			if hit then
				game.teleport(at, false)
			end
		end,
	})
	-- The explosion types, from the game's own table.
	local kinds = {}
	for name in ExplosionType :: { [string]: number } do
		table.insert(kinds, name)
	end
	table.sort(kinds)
	local kind = bind.choice(menu, "weapons.explosionKind", "Explosion", kinds, if table.find(kinds, "GRENADE") then "GRENADE" else kinds[1], "What the explosion gun makes")
	bind.feature(menu, "weapons.explosionGun", "Explosion gun", "Shots explode as you chose", {
		tick = function()
			local hit, at = weapon.getPedLastWeaponImpactCoord(me())
			if hit then
				fire.addOwnedExplosion(me(), at, (ExplosionType :: { [string]: number })[kind.option], 1, true, false, 1)
			end
		end,
	})
	bind.feature(menu, "weapons.deleteGun", "Delete gun", "What you shoot at goes", {
		tick = function()
			if not ped.isPedShooting(me()) then
				return
			end
			local target = aimed()
			if target and not (entity.isEntityAPed(target) and ped.isPedAPlayer(target :: Ped)) then
				entity.setEntityAsMissionEntity(target, true, true)
				entity.deleteEntity(target)
			end
		end,
	})
	bind.feature(menu, "weapons.forceGun", "Force gun", "What you shoot at flies away", {
		tick = function()
			if not ped.isPedShooting(me()) then
				return
			end
			local target = aimed()
			if target then
				local push = geo.direction(camera.getGameplayCamRot(2)) * 80
				entity.applyForceToEntityCenterOfMass(target, 1, push.x, push.y, push.z + 10, false, false, true, false)
			end
		end,
	})
	-- Held where the camera looks while aiming; thrown when shot.
	local holding: Entity? = nil
	bind.feature(menu, "weapons.gravityGun", "Gravity gun", "Aim to lift, shoot to throw", {
		tick = function()
			if not player.isPlayerFreeAiming(game.player()) then
				holding = nil
				return
			end
			holding = holding or aimed()
			local lifted = holding
			if not lifted or not entity.doesEntityExist(lifted) then
				holding = nil
				return
			end
			local dir = geo.direction(camera.getGameplayCamRot(2))
			if ped.isPedShooting(me()) then
				entity.setEntityVelocity(lifted, dir * 60)
				holding = nil
				return
			end
			local want = camera.getGameplayCamCoord() + dir * 10
			entity.setEntityVelocity(lifted, (want - entity.getEntityCoords(lifted, true)) * 8)
		end,
		off = function()
			holding = nil
		end,
	})
	bind.feature(menu, "weapons.pedGun", "People gun", "Shoot people of the city", bind.storyOnly({
		tick = function()
			if not ped.isPedShooting(me()) then
				return
			end
			local all = catalog.peds()
			if #all == 0 then
				return
			end
			local pick = joaat(all[math.random(1, #all)])
			task.spawn(function()
				if not streaming.load(pick) then
					return
				end
				local dir = geo.direction(camera.getGameplayCamRot(2))
				local p = ped.createPed(26, pick, camera.getGameplayCamCoord() + dir * 4, entity.getEntityHeading(me()), false, false)
				streaming.setModelAsNoLongerNeeded(pick)
				if p then
					ped.setPedToRagdoll(p, 4000, 4000, 0, false, false, false)
					entity.setEntityVelocity(p, dir * 40)
					entity.setPedAsNoLongerNeeded(p)
				end
			end)
		end,
	}))
	-- Things to throw (those this game has: checked when shot).
	local THINGS = { "prop_beachball_02", "prop_bowling_ball", "prop_toilet_01", "prop_barrel_02a", "prop_roadcone02a", "prop_byard_floatpile" }
	bind.feature(menu, "weapons.objectGun", "Object gun", "Shoot balls, barrels, toilets", bind.storyOnly({
		tick = function()
			if not ped.isPedShooting(me()) then
				return
			end
			local pick = THINGS[math.random(1, #THINGS)]
			if not streaming.isModelInCdimage(pick) then
				return
			end
			task.spawn(function()
				if not streaming.load(pick) then
					return
				end
				local dir = geo.direction(camera.getGameplayCamRot(2))
				local thing = object.createObject(pick, camera.getGameplayCamCoord() + dir * 4, false, false, true)
				streaming.setModelAsNoLongerNeeded(pick)
				if thing then
					entity.setEntityVelocity(thing, dir * 50)
					entity.setObjectAsNoLongerNeeded(thing)
				end
			end)
		end,
	}))
	bind.feature(menu, "weapons.reviveGun", "Revive gun", "The dead you shoot get up again", {
		tick = function()
			if not ped.isPedShooting(me()) then
				return
			end
			local target = aimed()
			if target and entity.isEntityAPed(target) and entity.isEntityDead(target, false) and not ped.isPedAPlayer(target :: Ped) then
				local p = target :: Ped
				ped.resurrectPed(p)
				ped.reviveInjuredPed(p)
				entity.setEntityHealth(p, 200, nil)
				task.clearPedTasksImmediately(p)
			end
		end,
	})
	bind.feature(menu, "weapons.magnetGun", "Magnet gun", "Aim to pull what is near your aim to it", {
		tick = function()
			if not player.isPlayerFreeAiming(game.player()) then
				return
			end
			local point = camera.getGameplayCamCoord() + geo.direction(camera.getGameplayCamRot(2)) * 15
			local near: { Entity } = {}
			local mine = game.vehicle()
			for _, v in world.nearby("vehicles", point, 12) do
				if v ~= mine then
					table.insert(near, v)
				end
			end
			for _, p in world.nearby("peds", point, 12) do
				if not ped.isPedAPlayer(p) then
					table.insert(near, p)
				end
			end
			for _, o in world.nearby("objects", point, 12) do
				table.insert(near, o)
			end
			for _, e in near do
				entity.setEntityVelocity(e, (point - entity.getEntityCoords(e, true)) * 3)
			end
		end,
	})
	bind.feature(menu, "weapons.paintGun", "Paint gun", "Vehicles you shoot get a new colour", {
		tick = function()
			if not ped.isPedShooting(me()) then
				return
			end
			local target = aimed()
			if target and entity.isEntityAVehicle(target) then
				local car = target :: Vehicle
				local r, g, b = math.random(0, 255), math.random(0, 255), math.random(0, 255)
				vehicle.setVehicleCustomPrimaryColour(car, r, g, b)
				vehicle.setVehicleCustomSecondaryColour(car, r, g, b)
			end
		end,
	})
	bind.feature(menu, "weapons.driveGun", "Drive-it gun", "Shoot a vehicle and you are at its wheel", {
		tick = function()
			if not ped.isPedShooting(me()) then
				return
			end
			local target = aimed()
			if not target or not entity.isEntityAVehicle(target) then
				return
			end
			local car = target :: Vehicle
			local driver = vehicle.getPedInVehicleSeat(car, VehicleSeat.DRIVER, false)
			if driver and ped.isPedAPlayer(driver) then
				return
			end
			if driver then
				task.clearPedTasksImmediately(driver)
			end
			ped.setPedIntoVehicle(me(), car, VehicleSeat.DRIVER)
		end,
	})
	bind.feature(menu, "weapons.swapGun", "Body-swap gun", "Story mode: become the person you shoot", bind.storyOnly({
		tick = function()
			if not ped.isPedShooting(me()) then
				return
			end
			local target = aimed()
			if target and entity.isEntityAPed(target) and not ped.isPedAPlayer(target :: Ped) and not entity.isEntityDead(target, false) then
				player.changePlayerPed(game.player(), target :: Ped, true, false)
			end
		end,
	}))
	bind.feature(menu, "weapons.vehicleGun", "Vehicle gun", "Shoot cars", bind.storyOnly({
		tick = function()
			if not ped.isPedShooting(me()) then
				return
			end
			local all = catalog.vehicles()
			if #all == 0 then
				return
			end
			local pick = all[math.random(1, #all)]
			-- Spawned in a task of its own: the model loads over a few frames.
			task.spawn(function()
				if not streaming.load(pick.hash) then
					return
				end
				local dir = geo.direction(camera.getGameplayCamRot(2))
				local car = vehicle.createVehicle(pick.hash, camera.getGameplayCamCoord() + dir * 8, entity.getEntityHeading(me()), false, false, false)
				streaming.setModelAsNoLongerNeeded(pick.hash)
				if car then
					entity.setEntityVelocity(car, dir * 60)
					entity.setEntityAsNoLongerNeeded(car)
				end
			end)
		end,
	}))
end

-- What the player's gun points at: from its muzzle (or the camera) along the camera's look.
local function aimLine(): (vector, vector)
	local dir = geo.direction(camera.getGameplayCamRot(2))
	local gun = weapon.getCurrentPedWeaponEntityIndex(game.me(), true)
	local from = if gun then entity.getEntityCoords(gun, true) else camera.getGameplayCamCoord()
	return from, camera.getGameplayCamCoord() + dir * 300
end

local function aiming(menu: Menu)
	menu:separator("Aiming")
	local SNIPERS = joaat("GROUP_SNIPER")
	bind.feature(menu, "weapons.laser", "Laser sight", "A red line where you aim", {
		tick = function()
			if not player.isPlayerFreeAiming(game.player()) then
				return
			end
			local from, to = aimLine()
			graphics.drawLine(from.x, from.y, from.z, to.x, to.y, to.z, 255, 0, 0, 200)
		end,
	})
	-- On only while a sniper rifle aims (and off again after: Thermal vision is Self's).
	local seeing = false
	bind.feature(menu, "weapons.thermal", "Thermal scope", "Snipers see heat when aiming", {
		tick = function()
			local now = player.isPlayerFreeAiming(game.player()) and weapon.getWeapontypeGroup(weapon.getSelectedPedWeapon(game.me())) == SNIPERS
			if now ~= seeing then
				graphics.setSeethrough(now)
				seeing = now
			end
		end,
		off = function()
			if seeing then
				graphics.setSeethrough(false)
				seeing = false
			end
		end,
	})
	-- Shots every frame while the trigger is held, however it is held (keyboard, pad, touch:
	-- the game shooting counts too, for a moment after each shot).
	local lastShot = 0
	bind.feature(menu, "weapons.rapid", "Rapid fire", "A shot every frame while you shoot", {
		tick = function()
			local me = game.me()
			-- 4: guns (not melee or thrown).
			if ped.isPedInAnyVehicle(me, false) or not weapon.isPedArmed(me, 4) then
				return
			end
			if ped.isPedShooting(me) then
				lastShot = os.clock()
			end
			local held = pad.isControlPressed(0, Control.ATTACK) or pad.isDisabledControlPressed(0, Control.ATTACK)
			if held or os.clock() - lastShot < 0.25 then
				local from, to = aimLine()
				misc.shootSingleBulletBetweenCoords(from, to, 30, true, weapon.getSelectedPedWeapon(me), me, true, false, -1)
			end
		end,
	})
	-- Slow while aiming; the game's speed again after (only what this feature set).
	local slowed = false
	bind.feature(menu, "weapons.slowAim", "Slow motion when aiming", "Time slows down while you aim", {
		tick = function()
			local now = player.isPlayerFreeAiming(game.player())
			if now ~= slowed then
				misc.setTimeScale(if now then 0.35 else 1)
				slowed = now
			end
		end,
		off = function()
			if slowed then
				misc.setTimeScale(1)
				slowed = false
			end
		end,
	})
	bind.feature(menu, "weapons.shotgun", "Shotgun bullets", "Each shot is eight, spread", {
		tick = function()
			local me = game.me()
			if not ped.isPedShooting(me) then
				return
			end
			local from, to = aimLine()
			for _ = 1, 7 do
				local spread = vector.create(math.random(-8, 8), math.random(-8, 8), math.random(-8, 8))
				misc.shootSingleBulletBetweenCoords(from, to + spread, 30, true, weapon.getSelectedPedWeapon(me), me, true, false, -1)
			end
		end,
	})
	bind.feature(menu, "weapons.cartoon", "Cartoon gun", "A clown's puff at the muzzle", {
		tick = function()
			local me = game.me()
			if not ped.isPedShooting(me) or not streaming.load("scr_rcbarry2", "ptfx", 0) then
				return
			end
			local gun = weapon.getCurrentPedWeaponEntityIndex(me, true)
			if gun then
				graphics.useParticleFxAsset("scr_rcbarry2")
				graphics.startParticleFxNonLoopedOnEntity("muz_clown", gun, vector.create(0.9, 0, 0), vector.zero, 1, false, false, false)
			end
		end,
	})
	-- The game's targeting: SET_PLAYER_TARGETING_MODE's 0-3.
	menu:choice("Targeting", { "Assisted", "Assisted partial", "Assisted free aim", "Free aim" }, 1, "How aiming picks targets", bind.safe("Targeting", function(i: number)
		player.setPlayerTargetingMode(i - 1)
	end))
end

local function parachute(menu: Menu)
	menu:separator("Parachute")
	menu:number("Canopy", player.getPlayerParachuteTintIndex(game.player()), { min = 0, max = 7, note = "Its colours" }, bind.safe("Canopy", function(v)
		player.setPlayerParachuteTintIndex(game.player(), v)
	end))
	local smoke = menu:color("Smoke colour", { r = 255, g = 255, b = 255 }, bind.safe("Smoke colour", function(c)
		player.setPlayerParachuteSmokeTrailColor(game.player(), c.r, c.g, c.b)
	end))
	bind.feature(menu, "weapons.smoke", "Smoke trail", "Behind you while you fly the parachute", {
		on = function()
			local c = smoke.value
			player.setPlayerParachuteSmokeTrailColor(game.player(), c.r, c.g, c.b)
			player.setPlayerCanLeaveParachuteSmokeTrail(game.player(), true)
		end,
		off = function()
			player.setPlayerCanLeaveParachuteSmokeTrail(game.player(), false)
		end,
	})
end

function M.build(root: Menu)
	local menu = root:submenu("Weapons", "Every weapon, attachments, loadouts, ammo, damage, aiming, trick guns, parachute")
	arsenal(menu)
	inHand(menu)
	loadouts(menu)
	ammo(menu)
	damage(menu)
	aiming(menu)
	guns(menu)
	parachute(menu)
end

return M
