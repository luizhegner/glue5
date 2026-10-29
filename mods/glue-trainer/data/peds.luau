--!strict
-- Kinds of ped models, by their names' prefixes (the game's naming: a_m_y_hipster_01 is an
-- ambient young man). In the order the trainer lists them; the last takes the rest.

export type Kind = { name: string, prefixes: { string } }

local kinds: { Kind } = {
	{ name = "Story characters", prefixes = { "player_", "ig_", "cs_", "csb_" } },
	{ name = "Online", prefixes = { "mp_" } },
	{ name = "People", prefixes = { "a_f_", "a_m_" } },
	{ name = "Gangs", prefixes = { "g_" } },
	{ name = "Services", prefixes = { "s_" } },
	{ name = "Unique", prefixes = { "u_" } },
	{ name = "Animals", prefixes = { "a_c_" } },
	{ name = "Other", prefixes = {} },
}

-- The kind of model `name`.
local function kindOf(name: string): string
	local lower = string.lower(name)
	-- Animals first: their prefix starts as people's do.
	if string.sub(lower, 1, 4) == "a_c_" then
		return "Animals"
	end
	for _, k in kinds do
		for _, p in k.prefixes do
			if string.sub(lower, 1, #p) == p then
				return k.name
			end
		end
	end
	return "Other"
end

return { kinds = kinds, kindOf = kindOf }
