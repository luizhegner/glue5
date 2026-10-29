// Hooks for the runtime's `audio_update.rs` (see ../hooks.rs).
&[
    // The game's audio update for a frame, started on the main thread: the
    // host holds it back while the audio thread is two frames behind
    // (audio_update.rs).
    ("audio_start_update", "audNorthAudioEngine::StartUpdate()"),
    // Semaphore waits: the main thread's for the update, at the end of each
    // frame (`FinishUpdate`), and for the controls' update
    // (`CControlMgr::WaitForUpdateThread`), timed for the stats.
    ("wait_sema", "rage::sysIpcWaitSema(rage::sysIpcSemaTag*, unsigned int)"),
]
