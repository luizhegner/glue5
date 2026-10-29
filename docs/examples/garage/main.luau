--!strict
-- Garage: any car in any colour, any weather, any hour, from the Mods menu (F7).

local cars = { "adder", "zentorno", "t20", "sanchez", "deluxo" }
local weathers = { "EXTRASUNNY", "CLEAR", "CLOUDS", "RAIN", "THUNDER", "FOGGY", "XMAS" }

local garage = ui.menu("Garage", "Any car, delivered")
local car = garage:choice("Car", cars, "zentorno")
local paint = garage:color("Paint", "#d01818")
local plate = garage:text("Plate", "", { placeholder = "GLUE 5", max = 8 })

local function deliver()
	local model = car.option
	local hash = joaat(model)
	streaming.requestModel(hash)
	while not streaming.hasModelLoaded(hash) do
		wait()
	end
	local me = player.playerPedId()
	local spawned = vehicle.createVehicle(hash, entity.getEntityCoords(me, true), entity.getEntityHeading(me), false, false, false)
	streaming.setModelAsNoLongerNeeded(hash)
	if spawned then
		local c = paint.value
		vehicle.setVehicleCustomPrimaryColour(spawned, c.r, c.g, c.b)
		if plate.value ~= "" then
			vehicle.setVehicleNumberPlateText(spawned, plate.value)
		end
		ped.setPedIntoVehicle(me, spawned, -1)
		ui.notify(`Your {model} is here`)
	end
end

garage:button("Deliver", { note = "Right where you stand", action = "Deliver" }, deliver)

local sky = garage:submenu("Weather & time")
sky:choice("Weather", weathers, 1, function(_, weather)
	misc.setWeatherTypeNowPersist(weather)
end)
local hour = sky:slider("Hour", 19, 0, 23, 1, function(h)
	clock.setClockTime(h, clock.getClockMinutes(), 0)
end)
local minute = sky:number("Minute", 0, { min = 0, max = 59, step = 5 }, function(m)
	clock.setClockTime(clock.getClockHours(), m, 0)
end)
sky:toggle("Freeze time", false, "The clock stops where it is", function(on)
	clock.pauseClock(on)
end)
-- The game's clock runs on: show where it is whenever the page opens.
sky:onOpen(function()
	hour.value = clock.getClockHours()
	minute.value = clock.getClockMinutes()
end)
