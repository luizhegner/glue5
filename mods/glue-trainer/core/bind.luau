--!strict
-- Menu items that keep their value between sessions (prefs); features, toggles whose work
-- runs every frame while they are on, undone when they go off or the trainer stops; and
-- commands, buttons with a name. Features and commands are listed for Search, Favourites and
-- key bindings, where they show again as themselves.

local prefs = require("./prefs")
local loop = require("./loop")
local game = require("./game")
local keys = require("./keys")

export type Feature = {
	-- Every frame while on (it must not wait).
	tick: (() -> ())?,
	-- When it goes on (at the start too, when kept on), and off (when the trainer stops too).
	on: (() -> ())?,
	off: (() -> ())?,
}

-- A feature or a command, as Search and Favourites show it.
export type Entry = {
	key: string,
	label: string,
	-- The menu it is in ("Self").
	place: string,
	-- A feature switches, a command runs.
	set: ((on: boolean) -> ())?,
	run: (() -> ())?,
}

-- The `off` of every feature that is on, by key.
local active: { [string]: () -> () } = {}
local entries: { Entry } = {}
-- The toggles that show each feature: its own, and those of Search and Favourites.
local shown: { [string]: { Toggle } } = {}
-- The mirrored toggles of each menu that has some, by the menu's id.
local mirrored: { [number]: { { key: string, item: Toggle } } } = {}

mod.onStop(function()
	for _, off in active do
		pcall(off)
	end
end)

local M = {}

-- `f`, its errors said to the player and logged instead of stopping the trainer (a native
-- refuses a vehicle that is gone, a model that is not there...).
function M.safe<A...>(label: string, f: (A...) -> ()): (A...) -> ()
	return function(...)
		local ok, err = pcall(f :: any, ...)
		if not ok then
			print(`{label}: {err}`)
			ui.notify(`{label}: {err}`)
		end
	end
end

-- Whether the feature kept under `key` is on.
function M.on(key: string): boolean
	return active[key] ~= nil
end

-- Every toggle of feature `key` shows `on`.
local function show(key: string, on: boolean)
	local items = shown[key]
	if items then
		for _, item in items do
			item.value = on
		end
	end
end

-- A toggle running feature `f` while on, kept under `key` (its key binding's name too).
-- Returns the item and a function that switches it as the player would.
function M.feature(menu: Menu, key: string, label: string, note: string?, f: Feature): (Toggle, (on: boolean) -> ())
	local function set(on: boolean)
		if M.on(key) == on then
			return
		end
		if on then
			active[key] = f.off or function() end
			if f.on then
				local ok, err = pcall(f.on :: any)
				if not ok then
					active[key] = nil
					show(key, false)
					print(`{label}: {err}`)
					ui.notify(`{label}: {err}`)
					return
				end
			end
			if f.tick then
				loop.add(key, f.tick, function(message)
					-- It failed: off, and the player hears why.
					active[key] = nil
					show(key, false)
					prefs.set(key, false)
					if f.off then
						pcall(f.off)
					end
					ui.notify(`{label} stopped: {message}`)
				end)
			end
		else
			loop.remove(key)
			local off = active[key]
			active[key] = nil
			local ok, err = pcall(off :: any)
			if not ok then
				print(`{label}: {err}`)
			end
		end
		show(key, on)
		prefs.set(key, on)
	end
	local item = menu:toggle(label, false, note, M.safe(label, set))
	shown[key] = { item }
	table.insert(entries, { key = key, label = label, place = menu.title, set = set })
	if prefs.get(key, false) then
		set(true)
	end
	keys.action(key, label, M.safe(label, function()
		set(not M.on(key))
		game.notify(`{label}: {if M.on(key) then "on" else "off"}`)
	end))
	return item, set
end

-- Feature `f` in story mode only: in GTA Online what it makes (local vehicles, people, objects)
-- would be this player's alone, solid to them and unseen by the others. It refuses to go on
-- there, and does nothing while the player is there with it on.
function M.storyOnly(f: Feature): Feature
	return {
		on = function()
			if game.online() then
				error("story mode only: in GTA Online the others would not see what it makes", 0)
			end
			if f.on then
				f.on()
			end
		end,
		tick = f.tick and function()
			if not game.online() then
				(f.tick :: () -> ())()
			end
		end,
		off = f.off,
	}
end

-- A button that says what it did, or why it could not: `run` returns the message (nil: none).
function M.action(menu: Menu, label: string, opts: (string | ButtonOpts)?, run: () -> string?): Button
	return menu:button(label, opts or {}, M.safe(label, function()
		local message = run()
		if message then
			game.notify(message)
		end
	end))
end

-- An action with a name, `key`: in Search and Favourites too, and a key can run it.
function M.command(menu: Menu, key: string, label: string, opts: (string | ButtonOpts)?, run: () -> string?): Button
	local press = M.safe(label, function()
		local message = run()
		if message then
			game.notify(message)
		end
	end)
	table.insert(entries, { key = key, label = label, place = menu.title, run = press })
	keys.action(key, label, press)
	return menu:button(label, opts or {}, press)
end

-- Every feature and command, in the order they were made.
function M.entries(): { Entry }
	return entries
end

-- Entry `e` in `menu` once more, working as where it was made. M.forget(menu) before its
-- items go.
function M.mirror(menu: Menu, e: Entry): Item
	if e.set then
		local item = menu:toggle(e.label, M.on(e.key), e.place, M.safe(e.label, e.set))
		table.insert(shown[e.key], item)
		mirrored[menu.id] = mirrored[menu.id] or {}
		table.insert(mirrored[menu.id], { key = e.key, item = item })
		return item
	end
	return menu:button(e.label, { note = e.place, action = "Run" }, e.run)
end

-- The toggles mirrored in `menu` stop following their features.
function M.forget(menu: Menu)
	local mirrors = mirrored[menu.id]
	if not mirrors then
		return
	end
	for _, m in mirrors do
		local items = shown[m.key]
		local i = table.find(items, m.item)
		if i then
			table.remove(items, i)
		end
	end
	mirrored[menu.id] = nil
end

-- A slider kept under `key`; `apply` runs with each value the player sets, and at the start
-- with a kept value other than `default`.
function M.slider(menu: Menu, key: string, label: string, default: number, min: number, max: number, opts: SliderOpts?, apply: ((value: number) -> ())?): Slider
	local kept = math.clamp(prefs.get(key, default), min, max)
	local item = menu:slider(label, kept, min, max, opts or {}, M.safe(label, function(v)
		prefs.set(key, v)
		if apply then
			apply(v)
		end
	end))
	if apply and kept ~= default then
		M.safe(label, apply)(kept)
	end
	return item
end

-- A choice kept under `key` (by its text, so that a list that changes keeps it); `apply` as
-- a slider's.
function M.choice(menu: Menu, key: string, label: string, options: { string }, default: string, note: string?, apply: ((index: number, option: string) -> ())?): Choice
	local kept = prefs.get(key, default)
	if not table.find(options, kept) then
		kept = default
	end
	local item = menu:choice(label, options, kept, note or {}, M.safe(label, function(i, option)
		prefs.set(key, option)
		if apply then
			apply(i, option)
		end
	end))
	if apply and kept ~= default then
		M.safe(label, apply)(item.value, item.option)
	end
	return item
end

return M
