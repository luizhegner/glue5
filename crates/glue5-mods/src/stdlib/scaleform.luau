-- Scaleform movies: `scaleform.load(name)`, then `sf:call(method, ...)`
-- and, each frame it shows, `sf:draw()`.

local wait = wait
local getGameTimer = misc.getGameTimer
local g = graphics

local Scaleform = {}
Scaleform.__index = Scaleform

-- Calls a method of the movie's ActionScript with numbers (whole ones as
-- ints), booleans and strings.
function Scaleform:call(method: string, ...: any)
	if not g.beginScaleformMovieMethod(self.handle, method) then
		error(`scaleform {self.name}: no method {method}`, 2)
	end
	for i = 1, select("#", ...) do
		local v = select(i, ...)
		local t = type(v)
		if t == "number" then
			if v == math.floor(v) and math.abs(v) < 2 ^ 31 then
				g.scaleformMovieMethodAddParamInt(v)
			else
				g.scaleformMovieMethodAddParamFloat(v)
			end
		elseif t == "boolean" then
			g.scaleformMovieMethodAddParamBool(v)
		elseif t == "string" then
			g.scaleformMovieMethodAddParamTextureNameString(v)
		else
			g.endScaleformMovieMethod()
			error(`scaleform {self.name}: {method}'s argument {i} is a {t} (numbers, booleans and strings go)`, 2)
		end
	end
	g.endScaleformMovieMethod()
end

-- Draws the movie this frame: over the whole screen, or in the rectangle
-- (centre and size in fractions of the screen).
function Scaleform:draw(x: number?, y: number?, width: number?, height: number?)
	if x then
		g.drawScaleformMovie(self.handle, x, y or 0.5, width or 1, height or 1, 255, 255, 255, 255, 0)
	else
		g.drawScaleformMovieFullscreen(self.handle, 255, 255, 255, 255, 0)
	end
end

-- Draws the movie this frame in the world, at `pos`, turned by `rot`.
function Scaleform:draw3d(pos: vector, rot: vector, scale: vector?)
	g.drawScaleformMovie3dSolid(self.handle, pos, rot, vector.create(2, 2, 1), scale or vector.one, 2)
end

-- The movie is no longer needed.
function Scaleform:release()
	g.setScaleformMovieAsNoLongerNeeded(self.handle)
end

scaleform = {
	-- The movie, once loaded (nil if it does not load in `timeoutMs`, 10 s).
	load = function(name: string, timeoutMs: number?): any
		local handle = g.requestScaleformMovie(name)
		if handle == 0 then
			return nil
		end
		local deadline = getGameTimer() + (timeoutMs or 10000)
		while not g.hasScaleformMovieLoaded(handle) do
			if getGameTimer() >= deadline then
				g.setScaleformMovieAsNoLongerNeeded(handle)
				return nil
			end
			wait()
		end
		return setmetatable({ handle = handle, name = name }, Scaleform)
	end,
}
