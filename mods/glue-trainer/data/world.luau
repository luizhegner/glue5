--!strict
-- The world's settings the game names only in its own data: weather types (weather.xml), its
-- gravity levels (SET_GRAVITY_LEVEL's 0 to 3), screen filters (timecycle modifiers of the base
-- game's timecycle_mods files; an unknown one does nothing), dispatch services, the scripts
-- that guard restricted areas.

return {
	-- The game's names, and as the trainer shows them.
	weathers = {
		{ "EXTRASUNNY", "Extra sunny" },
		{ "CLEAR", "Clear" },
		{ "CLOUDS", "Clouds" },
		{ "SMOG", "Smog" },
		{ "FOGGY", "Fog" },
		{ "OVERCAST", "Overcast" },
		{ "RAIN", "Rain" },
		{ "THUNDER", "Thunder" },
		{ "CLEARING", "Clearing" },
		{ "NEUTRAL", "Neutral" },
		{ "SNOW", "Snow" },
		{ "BLIZZARD", "Blizzard" },
		{ "SNOWLIGHT", "Light snow" },
		{ "XMAS", "Christmas" },
	},
	gravity = { "Earth", "Moon", "Low", "None" },
	-- { label, timecycle modifier }
	filters = {
		{ "Black and white", "CAMERA_BW" },
		{ "Security camera", "CAMERA_secuirity" },
		{ "Sniper scope", "Sniper" },
		{ "Red mist", "REDMIST" },
		{ "Drunk", "Drunk" },
		{ "Drugged", "DrugsMichaelAliensFight" },
		{ "Clowns", "DrugsTrevorClownsFight" },
		{ "Chop's eyes", "CHOP" },
		{ "Wasted", "MP_death_grade" },
		{ "Underwater", "underwater" },
		{ "Saturated", "rply_saturation" },
		{ "Vignette", "rply_vignette" },
		{ "Film 1", "NG_filmic01" },
		{ "Film 2", "NG_filmic02" },
		{ "Film 3", "NG_filmic03" },
		{ "Film 4", "NG_filmic04" },
		{ "Film 5", "NG_filmic05" },
		{ "Spectator", "spectator1" },
		{ "Blackout", "BlackOut" },
		{ "Hangover", "hud_def_blur" },
	},
	-- ENABLE_DISPATCH_SERVICE's services (police cars, helicopters, boats, roadblocks, SWAT...).
	dispatchServices = 15,
	-- The game's scripts that chase the player out of the army base, the prison and the airport.
	restrictedAreas = { "restrictedareas", "re_armybase", "re_prison", "re_lossantosintl", "am_armybase" },
}
