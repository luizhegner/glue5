--!strict
-- The trainer's own settings: a key for any feature, notifications, units, whether it keeps
-- what was on. All of it is the mod's settings.json, which can be edited by hand too.

local bind = require("../core/bind")
local game = require("../core/game")
local keys = require("../core/keys")
local prefs = require("../core/prefs")

local M = {}

-- No key for an action.
local NONE = "None"

-- A feature and a key for it, and the keys given so far. Keys reach the trainer while no page of
-- the app is open.
local function bindings(menu: Menu)
	local sub = menu:submenu("Keys", "A key for any feature or action")
	local filled = false
	sub:onOpen(bind.safe("Keys", function()
		if filled then
			return
		end
		filled = true
		local actions = keys.actions()
		local labels = {}
		for _, a in actions do
			table.insert(labels, a.label)
		end
		local names = { NONE }
		for _, k in input.keys() do
			table.insert(names, k)
		end
		-- No key is kept as "" (a missing one would be the default again).
		local function keyOf(i: number): string
			local k = prefs.data.keys[actions[i].name] or ""
			return if table.find(names, k) then k else NONE
		end
		local given: Label
		local function list()
			local lines = {}
			for _, a in actions do
				local k = prefs.data.keys[a.name]
				if k and k ~= "" then
					table.insert(lines, `{k}: {a.label}`)
				end
			end
			given:set(if #lines > 0 then table.concat(lines, " · ") else "No keys yet.")
		end
		local feature = sub:choice("Feature", labels, 1, "Pick one, with a search")
		local key = sub:choice("Key", names, keyOf(1), "Its key; None for none", function(_, k)
			prefs.data.keys[actions[feature.value].name] = if k == NONE then "" else k
			prefs.save()
			list()
		end)
		feature.onChange = function(i)
			key.value = table.find(names, keyOf(i)) or 1
		end
		sub:separator("Keys given")
		given = sub:label("")
		list()
	end))
end

function M.build(root: Menu)
	local menu = root:submenu("Settings", "Keys, notifications, units")
	bindings(menu)
	menu:toggle("Notifications", prefs.data.notify, "Say what the trainer did", function(on)
		prefs.data.notify = on
		prefs.save()
	end)
	menu:toggle("Keep what is on", prefs.data.remember, "Toggles and values come back at the next start", function(on)
		prefs.data.remember = on
		prefs.save()
	end)
	menu:toggle("Fly to teleports", prefs.data.flyTeleports, "Far teleports go through the sky, as the game's character switch", function(on)
		prefs.data.flyTeleports = on
		prefs.save()
	end)
	menu:choice("Speed in", { "km/h", "mph" }, prefs.data.speedUnit, function(_, unit)
		prefs.data.speedUnit = unit
		prefs.save()
	end)
	menu:button("Back to the defaults", { note = "Keys, settings, favourites and what is kept; from the next start", action = "Reset" }, function()
		prefs.reset()
		game.notify("Settings reset: restart the trainer for all of them")
	end)
	local about = json.decode(mod.read("mod.json") or "{}")
	menu:label(`Glue Trainer {about.version or ""} comes with GLUE 5. Its files are in the mods folder (glue-trainer): open them in the Studio to see how it is made, or to make it yours.`)
end

return M
