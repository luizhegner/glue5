--!strict
-- What the game keeps as numbers without names: its kinds of vehicle mods (eVehicleModType),
-- window tints and plate styles. Each list is by the game's index, from 0.

return {
	-- Mod slots the game names only for some vehicles (GET_MOD_SLOT_NAME).
	modSlots = {
		[0] = "Spoiler", "Front bumper", "Rear bumper", "Side skirt", "Exhaust", "Frame", "Grille", "Hood",
		"Left fender", "Right fender", "Roof", "Engine", "Brakes", "Transmission", "Horn", "Suspension",
		"Armour", "Nitrous", "Turbo", "Subwoofer", "Tyre smoke", "Hydraulics", "Xenon lights", "Wheels",
		"Rear wheels", "Plate holder", "Vanity plate", "Trim", "Ornaments", "Dashboard", "Dial", "Door speakers",
		"Seats", "Steering wheel", "Shift lever", "Plaques", "Speakers", "Trunk", "Hydraulics", "Engine block",
		"Air filter", "Struts", "Arch cover", "Aerials", "Trim", "Tank", "Windows", "Window", "Livery",
	},
	-- Slots that are on or off (toggled, not chosen).
	toggles = { [17] = true, [18] = true, [19] = true, [20] = true, [21] = true, [22] = true },
	windowTints = { [0] = "None", "Pure black", "Dark smoke", "Light smoke", "Stock", "Limo", "Green" },
	plates = { [0] = "Blue on white", "Yellow on black", "Yellow on blue", "Blue on white 2", "Blue on white 3", "Yankton" },
	-- SET_VEHICLE_WHEEL_TYPE's types of the base game.
	wheelTypes = { [0] = "Sport", "Muscle", "Lowrider", "SUV", "Offroad", "Tuner", "Bike", "High end" },
	-- SET_VEHICLE_DOOR_OPEN's doors.
	doors = { [0] = "Front left", "Front right", "Rear left", "Rear right", "Hood", "Trunk" },
	-- Driving styles (flag sets the game's scripts use), for the autopilot.
	drivingStyles = {
		{ name = "Normal", flags = 786603 },
		{ name = "Avoiding traffic", flags = 786468 },
		{ name = "Rushed", flags = 1074528293 },
		{ name = "Ignoring lights", flags = 2883621 },
	},
}
