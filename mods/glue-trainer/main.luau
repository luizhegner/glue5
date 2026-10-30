--!strict
-- Glue Trainer: GLUE 5's trainer. You, your looks, vehicles, weapons, teleports, the world, the
-- camera and the other players, from the Mods page's MENUS (F7), the Studio, or its own key
-- (F4); any feature can have a key of its own (Settings › Keys).
--
-- Each feature area is a module of features/ with a build(root) that adds its submenu; core/
-- holds what they share (preferences, the per-frame loop, features and key bindings, the
-- game's catalog), data/ the few tables the game does not give.

local keys = require("./core/keys")
local loop = require("./core/loop")
local hub = require("./features/hub")

local root = ui.menu("Glue Trainer", "You, vehicles, weapons, the world")
keys.action("menu", "Open the trainer", function()
	root:show()
end)

hub.rows(root)
for _, feature in {
	require("./features/self"),
	require("./features/looks"),
	require("./features/teleport"),
	require("./features/vehicle"),
	require("./features/spawner"),
	require("./features/weapons"),
	require("./features/world"),
	require("./features/movement"),
	require("./features/players"),
	require("./features/misc"),
	require("./features/settings"),
} do
	feature.build(root)
end
-- Search and Favourites list every feature: filled last.
hub.build()

loop.start()
