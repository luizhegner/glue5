--!strict
-- The player's preferences: the mod's settings.json (the defaults below with what the player
-- set over them, as mod.settings merges them), written back a moment after the menus change it.
-- Hand-edits of the file count too: it is read at every start.

export type Prefs = {
	-- Key bindings, by action (input key names: "F4", "LeftShift", "PadNorth").
	keys: { [string]: string },
	-- Toggles and values come back as they were at the next start.
	remember: boolean,
	-- The trainer's own notifications (spawned, teleported...).
	notify: boolean,
	-- "km/h" or "mph".
	speedUnit: string,
	-- Teleports far away fly there through the sky (game.teleport).
	flyTeleports: boolean,
	-- Toggles and values the menus keep (by key), when `remember` is on.
	state: { [string]: any },
	-- The features in Favourites, as "Label (Menu)".
	favourites: { string },
}

local DEFAULTS: Prefs = {
	-- Every feature can have one (Settings › Keys); these have one from the start.
	keys = {
		menu = "F4",
		["move.noclip"] = "F6",
		["move.freecam"] = "F8",
		["teleport.waypoint"] = "F9",
	},
	remember = true,
	notify = true,
	speedUnit = "km/h",
	flyTeleports = true,
	state = {},
	favourites = {},
}

local prefs = mod.settings(DEFAULTS)

local M = {}

M.data = prefs
M.defaults = DEFAULTS

local pending = false

-- Writes settings.json soon (several changes in a row write once).
function M.save()
	if pending then
		return
	end
	pending = true
	task.delay(1, function()
		pending = false
		mod.write("settings.json", json.encode(prefs, true) .. "\n")
	end)
end

-- A value the menus keep, `default` when there is none (or `remember` is off).
function M.get<T>(key: string, default: T): T
	if not prefs.remember then
		return default
	end
	local v = prefs.state[key]
	if v == nil then
		return default
	end
	return v
end

function M.set(key: string, value: any)
	if prefs.state[key] == value then
		return
	end
	prefs.state[key] = value
	M.save()
end

-- Back to the defaults (key bindings, options, kept values).
function M.reset()
	for k, v in DEFAULTS :: any do
		(prefs :: any)[k] = if type(v) == "table" then table.clone(v) else v
	end
	prefs.state = {}
	M.save()
end

return M
