//! Where fields of the game's objects are, for `memory`: `memory.offsets`,
//! what SHVDN reads with pattern scans, as constants of this game build
//! (`memory.readFloat(memory.entity(car) + memory.offsets.vehicle.rpm)`).
//! Each was found in the engine code that reads or writes it (a native, a
//! debug print naming it), and checked in a running game by a test mod.

use mlua::Lua;

/// A field: its name, its offset from the object's address, how it is
/// kept and what it is (the checker's hover).
struct Field(&'static str, u32, &'static str, &'static str);

/// The objects: their name in `memory.offsets`, what their address is,
/// their fields.
const OBJECTS: &[(&str, &str, &[Field])] = &[
    (
        "entity",
        "From an entity's address (memory.entity).",
        &[
            // GET_ENTITY_HEALTH, GET_ENTITY_MAX_HEALTH (CPhysical).
            Field("health", 672, "f32", "health (what GET_ENTITY_HEALTH gives, as a float)"),
            Field("maxHealth", 704, "f32", "the most health"),
            // CPhysical::HasBeenDamagedByEntity.
            Field("damageRecords", 712, "pointer", "who damaged it last (see damageRecords), 0 if no one has"),
        ],
    ),
    (
        "vehicle",
        "From a vehicle's address.",
        &[
            // GET_VEHICLE_PETROL_STATS; CVehicle::SetModelId fills both from the handling.
            Field("fuelLevel", 2164, "f32", "petrol in the tank (litres; starts at the handling's fPetrolTankVolume)"),
            Field("oilLevel", 2168, "f32", "oil (starts at the handling's fOilVolume)"),
            // CTransmission, at +2224: CVehicleControlDebugInfo prints "Gear: %d Clutch: %.2f
            // Throttle: %.2f Rev: %.2f" from these; CTransmission::Process copies the gear
            // into the previous one each frame, SetupTransmission sets the top gear and ratios.
            Field("nextGear", 2224, "u16", "the gear the gearbox is in or shifting to (0 reverse)"),
            Field("currentGear", 2226, "u16", "the gear at the start of this frame"),
            Field("highGear", 2230, "u8", "the top gear"),
            Field("gearRatios", 2232, "f32[11]", "each gear's ratio, reverse first"),
            Field("rpm", 2288, "f32", "engine revs, 0.2 (idle) to 1"),
            Field("clutch", 2300, "f32", "clutch, 0 (open, shifting) to 1"),
            Field("throttle", 2304, "f32", "the engine's throttle, -1 to 1 (SHVDN's Acceleration)"),
            Field("turbo", 2324, "f32", "turbo pressure (the gearbox's manifold pressure)"),
            // CVehicleControls, printed as "Steer: Gas: Brake: HandBrake: SteerBias:";
            // CVehicle::GetSteerRatioForAnim divides the steering angle by the steering lock.
            Field("steeringScale", 2508, "f32", "the player's steering input, smoothed, -1 to 1"),
            Field("steerBias", 2512, "f32", "steering bias (SET_VEHICLE_STEER_BIAS)"),
            Field("steeringAngle", 2516, "f32", "the front wheels' angle, radians"),
            Field("throttlePower", 2524, "f32", "the driver's throttle, -1 to 1"),
            Field("brakePower", 2528, "f32", "the driver's brake, 0 to 1"),
            Field("handbrake", 2532, "u8", "1 while the handbrake is on"),
            // CVehicle::DoProcessControl moves it towards the weather's temperature.
            Field("engineTemperature", 2708, "f32", "engine temperature, degrees C"),
            // CVehicleFactory::ModifyVehicleTopSpeed, SET_VEHICLE_CHEAT_POWER_INCREASE.
            Field("topSpeedModifier", 2824, "f32", "what MODIFY_VEHICLE_TOP_SPEED set (SHVDN's EnginePowerMultiplier)"),
            Field("cheatPowerIncrease", 5088, "f32", "what SET_VEHICLE_CHEAT_POWER_INCREASE set"),
            // CVehicle::RequestDials: the fastest wheel's surface speed, smoothed (the dial's).
            Field("wheelSpeed", 2760, "f32", "how fast the wheels turn, m/s at the tyre (the speedometer's)"),
            // CVehicle::GetBrakeForce.
            Field("handling", 2392, "pointer", "its CHandlingData (handling.address; fields by name: handling.get)"),
            // CVehicle::GetWheelFromId, GetNumContactWheels.
            Field("wheels", 3096, "pointer", "its wheels: an array of wheelCount CWheel pointers (see wheel)"),
            Field("wheelCount", 3104, "i32", "how many wheels it has"),
        ],
    ),
    (
        "wheel",
        "From a wheel's address (memory.readPtr(memory.readPtr(car + vehicle.wheels) + 8 * i), i from 0).",
        &[
            // CWheel::Init, GetWheelFromId; ProcessAsleep, SetBrakeAndDriveForce.
            Field("id", 264, "i16", "which wheel: 11 front left, 12 front right, 13 rear left, 14 rear right, 15/16 middle"),
            Field("radius", 272, "f32", "tyre radius, m"),
            Field("rimRadius", 276, "f32", "rim radius, m"),
            Field("width", 284, "f32", "tyre width, m"),
            Field("vehicle", 296, "pointer", "the vehicle it belongs to"),
            Field("frontRear", 320, "f32", "1 at the front, -1 at the back"),
            Field("compression", 360, "f32", "suspension compression, m"),
            Field("rotation", 364, "f32", "how far it has turned, radians (-pi to pi)"),
            Field("rotationSpeed", 368, "f32", "how fast it turns, radians/s (negative forwards)"),
            Field("tyreTemperature", 376, "f32", "tyre heat from skidding (a tyre bursts from it at 59)"),
            Field("steeringAngle", 460, "f32", "its steering angle, radians"),
            Field("brakeForce", 464, "f32", "brake force on it now"),
            Field("driveForce", 468, "f32", "drive force on it now"),
            // CWheel::ApplySuspensionDamage, ResetDamage; GET_TYRE_HEALTH and the tyre-wear natives.
            Field("suspensionHealth", 488, "f32", "suspension health, 0 to 1000"),
            Field("tyreHealth", 492, "f32", "tyre health: 1000 whole, below it burst, 0 on the rim"),
            Field("tyreWearRate", 496, "f32", "tyre wear (GET_TYRE_WEAR_RATE)"),
            Field("maxGripDifferenceDueToWearRate", 500, "f32", "SET_TYRE_MAXIMUM_GRIP_DIFFERENCE_DUE_TO_WEAR_RATE's value"),
            Field("wearRateScale", 504, "f32", "SET_TYRE_WEAR_RATE_SCALE's value"),
            // CWheel::ProbeProcessResults, IsWheelOnFire, SetBrakeAndDriveForce.
            Field("dynamicFlags", 512, "u32", "bits: 1 touching the ground, 4 driven now, 8 on fire, 256 burnout"),
            Field("configFlags", 516, "u32", "bits: 2 left, 4 rear, 16 driven"),
            Field("material", 522, "u8", "the surface material it touches (an id)"),
        ],
    ),
    (
        "handling",
        "From a handling's address (handling.address; its other fields: handling.get).",
        &[
            // CHandlingData::ConvertToGameUnits reads handling.meta's flag texts as hex into these.
            Field("modelFlags", 292, "u32", "strModelFlags as bits"),
            Field("handlingFlags", 296, "u32", "strHandlingFlags as bits"),
            Field("damageFlags", 300, "u32", "strDamageFlags as bits"),
        ],
    ),
    (
        "ped",
        "From a ped's address.",
        &[
            // GET_PED_ARMOUR, SET_PED_ARMOUR; CPed::IsInjured, IsFatallyInjured.
            Field("armour", 5512, "f32", "armour"),
            Field("injuryHealthThreshold", 5552, "f32", "below this health it is injured"),
            Field("fatalInjuryHealthThreshold", 5556, "f32", "below this health it is dying"),
            // SET_PED_MONEY, GET_PED_MONEY.
            Field("money", 5812, "i32", "money it drops"),
            // CPed::SetMyVehicle; GET_PED_SOURCE_OF_DEATH, _CAUSE_OF_DEATH, _TIME_OF_DEATH.
            Field("vehicle", 5576, "pointer", "the vehicle it is in or last used, 0 if none"),
            Field("sourceOfDeath", 5792, "pointer", "the entity that killed it, 0 if none"),
            Field("causeOfDeath", 5800, "u32", "the weapon hash that killed it"),
            Field("timeOfDeath", 5804, "u32", "the game time it died at, ms"),
            // The SET_PED_SEEING_RANGE family; SET_RUN_SPRINT_MULTIPLIER_FOR_PLAYER; GET_SELECTED_PED_WEAPON.
            Field("intelligence", 4320, "pointer", "its CPedIntelligence (see pedIntelligence)"),
            Field("playerInfo", 4328, "pointer", "its CPlayerInfo, 0 if it is not a player (see playerInfo)"),
            Field("weaponManager", 4344, "pointer", "its CPedWeaponManager (see weaponManager)"),
        ],
    ),
    (
        "pedIntelligence",
        "From a ped's CPedIntelligence (memory.readPtr(ped + ped.intelligence)): its perception.",
        &[
            Field("hearingRange", 6464, "f32", "how far it hears, m"),
            Field("seeingRange", 6468, "f32", "how far it sees, m"),
            Field("peripheralRange", 6472, "f32", "how far it sees at the side, m"),
            Field("visualFieldMinAngle", 6476, "f32", "its field of view's left edge, degrees"),
            Field("visualFieldMaxAngle", 6480, "f32", "its field of view's right edge, degrees"),
            Field("visualFieldMinElevationAngle", 6484, "f32", "its field of view's bottom, degrees"),
            Field("visualFieldMaxElevationAngle", 6488, "f32", "its field of view's top, degrees"),
            Field("visualFieldCenterAngle", 6492, "f32", "the centre angle of its view, degrees"),
            Field("identificationRange", 6496, "f32", "how far it recognises someone, m (SET_PED_ID_RANGE)"),
        ],
    ),
    (
        "playerInfo",
        "From a player's CPlayerInfo (memory.readPtr(ped + ped.playerInfo)).",
        &[
            Field("swimSpeedMultiplier", 736, "f32", "SET_SWIM_MULTIPLIER_FOR_PLAYER's value"),
            Field("runSprintSpeedMultiplier", 3872, "f32", "SET_RUN_SPRINT_MULTIPLIER_FOR_PLAYER's value"),
        ],
    ),
    ("weaponManager", "From a ped's CPedWeaponManager.", &[Field("selectedWeapon", 24, "u32", "the weapon in hand, a hash")]),
    (
        "damageRecords",
        "From an entity's damage records (memory.readPtr(entity + entity.damageRecords)): up to 3, 24 bytes each.",
        &[
            Field("count", 72, "i32", "how many records there are"),
            Field("size", 24, "bytes", "a record's size: record i is at + i * size"),
            Field("attacker", 0, "pointer", "a record's attacker entity"),
            Field("weapon", 8, "u32", "a record's weapon hash"),
            Field("time", 12, "u32", "a record's game time, ms"),
            Field("melee", 16, "u8", "a record's flag (the damage's last argument; melee, likely)"),
        ],
    ),
];

/// Adds `offsets` to `memory` (installed before).
pub fn install(lua: &Lua) -> mlua::Result<()> {
    let offsets = lua.create_table()?;
    for &(object, _, fields) in OBJECTS {
        let t = lua.create_table()?;
        for f in fields {
            t.set(f.0, f.1)?;
        }
        offsets.set(object, t)?;
    }
    lua.globals().get::<mlua::Table>("memory")?.set("offsets", offsets)
}

/// The checker's definitions: the `MemoryOffsets` type `memory.offsets` has.
pub fn definitions() -> String {
    let mut d = String::from(
        "\n-- Where fields of the game's objects are, in bytes from the object's address: this game build's\n\
         -- offsets (SHVDN's, found for this build; PC GTA V's are different). Read them with memory.*.\n\
         export type MemoryOffsets = {\n",
    );
    for &(object, what, fields) in OBJECTS {
        d += &format!("\t-- {what}\n\t{object}: {{\n");
        for f in fields {
            d += &format!("\t\t-- {} ({}).\n\t\t{}: number,\n", capital(f.3), f.2, f.0);
        }
        d += "\t},\n";
    }
    d + "}\n"
}

fn capital(s: &str) -> String {
    let mut c = s.chars();
    c.next().map(|f| f.to_uppercase().chain(c).collect()).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_are_unique() {
        let mut objects: Vec<_> = OBJECTS.iter().map(|o| o.0).collect();
        objects.sort();
        objects.dedup();
        assert_eq!(objects.len(), OBJECTS.len());
        for (object, _, fields) in OBJECTS {
            let mut names: Vec<_> = fields.iter().map(|f| f.0).collect();
            names.sort();
            names.dedup();
            assert_eq!(names.len(), fields.len(), "{object}");
        }
    }

    #[test]
    fn definitions_name_every_field() {
        let d = definitions();
        assert!(d.contains("\tvehicle: {\n") && d.contains("\t\trpm: number,\n"));
        assert!(d.contains("-- Engine revs, 0.2 (idle) to 1 (f32)."));
    }
}
