--!strict
-- Structures and arrays the natives fill, as tables (structs.rs): a thin
-- layer over native.invoke and memory. A layout lists a structure's
-- fields as the engine's scr* structures have them: one 8-byte script
-- slot each, a text label 8 (64 bytes).

local eventName = ... -- (type) -> string?, the engine's names (structs.rs)
local invoke, alloc, free = native.invoke, memory.alloc, memory.free
local readInt, readU32, readFloat, readString, writeInt =
	memory.readInt, memory.readU32, memory.readFloat, memory.readString, memory.writeInt

type Layout = { { string } }

local SLOTS = { text = 8 }

local function slots(layout: Layout): number
	local n = 0
	for _, f in layout do
		n += SLOTS[f[2]] or 1
	end
	return n
end

-- The fields of the structure at `at` into `t`; a handle of 0 is none.
local function decode(at: number, layout: Layout, t: { [string]: any }): { [string]: any }
	for _, f in layout do
		local name, kind = f[1], f[2]
		if kind == "int" then
			t[name] = readInt(at)
		elseif kind == "hash" then
			t[name] = readU32(at)
		elseif kind == "float" then
			t[name] = readFloat(at)
		elseif kind == "bool" then
			t[name] = readInt(at) ~= 0
		elseif kind == "handle" then
			local h = readInt(at)
			t[name] = if h ~= 0 then h else nil
		elseif kind == "text" then
			t[name] = readString(at) -- the engine ends it inside its 64 bytes
		end -- "pad": a slot the engine leaves as it is
		at += 8 * (SLOTS[kind] or 1)
	end
	return t
end

-- `f` with `bytes` of scratch memory, freed whatever happens.
local function scratch(bytes: number, f: (number) -> ...any): ...any
	local at = alloc(bytes)
	local r = table.pack(pcall(f, at))
	free(at)
	if not r[1] then
		error(r[2], 0)
	end
	return table.unpack(r, 2, r.n)
end

-- A native that fills a structure after its `inputs` arguments and says
-- whether it did: the structure as a table, or nil.
local function filled(name: string, layout: Layout, inputs: number): (...any) -> { [string]: any }?
	local bytes = 8 * slots(layout)
	return function(...)
		local args = { ... }
		return scratch(bytes, function(at)
			args[inputs + 1] = at
			if invoke(name, table.unpack(args, 1, inputs + 1)) then
				return decode(at, layout, {})
			end
			return nil
		end)
	end
end

-- A native that fills an array (its room first, the elements after) and
-- gives the count, as a list of at most `max` handles (16 by default, 64
-- at most). `typed`: GET_PED_NEARBY_PEDS, with the type of ped to leave
-- out after the array (none: -1).
local function listed(name: string, typed: boolean): (any, any, any) -> { number }
	return function(ped, a, b)
		local max = if typed then b else a
		local room = math.clamp(math.floor(tonumber(max) or 16), 1, 64)
		return scratch(8 * (room + 1), function(at)
			writeInt(at, room)
			local n = if typed then invoke(name, ped, at, a or -1) else invoke(name, ped, at)
			local out = {}
			for i = 1, math.min(n, room) do
				local h = readInt(at + 8 * i)
				if h ~= 0 then
					table.insert(out, h)
				end
			end
			return out
		end)
	end
end

local DLC_VEHICLE: Layout = { { "lockHash", "hash" }, { "model", "hash" }, { "cost", "int" } }
local DLC_WEAPON: Layout = {
	{ "lockHash", "hash" },
	{ "weapon", "hash" },
	{ "", "pad" }, -- the weapon again
	{ "cost", "int" },
	{ "ammoCost", "int" },
	{ "ammoType", "hash" },
	{ "clipSize", "int" },
	{ "nameLabel", "text" },
	{ "descLabel", "text" },
	{ "desc2Label", "text" },
	{ "upperNameLabel", "text" },
}
local DLC_COMPONENT: Layout = {
	{ "attachBone", "hash" },
	{ "isDefault", "bool" },
	{ "lockHash", "hash" },
	{ "component", "hash" },
	{ "", "pad" },
	{ "cost", "int" },
	{ "nameLabel", "text" },
	{ "descLabel", "text" },
}

rawset(extrametadata, "getDlcVehicleData", filled("GET_DLC_VEHICLE_DATA", DLC_VEHICLE, 1))
rawset(extrametadata, "getDlcWeaponData", filled("GET_DLC_WEAPON_DATA", DLC_WEAPON, 1))
rawset(extrametadata, "getDlcWeaponDataSp", filled("GET_DLC_WEAPON_DATA_SP", DLC_WEAPON, 1))
rawset(extrametadata, "getDlcWeaponComponentData", filled("GET_DLC_WEAPON_COMPONENT_DATA", DLC_COMPONENT, 2))
rawset(extrametadata, "getDlcWeaponComponentDataSp", filled("GET_DLC_WEAPON_COMPONENT_DATA_SP", DLC_COMPONENT, 2))
rawset(ped, "getPedNearbyVehicles", listed("GET_PED_NEARBY_VEHICLES", false))
rawset(ped, "getPedNearbyPeds", listed("GET_PED_NEARBY_PEDS", true))

-- The events' data by type (eEventType of this build), as the engine's
-- RetrieveData copies it; GET_EVENT_DATA checks the size against it.
local ENTITY: Layout = { { "entity", "handle" } }
local EVENTS: { [number]: Layout } = {
	[141] = ENTITY, -- EVENT_ENTITY_DAMAGED
	[142] = ENTITY, -- EVENT_ENTITY_DESTROYED
	[145] = { { "vehicle", "handle" } }, -- EVENT_PLAYER_UNABLE_TO_ENTER_VEHICLE
	[149] = { { "deadPed", "handle" }, { "finder", "handle" } }, -- EVENT_PED_SEEN_DEAD_PED
	[163] = { { "object", "handle" }, { "ped", "handle" } }, -- EVENT_NETWORK_PED_DROPPED_WEAPON
	[165] = { -- EVENT_NETWORK_PLAYER_COLLECTED_PICKUP
		{ "pickup", "handle" },
		{ "player", "int" },
		{ "pickupHash", "hash" },
		{ "amount", "int" },
		{ "model", "hash" },
		{ "collected", "int" },
	},
	[166] = { -- EVENT_NETWORK_PLAYER_COLLECTED_AMBIENT_PICKUP
		{ "pickupHash", "hash" },
		{ "amount", "int" },
		{ "player", "int" },
		{ "model", "hash" },
		{ "playerGift", "bool" },
		{ "droppedByPed", "bool" },
		{ "collected", "int" },
		{ "pickupIndex", "int" },
	},
	[186] = { -- EVENT_NETWORK_DAMAGE_ENTITY
		{ "victim", "handle" },
		{ "damager", "handle" },
		{ "damage", "float" },
		{ "enduranceDamage", "float" },
		{ "victimIncapacitated", "bool" },
		{ "victimDestroyed", "bool" },
		{ "weapon", "hash" },
		{ "victimSpeed", "float" },
		{ "damagerSpeed", "float" },
		{ "responsibleForCollision", "bool" },
		{ "headshot", "bool" },
		{ "melee", "bool" },
		{ "hitMaterial", "hash" },
	},
	[220] = { { "vehicle", "handle" }, { "damager", "handle" }, { "weapon", "hash" } }, -- EVENT_NETWORK_VEHICLE_UNDRIVABLE
	[240] = { { "player", "int" }, { "vehicle", "handle" } }, -- EVENT_NETWORK_PLAYER_ENTERED_VEHICLE
}

local GROUPS = { ai = 0, network = 1, errors = 2 }

local function group(what: string, g: any): number
	local n = GROUPS[g]
	if not n then
		error(`events.{what}: the group is "ai", "network" or "errors" (got {tostring(g)})`, 3)
	end
	return n
end

events = {
	count = function(g: any): number
		return invoke("GET_NUMBER_OF_EVENTS", group("count", g))
	end,
	at = function(g: any, index: number): (string?, number?)
		local t = invoke("GET_EVENT_AT_INDEX", group("at", g), index)
		if t < 0 then
			return nil, nil
		end
		return eventName(t), t
	end,
	data = function(g: any, index: number): ({ [string]: any }?, string?)
		local n = group("data", g)
		local t = invoke("GET_EVENT_AT_INDEX", n, index)
		if t < 0 then
			return nil, `no event {index} in the {g} group`
		end
		local name = eventName(t) or tostring(t)
		local layout = EVENTS[t]
		if not layout then
			return nil, `{name} has no data events.data knows; native.invoke("GET_EVENT_DATA", ...) reads it raw`
		end
		local size = slots(layout)
		return scratch(8 * size, function(at)
			if invoke("GET_EVENT_DATA", n, index, at, size) then
				return decode(at, layout, { type = name })
			end
			return nil, `the game gave no data for {name}`
		end)
	end,
}
