--!strict
-- Key bindings: the features name their actions here, the keys come from the preferences
-- (prefs.keys, editable in Settings or settings.json). Keys reach the trainer while no page of
-- the app is open.

local prefs = require("./prefs")

export type Action = { name: string, label: string, run: () -> () }

local actions: { Action } = {}

local M = {}

-- Action `name` (its key is prefs.data.keys[name]), shown in Settings as `label`.
function M.action(name: string, label: string, run: () -> ())
	table.insert(actions, { name = name, label = label, run = run })
end

-- The actions, in the order they were named.
function M.actions(): { Action }
	return actions
end

input.onKey(function(key, down)
	if not down then
		return
	end
	for _, a in actions do
		if prefs.data.keys[a.name] == key then
			a.run()
		end
	end
end)

return M
