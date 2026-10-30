//! The far lights' pools of light.
//!
//! `CLODLights::RenderLODLights` draws the far street lamps' light as
//! instanced point, spot and capsule lights. Each frame it writes their
//! instances from the start of the next of a ring of vertex buffers, locked
//! whole with no-overwrite (the ring keeps the GPU off the buffer it
//! writes). The game's D3D layer does not see writes: on unmapping it scans
//! the buffer against a copy of what it uploaded and uploads what differs.
//! For buffers over 128 KiB mapped no-overwrite it takes them for appends
//! and scans only from 4 KiB before where the last change ended, unless the
//! map came with a hint of the range written (`wasm_d3d_map_hint`), which
//! the port gives at its own appends but not here. So once a buffer of the
//! spot lights' ring (312 KiB) had held more lights, the instances written
//! at its start were not uploaded again (until a full scan, every 32nd
//! unmap): the GPU drew another place's lights from it. Seen as the lamps'
//! pools of light coming and going with the frames that drew from that
//! buffer, one in four.
//!
//! Each unlock in `RenderLODLights` of a buffer over 128 KiB now hints the
//! range it locked; the unmap that follows, on the same thread, takes the
//! hint and scans that range. Smaller buffers need none: the layer scans
//! them whole, or uploads them whole without looking for a hint (one given
//! would then stay in its table of four and crowd out the others).

use std::cell::Cell;

use glue5_game::{Env, orig};

use crate::memory::Mem;

/// The D3D layer takes no-overwrite maps of buffers larger than this for
/// appends.
const SCANNED_WHOLE: u32 = 128 << 10;

thread_local! {
    /// In `RenderLODLights`, on this thread.
    static RENDERING: Cell<bool> = const { Cell::new(false) };
}

/// Hook for `CLODLights::RenderLODLights()`.
pub(crate) unsafe extern "C" fn render_lod_lights(env: *mut Env) {
    RENDERING.set(true);
    unsafe { orig::render_lod_lights(env) };
    RENDERING.set(false);
}

/// Hook for `rage::grcVertexBufferD3D11::UnlockInternal() const`.
pub(crate) unsafe extern "C" fn vertex_buffer_unlock(env: *mut Env, buffer: u64) {
    if RENDERING.get() {
        // The vertex buffer's grcBufferD3D11 is at +80: the D3D buffer, then
        // at +16 and +20 the locked range.
        let mem = unsafe { Mem::of(env) };
        let (resource, offset, size) = (mem.u64(buffer + 80), mem.u32(buffer + 96), mem.u32(buffer + 100));
        if resource != 0 && size > SCANNED_WHOLE {
            unsafe { orig::d3d_map_hint(env, resource, offset, size) };
        }
    }
    unsafe { orig::vertex_buffer_unlock(env, buffer) }
}

/// This module's hooks.
pub(crate) fn hooks(hooks: &mut glue5_game::Hooks) {
    hooks.render_lod_lights = Some(render_lod_lights);
    hooks.vertex_buffer_unlock = Some(vertex_buffer_unlock);
}
