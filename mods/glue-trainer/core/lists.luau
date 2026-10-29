--!strict
-- Long lists (every vehicle, every ped): submenus with a search, filled the first time a
-- player opens them, so that the trainer starts at once and holds only the lists people use.

local bind = require("./bind")

local M = {}

-- A submenu with a search, filled by `fill` when first opened. `fill` returns false when it
-- had nothing yet (the game's lists still loading): it runs again at the next opening.
function M.lazy(parent: Menu, label: string, note: string?, fill: (menu: Menu) -> boolean): Menu
	local menu = parent:submenu(label, { note = note, search = true })
	local filled = false
	menu:onOpen(bind.safe(label, function()
		if filled then
			return
		end
		menu:clear()
		filled = fill(menu)
		if not filled then
			menu:label("The game's list is not read yet: open this again in a moment.")
		end
	end))
	return menu
end

return M
