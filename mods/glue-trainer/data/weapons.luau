--!strict
-- Weapons' names the game keeps only as hashes: its groups (GET_WEAPONTYPE_GROUP gives their
-- names' hashes), as the trainer names and orders them (a group not here shows by its hash,
-- last); attachments; tints.

local groups = {
	{ "GROUP_MELEE", "Melee" },
	{ "GROUP_PISTOL", "Pistols" },
	{ "GROUP_SMG", "Submachine guns" },
	{ "GROUP_RIFLE", "Assault rifles" },
	{ "GROUP_MG", "Machine guns" },
	{ "GROUP_SHOTGUN", "Shotguns" },
	{ "GROUP_SNIPER", "Sniper rifles" },
	{ "GROUP_HEAVY", "Heavy weapons" },
	{ "GROUP_THROWN", "Throwables" },
	{ "GROUP_STUNGUN", "Stun guns" },
	{ "GROUP_PETROLCAN", "Jerry cans" },
	{ "GROUP_FIREEXTINGUISHER", "Fire extinguishers" },
	{ "GROUP_PARACHUTE", "Parachutes" },
	{ "GROUP_DIGISCANNER", "Gadgets" },
	{ "GROUP_NIGHTVISION", "Gadgets" },
}

local byHash: { [number]: string } = {}
local rank: { [string]: number } = {}
for i, g in groups do
	byHash[joaat(g[1])] = g[2]
	rank[g[2]] = rank[g[2]] or i
end

-- Attachments many weapons share (COMPONENT_AT_*), as the trainer names them. A weapon takes
-- those the game says it does (DOES_WEAPON_TAKE_WEAPON_COMPONENT).
local attachments = {
	{ "COMPONENT_AT_PI_FLSH", "Flashlight" },
	{ "COMPONENT_AT_AR_FLSH", "Flashlight" },
	{ "COMPONENT_AT_PI_SUPP", "Suppressor" },
	{ "COMPONENT_AT_PI_SUPP_02", "Suppressor" },
	{ "COMPONENT_AT_AR_SUPP", "Suppressor" },
	{ "COMPONENT_AT_AR_SUPP_02", "Suppressor" },
	{ "COMPONENT_AT_SR_SUPP", "Suppressor" },
	{ "COMPONENT_AT_SCOPE_MACRO", "Scope" },
	{ "COMPONENT_AT_SCOPE_MACRO_02", "Scope" },
	{ "COMPONENT_AT_SCOPE_SMALL", "Scope" },
	{ "COMPONENT_AT_SCOPE_SMALL_02", "Scope" },
	{ "COMPONENT_AT_SCOPE_MEDIUM", "Scope" },
	{ "COMPONENT_AT_SCOPE_LARGE", "Large scope" },
	{ "COMPONENT_AT_SCOPE_MAX", "Advanced scope" },
	{ "COMPONENT_AT_AR_AFGRIP", "Grip" },
}

-- A weapon's own components are named after it (COMPONENT_PISTOL_CLIP_02): these endings.
local own = {
	{ "CLIP_02", "Extended clip" },
	{ "CLIP_03", "Drum magazine" },
	{ "VARMOD_LUXE", "Luxury finish" },
	{ "VARMOD_LOWRIDER", "Lowrider finish" },
}

return {
	attachments = attachments,
	own = own,
	-- SET_PED_WEAPON_TINT_INDEX's tints (GET_WEAPON_TINT_COUNT says how many a weapon has).
	tints = { [0] = "Normal", "Green", "Gold", "Pink", "Army", "LSPD", "Orange", "Platinum" },
	groupName = function(hash: number): string
		return byHash[hash] or string.format("Group %08X", hash)
	end,
	order = function(name: string): number
		return rank[name] or #groups + 1
	end,
}
