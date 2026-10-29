//! The module's linear memory: reserved once (so it never moves while other
//! threads run) and accessed by host code through [`Mem`].

use std::ffi::CStr;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};

use anyhow::{Result, bail};
use glue5_game::{Env, MEMORY_INITIAL_PAGES, MEMORY_MAX_PAGES, Memory, PAGE_SIZE, orig};

pub struct WasmMemory {
    pub desc: *mut Memory,
    base: *mut u8,
    max_pages: u64,
}

unsafe impl Send for WasmMemory {}
unsafe impl Sync for WasmMemory {}

impl WasmMemory {
    /// Reserves address space for the largest memory the module may grow to
    /// (or the most the OS grants, but at least the initial size). Pages count
    /// once written; on Windows, which does not overcommit, the memory's
    /// current size is committed too ([`commit`]).
    pub fn reserve() -> Result<Self> {
        let mut max_pages = MEMORY_MAX_PAGES;
        loop {
            let bytes = (max_pages * PAGE_SIZE) as usize;
            if let Some(base) = map(bytes) {
                if !commit(base, 0, MEMORY_INITIAL_PAGES * PAGE_SIZE) {
                    bail!("cannot commit {} MiB for the wasm memory", (MEMORY_INITIAL_PAGES * PAGE_SIZE) >> 20);
                }
                log::info!("wasm memory: {} MiB reserved at {base:p}", bytes >> 20);
                let desc = unsafe { Memory::new(base, MEMORY_INITIAL_PAGES, max_pages) };
                return Ok(WasmMemory { desc, base, max_pages });
            }
            if max_pages / 2 < MEMORY_INITIAL_PAGES {
                bail!("cannot reserve {} MiB for the wasm memory", (MEMORY_INITIAL_PAGES * PAGE_SIZE) >> 20);
            }
            max_pages /= 2;
        }
    }

    pub fn mem(&self) -> Mem {
        Mem { base: self.base }
    }

    pub fn max_bytes(&self) -> u64 {
        self.max_pages * PAGE_SIZE
    }

    pub fn size(&self) -> u64 {
        unsafe { (*self.desc).size }
    }

    /// `emscripten_resize_heap`: memory is already reserved, growing only
    /// publishes the new size (and commits it, on Windows).
    pub fn grow_to(&self, bytes: u64) -> bool {
        if bytes > self.max_bytes() {
            return false;
        }
        let pages = bytes.div_ceil(PAGE_SIZE);
        // Committed before it is published: committing pages that are
        // already is harmless, so racing growers need no lock.
        if !commit(self.base, self.size(), pages * PAGE_SIZE) {
            return false;
        }
        unsafe {
            if pages > (*self.desc).pages {
                (*self.desc).pages = pages;
                (*self.desc).size = pages * PAGE_SIZE;
            }
        }
        true
    }
}

#[cfg(unix)]
fn map(bytes: usize) -> Option<*mut u8> {
    let ptr = unsafe {
        libc::mmap(
            std::ptr::null_mut(),
            bytes,
            libc::PROT_READ | libc::PROT_WRITE,
            libc::MAP_PRIVATE | libc::MAP_ANON | libc::MAP_NORESERVE,
            -1,
            0,
        )
    };
    (ptr != libc::MAP_FAILED).then_some(ptr as *mut u8)
}

/// Makes `[from, to)` of the mapping usable. Unix systems overcommit: the
/// whole mapping is, already.
#[cfg(unix)]
fn commit(_base: *mut u8, _from: u64, _to: u64) -> bool {
    true
}

/// Address space only: committed memory counts against the system's commit
/// limit (RAM and page file), however little of it is touched.
#[cfg(windows)]
fn map(bytes: usize) -> Option<*mut u8> {
    use windows_sys::Win32::System::Memory::{MEM_RESERVE, PAGE_NOACCESS, VirtualAlloc};
    let ptr = unsafe { VirtualAlloc(std::ptr::null(), bytes, MEM_RESERVE, PAGE_NOACCESS) };
    (!ptr.is_null()).then_some(ptr as *mut u8)
}

/// Commits `[from, to)` of the reservation, as the memory grows: its only
/// growth is `emscripten_resize_heap` ([`WasmMemory::grow_to`]). Committed
/// pages read as zeros, as wasm's do.
#[cfg(windows)]
fn commit(base: *mut u8, from: u64, to: u64) -> bool {
    use windows_sys::Win32::System::Memory::{MEM_COMMIT, PAGE_READWRITE, VirtualAlloc};
    if to <= from {
        return true;
    }
    let ptr = unsafe { VirtualAlloc(base.add(from as usize).cast(), (to - from) as usize, MEM_COMMIT, PAGE_READWRITE) };
    !ptr.is_null()
}

/// A view of wasm memory for host code. Addresses are wasm (linear memory)
/// addresses. The memory is shared with running wasm threads; plain accessors
/// are unordered, like non-atomic wasm accesses.
#[derive(Clone, Copy)]
pub struct Mem {
    base: *mut u8,
}

unsafe impl Send for Mem {}
unsafe impl Sync for Mem {}

impl Mem {
    /// # Safety
    /// `env` must be a live instance's environment.
    pub unsafe fn of(env: *mut Env) -> Mem {
        Mem { base: unsafe { (*(*env).memory).data } }
    }

    /// A view of `buf` as if it were wasm memory (tests).
    #[cfg(test)]
    pub(crate) fn over(buf: &mut [u8]) -> Mem {
        Mem { base: buf.as_mut_ptr() }
    }

    pub fn ptr(self, addr: u64) -> *mut u8 {
        unsafe { self.base.add(addr as usize) }
    }

    pub fn u8(self, addr: u64) -> u8 {
        unsafe { *self.ptr(addr) }
    }
    pub fn u16(self, addr: u64) -> u16 {
        unsafe { self.ptr(addr).cast::<u16>().read_unaligned() }
    }
    pub fn u32(self, addr: u64) -> u32 {
        unsafe { self.ptr(addr).cast::<u32>().read_unaligned() }
    }
    pub fn i32(self, addr: u64) -> i32 {
        self.u32(addr) as i32
    }
    pub fn u64(self, addr: u64) -> u64 {
        unsafe { self.ptr(addr).cast::<u64>().read_unaligned() }
    }
    pub fn f64(self, addr: u64) -> f64 {
        f64::from_bits(self.u64(addr))
    }

    pub fn set_u8(self, addr: u64, v: u8) {
        unsafe { *self.ptr(addr) = v }
    }
    pub fn set_u16(self, addr: u64, v: u16) {
        unsafe { self.ptr(addr).cast::<u16>().write_unaligned(v) }
    }
    pub fn set_u32(self, addr: u64, v: u32) {
        unsafe { self.ptr(addr).cast::<u32>().write_unaligned(v) }
    }
    pub fn set_i32(self, addr: u64, v: i32) {
        self.set_u32(addr, v as u32)
    }
    pub fn set_u64(self, addr: u64, v: u64) {
        unsafe { self.ptr(addr).cast::<u64>().write_unaligned(v) }
    }
    pub fn set_i64(self, addr: u64, v: i64) {
        self.set_u64(addr, v as u64)
    }

    pub fn bytes<'a>(self, addr: u64, len: usize) -> &'a [u8] {
        unsafe { std::slice::from_raw_parts(self.ptr(addr), len) }
    }
    #[allow(clippy::mut_from_ref)]
    pub fn bytes_mut<'a>(self, addr: u64, len: usize) -> &'a mut [u8] {
        unsafe { std::slice::from_raw_parts_mut(self.ptr(addr), len) }
    }

    /// NUL-terminated string at `addr` (lossy UTF-8).
    pub fn str(self, addr: u64) -> String {
        if addr == 0 {
            return String::new();
        }
        unsafe { CStr::from_ptr(self.ptr(addr) as *const _) }.to_string_lossy().into_owned()
    }

    /// Emscripten's `stringToUTF8`: writes at most `cap - 1` bytes plus a NUL.
    /// Returns the bytes written, without the NUL.
    pub fn write_str(self, addr: u64, s: &str, cap: usize) -> usize {
        if cap == 0 {
            return 0;
        }
        let mut n = s.len().min(cap - 1);
        while !s.is_char_boundary(n) {
            n -= 1;
        }
        self.bytes_mut(addr, n).copy_from_slice(&s.as_bytes()[..n]);
        self.set_u8(addr + n as u64, 0);
        n
    }

    pub fn atomic_u32<'a>(self, addr: u64) -> &'a AtomicU32 {
        unsafe { &*(self.ptr(addr) as *const AtomicU32) }
    }
    pub fn atomic_u64<'a>(self, addr: u64) -> &'a AtomicU64 {
        unsafe { &*(self.ptr(addr) as *const AtomicU64) }
    }

    /// `Atomics.store` + `Atomics.notify` on a 32-bit word.
    pub fn store_notify(self, addr: u64, v: u32) {
        let word = self.atomic_u32(addr);
        word.store(v, Ordering::SeqCst);
        glue5_game::futex::notify(word, u32::MAX);
    }
}

/// The process's memory footprint (what iOS limits; the resident set on
/// Android and Linux) and, on iOS and tvOS, how much more it may take before
/// the system terminates it, in bytes.
pub fn footprint() -> (Option<u64>, Option<u64>) {
    #[cfg(target_vendor = "apple")]
    let used = unsafe {
        // libproc: in the system on iOS too, though its header is macOS-only.
        unsafe extern "C" {
            fn proc_pid_rusage(pid: libc::c_int, flavor: libc::c_int, buffer: *mut libc::c_void) -> libc::c_int;
        }
        let mut info: libc::rusage_info_v4 = std::mem::zeroed();
        let ok = proc_pid_rusage(libc::getpid(), libc::RUSAGE_INFO_V4, &mut info as *mut _ as *mut libc::c_void) == 0;
        ok.then_some(info.ri_phys_footprint)
    };
    #[cfg(any(target_os = "android", target_os = "linux"))]
    let used = std::fs::read_to_string("/proc/self/statm").ok().and_then(|s| {
        let pages: u64 = s.split_whitespace().nth(1)?.parse().ok()?;
        Some(pages * unsafe { libc::sysconf(libc::_SC_PAGESIZE) } as u64)
    });
    #[cfg(not(any(target_vendor = "apple", target_os = "android", target_os = "linux")))]
    let used = None;
    #[cfg(any(target_os = "ios", target_os = "tvos"))]
    let available = {
        unsafe extern "C" {
            fn os_proc_available_memory() -> usize;
        }
        Some(unsafe { os_proc_available_memory() } as u64)
    };
    #[cfg(not(any(target_os = "ios", target_os = "tvos")))]
    let available = None;
    (used, available)
}

// ---- the engine's resource heap ----------------------------------------------

/// The resource heap's budget, in bytes (0: none).
static RESOURCE_BUDGET: AtomicU64 = AtomicU64::new(0);

thread_local! {
    /// The streaming engine is asking how much memory is left.
    static ASKING: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// Budgets the resource heap at `mb` MiB (the engine's `gameResourceMB`).
pub(crate) fn budget_resource_heap(mb: u32) {
    RESOURCE_BUDGET.store((mb as u64) << 20, Ordering::Relaxed);
}

/// Hook for `rage::sysMemGrowBuddyAllocator::GrowHeap()`. The PC build's
/// resource heap starts at `gameResourceMB` and grows by up to 256 MiB, for
/// an allocation and also whenever the streaming engine asks how much memory
/// is left (answering grows it), so it holds whatever the player has passed
/// by: on a phone, past what the system allows. With a budget, answering no
/// longer grows it past the budget, nor counts what lies past it as free:
/// the streaming engine sees it full and evicts, as on consoles. Allocations
/// still grow it, though: the streaming engine does not survive one failing
/// ("read request ... using destination addresses, some of which are NULL",
/// a fatal error). -1: no growth.
pub(crate) unsafe extern "C" fn grow_heap(env: *mut Env, this: u64) -> u32 {
    // Heaps come a little short of whole MiB (255, 511, ...): within the
    // smallest growth step of the budget counts as there.
    const STEP: u64 = 16 << 20;
    let budget = RESOURCE_BUDGET.load(Ordering::Relaxed);
    if budget != 0 && ASKING.get() && unsafe { orig::heap_size(env, this) } + STEP > budget {
        return u32::MAX;
    }
    unsafe { orig::grow_heap(env, this) }
}

/// Hook for `rage::sysMemGrowBuddyAllocator::GetMemoryAvailable()`: past
/// the budget (grown by allocations), what lies beyond it does not count as
/// free, so the streaming engine evicts back under it.
pub(crate) unsafe extern "C" fn memory_available(env: *mut Env, this: u64) -> u64 {
    // The two ask each other: inner calls are answered as they are.
    if ASKING.get() {
        return unsafe { orig::memory_available(env, this) };
    }
    let available = asking(|| unsafe { orig::memory_available(env, this) });
    available.saturating_sub(unsafe { over_budget(env, this) })
}

/// Hook for `rage::sysMemGrowBuddyAllocator::GetLargestAvailableBlock()`
/// (no larger than what [`memory_available`] says is free).
pub(crate) unsafe extern "C" fn largest_block(env: *mut Env, this: u64) -> u64 {
    if ASKING.get() {
        return unsafe { orig::largest_block(env, this) };
    }
    let largest = asking(|| unsafe { orig::largest_block(env, this) });
    match unsafe { over_budget(env, this) } {
        0 => largest,
        over => largest.min(asking(|| unsafe { orig::memory_available(env, this) }).saturating_sub(over)),
    }
}

/// How far the heap has grown past the budget, in bytes.
unsafe fn over_budget(env: *mut Env, this: u64) -> u64 {
    match RESOURCE_BUDGET.load(Ordering::Relaxed) {
        0 => 0,
        budget => unsafe { orig::heap_size(env, this) }.saturating_sub(budget),
    }
}

fn asking<R>(f: impl FnOnce() -> R) -> R {
    ASKING.set(true);
    let r = f();
    ASKING.set(false);
    r
}

// ---- what the engine's heaps hold, on the 10 s stats -------------------------

/// The engine's heaps (addresses in this build, from `InitGameHeap`): the
/// game heap and the replay's (`sysMemSimpleAllocator`); the resource heap
/// and the one for "physical" memory, little used here
/// (`sysMemGrowBuddyAllocator`s).
const GAME_HEAP: u64 = 8017912;
const REPLAY_HEAP: u64 = 8060376;
const RESOURCE_HEAP: u64 = 8043728;
const PHYSICAL_HEAP: u64 = 8026992;

/// When the heaps were last reported (`Clock` ms).
static REPORTED: AtomicU64 = AtomicU64::new(0);

/// Logs what the engine's heaps hold, every 10 s (from the render thread):
/// how the wasm memory, which never shrinks, is made up.
pub(crate) fn report_heaps(mem: Mem) {
    let now = crate::rt().clock.now_ms() as u64;
    if now < REPORTED.load(Ordering::Relaxed) + 10_000 {
        return;
    }
    REPORTED.store(now, Ordering::Relaxed);
    let mb = |b: u64| b >> 20;
    let (game, replay) = (simple_heap(mem, GAME_HEAP), simple_heap(mem, REPLAY_HEAP));
    let (resources, physical) = (buddy_heap(mem, RESOURCE_HEAP), buddy_heap(mem, PHYSICAL_HEAP));
    log::info!(
        "memory: wasm memory {} MiB; game heap {}/{} MiB ({} at most), resource heap {}/{} MiB, physical heap {}/{} MiB, replay heap {}/{} MiB; {} MiB freed given back in all ({} MiB taken back)",
        mb(crate::rt().memory.size()),
        mb(game.0),
        mb(game.1),
        mb(game.2),
        mb(resources.0),
        mb(resources.1),
        mb(physical.0),
        mb(physical.1),
        mb(replay.0),
        mb(replay.1),
        mb(GIVEN.load(Ordering::Relaxed)),
        mb(TAKEN_BACK.load(Ordering::Relaxed)),
    );
}

/// A `sysMemSimpleAllocator`'s bytes in use, size and most in use so far
/// (its size less the least it had free).
fn simple_heap(mem: Mem, at: u64) -> (u64, u64, u64) {
    let size = mem.u64(at + 280);
    (mem.u64(at + 296), size, size.saturating_sub(mem.u64(at + 568)))
}

/// A `sysMemGrowBuddyAllocator`'s bytes in use and size.
fn buddy_heap(mem: Mem, at: u64) -> (u64, u64) {
    segments(mem, at).fold((0, 0), |(used, size), s| (used + s.used, size + s.size))
}

/// A segment of a `sysMemGrowBuddyAllocator` (a `sysMemBuddyAllocator`).
struct Segment {
    base: u64,
    size: u64,
    /// Leaves in use times the leaf size.
    used: u64,
}

/// A `sysMemGrowBuddyAllocator`'s segments, in the order it grew.
fn segments(mem: Mem, at: u64) -> impl Iterator<Item = Segment> {
    (0..mem.i32(at + 508).max(0) as u64).filter_map(move |i| {
        let base = mem.u64(at + 16384 + i * 8);
        let segment = at + 512 + i * 488;
        let used = mem.u64(segment + 32) * mem.u64(segment + 56);
        (base != 0).then(|| Segment { base, size: mem.u64(segment + 24), used })
    })
}

// ---- freed resource memory back to the system, past the budget -----------------

/// Hook for `rage::sysMemGrowBuddyAllocator::Free()`. Past the resource
/// heap's budget lie segments grown for allocations: the streaming engine
/// keeps what it uses under the budget, but spread over every segment, so
/// the heap holds hundreds of MiB it has written and freed (the low-memory
/// benchmark freed 450 MiB past the budget, of which a third was taken
/// again). A block freed there goes back to the system, its whole pages,
/// before it is freed (until then it is the caller's alone; the allocator
/// keeps nothing in free blocks). Not inside the budget: that is the cache
/// the streaming engine is meant to keep.
pub(crate) unsafe extern "C" fn buddy_free(env: *mut Env, this: u64, ptr: u64) {
    let budget = RESOURCE_BUDGET.load(Ordering::Relaxed);
    if this == RESOURCE_HEAP && budget != 0 && ptr != 0 {
        let mem = unsafe { Mem::of(env) };
        // Segments come a little short of whole MiB, as in `grow_heap`.
        let mut start = 0;
        for s in segments(mem, this) {
            if (s.base..s.base + s.size).contains(&ptr) {
                if start + (16 << 20) > budget {
                    give_back(mem, ptr, unsafe { orig::buddy_block_size(env, this, ptr) });
                }
                break;
            }
            start += s.size;
        }
    }
    unsafe { orig::buddy_free(env, this, ptr) }
}

/// Hook for `rage::sysMemGrowBuddyAllocator::TryAllocate()` (Apple): pages
/// given back come back into the process's footprint before the block is
/// handed out, as with the system's malloc.
#[cfg(target_vendor = "apple")]
pub(crate) unsafe extern "C" fn buddy_try_allocate(env: *mut Env, this: u64, size: u64, align: u64, bucket: u32) -> u64 {
    let ptr = unsafe { orig::buddy_try_allocate(env, this, size, align, bucket) };
    if ptr != 0
        && let Some(given) = GIVEN_BACK.get()
    {
        let size = unsafe { orig::buddy_block_size(env, this, ptr) }.max(size);
        let (first, end) = (ptr / page_size(), (ptr + size).div_ceil(page_size()));
        let mut taken = 0;
        for (word, mask) in page_bits(first, end) {
            if given[word].load(Ordering::Acquire) & mask != 0 {
                taken += (given[word].fetch_and(!mask, Ordering::AcqRel) & mask).count_ones() as u64;
            }
        }
        if taken != 0 {
            TAKEN_BACK.fetch_add(taken * page_size(), Ordering::Relaxed);
            let mem = unsafe { Mem::of(env) };
            let len = (end - first) * page_size();
            unsafe { libc::madvise(mem.ptr(first * page_size()).cast(), len as usize, libc::MADV_FREE_REUSE) };
        }
    }
    ptr
}

/// Pages given back (Apple: a bit each, by wasm address), which allocations
/// take back.
#[cfg(target_vendor = "apple")]
static GIVEN_BACK: OnceLock<Box<[AtomicU64]>> = OnceLock::new();

/// Bytes given back, and taken back (Apple), in all.
static GIVEN: AtomicU64 = AtomicU64::new(0);
static TAKEN_BACK: AtomicU64 = AtomicU64::new(0);

/// The bits of pages `[first, end)`: (word, mask) pairs.
#[cfg(target_vendor = "apple")]
fn page_bits(first: u64, end: u64) -> impl Iterator<Item = (usize, u64)> {
    (first / 64..end.div_ceil(64)).map(move |word| {
        let (lo, hi) = (first.max(word * 64) - word * 64, end.min(word * 64 + 64) - word * 64);
        (word as usize, (u64::MAX >> (64 - (hi - lo))) << lo)
    })
}

/// Gives the whole pages of `[at, at + len)` back to the system. Apple:
/// out of the footprint at once, though the pages stay until the system
/// needs them, and taken back by [`buddy_try_allocate`]. Elsewhere (Unix):
/// dropped, zero when touched again.
fn give_back(mem: Mem, at: u64, len: u64) {
    let (first, end) = (at.div_ceil(page_size()), (at + len) / page_size());
    if end <= first || cfg!(not(unix)) {
        return;
    }
    let ptr = mem.ptr(first * page_size()).cast::<std::ffi::c_void>();
    let len = ((end - first) * page_size()) as usize;
    GIVEN.fetch_add(len as u64, Ordering::Relaxed);
    #[cfg(target_vendor = "apple")]
    {
        let words = (crate::rt().memory.max_bytes() / page_size()).div_ceil(64) as usize;
        let given = GIVEN_BACK.get_or_init(|| (0..words).map(|_| AtomicU64::new(0)).collect());
        // Marked first: the block is freed only after this returns.
        for (word, mask) in page_bits(first, end) {
            given[word].fetch_or(mask, Ordering::AcqRel);
        }
        unsafe { libc::madvise(ptr, len, libc::MADV_FREE_REUSABLE) };
    }
    #[cfg(all(unix, not(target_vendor = "apple")))]
    unsafe {
        libc::madvise(ptr, len, libc::MADV_DONTNEED)
    };
    #[cfg(windows)]
    let _ = ptr;
}

/// The system's page size.
fn page_size() -> u64 {
    static SIZE: OnceLock<u64> = OnceLock::new();
    *SIZE.get_or_init(|| {
        #[cfg(unix)]
        let size = unsafe { libc::sysconf(libc::_SC_PAGESIZE) } as u64;
        #[cfg(windows)]
        let size = 4096;
        size
    })
}

/// Hook for libc++'s `operator_new_impl` (its nothrow `operator new`, which
/// std::stable_sort takes its buffers from: a cutscene's load events, the
/// HUD's markers, past 128 of them): rage's `operator delete` frees them,
/// into the thread's rage heap, which owns no dlmalloc block (it stops the
/// game, or takes the block as its own). Rage's `operator new` makes them,
/// as zlib's default allocator does.
pub(crate) unsafe extern "C" fn nothrow_new(env: *mut Env, size: u64) -> u64 {
    match u32::try_from(size.max(1)) {
        Ok(size) => unsafe { orig::zcalloc(env, 0, 1, size) },
        Err(_) => 0,
    }
}

/// This module's hooks.
pub(crate) fn hooks(hooks: &mut glue5_game::Hooks) {
    hooks.nothrow_new = Some(nothrow_new);
    hooks.grow_heap = Some(grow_heap);
    hooks.memory_available = Some(memory_available);
    hooks.largest_block = Some(largest_block);
    hooks.buddy_free = Some(buddy_free);
    #[cfg(target_vendor = "apple")]
    {
        hooks.buddy_try_allocate = Some(buddy_try_allocate);
    }
}

#[cfg(all(test, target_vendor = "apple"))]
#[test]
fn page_bits_cover_the_pages() {
    let bits: Vec<_> = page_bits(3, 130).collect();
    assert_eq!(bits, [(0, u64::MAX << 3), (1, u64::MAX), (2, 0b11)]);
    assert_eq!(page_bits(64, 128).collect::<Vec<_>>(), [(1, u64::MAX)]);
    assert_eq!(page_bits(5, 6).collect::<Vec<_>>(), [(0, 1 << 5)]);
}
