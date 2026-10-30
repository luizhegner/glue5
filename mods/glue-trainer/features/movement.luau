--!strict
-- Moving freely: noclip (you, or your vehicle, fly through everything where the camera looks)
-- and a free camera (the view leaves you where you are). Both with the movement controls, a
-- pad's sticks or the keyboard; Sprint goes faster, Jump and Duck go up and down.

local bind = require("../core/bind")
local game = require("../core/game")

local M = {}

-- How far the controls move per second, at the speed chosen, and faster while sprinting.
local speed = 10

-- This frame's move, as the controls point relative to `rot` (the camera's): forward/back and
-- sideways on the view, up and down on the world's vertical.
local function steer(rot: vector): vector
	local forward = geo.direction(rot)
	local right = vector.normalize(vector.cross(forward, vector.create(0, 0, 1)))
	local ahead = -pad.getDisabledControlNormal(0, Control.MOVE_UD)
	local side = pad.getDisabledControlNormal(0, Control.MOVE_LR)
	local up = 0
	if pad.isDisabledControlPressed(0, Control.JUMP) then
		up += 1
	end
	if pad.isDisabledControlPressed(0, Control.DUCK) then
		up -= 1
	end
	local fast = if pad.isDisabledControlPressed(0, Control.SPRINT) then 4 else 1
	return (forward * ahead + right * side + vector.create(0, 0, up)) * speed * fast * misc.getFrameTime()
end

-- The controls the flight uses, kept from the game while it flies.
local function holdControls()
	for _, c in { Control.MOVE_UD, Control.MOVE_LR, Control.JUMP, Control.DUCK, Control.SPRINT, Control.COVER, Control.ENTER, Control.VEH_EXIT, Control.VEH_ACCELERATE, Control.VEH_BRAKE, Control.VEH_HANDBRAKE } do
		pad.disableControlAction(0, c, true)
	end
end

-- Each one's switch, for the other to turn it off.
local turn: { [string]: (on: boolean) -> () } = {}

local function noclip(menu: Menu)
	local moved: Entity? = nil
	local function release()
		if moved and entity.doesEntityExist(moved) then
			entity.freezeEntityPosition(moved, false)
			entity.setEntityCollision(moved, true, true)
		end
		moved = nil
	end
	local _, setNoclip = bind.feature(menu, "move.noclip", "Noclip", "Fly through everything, where the camera looks", {
		-- One flight at a time.
		on = function()
			if bind.on("move.freecam") then
				turn.freecam(false)
			end
		end,
		tick = function()
			local e = game.mover()
			if e ~= moved then
				release()
				moved = e
			end
			holdControls()
			entity.freezeEntityPosition(e, true)
			entity.setEntityCollision(e, false, false)
			local rot = camera.getGameplayCamRot(2)
			local pos = entity.getEntityCoords(e, true) + steer(rot)
			entity.setEntityCoordsNoOffset(e, pos, true, true, true)
			entity.setEntityHeading(e, rot.z)
		end,
		off = release,
	})
	turn.noclip = setNoclip
end

local function freecam(menu: Menu)
	local view: Cam? = nil
	local function close()
		if view then
			camera.renderScriptCams(false, false, 0, true, false, 0)
			camera.destroyCam(view, false)
			streaming.clearFocus()
			view = nil
		end
	end
	local _, setFreecam = bind.feature(menu, "move.freecam", "Free camera", "The view flies; you stay where you are", {
		on = function()
			if bind.on("move.noclip") then
				turn.noclip(false)
			end
			view = camera.createCamWithParams("DEFAULT_SCRIPTED_CAMERA", camera.getGameplayCamCoord(), camera.getGameplayCamRot(2), camera.getGameplayCamFov(), true, 2)
			camera.renderScriptCams(true, false, 0, true, false, 0)
		end,
		tick = function()
			local c = view
			if not c then
				return
			end
			holdControls()
			-- Look with the mouse or the right stick.
			local rot = camera.getCamRot(c, 2)
			local pitch = math.clamp(rot.x - pad.getDisabledControlNormal(0, Control.LOOK_UD) * 6, -89, 89)
			local yaw = rot.z - pad.getDisabledControlNormal(0, Control.LOOK_LR) * 6
			rot = vector.create(pitch, 0, yaw)
			camera.setCamRot(c, rot, 2)
			local pos = camera.getCamCoord(c) + steer(rot)
			camera.setCamCoord(c, pos)
			-- The world loads around the camera, not the player.
			streaming.setFocusPosAndVel(pos.x, pos.y, pos.z, 0, 0, 0)
		end,
		off = close,
	})
	turn.freecam = setFreecam
	bind.slider(menu, "move.speed", "Flying speed", 10, 1, 100, { step = 1, unit = "m/s", note = "Sprint for four times as fast" }, function(v)
		speed = v
	end)
end

-- The game's camera distances behind the player on foot (CAM_VIEW_MODE), and first person.
local VIEWS = { "Near", "Middle", "Far", "Cinematic", "First person" }

local function view(menu: Menu)
	menu:separator("View")
	menu:choice("Camera", VIEWS, math.clamp(camera.getFollowPedCamViewMode() + 1, 1, #VIEWS), "How far behind you the camera is", bind.safe("Camera", function(i: number)
		camera.setFollowPedCamViewMode(i - 1)
	end))
	bind.feature(menu, "move.noBoundary", "No edge of the world", "Fly or sail as far as you like", {
		on = function()
			player.extendWorldBoundaryForPlayer(vector.create(100000, 100000, 0))
			player.extendWorldBoundaryForPlayer(vector.create(-100000, -100000, 0))
		end,
		off = function()
			player.resetWorldBoundaryForPlayer()
		end,
	})
end

function M.build(root: Menu)
	local menu = root:submenu("Movement", "Noclip, a free camera, the view, the edge of the world")
	noclip(menu)
	freecam(menu)
	view(menu)
end

return M
