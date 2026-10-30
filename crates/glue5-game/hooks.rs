// Game functions the host can take over.
//
// Each entry pairs a field name with the function's name in the wasm name
// section. The build script finds the function, derives its signature from the
// module and routes every call to it through `Hooks::<field>`. An unset hook
// falls back to the original, and a hook can call the original through
// `orig::<field>`. Hooks receive the same `*mut Env` as imports. An entry
// in `CALLED_ONLY` is only a way for the host to call that game function.
//
// This file is included by the build script.
//
// The hooks are grouped by the runtime module that implements them
// (`crates/glue5-runtime/src/<group>.rs`, whose `hooks()` sets them; the
// `mods` group is glue5-mods').

pub const HOOKS: &[&[(&str, &str)]] = &[
    include!("hooks/input.rs"),
    include!("hooks/exit.rs"),
    include!("hooks/gamelog.rs"),
    include!("hooks/memory.rs"),
    include!("hooks/files.rs"),
    include!("hooks/blips.rs"),
    include!("hooks/skin.rs"),
    include!("hooks/shader_settings.rs"),
    include!("hooks/shadow_reveal.rs"),
    include!("hooks/lod_lights.rs"),
    include!("hooks/device_reset.rs"),
    include!("hooks/readback.rs"),
    include!("hooks/audio_update.rs"),
    include!("hooks/frame_pace.rs"),
    include!("hooks/clib.rs"),
    include!("hooks/waits.rs"),
    include!("hooks/granular.rs"),
    include!("hooks/script_assets.rs"),
    include!("hooks/gamer_tags.rs"),
    include!("hooks/scenario.rs"),
    include!("hooks/tour.rs"),
    include!("hooks/online.rs"),
    include!("hooks/vnet.rs"),
    include!("hooks/cloud.rs"),
    include!("hooks/jobs.rs"),
    include!("hooks/friends.rs"),
    include!("hooks/texts.rs"),
    include!("hooks/stats.rs"),
    include!("hooks/upscale.rs"),
    include!("hooks/boot_screen.rs"),
    include!("hooks/stream.rs"),
    include!("hooks/prompts.rs"),
    include!("hooks/draw_lists.rs"),
    include!("hooks/mods.rs"),
];

// The entries the host only calls (`orig::<field>`), never takes over: the
// game's own calls go to the function itself, not through a stub that looks
// at `Hooks` first (across files, where the compiler could not inline it).
// `Hooks` has no field for them.
pub const CALLED_ONLY: &[&str] = &[
    // input
    "scan_for_vehicle_to_enter", "pause_menu_is_active", "pause_menu_in_map", "screen_faded_out",
    "loading_screens_active",
    // memory
    "heap_size", "builtin_malloc", "builtin_free", "zcalloc", "buddy_block_size",
    // blips
    "blip_from_id", "blip_display",
    // shader_settings
    "settings_instance",
    // shadow_reveal
    "effect_lookup_var", "effect_set_local",
    // lod_lights
    "d3d_map_hint",
    // device_reset
    "gpu_fence", "sprite_depth_texture", "effect_set_texture",
    // readback
    "wait_readback",
    // granular
    "opus_init", "opus_submit", "opus_update", "opus_read", "opus_advance", "opus_shutdown",
    // script_assets
    "bank_index_from_name", "hash_string", "anim_dict_exists", "clip_set", "scaleform_in_image", "archetype_from_hash",
    "set_entity_visible", "set_entity_collision", "interior_at", "additional_text_requesting",
    // tour
    "find_player_ped", "warp", "clock_set_time", "weather_index", "weather_force_now", "height_max",
    // online
    "network_tunables_read",
    // cloud
    "grow_buffer_append",
    // jobs
    "ugc_metadata_new", "ugc_result", "grow_buffer_clear",
    // friends
    "friends_activate", "friends_deactivate", "presence_process", "rson_reader", "join_request", "invite_auto_confirm",
    "session_info", "session_info_text", "session_info_parse",
    // upscale
    "gpu_emit",
    // prompts
    "control_input_by_name", "control_input_source", "io_value_set_next", "warning_screen_active",
    // mods
    "script_active_thread", "script_get_thread", "script_kill_thread", "script_get_program", "script_insert_program",
    "hash_name", "script_guid",
];
