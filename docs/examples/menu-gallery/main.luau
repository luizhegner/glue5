--!strict
-- Menu gallery: every kind of menu item, each with what it can do. Open it in the
-- Mods page's MENUS (F7) or the Studio; what you do with it shows in the mod's log.

local gallery = ui.menu("Gallery", "Every menu item")

-- One of each, on the menu's first page.
gallery:separator("Actions")
gallery:button("Button", "A row with an action pill", function()
	ui.notify("Pressed")
end)
gallery:button("Custom action", { note = "The pill's text is the mod's", action = "Go" }, function()
	print("go")
end)
gallery:button("Disabled", { note = "Dimmed, not usable", enabled = false })

gallery:separator("Values")
gallery:toggle("Toggle", true, "On or off", function(on)
	print("toggle", on)
end)
gallery:slider("Slider", 0.4, 0, 1, { step = 0.05, unit = "%", note = "A range; this one 0 to 1, shown in %" }, function(v)
	print("slider", v)
end)
gallery:number("Number", 1000, { min = 0, step = 100, unit = "$", note = "Steps, or type an amount" }, function(v)
	print("number", v)
end)
gallery:choice("Choice", { "Sunny", "Cloudy", "Rain", "Thunder" }, "Cloudy", "‹ › or the full list", function(i, option)
	print("choice", i, option)
end)
local weathers = {
	"EXTRASUNNY", "CLEAR", "CLOUDS", "SMOG", "FOGGY", "OVERCAST", "RAIN",
	"THUNDER", "CLEARING", "NEUTRAL", "SNOW", "BLIZZARD", "SNOWLIGHT", "XMAS",
}
gallery:multi("Multi", weathers, { "CLEAR", "RAIN" }, "Several at once: a list with search, All and None", function(indices, options)
	print("multi", #indices, table.concat(options, ","))
end)
gallery:text("Text", "", { placeholder = "Your plate", max = 8, note = "Sent when you finish typing" }, function(text)
	print("text", text)
end)
gallery:color("Colour", "#3c78f0", "Palette, hue, saturation, brightness, hex", function(c)
	print("colour", c.r, c.g, c.b)
end)
gallery:color("Colour with opacity", { r = 240, g = 140, b = 50, a = 128 }, { alpha = true }, function(c)
	print("colour", c.r, c.g, c.b, c.a)
end)

gallery:separator("Read-only")
gallery:label("A label: muted text that wraps over as many lines as it needs, for descriptions and stats.")
gallery:progress("Progress", 0.4, "A bar with a percentage")
gallery:separator() -- a gap

-- Submenus open over the menu they are in, as deep as a mod likes: four levels here.
gallery:separator("Submenus")
local second = gallery:submenu("Deeper", "A submenu: back with ‹, B or Esc")
second:label("Level 2")
local third = second:submenu("Deeper still")
third:label("Level 3")
local fourth = third:submenu("Bottom")
fourth:label("Level 4. Back with ‹, B or Esc, one level at a time.")

-- A long list: every vehicle model of the installed game (the list is empty until the
-- game has loaded once).
local cars = gallery:submenu("Vehicles", "Every model of the game")
local model = cars:choice("Model", { "adder" }, 1, "‹ › or search the full list")
cars:button("Deliver", { action = "Deliver" }, function()
	local hash = joaat(model.option)
	streaming.requestModel(hash)
	while not streaming.hasModelLoaded(hash) do
		wait()
	end
	local me = player.playerPedId()
	local car = vehicle.createVehicle(hash, entity.getEntityCoords(me, true), entity.getEntityHeading(me), false, false, false)
	streaming.setModelAsNoLongerNeeded(hash)
	if car then
		ped.setPedIntoVehicle(me, car, -1)
	end
end)
task.spawn(function()
	local models = world.vehicleModels()
	while #models == 0 do
		wait(1000)
		models = world.vehicleModels()
	end
	local options: { string } = {}
	for _, name in models do
		table.insert(options, name)
	end
	model.options = options
	print("models:", #model.options)
end)

-- A list made when the page opens: the vehicles around the player, the closest first.
local around = gallery:submenu("Around you", "Filled each time it opens")
local names: { [number]: string } = {}
around:onOpen(function()
	if next(names) == nil then
		for _, name in world.vehicleModels() do
			names[joaat(name)] = name
		end
	end
	around:clear()
	local here = entity.getEntityCoords(player.playerPedId(), true)
	local found = 0
	for _, v in world.vehicles({ near = here, radius = 100, max = 20 }) do
		local name = names[entity.getEntityModel(v)] or "vehicle"
		local metres = vector.magnitude(entity.getEntityCoords(v, true) - here)
		around:button(name, `{math.floor(metres)} m away`, function()
			vehicle.setVehicleFixed(v)
			ui.notify(`{name}: fixed`)
		end)
		found += 1
	end
	if found == 0 then
		around:label("No vehicles around.")
	end
	print("around:", found)
end)
around:onClose(function()
	print("around: closed")
end)

-- Every property, changed by the mod each second while "Animate" is on. The player's
-- input and the mod's assignments meet in `value`: the last one wins.
local live = gallery:submenu("Live", "Changed by the mod each second")
local animate = live:toggle("Animate", true, "Off: try the items by hand")
local button = live:button("Button", "", function()
	print("live button")
end)
local toggle = live:toggle("Toggle", false)
local slider = live:slider("Slider", 0, 0, 10, 1)
local number = live:number("Number", 0)
local choice = live:choice("Choice", { "one", "two", "three" })
local multi = live:multi("Multi", { "a", "b", "c" })
local text = live:text("Text", "")
local colour = live:color("Colour", "#ff0000")
local label = live:label("Label")
local progress = live:progress("Progress", 0)
local inner = live:submenu("Submenu")
inner:label("Its row in Live changes title, and hides and shows.")

local palette = { "#e03c3c", "#f08c32", "#f0d232", "#2fa85a", "#3cc8dc", "#3c78f0", "#9650dc" }

task.spawn(function()
	local tick = 0
	while true do
		wait(1000)
		if animate.value then
			tick += 1
			local even = tick % 2 == 0
			gallery.note = `Every menu item · tick {tick}`
			live.title = if even then "Live" else "Live ·"
			button.label = `Button {tick}`
			button.note = if even then "Enabled" else "Disabled for a second"
			button.enabled = even
			toggle.value = even
			toggle.visible = tick % 5 ~= 0
			slider.max = if even then 10 else 20
			slider.step = if even then 1 else 2
			slider.unit = if even then "m" else "km"
			slider.value = tick % 11
			number.min = 0
			number.max = if even then 500 else nil
			number.step = if even then 50 else 0.5
			number.unit = if even then "$" else nil
			number.value = tick * 25
			number.note = `step {number.step}`
			choice.options = if even then { "one", "two", "three" } else { "uno", "due", "tre", "quattro" }
			choice.value = tick % #choice.options + 1
			multi.options = if even then { "a", "b", "c" } else { "a", "b", "c", "d", "e", "f", "g", "h" }
			multi.value = { tick % #multi.options + 1 }
			text.value = `tick {tick}`
			text.placeholder = if even then "Even" else "Odd"
			text.max = if even then 16 else 32
			colour.alpha = even
			colour:set(palette[tick % #palette + 1])
			label.label = `Label at tick {tick}`
			progress.value = (tick % 11) / 10
			inner.title = `Submenu {tick}`
			inner.enabled = tick % 3 ~= 0
			inner.visible = tick % 7 ~= 0
		end
	end
end)

gallery:onOpen(function()
	print("gallery: open")
end)
gallery:onClose(function()
	print("gallery: closed")
end)
