--!strict
-- What the features share about the game: the player, their vehicle, the game's own texts,
-- moving the player, telling them what happened.

local prefs = require("./prefs")

local M = {}

function M.me(): Ped
	return player.playerPedId()
end

function M.player(): Player
	return player.playerId()
end

-- The vehicle the player is in (any seat), nil on foot.
function M.vehicle(): Vehicle?
	local me = M.me()
	if not ped.isPedInAnyVehicle(me, false) then
		return nil
	end
	return ped.getVehiclePedIsIn(me, false)
end

-- What the player moves: their vehicle when they drive it, else their ped.
function M.mover(): Entity
	local car = M.vehicle()
	if car and vehicle.getPedInVehicleSeat(car, VehicleSeat.DRIVER, false) == M.me() then
		return car
	end
	return M.me()
end

-- A game text by its label ("VEH_CLASS_0"), in the game's language; `fallback` (the label) when
-- the game has none.
function M.label(name: string, fallback: string?): string
	if name ~= "" and hud.doesTextLabelExist(name) then
		local text = hud.getFilenameForAudioConversation(name)
		if text ~= "" and text ~= "NULL" then
			return text
		end
	end
	return fallback or name
end

-- "WEAPON_ASSAULTRIFLE_MK2" as people read it: "Assaultrifle Mk2" (for names the game has no
-- text for).
function M.pretty(name: string): string
	local words = {}
	for word in string.gmatch(name, "[^_]+") do
		table.insert(words, string.upper(string.sub(word, 1, 1)) .. string.lower(string.sub(word, 2)))
	end
	return table.concat(words, " ")
end

-- The trainer's own notifications, unless the player turned them off.
function M.notify(text: string)
	if prefs.data.notify then
		ui.notify(text)
	end
end

-- Names in alphabetical order, case aside (for table.sort).
function M.byName(a: string, b: string): boolean
	return string.lower(a) < string.lower(b)
end

-- The ground's height at x, y: the world there is loaded first (the player is moved up
-- there while it loads, as teleports must). nil if none was found.
local function findGround(e: Entity, at: vector): number?
	for _, z in { 1000, 800, 600, 400, 300, 200, 150, 100, 75, 50, 25, 10, 0 } do
		local probe = vector.create(at.x, at.y, z)
		entity.setEntityCoordsNoOffset(e, probe, false, false, true)
		streaming.requestCollisionAtCoord(at.x, at.y, z)
		wait(30)
		local found = world.groundZ(probe)
		if found then
			return found
		end
	end
	return nil
end

-- Teleports this far or farther fly there (when the player wants it).
local FLY_FROM = 150

-- The camera's journey for a teleport, as the game's character switch looks: up into the sky
-- above where the player is, across to above `to`, and down to them there. `move` moves the
-- player meanwhile, under the camera (it may wait for the ground).
local function journey(to: vector, move: () -> ())
	local from = M.position()
	local distance = vector.magnitude(vector.create(to.x - from.x, to.y - from.y, 0))
	local high = math.max(from.z, to.z) + math.clamp(distance * 0.2, 150, 600)
	local rot = camera.getGameplayCamRot(2)
	local down = vector.create(-89, 0, rot.z)
	local name = "DEFAULT_SCRIPTED_CAMERA"
	local start = camera.createCamWithParams(name, camera.getGameplayCamCoord(), rot, camera.getGameplayCamFov(), false, 2)
	local up = camera.createCamWithParams(name, vector.create(from.x, from.y, high), down, 50, false, 2)
	local over = camera.createCamWithParams(name, vector.create(to.x, to.y, high), down, 50, false, 2)
	local ok, err = pcall(function()
		camera.setCamActive(start, true)
		camera.renderScriptCams(true, false, 0, true, true, 0)
		camera.setCamActiveWithInterp(up, start, 900, 1, 1)
		wait(900)
		-- Across in 1 to 2.5 s, the world there loading meanwhile.
		local across = math.floor(math.clamp(distance / 3, 1000, 2500))
		local left = os.clock() + across / 1000
		streaming.setFocusPosAndVel(to.x, to.y, to.z, 0, 0, 0)
		camera.setCamActiveWithInterp(over, up, across, 1, 1)
		move()
		wait(math.max(0, math.floor((left - os.clock()) * 1000)))
		streaming.clearFocus()
		-- Down to the gameplay camera, wherever the player is now.
		camera.renderScriptCams(false, true, 1200, true, true, 0)
		wait(1200)
	end)
	for _, cam in { start, up, over } do
		if cam and camera.doesCamExist(cam) then
			camera.destroyCam(cam, false)
		end
	end
	if not ok then
		streaming.clearFocus()
		camera.renderScriptCams(false, false, 0, true, true, 0)
		error(err, 0)
	end
end

-- Moves the player (with their vehicle) to `at`; with `ground`, at.z is looked up (a map
-- point has none). Facing `heading` when given. Far teleports fly there, when the player
-- wants it (Settings).
function M.teleport(at: vector, ground: boolean?, heading: number?)
	local function move()
		M.teleportNow(at, ground, heading)
	end
	if prefs.data.flyTeleports and vector.magnitude(at - M.position()) >= FLY_FROM then
		journey(at, move)
	else
		move()
	end
end

-- As M.teleport, at once.
function M.teleportNow(at: vector, ground: boolean?, heading: number?)
	local e = M.mover()
	local z = at.z
	if ground then
		local found = findGround(e, at)
		if found then
			z = found + 1
		else
			-- Water or not loaded: from up high, with a parachute.
			z = 1000
			weapon.giveWeaponToPed(M.me(), joaat("GADGET_PARACHUTE"), 1, false, false)
		end
	end
	entity.setEntityCoordsNoOffset(e, vector.create(at.x, at.y, z), false, false, true)
	if heading then
		entity.setEntityHeading(e, heading)
	end
end

-- In a GTA Online session (where what the trainer makes is networked).
function M.online(): boolean
	return network.networkIsSessionStarted()
end

-- Where the player is.
function M.position(): vector
	return entity.getEntityCoords(M.me(), true)
end

return M
