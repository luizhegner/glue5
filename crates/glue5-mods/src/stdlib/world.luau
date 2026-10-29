-- The world around: entities near a place, raycasts, the ground, the
-- waypoint, the names of places.

local wait = wait
local coords = entity.getEntityCoords

local pools = {
	vehicles = world.vehicles,
	peds = world.peds,
	objects = world.objects,
	pickups = world.pickups,
}

-- The pool's entities within `radius` of `around` (a place, or an entity
-- left out of them), the closest first, `max` of them.
local function within(what: string, kind: string, around: any, radius: number, max: number?): { number }
	local pool = pools[kind]
	if not pool then
		error(`world.{what}: no kind "{kind}" (vehicles, peds, objects, pickups)`, 3)
	end
	if type(around) == "vector" then
		return pool({ near = around, radius = radius, max = max })
	end
	local found = pool({ near = coords(around, false), radius = radius, max = max and max + 1 })
	local i = table.find(found, around)
	if i then
		table.remove(found, i)
	elseif max and #found > max then
		table.remove(found)
	end
	return found
end

world.nearby = function(kind: string, around: any, radius: number): { number }
	return within("nearby", kind, around, radius)
end

world.closest = function(kind: string, around: any, radius: number?): number?
	return within("closest", kind, around, radius or math.huge, 1)[1]
end

local startProbe = shapetest.startShapeTestLosProbe
local probeResult = shapetest.getShapeTestResultIncludingMaterial

world.raycast = function(from: vector, to: vector, options: { flags: number?, ignore: number? }?): any
	local flags = options and options.flags or -1
	local ignore = options and options.ignore
	-- 7: the collider types the game's scripts ask for.
	local probe = startProbe(from, to, flags, ignore, 7)
	if probe == 0 then
		return nil
	end
	while true do
		local status, hit, pos, normal, material, e = probeResult(probe)
		if status == 2 then
			if not hit or hit == 0 then
				return nil
			end
			return { pos = pos, normal = normal, entity = e, material = material }
		elseif status == 0 then
			return nil
		end
		wait()
	end
end

local raycast = world.raycast
local camCoord, camRot = camera.getGameplayCamCoord, camera.getGameplayCamRot
local direction = geo.direction
local playerPedId = player.playerPedId

world.crosshair = function(options: { distance: number?, flags: number?, ignore: number? }?): any
	local from = camCoord()
	local to = from + direction(camRot(2)) * (options and options.distance or 1000)
	return raycast(from, to, { flags = options and options.flags, ignore = options and options.ignore or playerPedId() })
end

local groundZ = misc.getGroundZFor3dCoord

world.groundZ = function(pos: vector): number?
	local found, z = groundZ(pos, false, false)
	return if found then z else nil
end

local h = hud

world.waypoint = function(): vector?
	if not h.isWaypointActive() then
		return nil
	end
	return h.getBlipInfoIdCoord(h.getFirstBlipInfoId(h.getWaypointBlipEnumId()))
end

world.setWaypoint = function(pos: vector?)
	if pos then
		h.setNewWaypoint(pos.x, pos.y)
	else
		h.setWaypointOff()
	end
end

world.zoneName = function(pos: vector): (string, string)
	local code = zone.getNameOfZone(pos)
	return h.getFilenameForAudioConversation(code), code
end

world.streetName = function(pos: vector): (string, string?)
	local street, crossing = path.getStreetNameAtCoord(pos.x, pos.y, pos.z)
	return h.getStreetNameFromHashKey(street), if crossing ~= 0 then h.getStreetNameFromHashKey(crossing) else nil
end
