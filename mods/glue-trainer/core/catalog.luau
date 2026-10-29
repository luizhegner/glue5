--!strict
-- The installed game's vehicles, peds and weapons: the names come from what the game loaded
-- (world's catalog), their texts, classes and groups from the game itself. Read once, the
-- first time a feature asks (the catalog fills as the game loads).

local game = require("./game")
local peds = require("../data/peds")
local weaponGroups = require("../data/weapons")

export type VehicleEntry = { model: string, hash: number, name: string, class: number }
export type VehicleClass = { class: number, name: string, vehicles: { VehicleEntry } }
export type PedGroup = { name: string, models: { string } }
export type WeaponEntry = { model: string, hash: number, name: string }
export type WeaponGroup = { name: string, weapons: { WeaponEntry } }

local M = {}

local vehicles: { VehicleEntry }? = nil

-- Every vehicle the game can spawn, by its name in the game.
function M.vehicles(): { VehicleEntry }
	if vehicles then
		return vehicles
	end
	local list: { VehicleEntry } = {}
	for _, model in world.vehicleModels() do
		local hash = joaat(model)
		if streaming.isModelInCdimage(hash) and streaming.isModelAVehicle(hash) then
			local label = vehicle.getDisplayNameFromVehicleModel(hash)
			table.insert(list, {
				model = model,
				hash = hash,
				name = game.label(label, model),
				class = vehicle.getVehicleClassFromName(hash),
			})
		end
	end
	table.sort(list, function(a, b)
		return game.byName(a.name, b.name)
	end)
	-- Not read yet (the game is loading): tried again next time.
	if #list > 0 then
		vehicles = list
	end
	return list
end

-- The vehicles by class, in the game's order of classes, named as the game names them.
function M.vehicleClasses(): { VehicleClass }
	local byClass: { [number]: VehicleClass } = {}
	local classes: { VehicleClass } = {}
	for _, v in M.vehicles() do
		if not byClass[v.class] then
			local c = { class = v.class, name = game.label(`VEH_CLASS_{v.class}`, `Class {v.class}`), vehicles = {} }
			byClass[v.class] = c
			table.insert(classes, c)
		end
		table.insert(byClass[v.class].vehicles, v)
	end
	table.sort(classes, function(a, b)
		return a.class < b.class
	end)
	return classes
end

local pedModels: { string }? = nil

-- Every ped model, by name.
function M.peds(): { string }
	if pedModels then
		return pedModels
	end
	local list: { string } = {}
	for _, model in world.pedModels() do
		table.insert(list, model)
	end
	table.sort(list)
	if #list > 0 then
		pedModels = list
	end
	return list
end

-- The ped models by kind (their names' prefixes, data/peds.luau), the kinds in its order.
function M.pedGroups(): { PedGroup }
	local groups: { PedGroup } = {}
	local byName: { [string]: PedGroup } = {}
	for _, kind in peds.kinds do
		local g = { name = kind.name, models = {} }
		byName[kind.name] = g
		table.insert(groups, g)
	end
	for _, model in M.peds() do
		local kind = peds.kindOf(model)
		table.insert(byName[kind].models, model)
	end
	local kept = {}
	for _, g in groups do
		if #g.models > 0 then
			table.insert(kept, g)
		end
	end
	return kept
end

local weapons: { WeaponEntry }? = nil

-- Every weapon a ped can hold, by its name (the game's text for the DLC ones).
function M.weapons(): { WeaponEntry }
	if weapons then
		return weapons
	end
	-- DLC weapons have their texts in the game's tables.
	local names: { [number]: string } = {}
	for i = 0, extrametadata.getNumDlcWeapons() - 1 do
		local data = extrametadata.getDlcWeaponData(i)
		if data and data.nameLabel ~= "" then
			names[data.weapon] = game.label(data.nameLabel, "")
		end
	end
	-- The game's damage types (falling, drowning, animals' bites) are weapons of no group, or of
	-- the fists': not ones to hold.
	local unarmed = joaat("GROUP_UNARMED")
	local list: { WeaponEntry } = {}
	for _, model in world.weapons() do
		local hash = joaat(model)
		local group = weapon.getWeapontypeGroup(hash)
		if string.sub(model, 1, 7) == "WEAPON_" and weapon.isWeaponValid(hash) and group ~= 0 and group ~= unarmed then
			local name = names[hash]
			if not name or name == "" then
				name = game.pretty(string.sub(model, 8))
			end
			table.insert(list, { model = model, hash = hash, name = name })
		end
	end
	table.sort(list, function(a, b)
		return game.byName(a.name, b.name)
	end)
	if #list > 0 then
		weapons = list
	end
	return list
end

export type Component = { hash: number, name: string }

-- The components weapon `w` takes: the DLC ones (their names from the game), those named after
-- the weapon, the shared attachments.
function M.components(w: WeaponEntry): { Component }
	local found: { Component } = {}
	local seen: { [number]: boolean } = {}
	local function add(hash: number, name: string)
		if not seen[hash] and weapon.doesWeaponTakeWeaponComponent(w.hash, hash) then
			seen[hash] = true
			table.insert(found, { hash = hash, name = name })
		end
	end
	for i = 0, extrametadata.getNumDlcWeapons() - 1 do
		local data = extrametadata.getDlcWeaponData(i)
		if data and data.weapon == w.hash then
			for c = 0, extrametadata.getNumDlcWeaponComponents(i) - 1 do
				local comp = extrametadata.getDlcWeaponComponentData(i, c)
				if comp and not comp.isDefault then
					add(comp.component, game.label(comp.nameLabel, string.format("Part %08X", comp.component)))
				end
			end
		end
	end
	local base = string.sub(w.model, 8)
	for _, o in weaponGroups.own do
		add(joaat(`COMPONENT_{base}_{o[1]}`), o[2])
	end
	for _, a in weaponGroups.attachments do
		add(joaat(a[1]), a[2])
	end
	return found
end

-- The weapons by the game's groups (pistols, rifles...), named in data/weapons.luau.
function M.weaponGroups(): { WeaponGroup }
	local groups: { WeaponGroup } = {}
	local byHash: { [number]: WeaponGroup } = {}
	for _, w in M.weapons() do
		local hash = weapon.getWeapontypeGroup(w.hash)
		if not byHash[hash] then
			local g = { name = weaponGroups.groupName(hash), weapons = {} }
			byHash[hash] = g
			table.insert(groups, g)
		end
		table.insert(byHash[hash].weapons, w)
	end
	table.sort(groups, function(a, b)
		return weaponGroups.order(a.name) < weaponGroups.order(b.name)
	end)
	return groups
end

return M
