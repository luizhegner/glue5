--!strict
-- The trainer's first two rows: Search, every feature and command of every menu in one list
-- that filters as you type; and Favourites, those the player picked. Both show them as
-- themselves: a toggle there is the feature's own, in step with its menu.
--
-- Built at the end (the entries are every menu's), filled when first opened.

local bind = require("../core/bind")
local prefs = require("../core/prefs")

local M = {}

-- Search and Favourites, made first so that they are the trainer's first rows.
local search: Menu
local favourites: Menu

function M.rows(root: Menu)
	search = root:submenu("Search", { note = "Every feature, by name", search = true })
	favourites = root:submenu("Favourites", "The features you use most")
end

-- `label (place)`: an entry as the favourites' list names it.
local function named(e: bind.Entry): string
	return `{e.label} ({e.place})`
end

function M.build()
	local entries = bind.entries()
	local filled = false
	search:onOpen(bind.safe("Search", function()
		if filled then
			return
		end
		filled = true
		for _, e in entries do
			bind.mirror(search, e)
		end
	end))

	local names = {}
	for _, e in entries do
		table.insert(names, named(e))
	end
	-- The picked ones, under the list (which stays: rebuilding it would close its page).
	local rows: { Item } = {}
	local function show()
		bind.forget(favourites)
		for _, row in rows do
			row:remove()
		end
		rows = {}
		for _, e in entries do
			if table.find(prefs.data.favourites, named(e)) then
				table.insert(rows, bind.mirror(favourites, e))
			end
		end
	end
	local kept = {}
	for _, n in prefs.data.favourites do
		if table.find(names, n) then
			table.insert(kept, n)
		end
	end
	favourites:multi("Pick them", names, kept, "From every menu, with a search", bind.safe("Favourites", function(_, chosen)
		prefs.data.favourites = chosen
		prefs.save()
		show()
	end))
	favourites:separator()
	show()
end

return M
