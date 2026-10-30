//! The engine's own exit: "Exit Game" in the pause menu, or the restart the
//! graphics settings ask for after a change that needs one (texture quality,
//! ...). On PC the game shuts down and its launcher starts it again; here
//! the shutdown never finishes (it waits on work that never comes), so the
//! engine is kept running and the host ends the process instead, starting it
//! again for a restart.
//!
//! And the way it ends without asking: a thread of its failing ([`failed`]).

use std::cell::Cell;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicU8, Ordering};
use std::time::Duration;

use glue5_game::{Env, orig};

/// What the engine asked for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Exit {
    Quit,
    /// To start again with the settings it has just saved.
    Restart,
}

static REQUEST: AtomicU8 = AtomicU8::new(0);

/// What the engine has asked for, if it has: the host should end the process.
pub fn requested() -> Option<Exit> {
    match REQUEST.load(Ordering::Relaxed) {
        1 => Some(Exit::Quit),
        2 => Some(Exit::Restart),
        _ => None,
    }
}

static FAILURE: OnceLock<String> = OnceLock::new();

/// Why the game has stopped working, if it has: a thread of the engine's hit
/// a trap (an `unreachable`, a memory fault), or a host thread serving it
/// panicked. Its other threads then wait for it forever and the screen
/// stops: the host should say so and end.
pub fn failed() -> Option<&'static str> {
    FAILURE.get().map(String::as_str)
}

thread_local! {
    static RECOVERABLE: Cell<bool> = const { Cell::new(false) };
}

/// Marks this thread as one the game does without if it panics: whoever
/// joins it goes on without its result (FSR made in the background, ...).
pub fn panic_recoverable() {
    RECOVERABLE.set(true);
}

/// Runs `f` with this thread's panics marked as ones the game does without
/// (its caller catches them), as it was afterwards.
pub fn recoverable<R>(f: impl FnOnce() -> R) -> R {
    let before = RECOVERABLE.replace(true);
    let out = f();
    RECOVERABLE.set(before);
    out
}

/// Whether this thread's panic leaves the game running ([`panic_recoverable`]).
pub fn is_panic_recoverable() -> bool {
    RECOVERABLE.get()
}

/// Ends the process with `code`: every way out goes here.
///
/// The engine's threads never end, nor do the renderer's that serve them,
/// so the process ends without the libraries' static destructors (unix's
/// `exit` runs them with every other thread still going: they destroyed
/// state those threads were using, the GPU driver's or Android's
/// performance hints' mutexes, and bionic aborted the process). The
/// standard outputs, Rust's and C's, are written out first. Windows' exit
/// stops the other threads before the libraries are told.
pub fn process(code: i32) -> ! {
    #[cfg(unix)]
    {
        use std::io::Write;
        let _ = std::io::stdout().flush();
        let _ = std::io::stderr().flush();
        unsafe {
            libc::fflush(std::ptr::null_mut());
            libc::_exit(code)
        }
    }
    #[cfg(not(unix))]
    std::process::exit(code)
}

/// Records why the game has stopped (the first reason: what follows may be
/// its consequence).
pub fn fail(why: impl Into<String>) {
    let why = why.into();
    log::error!("the game has stopped: {why}");
    let _ = FAILURE.set(why);
}

/// The engine's "restart pending" flag (a byte, at this address in this
/// build): set by the settings menu once it has saved settings that need a
/// restart (and by the RESTART_GAME script command), read by its exit flow.
const RESTART_PENDING: u64 = 19566058;

/// Hook for `CSystem::WantToExit()`: never lets the main loop end.
pub(crate) unsafe extern "C" fn want_to_exit(env: *mut Env) -> u32 {
    if unsafe { orig::want_to_exit(env) } != 0 {
        request("main loop");
    }
    0
}

/// Hook for `rage::rlShutdown()`, which the engine calls only on its way
/// down (a restart goes straight there, from within a frame): the engine
/// thread stops here, after a moment for the saves it has just started.
pub(crate) unsafe extern "C" fn rl_shutdown(_env: *mut Env) {
    std::thread::sleep(Duration::from_millis(500));
    request("shutdown");
    loop {
        std::thread::park();
    }
}

fn request(from: &str) {
    if requested().is_some() {
        return;
    }
    let restart = crate::rt().memory.mem().u8(RESTART_PENDING) != 0;
    log::info!("the engine asks to {} ({from})", if restart { "restart" } else { "exit" });
    REQUEST.store(if restart { 2 } else { 1 }, Ordering::Relaxed);
}

/// This module's hooks.
pub(crate) fn hooks(hooks: &mut glue5_game::Hooks) {
    hooks.want_to_exit = Some(want_to_exit);
    hooks.rl_shutdown = Some(rl_shutdown);
}
