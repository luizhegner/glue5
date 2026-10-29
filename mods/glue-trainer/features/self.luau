--!strict
-- You: health and protection, the police, abilities, sight.

local bind = require("../core/bind")
local game = require("../core/game")
local loop = require("../core/loop")
local prefs = require("../core/prefs")

local M = {}

-- What SET_ENTITY_PROOFS protects from, in its order.
local PROOFS = { "Bullets", "Fire", "Explosions", "Collisions", "Melee", "Steam", "Drowning" }

-- The proofs chosen, every frame while any is (missions reset them).
local function proofs(root: Menu)
	local item: Multi
	local function apply()
		local on = {}
		for _, i in item.value do
			on[i] = true
		end
		entity.setEntityProofs(game.me(), on[1] == true, on[2] == true, on[3] == true, on[4] == true, on[5] == true, on[6] == true, false, on[7] == true)
	end
	local function follow()
		prefs.set("self.proofs", item.value)
		if #item.value > 0 then
			loop.add("self.proofs", apply)
		else
			loop.remove("self.proofs")
			apply()
		end
	end
	item = root:multi("Proofs", PROOFS, prefs.get("self.proofs", {}), "Hurt by everything but these", bind.safe("Proofs", follow))
	if #item.value > 0 then
		follow()
	end
end

-- Story mode: up again where you fell, without the hospital and its bill. GTA Online's
-- scripts respawn the player their own way: left to them.
local function respawn(root: Menu)
	local function story(): boolean
		return not network.networkIsSessionStarted()
	end
	bind.feature(root, "self.respawn", "Get up where you fell", "Story mode: no hospital, nothing to pay", {
		on = function()
			misc.setFadeOutAfterDeath(false)
		end,
		tick = function()
			if not story() then
				return
			end
			-- The game's restart (hospital, fees) is skipped from the moment the player dies.
			misc.pauseDeathArrestRestart(true)
			misc.ignoreNextRestart(true)
			local me = game.me()
			if not entity.isEntityDead(me, false) then
				return
			end
			local at = entity.getEntityCoords(me, true)
			network.networkResurrectLocalPlayer(at, entity.getEntityHeading(me), 0, false, false, 0, 0)
			me = game.me()
			entity.setEntityHealth(me, entity.getEntityMaxHealth(me), nil)
			task.clearPedTasksImmediately(me)
			camera.doScreenFadeIn(500)
		end,
		off = function()
			misc.pauseDeathArrestRestart(false)
			misc.ignoreNextRestart(false)
			misc.setFadeOutAfterDeath(true)
		end,
	})
end

local function health(root: Menu)
	root:separator("Health")
	bind.feature(root, "self.god", "God mode", "Nothing hurts you", {
		-- Every frame: missions switch it off, and a new model is a new ped.
		tick = function()
			player.setPlayerInvincible(game.player(), true)
		end,
		off = function()
			player.setPlayerInvincible(game.player(), false)
		end,
	})
	proofs(root)
	bind.slider(root, "self.regen", "Health regeneration", 1, 0, 10, { step = 0.5, unit = "×", note = "How fast health comes back" }, function(v)
		player.setPlayerHealthRechargeMultiplier(game.player(), v)
	end)
	-- Invincible only while falling (God mode, when on, keeps it).
	local falling = false
	bind.feature(root, "self.noFall", "No fall damage", "Land from any height", {
		tick = function()
			local me = game.me()
			local now = ped.isPedFalling(me) or ped.isPedInParachuteFreeFall(me)
			if now ~= falling and not bind.on("self.god") then
				player.setPlayerInvincible(game.player(), now)
			end
			falling = now
		end,
		off = function()
			if falling and not bind.on("self.god") then
				player.setPlayerInvincible(game.player(), false)
			end
			falling = false
		end,
	})
	bind.feature(root, "self.noCritical", "No headshots", "No critical hits", {
		tick = function()
			ped.setPedSuffersCriticalHits(game.me(), false)
		end,
		off = function()
			ped.setPedSuffersCriticalHits(game.me(), true)
		end,
	})
	bind.feature(root, "self.ragdoll", "No ragdoll", "Never knocked over", {
		tick = function()
			ped.setPedCanRagdoll(game.me(), false)
			ped.setPedCanRagdollFromPlayerImpact(game.me(), false)
		end,
		off = function()
			ped.setPedCanRagdoll(game.me(), true)
			ped.setPedCanRagdollFromPlayerImpact(game.me(), true)
		end,
	})
	bind.command(root, "self.heal", "Heal", { note = "Full health and armour", action = "Heal" }, function(): string?
		local me = game.me()
		entity.setEntityHealth(me, entity.getEntityMaxHealth(me), nil)
		ped.setPedArmour(me, player.getPlayerMaxArmour(game.player()))
		return "Healed"
	end)
	bind.command(root, "self.clean", "Clean up", { note = "Blood, dirt and water off", action = "Clean" }, function(): string?
		local me = game.me()
		ped.clearPedBloodDamage(me)
		ped.resetPedVisibleDamage(me)
		ped.clearPedWetness(me)
		return nil
	end)
	bind.command(root, "self.die", "Die", { action = "Die" }, function(): string?
		entity.setEntityHealth(game.me(), 0, nil)
		return nil
	end)
	bind.feature(root, "self.oxygen", "Unlimited oxygen", "Stay under water as long as you like", {
		tick = function()
			player.setPlayerUnderwaterBreathPercentRemaining(game.player(), 100)
		end,
	})
	respawn(root)
end

local function police(root: Menu)
	root:separator("Police")
	bind.feature(root, "self.neverWanted", "Never wanted", "Stars go as soon as they come", {
		tick = function()
			local me = game.player()
			if player.getPlayerWantedLevel(me) > 0 then
				player.clearPlayerWantedLevel(me)
			end
		end,
	})
	local wanted = root:number("Wanted level", 0, { min = 0, max = 5, note = "Stars, now" }, bind.safe("Wanted level", function(stars)
		local me = game.player()
		player.setPlayerWantedLevel(me, stars, false)
		player.setPlayerWantedLevelNow(me, false)
	end))
	root:onOpen(bind.safe("Self", function()
		wanted.value = player.getPlayerWantedLevel(game.player())
	end))
	bind.feature(root, "self.ignored", "Ignored by everyone", "Police and gangs leave you be", {
		tick = function()
			player.setEveryoneIgnorePlayer(game.player(), true)
			player.setPoliceIgnorePlayer(game.player(), true)
		end,
		off = function()
			player.setEveryoneIgnorePlayer(game.player(), false)
			player.setPoliceIgnorePlayer(game.player(), false)
		end,
	})
end

local function abilities(root: Menu)
	root:separator("Abilities")
	bind.feature(root, "self.superJump", "Super jump", "Jump over houses", {
		tick = function()
			misc.setSuperJumpThisFrame(game.player())
		end,
	})
	bind.feature(root, "self.superRun", "Super run", "Sprint as fast as a car", {
		tick = function()
			local me = game.me()
			if pad.isControlPressed(0, Control.SPRINT) and not ped.isPedInAnyVehicle(me, false) and not entity.isEntityInAir(me) then
				local ahead = entity.getEntityForwardVector(me) * 25
				entity.setEntityVelocity(me, vector.create(ahead.x, ahead.y, entity.getEntityVelocity(me).z))
			end
		end,
	})
	-- Flying while Jump is held, where the camera looks; let go and you glide down.
	local flying = false
	bind.feature(root, "self.fly", "Fly like a superhero", "Hold Jump to fly where you look; let go to glide", {
		tick = function()
			local me = game.me()
			if ped.isPedInAnyVehicle(me, false) then
				flying = false
				return
			end
			if pad.isControlPressed(0, Control.JUMP) then
				flying = true
				entity.setEntityVelocity(me, geo.direction(camera.getGameplayCamRot(2)) * 35)
			elseif flying then
				if entity.isEntityInAir(me) then
					local v = entity.getEntityVelocity(me)
					entity.setEntityVelocity(me, vector.create(v.x * 0.97, v.y * 0.97, math.max(v.z, -3)))
				else
					flying = false
				end
			end
		end,
		off = function()
			flying = false
		end,
	})
	-- People and vehicles within reach pushed out of it (every other frame: the pools are
	-- walked each time).
	local frame = 0
	bind.feature(root, "self.forceField", "Force field", "People and vehicles near you are thrown back", {
		tick = function()
			frame += 1
			if frame % 2 == 1 then
				return
			end
			local me = game.me()
			local mine = game.vehicle()
			local at = entity.getEntityCoords(me, true)
			local function push(e: Entity)
				if mine and e == (mine :: Entity) then
					return
				end
				local away = entity.getEntityCoords(e, true) - at
				local out = vector.normalize(vector.create(away.x, away.y, 0)) * 25
				entity.setEntityVelocity(e, vector.create(out.x, out.y, 6))
			end
			for _, p in world.nearby("peds", me, 8) do
				if not ped.isPedInAnyVehicle(p, false) then
					push(p)
				end
			end
			for _, v in world.nearby("vehicles", me, 10) do
				push(v)
			end
		end,
	})
	-- The game takes 1 to 1.49 (more is ignored).
	bind.slider(root, "self.run", "Run speed", 1, 1, 1.49, { step = 0.01, unit = "×" }, function(v)
		player.setRunSprintMultiplierForPlayer(game.player(), v)
	end)
	bind.slider(root, "self.swim", "Swim speed", 1, 1, 1.49, { step = 0.01, unit = "×" }, function(v)
		player.setSwimMultiplierForPlayer(game.player(), v)
	end)
	bind.feature(root, "self.stamina", "Unlimited stamina", "Sprint and swim for ever", {
		tick = function()
			player.restorePlayerStamina(game.player(), 1)
		end,
	})
	bind.feature(root, "self.special", "Unlimited special ability", "A story character's, always full", {
		tick = function()
			player.specialAbilityFillMeter(game.player(), true, 0)
		end,
	})
	bind.feature(root, "self.quiet", "Noiseless", "Nobody hears you", {
		tick = function()
			player.setPlayerNoiseMultiplier(game.player(), 0)
		end,
		off = function()
			player.setPlayerNoiseMultiplier(game.player(), 1)
		end,
	})
	bind.feature(root, "self.parachute", "Always a parachute", "One on your back whenever you have none", {
		tick = function()
			local me = game.me()
			local chute = joaat("GADGET_PARACHUTE")
			if not weapon.hasPedGotWeapon(me, chute, 0) then
				weapon.giveWeaponToPed(me, chute, 1, false, false)
			end
		end,
	})
	bind.feature(root, "self.invisible", "Invisible", "Others do not see you; you still do", {
		tick = function()
			entity.setEntityVisible(game.me(), false, false)
			network.setEntityLocallyVisible(game.me())
		end,
		off = function()
			entity.setEntityVisible(game.me(), true, false)
		end,
	})
	bind.feature(root, "self.burn", "On fire", "Burning, and fireproof", {
		on = function()
			entity.setEntityProofs(game.me(), false, true, false, false, false, false, false, false)
			fire.startEntityFire(game.me())
		end,
		off = function()
			fire.stopEntityFire(game.me())
			entity.setEntityProofs(game.me(), false, false, false, false, false, false, false, false)
		end,
	})
end

-- The game's particle effects that work on a person: name, asset, effect.
local EFFECTS = {
	{ "Clown flowers", "scr_rcbarry2", "scr_clown_appears" },
	{ "Alien light", "scr_rcbarry1", "scr_alien_disintegrate" },
	{ "Alien teleport", "scr_rcbarry1", "scr_alien_teleport" },
	{ "Sparks", "core", "ent_brk_sparking_wires" },
	{ "Banknotes", "core", "ent_brk_banknotes" },
	{ "Fireworks", "scr_indep_fireworks", "scr_indep_firework_trailburst" },
}

local function body(root: Menu)
	root:separator("Body")
	local names = {}
	for _, e in EFFECTS do
		table.insert(names, e[1])
	end
	local effect = bind.choice(root, "self.effect", "Effect", names, names[1], "What Effects shows on you")
	-- Shown again every half a second, as the effects are bursts.
	local next = 0
	bind.feature(root, "self.effects", "Effects", "Particles around you, as chosen above", {
		tick = function()
			if os.clock() < next then
				return
			end
			next = os.clock() + 0.5
			local e = EFFECTS[effect.value]
			streaming.requestNamedPtfxAsset(e[2])
			if not streaming.hasNamedPtfxAssetLoaded(e[2]) then
				return
			end
			graphics.useParticleFxAsset(e[2])
			-- In GTA Online, for everyone to see.
			if game.online() then
				graphics.startNetworkedParticleFxNonLoopedOnEntity(e[3], game.me(), vector.zero, vector.zero, 1, false, false, false)
			else
				graphics.startParticleFxNonLoopedOnEntity(e[3], game.me(), vector.zero, vector.zero, 1, false, false, false)
			end
		end,
		off = function()
			next = 0
		end,
	})
	-- Fire from the mouth, as long as it is on (and fireproof meanwhile).
	local flame: number? = nil
	bind.feature(root, "self.breatheFire", "Breathe fire", "Flames from your mouth", {
		on = function()
			if not streaming.load("core", "ptfx") then
				error("the flames did not load", 0)
			end
			local me = game.me()
			entity.setEntityProofs(me, false, true, false, false, false, false, false, false)
			graphics.useParticleFxAsset("core")
			-- Out of the mouth, forward (the head bone's axes).
			flame = graphics.startParticleFxLoopedOnEntityBone("ent_sht_flame", me, -0.02, 0.2, 0, 90, 100, 90, ped.getPedBoneIndex(me, Bone.HEAD), 1, false, false, false)
		end,
		off = function()
			if flame then
				graphics.stopParticleFxLooped(flame, false)
				flame = nil
			end
			entity.setEntityProofs(game.me(), false, false, false, false, false, false, false, false)
		end,
	})
	bind.command(root, "self.clone", "Clone yourself", { note = "A copy of you, standing there", action = "Clone" }, function(): string?
		local copy = ped.clonePed(game.me(), game.online(), false, true)
		if copy then
			entity.setPedAsNoLongerNeeded(copy)
		end
		return nil
	end)
	bind.command(root, "self.ragdoll", "Fall over", { action = "Fall" }, function(): string?
		ped.setPedToRagdoll(game.me(), 3000, 3000, 0, false, false, false)
		return nil
	end)
	-- The game's damage looks: name, pack.
	local packs = {
		{ "Hit by a car", "BigHitByVehicle" },
		{ "Run over", "BigRunOverByVehicle" },
		{ "Explosion", "Explosion_Large" },
		{ "Burnt", "Burnt_Ped_0" },
		{ "Fell", "Fall" },
		{ "Beaten", "Skin_Melee_0" },
		{ "Tortured", "SCR_Torture" },
		{ "Dumpster", "SCR_Dumpster" },
	}
	local packNames = {}
	for _, pk in packs do
		table.insert(packNames, pk[1])
	end
	local pack = bind.choice(root, "self.damagePack", "Wounds", packNames, packNames[1], "What Look hurt puts on you")
	bind.command(root, "self.hurt", "Look hurt", { note = "Blood and bruises, no harm (Clean up takes them off)", action = "Apply" }, function(): string?
		ped.applyPedDamagePack(game.me(), packs[pack.value][2], 0, 1)
		return nil
	end)
	bind.slider(root, "self.wet", "Wet", 0, 0, 2, { step = 0.1, unit = "m", note = "How high the water came up" }, function(v)
		if v > 0 then
			ped.setPedWetnessHeight(game.me(), v)
		else
			ped.clearPedWetness(game.me())
		end
	end)
	bind.slider(root, "self.sweat", "Sweat", 0, 0, 100, { step = 10, unit = "%" }, function(v)
		ped.setPedSweat(game.me(), v)
	end)
	-- Lines every ped's voice has (the game picks the voice's own recording).
	local lines = {
		{ "Hello", "GENERIC_HI" },
		{ "Goodbye", "GENERIC_BYE" },
		{ "Thanks", "GENERIC_THANKS" },
		{ "How's it going", "GENERIC_HOWS_IT_GOING" },
		{ "Whatever", "GENERIC_WHATEVER" },
		{ "Insult", "GENERIC_INSULT_HIGH" },
		{ "Curse", "GENERIC_CURSE_HIGH" },
		{ "Shocked", "GENERIC_SHOCKED_HIGH" },
		{ "Scared", "GENERIC_FRIGHTENED_HIGH" },
		{ "Kifflom", "KIFFLOM_GREET" },
	}
	local says = {}
	for _, l in lines do
		table.insert(says, l[1])
	end
	local line = bind.choice(root, "self.line", "Line", says, says[1], "What Say it says, in your voice")
	bind.command(root, "self.say", "Say it", { action = "Say" }, function(): string?
		audio.playPedAmbientSpeechNative(game.me(), lines[line.value][2], "SPEECH_PARAMS_FORCE_SHOUTED", 0)
		return nil
	end)
end

local function sight(root: Menu)
	root:separator("Sight")
	bind.feature(root, "self.nightVision", "Night vision", nil, {
		on = function()
			graphics.setNightvision(true)
		end,
		off = function()
			graphics.setNightvision(false)
		end,
	})
	bind.feature(root, "self.thermal", "Thermal vision", nil, {
		on = function()
			graphics.setSeethrough(true)
		end,
		off = function()
			graphics.setSeethrough(false)
		end,
	})
	bind.feature(root, "self.drunk", "Drunk", "Walk, sway and see as after a night out", {
		on = function()
			streaming.load("move_m@drunk@verydrunk", "clipSet")
			ped.setPedMovementClipset(game.me(), "move_m@drunk@verydrunk", 1)
			camera.shakeGameplayCam("DRUNK_SHAKE", 1)
			graphics.setTimecycleModifier("Drunk")
		end,
		off = function()
			ped.resetPedMovementClipset(game.me(), 1)
			camera.stopGameplayCamShaking(true)
			graphics.clearTimecycleModifier()
		end,
	})
end

function M.build(root: Menu)
	local me = root:submenu("Self", "Health, protection, police, abilities, body, sight")
	health(me)
	police(me)
	abilities(me)
	body(me)
	sight(me)
end

return M
