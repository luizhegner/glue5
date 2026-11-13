//! The frame's budgets for skeletons. Every skinned entity drawn (a ped,
//! a vehicle, again for each shadow and reflection) copies its skeleton
//! into the draw list: an entry of the command buffer's matrix sets (400
//! per frame) and a dependency for the copy (512 per draw list creation, a
//! fixed array inside the draw list manager). When either runs out the
//! engine stops (an array's bounds check, a trap in this build, or
//! "increase MAX_DEPENDENCY_COUNT"): some 114 peds in view, which a mod
//! spawning peds, or a crowded scene, reaches.
//!
//! The engine already has a way not to draw an entity:
//! `CEntityDrawHandler::ShouldSkipEntity`, which every draw handler asks
//! first. Once a budget is nearly spent, it says so for peds and vehicles
//! too, for the rest of that frame: they are missing from it rather than
//! stopping the game.

use std::sync::OnceLock;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::time::Instant;

use glue5_game::{Env, orig};

use crate::memory::Mem;

/// The command buffer (`rage::dlDrawCommandBuffer*`): its matrix sets'
/// count and capacity (u16) at +16904 and +16906.
const COMMAND_BUFFER: u64 = 11012104;
const MATRIX_SETS: u64 = 16904;
/// The draw list manager: dependencies used (i32) at +83808, of 512.
const DRAW_LISTS: u64 = 11012152;
const DEPENDENCIES: u64 = 83808;
const MAX_DEPENDENCIES: u32 = 512;
/// What one entity may take at most, kept free: a ped's skeleton and its
/// props, and other dependencies made meanwhile.
const MATRIX_SET_MARGIN: u32 = 8;
const DEPENDENCY_MARGIN: u32 = 32;

/// CEntity's type byte: 3 a vehicle, 4 a ped.
const ENTITY_TYPE: u64 = 40;

static SKIPPED: AtomicU32 = AtomicU32::new(0);
static LOGGED: AtomicU64 = AtomicU64::new(0);

/// Whether this frame's budgets for skeletons are (nearly) spent.
fn spent(m: Mem) -> bool {
    let buffer = m.u64(COMMAND_BUFFER);
    if buffer != 0 {
        let (count, capacity) = (u32::from(m.u16(buffer + MATRIX_SETS)), u32::from(m.u16(buffer + MATRIX_SETS + 2)));
        if count + MATRIX_SET_MARGIN >= capacity {
            return true;
        }
    }
    let lists = m.u64(DRAW_LISTS);
    lists != 0 && m.u32(lists + DEPENDENCIES) + DEPENDENCY_MARGIN >= MAX_DEPENDENCIES
}

/// Hook for `CEntityDrawHandler::ShouldSkipEntity(CEntity const*)`.
unsafe extern "C" fn draw_skip_entity(env: *mut Env, entity: u64) -> u32 {
    let m = unsafe { Mem::of(env) };
    if entity != 0 && matches!(m.u8(entity + ENTITY_TYPE), 3 | 4) && spent(m) {
        let skipped = SKIPPED.fetch_add(1, Ordering::Relaxed) + 1;
        // At most a line a second.
        static START: OnceLock<Instant> = OnceLock::new();
        let now = START.get_or_init(Instant::now).elapsed().as_millis() as u64 + 1000;
        let last = LOGGED.load(Ordering::Relaxed);
        if now.saturating_sub(last) >= 1000
            && LOGGED.compare_exchange(last, now, Ordering::Relaxed, Ordering::Relaxed).is_ok()
        {
            SKIPPED.store(0, Ordering::Relaxed);
            log::warn!("draw lists: {skipped} peds and vehicles not drawn: the frame's budget for skeletons is spent");
        }
        return 1;
    }
    unsafe { orig::draw_skip_entity(env, entity) }
}

/// This module's hooks.
pub(crate) fn hooks(hooks: &mut glue5_game::Hooks) {
    hooks.draw_skip_entity = Some(draw_skip_entity);
}
