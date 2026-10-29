-- The game's own text on screen: the feed (ui.notify with options),
-- subtitles, help, the spinner, the HUD hidden, the keyboard; and
-- instructional buttons.

local wait = wait
local spawn = task.spawn
local h, g, m = hud, graphics, misc

local addText = h.addTextComponentSubstringPlayerName

-- Starts a text command with `text`: one component holds 99 bytes, so a
-- longer text goes in pieces (cut where characters start) under a label
-- of ten components, 990 bytes at most.
local function begin(command: (string) -> (), text: string)
	if type(text) ~= "string" then
		error(`the text is a string, not {type(text)}`, 3)
	end
	if #text <= 99 then
		command("STRING")
		addText(text)
		return
	end
	command("CELL_EMAIL_BCON")
	local i, n = 1, #text
	for _ = 1, 10 do
		if i > n then
			break
		end
		local j = math.min(i + 98, n)
		while j < n and j > i do
			local b = string.byte(text, j + 1)
			if b < 0x80 or b >= 0xC0 then
				break
			end
			j -= 1
		end
		addText(string.sub(text, i, j))
		i = j + 1
	end
end

local appNotify = ui.notify

-- Without options, the app's notification (as before); with them, a
-- message in the game's feed: `icon` a picture's texture dictionary
-- ("CHAR_LESTER"), `title` and `subtitle` beside it, `blink`.
ui.notify = function(text: string, options: { icon: string?, title: string?, subtitle: string?, blink: boolean? }?)
	if options == nil then
		return appNotify(text)
	end
	local icon = options.icon or ((options.title or options.subtitle) and "CHAR_DEFAULT")
	local function post()
		begin(h.beginTextCommandThefeedPost, text)
		if icon then
			h.endTextCommandThefeedPostMessagetext(icon, icon, false, 0, options.title or "", options.subtitle or "")
		end
		h.endTextCommandThefeedPostTicker(options.blink == true, true)
	end
	if icon and not g.hasStreamedTextureDictLoaded(icon) then
		-- The picture first, beside the mod's code.
		spawn(function()
			streaming.load(icon, "texture", 2000)
			post()
		end)
	else
		post()
	end
end

-- A subtitle at the bottom, for `ms` milliseconds (2.5 s).
ui.subtitle = function(text: string, ms: number?)
	begin(h.beginTextCommandPrint, text)
	h.endTextCommandPrint(ms or 2500, true)
end

-- The help box at the top left (`~INPUT_CONTEXT~` shows a button); called
-- each frame it stays. nil clears it.
ui.help = function(text: string?, beep: boolean?)
	if text == nil then
		h.clearAllHelpMessages()
		return
	end
	begin(h.beginTextCommandDisplayHelp, text)
	h.endTextCommandDisplayHelp(0, false, beep == true, -1)
end

-- The spinner at the bottom right, with `text`; nil (or false) stops it.
ui.loading = function(text: (string | false)?)
	if text then
		begin(h.beginTextCommandBusyspinnerOn, text)
		h.endTextCommandBusyspinnerOn(4)
	else
		h.busyspinnerOff()
	end
end

-- Hides a HUD component (HudComponent) this frame; nil: the HUD and the
-- radar.
ui.hideHud = function(component: number?)
	if component then
		h.hideHudComponentThisFrame(component)
	else
		h.hideHudAndRadarThisFrame()
	end
end

-- The game's keyboard: what the player typed, nil if they cancelled.
-- `title` is a text label of the game's ("FMMC_KEY_TIP8"); `max` bytes
-- (60).
ui.prompt = function(title: string?, default: string?, max: number?): string?
	m.displayOnscreenKeyboard(0, title or "", "", default or "", "", "", "", "", "", "", "", (max or 60) + 1)
	while true do
		wait()
		local status = m.updateOnscreenKeyboard()
		if status == 1 then
			return m.getOnscreenKeyboardResult()
		elseif status ~= 0 then
			return nil
		end
	end
end

-- Instructional buttons: what the bar shows (nil: nothing), whether it
-- changed, whether a task draws it.
local bar, changed, drawing = nil, false, false
local buttonString = pad.getControlInstructionalButtonsString
local usingKeyboard = pad.isUsingKeyboardAndMouse

-- The bar at the bottom right, each frame until it changes: `{ { control =
-- Control.JUMP, label = "Jump" }, ... }`; nil or {} hides it.
ui.buttons = function(buttons: { { control: number, label: string } }?)
	if buttons ~= nil and type(buttons) ~= "table" then
		error("ui.buttons: a list of { control, label }, or nil", 2)
	end
	for i, b in buttons or {} do
		if type(b) ~= "table" or type(b.control) ~= "number" or type(b.label) ~= "string" then
			error(`ui.buttons: button {i} is not \{ control = number, label = string }`, 2)
		end
	end
	bar = if buttons and #buttons > 0 then table.clone(buttons) else nil
	changed = true
	if not bar or drawing then
		return
	end
	drawing = true
	spawn(function()
		local sf = scaleform.load("instructional_buttons")
		-- Drawn while there is a bar; a failure stops the drawing (and is the
		-- task's error), the next bar starts it again.
		local ok, e = pcall(function()
			local keyboard = nil
			while bar and sf do
				-- Buttons are drawn for the keyboard or the pad, whichever is used.
				if changed or usingKeyboard(2) ~= keyboard then
					changed, keyboard = false, usingKeyboard(2)
					sf:call("CLEAR_ALL")
					sf:call("SET_CLEAR_SPACE", 200)
					for i, b in bar do
						sf:call("SET_DATA_SLOT", i - 1, buttonString(2, b.control, true), b.label)
					end
					sf:call("SET_BACKGROUND_COLOUR", 0, 0, 0, 80)
					sf:call("DRAW_INSTRUCTIONAL_BUTTONS", -1)
				end
				sf:draw()
				wait()
			end
		end)
		if sf then
			sf:release()
		end
		drawing = false
		if not ok then
			error(e, 0)
		end
	end)
end
