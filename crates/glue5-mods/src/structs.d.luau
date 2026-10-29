-- Structures the natives fill, as tables (structs.luau).

export type DlcVehicleData = { lockHash: Hash, model: Hash, cost: number }
-- The labels are text labels: hud.getFilenameForAudioConversation(label) gives their text.
export type DlcWeaponData = {
	lockHash: Hash,
	weapon: Hash,
	cost: number,
	ammoCost: number,
	ammoType: Hash,
	clipSize: number,
	nameLabel: string,
	descLabel: string,
	desc2Label: string,
	upperNameLabel: string,
}
export type DlcWeaponComponentData = {
	attachBone: Hash,
	isDefault: boolean,
	lockHash: Hash,
	component: Hash,
	cost: number,
	nameLabel: string,
	descLabel: string,
}

export type EventGroup = "ai" | "network" | "errors"
export type EntityEvent = { type: "EVENT_ENTITY_DAMAGED" | "EVENT_ENTITY_DESTROYED", entity: Entity? }
export type UnableToEnterVehicleEvent = { type: "EVENT_PLAYER_UNABLE_TO_ENTER_VEHICLE", vehicle: Vehicle? }
export type SeenDeadPedEvent = { type: "EVENT_PED_SEEN_DEAD_PED", deadPed: Ped?, finder: Ped? }
export type DroppedWeaponEvent = { type: "EVENT_NETWORK_PED_DROPPED_WEAPON", object: Object?, ped: Ped? }
export type CollectedPickupEvent = {
	type: "EVENT_NETWORK_PLAYER_COLLECTED_PICKUP",
	pickup: Pickup?,
	player: Player,
	pickupHash: Hash,
	amount: number,
	model: Hash,
	collected: number,
}
export type CollectedAmbientPickupEvent = {
	type: "EVENT_NETWORK_PLAYER_COLLECTED_AMBIENT_PICKUP",
	pickupHash: Hash,
	amount: number,
	player: Player,
	model: Hash,
	playerGift: boolean,
	droppedByPed: boolean,
	collected: number,
	pickupIndex: number,
}
export type DamageEvent = {
	type: "EVENT_NETWORK_DAMAGE_ENTITY",
	victim: Entity?,
	damager: Entity?,
	damage: number,
	enduranceDamage: number,
	victimIncapacitated: boolean,
	victimDestroyed: boolean,
	weapon: Hash,
	victimSpeed: number,
	damagerSpeed: number,
	responsibleForCollision: boolean,
	headshot: boolean,
	melee: boolean,
	hitMaterial: Hash,
}
export type VehicleUndrivableEvent = { type: "EVENT_NETWORK_VEHICLE_UNDRIVABLE", vehicle: Vehicle?, damager: Entity?, weapon: Hash }
export type EnteredVehicleEvent = { type: "EVENT_NETWORK_PLAYER_ENTERED_VEHICLE", player: Player, vehicle: Vehicle? }
export type EventData =
	EntityEvent
	| UnableToEnterVehicleEvent
	| SeenDeadPedEvent
	| DroppedWeaponEvent
	| CollectedPickupEvent
	| CollectedAmbientPickupEvent
	| DamageEvent
	| VehicleUndrivableEvent
	| EnteredVehicleEvent

-- The game's script events of this frame, by group: "ai" (EVENT_ENTITY_DAMAGED...), "network"
-- (EVENT_NETWORK_DAMAGE_ENTITY..., in single player too), "errors". Indices from 0, as the natives'.
declare events: {
	count: (group: EventGroup) -> number,
	-- The event's type: its name (`"EVENT_NETWORK_DAMAGE_ENTITY"`) and number; nil past the last.
	at: (group: EventGroup, index: number) -> (string?, number?),
	-- The event's data as a table with its `type`, for the types listed in EventData; else nil and why.
	data: (group: EventGroup, index: number) -> (EventData?, string?),
}

