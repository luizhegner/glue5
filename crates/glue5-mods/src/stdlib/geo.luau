-- Rotations as the game has them, in degrees: x the pitch, y the roll, z
-- the yaw (the heading); an entity's rotation (order 2) turns by y, then x,
-- then z. Luau's `vector` library has the rest (lerp, angle, dot, cross).

local rad, deg = math.rad, math.deg
local sin, cos, asin, atan2 = math.sin, math.cos, math.asin, math.atan2
local create, magnitude = vector.create, vector.magnitude

local function heading(h: number): number
	h %= 360
	return h
end

geo = {
	-- Where a rotation points: a unit vector.
	direction = function(rot: vector): vector
		local x, z = rad(rot.x), rad(rot.z)
		local c = math.abs(cos(x))
		return create(-sin(z) * c, cos(z) * c, sin(x))
	end,

	-- The rotation (no roll) that points along `dir`.
	rotation = function(dir: vector): vector
		local flat = math.sqrt(dir.x * dir.x + dir.y * dir.y)
		return create(deg(atan2(dir.z, flat)), 0, heading(deg(atan2(-dir.x, dir.y))))
	end,

	-- The way a heading looks, on the ground.
	fromHeading = function(h: number): vector
		local r = rad(h)
		return create(-sin(r), cos(r), 0)
	end,

	-- The heading of a direction, 0 to 360 (0 north, 90 west).
	heading = function(dir: vector): number
		return heading(deg(atan2(-dir.x, dir.y)))
	end,

	-- The distance from `a` to `b` (on the ground with `flat`).
	distance = function(a: vector, b: vector, flat: boolean?): number
		local d = b - a
		if flat then
			d = create(d.x, d.y, 0)
		end
		return magnitude(d)
	end,

	-- A rotation's quaternion, x, y, z, w (entity.setEntityQuaternion).
	quaternion = function(rot: vector): (number, number, number, number)
		local hx, hy, hz = rad(rot.x) / 2, rad(rot.y) / 2, rad(rot.z) / 2
		local sx, cx, sy, cy, sz, cz = sin(hx), cos(hx), sin(hy), cos(hy), sin(hz), cos(hz)
		return cz * sx * cy - sz * cx * sy,
			cz * cx * sy + sz * sx * cy,
			cz * sx * sy + sz * cx * cy,
			cz * cx * cy - sz * sx * sy
	end,

	-- The rotation of a quaternion (entity.getEntityQuaternion's four values).
	fromQuaternion = function(x: number, y: number, z: number, w: number): vector
		local pitch = asin(math.clamp(2 * (y * z + w * x), -1, 1))
		local roll = atan2(-2 * (x * z - w * y), 1 - 2 * (x * x + y * y))
		local yaw = atan2(-2 * (x * y - w * z), 1 - 2 * (x * x + z * z))
		return create(deg(pitch), deg(roll), deg(yaw))
	end,
}
