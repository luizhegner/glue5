-- `streaming.load` and `spawn.*`: the game's assets loaded, waiting for
-- them as the game's scripts do; entities made where the player is.

local wait = wait
local getGameTimer = misc.getGameTimer
local s, g = streaming, graphics

-- Each kind of asset: how it is asked for, how it is known loaded.
local kinds = {
	model = { s.requestModel, s.hasModelLoaded },
	anim = { s.requestAnimDict, s.hasAnimDictLoaded },
	animSet = { s.requestAnimSet, s.hasAnimSetLoaded },
	clipSet = { s.requestClipSet, s.hasClipSetLoaded },
	ptfx = { s.requestNamedPtfxAsset, s.hasNamedPtfxAssetLoaded },
	texture = { function(name)
		g.requestStreamedTextureDict(name, false)
	end, g.hasStreamedTextureDictLoaded },
}

local isModelInCdimage = s.isModelInCdimage

streaming.load = function(asset: string | number, kind: string?, timeoutMs: number?): boolean
	local k = kinds[kind or "model"]
	if not k then
		error(`streaming.load: no kind of asset "{kind}" (model, anim, animSet, clipSet, ptfx, texture)`, 2)
	end
	if (kind or "model") == "model" and not isModelInCdimage(asset) then
		return false
	end
	local request, loaded = k[1], k[2]
	local deadline = getGameTimer() + (timeoutMs or 10000)
	while true do
		request(asset)
		if loaded(asset) then
			return true
		end
		if getGameTimer() >= deadline then
			return false
		end
		wait()
	end
end

local load = streaming.load
local done = s.setModelAsNoLongerNeeded
local playerPedId = player.playerPedId
local coordsAhead, entityHeading = entity.getOffsetFromEntityInWorldCoords, entity.getEntityHeading

-- Where a new entity goes: `pos`, or 5 m ahead of the player; facing as
-- `heading`, or as the player.
local function place(pos: vector?, heading: number?): (vector, number, boolean)
	local me = playerPedId()
	return pos or coordsAhead(me, vector.create(0, 5, 0)), heading or entityHeading(me), pos == nil
end

-- A model of the kind the spawn makes, loaded (nil: it could not load).
local function model(what: string, m: any, is: (any) -> boolean): boolean
	if not load(m, "model") then
		return false
	end
	if not is(m) then
		done(m)
		error(`spawn.{what}: {m} is no {what} model`, 3)
	end
	return true
end

spawn = {
	vehicle = function(m, pos: vector?, heading: number?, options: { warpIn: boolean? }?): any
		if not model("vehicle", m, s.isModelAVehicle) then
			return nil
		end
		local at, facing, ahead = place(pos, heading)
		local car = vehicle.createVehicle(m, at, facing, false, false, false)
		done(m)
		if car then
			if ahead then
				vehicle.setVehicleOnGroundProperly(car, 5)
			end
			if options and options.warpIn then
				ped.setPedIntoVehicle(playerPedId(), car, -1)
			end
		end
		return car
	end,

	ped = function(m, pos: vector?, heading: number?): any
		if not model("ped", m, s.isModelAPed) then
			return nil
		end
		local at, facing = place(pos, heading)
		-- 26: PED_TYPE_MISSION, as the game's scripts make their own.
		local p = ped.createPed(26, m, at, facing, false, false)
		done(m)
		return p
	end,

	prop = function(m, pos: vector?, options: { dynamic: boolean?, ground: boolean? }?): any
		if not load(m, "model") then
			return nil
		end
		local at = place(pos)
		local o = object.createObject(m, at, false, false, options ~= nil and options.dynamic == true)
		done(m)
		if o and options and options.ground then
			object.placeObjectOnGroundProperly(o)
		end
		return o
	end,
}
