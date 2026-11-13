//! The engine's GPU command ring (the protocol of the browser build's
//! wgpu_worker.js and the engine's wgpu_backend): engine threads append
//! D3D11-style commands; one host thread, the GPU backend, executes them.
//!
//! Header (u32 words at the ring's base): head, tail, state, executed
//! commands, errors, sleeping. Commands start at [`RING_DATA`]: an opcode, the
//! payload length in words, the payload. [`op::WRAP`] continues at
//! [`RING_DATA`].
//!
//! The backend is chosen by the host ([`GpuBackend`]): [`null`] completes
//! everything without rendering; glue5-gpu renders with wgpu.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::{Duration, Instant};

use crate::memory::Mem;

pub const RING_DATA: u32 = 64;
const HEAD: u64 = 0;
const TAIL: u64 = 4;
const STATE: u64 = 8;
const EXECUTED: u64 = 12;
const ERRORS: u64 = 16;
const SLEEPING: u64 = 20;

/// Opcodes (wgpu_backend.h).
pub mod op {
    pub const NOP: u32 = 0;
    pub const WRAP: u32 = 1;
    pub const INIT: u32 = 2;
    pub const CREATE_TEXTURE: u32 = 3;
    pub const DESTROY_OBJECT: u32 = 4;
    pub const CLEAR_RT: u32 = 5;
    pub const CLEAR_DS: u32 = 6;
    pub const PRESENT: u32 = 7;
    pub const FENCE: u32 = 8;
    pub const SCREENSHOT: u32 = 9;
    pub const LOG: u32 = 10;
    pub const CREATE_BUFFER: u32 = 11;
    pub const UPLOAD_BUFFER: u32 = 12;
    pub const UPLOAD_TEXTURE: u32 = 13;
    pub const CREATE_SHADER: u32 = 14;
    pub const CREATE_LAYOUT: u32 = 15;
    pub const CREATE_STATE: u32 = 16;
    pub const CREATE_SRV: u32 = 17;
    pub const SET_SHADER: u32 = 18;
    pub const SET_INPUT_LAYOUT: u32 = 19;
    pub const SET_TOPOLOGY: u32 = 20;
    pub const SET_VERTEX_BUFFERS: u32 = 21;
    pub const SET_INDEX_BUFFER: u32 = 22;
    pub const SET_CBUFFERS: u32 = 23;
    pub const SET_SRVS: u32 = 24;
    pub const SET_SAMPLERS: u32 = 25;
    pub const SET_RENDER_TARGETS: u32 = 26;
    pub const SET_BLEND: u32 = 27;
    pub const SET_DEPTH_STENCIL: u32 = 28;
    pub const SET_RASTER: u32 = 29;
    pub const SET_VIEWPORTS: u32 = 30;
    pub const SET_SCISSORS: u32 = 31;
    pub const DRAW: u32 = 32;
    pub const CLEAR_STATE: u32 = 33;
    pub const COPY_REGION: u32 = 34;
    pub const COPY_RESOURCE: u32 = 35;
    pub const RESOLVE: u32 = 36;
    pub const CREATE_UAV: u32 = 37;
    pub const SET_UAVS: u32 = 38;
    pub const DISPATCH: u32 = 39;
    pub const CLEAR_UAV: u32 = 40;
    pub const GENERATE_MIPS: u32 = 41;
    pub const READBACK: u32 = 42;
    pub const COPY_COUNTER: u32 = 43;
    pub const CREATE_QUERY: u32 = 44;
    pub const BEGIN_QUERY: u32 = 45;
    pub const END_QUERY: u32 = 46;
    pub const UPLOAD_BUFFER_NO_OVERWRITE: u32 = 47;
    /// Host commands, put in the ring by the host's hooks (not the engine's
    /// backend): the scene is finished in the back buffer, the HUD follows.
    pub const SCENE_DONE: u32 = 64;

    /// Human-readable command name for diagnostics.
    pub const fn name(op: u32) -> &'static str {
        match op {
            NOP => "NOP",
            WRAP => "WRAP",
            INIT => "INIT",
            CREATE_TEXTURE => "CREATE_TEXTURE",
            DESTROY_OBJECT => "DESTROY_OBJECT",
            CLEAR_RT => "CLEAR_RT",
            CLEAR_DS => "CLEAR_DS",
            PRESENT => "PRESENT",
            FENCE => "FENCE",
            SCREENSHOT => "SCREENSHOT",
            LOG => "LOG",
            CREATE_BUFFER => "CREATE_BUFFER",
            UPLOAD_BUFFER => "UPLOAD_BUFFER",
            UPLOAD_TEXTURE => "UPLOAD_TEXTURE",
            CREATE_SHADER => "CREATE_SHADER",
            CREATE_LAYOUT => "CREATE_LAYOUT",
            CREATE_STATE => "CREATE_STATE",
            CREATE_SRV => "CREATE_SRV",
            SET_SHADER => "SET_SHADER",
            SET_INPUT_LAYOUT => "SET_INPUT_LAYOUT",
            SET_TOPOLOGY => "SET_TOPOLOGY",
            SET_VERTEX_BUFFERS => "SET_VERTEX_BUFFERS",
            SET_INDEX_BUFFER => "SET_INDEX_BUFFER",
            SET_CBUFFERS => "SET_CBUFFERS",
            SET_SRVS => "SET_SRVS",
            SET_SAMPLERS => "SET_SAMPLERS",
            SET_RENDER_TARGETS => "SET_RENDER_TARGETS",
            SET_BLEND => "SET_BLEND",
            SET_DEPTH_STENCIL => "SET_DEPTH_STENCIL",
            SET_RASTER => "SET_RASTER",
            SET_VIEWPORTS => "SET_VIEWPORTS",
            SET_SCISSORS => "SET_SCISSORS",
            DRAW => "DRAW",
            CLEAR_STATE => "CLEAR_STATE",
            COPY_REGION => "COPY_REGION",
            COPY_RESOURCE => "COPY_RESOURCE",
            RESOLVE => "RESOLVE",
            CREATE_UAV => "CREATE_UAV",
            SET_UAVS => "SET_UAVS",
            DISPATCH => "DISPATCH",
            CLEAR_UAV => "CLEAR_UAV",
            GENERATE_MIPS => "GENERATE_MIPS",
            READBACK => "READBACK",
            COPY_COUNTER => "COPY_COUNTER",
            CREATE_QUERY => "CREATE_QUERY",
            BEGIN_QUERY => "BEGIN_QUERY",
            END_QUERY => "END_QUERY",
            UPLOAD_BUFFER_NO_OVERWRITE => "UPLOAD_BUFFER_NO_OVERWRITE",
            SCENE_DONE => "SCENE_DONE",
            _ => "UNKNOWN",
        }
    }
}

/// Starts a backend on the ring; called once, from the engine thread that
/// starts the GPU. It must return promptly (run on a thread of its own).
pub type GpuBackend = Box<dyn FnOnce(GpuRing) + Send>;

/// The backend's state, as the engine sees it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u32)]
pub enum State {
    Starting = 0,
    Running = 1,
    /// Commands are drained and dropped.
    Failed = 2,
}

/// One command: its opcode and payload, in place in wasm memory.
#[derive(Clone, Copy)]
pub struct Command<'a> {
    pub op: u32,
    pub payload: &'a [u32],
}

impl<'a> Command<'a> {
    /// Payload word `i` (0 if the command is shorter).
    #[inline]
    pub fn w(&self, i: usize) -> u32 {
        self.payload.get(i).copied().unwrap_or(0)
    }

    pub fn f(&self, i: usize) -> f32 {
        f32::from_bits(self.w(i))
    }

    /// A 64-bit wasm address in words `i`, `i + 1`.
    pub fn addr(&self, i: usize) -> u64 {
        self.w(i) as u64 | (self.w(i + 1) as u64) << 32
    }

    /// The payload's bytes from word `i` on.
    pub fn bytes_from(&self, i: usize) -> &'a [u8] {
        let words = &self.payload[i.min(self.payload.len())..];
        unsafe { std::slice::from_raw_parts(words.as_ptr() as *const u8, words.len() * 4) }
    }
}

/// The engine's command ring, for a backend.
#[derive(Clone, Copy)]
pub struct GpuRing {
    mem: Mem,
    at: u64,
    size: u64,
}

impl GpuRing {
    pub(crate) fn new(mem: Mem, at: u64, size: u64) -> Self {
        GpuRing { mem, at, size }
    }

    pub fn mem(&self) -> Mem {
        self.mem
    }

    pub fn size(&self) -> u64 {
        self.size
    }

    fn word(&self, offset: u64) -> &AtomicU32 {
        self.mem.atomic_u32(self.at + offset)
    }

    pub fn set_state(&self, state: State) {
        self.word(STATE).store(state as u32, Ordering::SeqCst);
    }

    /// Counts device errors (the engine reports them).
    pub fn add_errors(&self, n: u32) {
        self.word(ERRORS).fetch_add(n, Ordering::Relaxed);
    }

    /// A reader starting at the beginning of the ring (publishes its tail).
    pub fn reader(&self) -> Reader {
        self.word(TAIL).store(RING_DATA, Ordering::SeqCst);
        Reader { ring: *self, tail: RING_DATA, executed: 0 }
    }
}

/// Consumes commands in order.
pub struct Reader {
    ring: GpuRing,
    tail: u32,
    executed: u32,
}

impl Reader {
    /// The engine's head: commands up to here are complete.
    pub fn head(&self) -> u32 {
        self.ring.word(HEAD).load(Ordering::Acquire)
    }

    pub fn is_empty(&self, head: u32) -> bool {
        head == self.tail
    }

    /// The next command before `head`, if any (wraps are followed).
    /// (Its payload lasts until the reader releases it.)
    pub fn next(&mut self, head: u32) -> Option<Command<'_>> {
        loop {
            if self.tail == head {
                return None;
            }
            let base = self.ring.at + self.tail as u64;
            let (op, n) = (self.ring.mem.u32(base), self.ring.mem.u32(base + 4));
            if op == op::WRAP {
                self.tail = RING_DATA;
                continue;
            }
            let payload = unsafe { std::slice::from_raw_parts(self.ring.mem.ptr(base + 8) as *const u32, n as usize) };
            self.tail += n * 4 + 8;
            self.executed += 1;
            return Some(Command { op, payload });
        }
    }

    /// Lets the engine reuse the space of the commands consumed so far. Data
    /// borrowed from those commands must not be used afterwards.
    pub fn release(&mut self) {
        let tail = self.ring.word(TAIL);
        tail.store(self.tail, Ordering::SeqCst);
        glue5_game::futex::notify(tail, u32::MAX);
        self.ring.word(EXECUTED).fetch_add(std::mem::take(&mut self.executed), Ordering::Relaxed);
    }

    /// Waits up to `timeout` for new commands. Producers only notify while
    /// the sleeping word is set: set it, then re-check the head.
    pub fn wait(&mut self, timeout: Duration) {
        self.release();
        let sleeping = self.ring.word(SLEEPING);
        sleeping.store(1, Ordering::SeqCst);
        let head = self.ring.word(HEAD);
        if head.load(Ordering::SeqCst) == self.tail {
            glue5_game::futex::wait32(head, self.tail, Some(timeout));
        }
        sleeping.store(0, Ordering::SeqCst);
    }
}

/// The sample count of an occlusion query whose real count is unknown (the
/// null backend draws nothing): visible, above any pixel threshold the
/// engine compares counts with.
pub const OCCLUSION_VISIBLE_COUNT: u64 = 65536;

/// D3D11_QUERY types.
pub mod query {
    pub const EVENT: u32 = 0;
    pub const OCCLUSION: u32 = 1;
    pub const TIMESTAMP: u32 = 2;
    pub const OCCLUSION_PREDICATE: u32 = 5;
}

/// Stores a query result into its slot in wasm memory: the 64-bit result at
/// +8, then `seq` at +0 (written last), and wakes waiters, those on
/// [`COMPLETED`] too.
pub fn store_query(mem: Mem, at: u64, result: u64, seq: u32) {
    mem.set_u32(at + 8, result as u32);
    mem.set_u32(at + 12, (result >> 32) as u32);
    mem.store_notify(at, seq);
    COMPLETED.fetch_add(1, Ordering::Release);
    glue5_game::futex::notify(&COMPLETED, u32::MAX);
}

/// Queries completed (fences among them), counted: the render thread waits
/// on it for its frame's fence (`waits.rs`).
pub(crate) static COMPLETED: AtomicU32 = AtomicU32::new(0);

/// A backend that renders nothing: it drains the ring and completes
/// everything the engine waits on (fences, readbacks, queries).
pub fn null() -> GpuBackend {
    Box::new(|ring| {
        if let Err(e) = std::thread::Builder::new().name("gpu (null)".into()).spawn(move || run_null(ring)) {
            crate::exit::fail(format!("no thread for the renderer: {e}"));
        }
    })
}

fn run_null(ring: GpuRing) {
    let mem = ring.mem();
    let start = Instant::now();
    let mut queries: HashMap<u32, (u32, u64)> = HashMap::new(); // id -> (type, result address)
    let mut reader = ring.reader();
    ring.set_state(State::Running);
    log::info!("gpu: null backend running");
    loop {
        let head = reader.head();
        if reader.is_empty(head) {
            reader.wait(Duration::from_millis(100));
            continue;
        }
        let mut n = 0;
        while let Some(c) = reader.next(head) {
            match c.op {
                op::FENCE => mem.store_notify(c.addr(0), c.w(2)),
                op::READBACK => {
                    if c.addr(15) != 0 {
                        mem.store_notify(c.addr(15), 0);
                    }
                }
                op::CREATE_QUERY => {
                    queries.insert(c.w(0), (c.w(1), c.addr(2)));
                }
                op::END_QUERY => {
                    if let Some(&(kind, at)) = queries.get(&c.w(0)) {
                        let result = match kind {
                            query::OCCLUSION | query::OCCLUSION_PREDICATE => OCCLUSION_VISIBLE_COUNT,
                            query::TIMESTAMP => start.elapsed().as_nanos() as u64,
                            _ => 1,
                        };
                        store_query(mem, at, result, c.w(1));
                    }
                }
                op::DESTROY_OBJECT => {
                    queries.remove(&c.w(0));
                }
                _ => {}
            }
            n += 1;
            if n % 512 == 0 {
                reader.release();
            }
        }
        reader.release();
    }
}
