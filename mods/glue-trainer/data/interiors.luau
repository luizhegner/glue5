--!strict
-- Interiors the game hides until a script loads them (IPLs), and the well-known rooms of the
-- base game: where to stand in each, and what to load first.

export type Interior = { name: string, ipls: { string }, x: number, y: number, z: number, heading: number }

local function at(name: string, ipls: { string }, x: number, y: number, z: number, heading: number?): Interior
	return { name = name, ipls = ipls, x = x, y = y, z = z, heading = heading or 0 }
end

-- North Yankton, as the prologue has it.
local YANKTON = {
	"plg_01", "prologue01", "prologue01_lod", "prologue01c", "prologue01c_lod", "prologue01d", "prologue01d_lod",
	"prologue01e", "prologue01e_lod", "prologue01f", "prologue01f_lod", "prologue01g", "prologue01h", "prologue01h_lod",
	"prologue01i", "prologue01i_lod", "prologue01j", "prologue01j_lod", "prologue01k", "prologue01k_lod", "prologue01z",
	"prologue01z_lod", "plg_02", "prologue02", "prologue02_lod", "plg_03", "prologue03", "prologue03_lod", "prologue03b",
	"prologue03b_lod", "prologue03_grv_dug", "prologue03_grv_dug_lod", "prologue_grv_torch", "plg_04", "prologue04",
	"prologue04_lod", "prologue04b", "prologue04b_lod", "prologue04_cover", "des_protree_end", "des_protree_start",
	"des_protree_start_lod", "plg_05", "prologue05", "prologue05_lod", "prologue05b", "prologue05b_lod", "plg_06",
	"prologue06", "prologue06_lod", "prologue06b", "prologue06b_lod", "prologue06_int", "prologue06_int_lod",
	"prologue06_pannel", "prologue06_pannel_lod", "prologue_m2_door", "prologue_m2_door_lod", "plg_occl_00",
	"prologue_occl", "plg_rd", "prologuerd", "prologuerdb", "prologuerd_lod",
}

return {
	at("North Yankton", YANKTON, 3360.2, -4849.7, 111.8),
	at("FIB lobby", { "FIBlobby" }, 110.4, -744.2, 45.7, 70),
	at("FIB offices, top floor", {}, 136.0, -749.0, 258.2),
	at("IAA offices", {}, 128.0, -617.0, 206.0),
	at("Lifeinvader", { "facelobby" }, -1047.9, -233.0, 39.0, 120),
	at("Simeon's showroom", { "shr_int" }, -56.5, -1096.6, 26.4, 30),
	at("Union Depository vault", { "FINBANK" }, 2.7, -667.0, 16.1),
	at("Pacific Standard vault", {}, 255.3, 217.1, 101.7),
	at("O'Neil ranch", { "farm", "farm_props", "farmint" }, 2441.2, 4968.6, 51.7),
	at("Hospital, Pillbox Hill", { "RC12B_HospitalInterior" }, 356.8, -590.1, 43.3),
	at("Cargo ship", { "cargoship" }, -90.0, -2365.8, 14.3),
	at("Humane Labs", {}, 3619.7, 3742.8, 28.7),
	at("Lester's factory", {}, 716.8, -962.0, 30.4),
	at("Morgue", {}, 275.4, -1361.4, 24.5),
	at("Torture room", {}, 147.4, -2201.4, 4.7),
}
