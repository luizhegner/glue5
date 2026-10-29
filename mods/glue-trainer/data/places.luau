--!strict
-- Places to go, by kind: where the player stands there (x, y, z) and faces (heading). The map has
-- no list of its own; these are the well-known spots of Los Santos and Blaine County.

export type Place = { name: string, x: number, y: number, z: number, heading: number }
export type Kind = { name: string, places: { Place } }

local function at(name: string, x: number, y: number, z: number, heading: number?): Place
	return { name = name, x = x, y = y, z = z, heading = heading or 0 }
end

local kinds: { Kind } = {
	{
		name = "Homes",
		places = {
			at("Michael's house", -852.4, 160.0, 65.6, 90),
			at("Franklin's house, Strawberry", -14.8, -1454.0, 30.5, 0),
			at("Franklin's house, Vinewood Hills", 7.9, 548.1, 175.5, 330),
			at("Trevor's trailer", 1985.7, 3812.2, 32.2, 30),
			at("Lester's house", 1273.9, -1719.3, 54.8, 30),
		},
	},
	{
		name = "Landmarks",
		places = {
			at("Mount Chiliad", 501.8, 5604.0, 797.9, 180),
			at("Maze Bank Tower, roof", -75.0, -818.0, 326.2, 0),
			at("FIB Building, roof", 135.5, -749.0, 258.2, 0),
			at("Vinewood sign", 711.4, 1198.1, 348.5, 180),
			at("Galileo Observatory", -438.8, 1076.1, 352.4, 0),
			at("Kortz Center", -2243.8, 264.0, 174.6, 0),
			at("Del Perro Pier", -1850.1, -1231.7, 13.0, 0),
			at("Legion Square", 195.2, -933.8, 30.7, 0),
			at("Vespucci Beach", -1223.0, -1491.0, 4.4, 0),
			at("Chumash Pier", -3426.7, 967.5, 8.3, 90),
		},
	},
	{
		name = "Airports",
		places = {
			at("Los Santos International", -1336.0, -3044.0, 13.9, 240),
			at("Sandy Shores Airfield", 1747.0, 3273.7, 41.1, 105),
			at("McKenzie Field", 2121.7, 4796.3, 41.1, 25),
			at("Fort Zancudo", -2047.4, 3132.1, 32.8, 60),
		},
	},
	{
		name = "Shops and services",
		places = {
			at("Los Santos Customs, Burton", -365.4, -131.8, 37.9, 250),
			at("Ammu-Nation, Pillbox Hill", 19.6, -1112.8, 29.8, 140),
			at("Premium Deluxe Motorsport", -56.4, -1097.5, 26.4, 30),
			at("Pillbox Hill Medical Center", 298.3, -584.5, 43.3, 70),
			at("Mission Row Police Station", 425.1, -979.6, 30.7, 90),
			at("Vanilla Unicorn", 127.4, -1307.7, 29.2, 210),
			at("Pacific Standard Bank", 235.0, 216.4, 106.3, 290),
			at("Beeker's Garage, Paleto Bay", 110.3, 6626.4, 31.8, 225),
			at("Yellow Jack Inn", 1986.6, 3054.0, 47.2, 330),
		},
	},
	{
		name = "Out of town",
		places = {
			at("Humane Labs", 3617.2, 3739.9, 28.7, 140),
			at("Bolingbroke Penitentiary", 1679.0, 2513.7, 45.6, 0),
			at("Altruist Camp", -1170.8, 4926.0, 224.3, 0),
			at("Paleto Bay", -276.0, 6226.0, 31.5, 45),
			at("Grapeseed", 1700.0, 4800.0, 42.0, 0),
		},
	},
}

return kinds
