--!strict
-- Lines over the game: frames per second, the speed, where you are (coordinates, heading,
-- street and area).

local bind = require("../core/bind")
local game = require("../core/game")
local prefs = require("../core/prefs")
local loop = require("../core/loop")

local M = {}

-- Bottom left, over the radar's top; one line each.
local X, Y, SIZE, GAP = 0.015, 0.68, 0.024, 0.03

-- The text's colour, kept as "#rrggbbaa".
local color: string = prefs.get("overlay.color", "#ffffffe6")
local panel = draw.style({ rounding = 0.008 })

local function speedLine(): string
	local mps = entity.getEntitySpeed(game.mover())
	if prefs.data.speedUnit == "mph" then
		return string.format("%d mph", math.floor(mps * 2.23694 + 0.5))
	end
	return string.format("%d km/h", math.floor(mps * 3.6 + 0.5))
end

local function placeLines(): { string }
	local pos = game.position()
	local street = world.streetName(pos)
	return {
		string.format("%.1f  %.1f  %.1f", pos.x, pos.y, pos.z),
		string.format("%03d°  %s", math.floor(entity.getEntityHeading(game.mover())), (world.zoneName(pos))),
		street,
	}
end

function M.build(root: Menu)
	local menu = root:submenu("Overlay", "Frame rate, speed and position over the game")
	local showFps, showSpeed, showPlace = false, false, false
	-- Frames and their time since the lines were last read: the frame rate over that time.
	local frames, elapsed = 0, 0
	-- The lines, and their width, read again five times a second (they are drawn every frame).
	local lines: { string } = {}
	local width = 0
	local age = math.huge
	local function drawLines()
		local dt = misc.getFrameTime()
		age += dt
		frames += 1
		elapsed += dt
		if age >= 0.2 then
			age = 0
			lines = {}
			if showFps and elapsed > 0 then
				table.insert(lines, string.format("%d fps", math.floor(frames / elapsed + 0.5)))
			end
			frames, elapsed = 0, 0
			if showSpeed then
				table.insert(lines, speedLine())
			end
			if showPlace then
				for _, l in placeLines() do
					table.insert(lines, l)
				end
			end
			width = 0
			for _, l in lines do
				width = math.max(width, (draw.measureText(l, SIZE)))
			end
		end
		if #lines == 0 then
			return
		end
		local height = #lines * GAP
		draw.rect(X + width / 2, Y + height / 2 - 0.004, width + 0.016, height + 0.008, draw.rgba(0, 0, 0, 140), panel)
		for i, l in lines do
			draw.text(l, X, Y + (i - 1) * GAP, SIZE, color)
		end
	end
	local function update()
		age = math.huge
		if showFps or showSpeed or showPlace then
			loop.add("overlay", drawLines)
		else
			loop.remove("overlay")
		end
	end
	bind.feature(menu, "overlay.fps", "Frame rate", "Frames per second", {
		on = function()
			showFps = true
			update()
		end,
		off = function()
			showFps = false
			update()
		end,
	})
	bind.feature(menu, "overlay.speed", "Speed", "Yours or your vehicle's", {
		on = function()
			showSpeed = true
			update()
		end,
		off = function()
			showSpeed = false
			update()
		end,
	})
	bind.feature(menu, "overlay.place", "Position", "Coordinates, heading, street and area", {
		on = function()
			showPlace = true
			update()
		end,
		off = function()
			showPlace = false
			update()
		end,
	})
	menu:color("Text colour", color, { alpha = true }, function(c)
		color = string.format("#%02x%02x%02x%02x", c.r, c.g, c.b, c.a or 255)
		prefs.set("overlay.color", color)
	end)
end

return M
