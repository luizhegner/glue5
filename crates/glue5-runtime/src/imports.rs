//! The module's imports, one function per import (named like it), collected
//! by `glue5_game::imports_from!`. Structure layouts are the wasm64 ones used
//! by the browser build's game.js.

#![allow(clippy::missing_safety_doc)]

use std::sync::atomic::Ordering;

use glue5_game::{Env, exports};

use crate::memory::Mem;
use crate::time;
use crate::vfs::{self, Errno, errno};
use crate::{gamelog, gpu, rt, threads};

fn mem(env: *mut Env) -> Mem {
    // Every import receives a live instance's environment.
    unsafe { Mem::of(env) }
}

/// Syscall result: value, or negative errno.
fn sys<T: Into<i64>>(r: Result<T, Errno>) -> u32 {
    match r {
        Ok(v) => v.into() as u32,
        Err(e) => (-(e as i32)) as u32,
    }
}

/// WASI result: 0 or errno.
fn wasi(r: Result<(), Errno>) -> u32 {
    r.err().unwrap_or(0) as u32
}

fn path_at(env: *mut Env, dirfd: u32, path: u64) -> Result<String, Errno> {
    rt().vfs.at(dirfd as i32, &mem(env).str(path))
}

fn unsupported(name: &str) {
    log::warn!("unsupported import called: {name}");
}

// ---- game (game.js) ----------------------------------------------------------

pub unsafe extern "C" fn wasm_now_ms(_: *mut Env) -> f64 {
    rt().clock.now_ms()
}

pub unsafe extern "C" fn wasm_print_line_js(env: *mut Env, line: u64) {
    gamelog::line(&mem(env).str(line));
}

pub unsafe extern "C" fn wasm_hang_line_js(env: *mut Env, line: u64) {
    gamelog::line(&format!("[hang] {}", mem(env).str(line)));
}

pub unsafe extern "C" fn wasm_audio_publish_js(_: *mut Env, ring: u64, capacity: u32) {
    log::info!("audio: ring at {ring:#x}, {capacity} frames");
    rt().audio_ring.store(ring, Ordering::Release);
}

pub unsafe extern "C" fn wasm_input_publish_js(_: *mut Env, block: u64) {
    log::info!("input: block at {block:#x}");
    rt().input_block.store(block, Ordering::Release);
}

pub unsafe extern "C" fn wgpu_start_worker(env: *mut Env, ring: u64, size: u64, _dbg: u64) {
    log::info!("gpu: ring at {ring:#x}, {} MiB", size >> 20);
    rt().gpu_ring.store(ring, Ordering::Release);
    match rt().gpu_backend.lock().unwrap().take() {
        Some(backend) => backend(gpu::GpuRing::new(mem(env), ring, size)),
        None => log::error!("gpu: the engine started a second GPU worker"),
    }
}

/// The engine's HTTP file system: when the game streams, its manifest of the
/// `/game` files (copied when `cap` holds it and a NUL, the length always);
/// otherwise -1, and the engine reads /game through the syscalls.
pub unsafe extern "C" fn wasm_httpfs_manifest_js(env: *mut Env, buf: u64, cap: u32) -> u32 {
    let Some(manifest) = crate::stream::manifest() else { return u32::MAX };
    if cap as usize > manifest.len() {
        let m = mem(env);
        m.bytes_mut(buf, manifest.len()).copy_from_slice(manifest);
        m.set_u8(buf + manifest.len() as u64, 0);
    }
    manifest.len() as u32
}

pub unsafe extern "C" fn wasm_httpfs_io_start_js(_: *mut Env, table: u64) {
    if !crate::stream::start_announcements(table) {
        unsupported("wasm_httpfs_io_start_js");
    }
}

/// 0: saves are real files under /userdata, no mirroring needed.
pub unsafe extern "C" fn wasm_userdata_page_js(_: *mut Env) -> u32 {
    0
}

pub unsafe extern "C" fn wasm_userdata_put_js(_: *mut Env, _path: u64, _data: u64, _size: u64, _mtime: f64) {}

pub unsafe extern "C" fn wasm_userdata_delete_js(_: *mut Env, _path: u64) {}

/// 1: like the browser (the start mode comes from -wasmstartchoice).
pub unsafe extern "C" fn wasm_has_page_js(_: *mut Env) -> u32 {
    1
}

pub unsafe extern "C" fn wasm_module_int_js(env: *mut Env, name: u64, default: u32) -> u32 {
    let name = mem(env).str(name);
    let low = rt().config.low_memory;
    let value = match std::env::var(name.to_ascii_uppercase()).ok().and_then(|v| v.parse().ok()) {
        Some(v) => v,
        None if name == "gameResourceMB" && low => default.min(768),
        // The engine skips some threads, unless their bit is set here.
        None if name == "keepThreads" => {
            // The videos' thread (bit 4, `AsyncBink`). TVs and some
            // cutscenes queue their video for it and wait, with no time
            // limit, until it has tried to open it; without the thread
            // nothing answered, and the queue (32 requests) filled up. It
            // fails at once (the build has no Bink, the data no videos)
            // and marks the video as failed, as the engine expects.
            let videos = default | 4;
            // Online, the network's threads run (bit 2).
            if rt().config.online.is_some() { videos | 2 } else { videos }
        }
        None => default,
    };
    // Phones and the Apple TV, and low memory anywhere: the resource heap's budget is its
    // starting size. Unbudgeted, the streaming engine keeps it full and it
    // grows by 256 MiB each time: past 2.5 GiB in the game's benchmark.
    let phone = cfg!(any(target_os = "ios", target_os = "android", target_os = "tvos"));
    if name == "gameResourceMB" && (low || phone) {
        crate::memory::budget_resource_heap(value);
    }
    value
}

// ---- Emscripten runtime ----------------------------------------------------------

pub unsafe extern "C" fn emscripten_get_now(_: *mut Env) -> f64 {
    rt().clock.now_ms()
}

pub unsafe extern "C" fn emscripten_date_now(_: *mut Env) -> f64 {
    time::date_now()
}

pub unsafe extern "C" fn emscripten_get_heap_max(_: *mut Env) -> u64 {
    rt().memory.max_bytes()
}

pub unsafe extern "C" fn emscripten_resize_heap(_: *mut Env, requested: u64) -> u32 {
    let memory = &rt().memory;
    let old = memory.size();
    if requested <= old || requested > memory.max_bytes() {
        return 0;
    }
    // Same over-allocation as Emscripten: +20 %, capped at the request + 96 MiB.
    let over = ((old as f64 * 1.2) as u64).min(requested + (96 << 20));
    let new = requested.max(over).next_multiple_of(65536).min(memory.max_bytes());
    memory.grow_to(new) as u32
}

pub unsafe extern "C" fn emscripten_num_logical_cores(_: *mut Env) -> u32 {
    // All of them: the browser's low-memory profile reported 2 because each
    // Web Worker was a heavy instance; native threads are not.
    crate::cores() as u32
}

pub unsafe extern "C" fn emscripten_stack_snapshot(_: *mut Env) -> u64 {
    0
}

pub unsafe extern "C" fn emscripten_stack_unwind_buffer(_: *mut Env, _addr: u64, _buffer: u64, _count: u32) -> u32 {
    0
}

pub unsafe extern "C" fn emscripten_check_blocking_allowed(_: *mut Env) {}

pub unsafe extern "C" fn emscripten_exit_with_live_runtime(_: *mut Env) {
    glue5_game::unwind()
}

pub unsafe extern "C" fn exit(_: *mut Env, code: u32) {
    log::info!("engine exit({code})");
    crate::exit::process(code as i32)
}

pub unsafe extern "C" fn proc_exit(env: *mut Env, code: u32) {
    unsafe { exit(env, code) }
}

pub unsafe extern "C" fn _abort_js(_: *mut Env) {
    log::error!("engine abort()");
    std::process::abort()
}

pub unsafe extern "C" fn _emscripten_system(_: *mut Env, command: u64) -> u32 {
    if command == 0 { 0 } else { (-(errno::NOSYS as i32)) as u32 }
}

/// EM_ASM blocks of this build, by code address (ASM_CONSTS in game.js).
const EM_ASM_LANGUAGE: u64 = 7980558;
const EM_ASM_WINDOW_OPEN: u64 = 7980673;
const EM_ASM_RELOAD: u64 = 7980768;

pub unsafe extern "C" fn emscripten_asm_const_int(env: *mut Env, code: u64, sig: u64, args: u64) -> u32 {
    let m = mem(env);
    let args = em_asm_args(m, sig, args);
    match code {
        EM_ASM_LANGUAGE => {
            let (buf, cap) = (args.first().copied().unwrap_or(0), args.get(1).copied().unwrap_or(0));
            m.write_str(buf, &language(), cap as usize);
            0
        }
        EM_ASM_WINDOW_OPEN | EM_ASM_RELOAD => 0,
        other => {
            log::warn!("unknown EM_ASM block {other}");
            0
        }
    }
}

pub unsafe extern "C" fn emscripten_asm_const_int_sync_on_main_thread(
    env: *mut Env,
    code: u64,
    sig: u64,
    args: u64,
) -> u32 {
    unsafe { emscripten_asm_const_int(env, code, sig, args) }
}

/// `readEmAsmArgs`: `i` is 4 bytes; `p`/`j`/`d` are 8 bytes, 8-aligned.
fn em_asm_args(m: Mem, mut sig: u64, mut buf: u64) -> Vec<u64> {
    let mut args = Vec::new();
    loop {
        let ch = m.u8(sig);
        if ch == 0 {
            break;
        }
        sig += 1;
        if ch == b'i' {
            args.push(m.i32(buf) as i64 as u64);
            buf += 4;
        } else {
            buf = buf.next_multiple_of(8);
            args.push(m.u64(buf));
            buf += 8;
        }
    }
    args
}

fn language() -> String {
    rt().config
        .language
        .clone()
        .or_else(|| std::env::var("LANG").ok().and_then(|l| l.split('.').next().map(|l| l.replace('_', "-"))))
        .filter(|l| !l.is_empty() && l != "C" && l != "POSIX")
        .unwrap_or_else(|| "en-US".into())
}

// ---- threads -----------------------------------------------------------------

pub unsafe extern "C" fn _emscripten_init_main_thread_js(env: *mut Env, pthread: u64) {
    unsafe { threads::init_main_thread(env, pthread) }
}

pub unsafe extern "C" fn __pthread_create_js(env: *mut Env, pthread: u64, _attr: u64, start: u64, arg: u64) -> u32 {
    unsafe { threads::create(env, pthread, start, arg) }
}

pub unsafe extern "C" fn _emscripten_thread_cleanup(env: *mut Env, pthread: u64) {
    unsafe { threads::cleanup(env, pthread) }
}

pub unsafe extern "C" fn _emscripten_thread_set_strongref(_: *mut Env, _pthread: u64) {}

pub unsafe extern "C" fn _emscripten_thread_mailbox_await(_: *mut Env, _pthread: u64) {}

pub unsafe extern "C" fn _emscripten_notify_mailbox_postmessage(env: *mut Env, target: u64, current: u64) {
    if target == current {
        // Emscripten defers this to the event loop; run it now.
        let _ = unsafe { exports::_emscripten_check_mailbox(env) };
    } else {
        unsupported("_emscripten_notify_mailbox_postmessage (other thread)");
    }
}

pub unsafe extern "C" fn _emscripten_receive_on_main_thread_js(
    _: *mut Env,
    func: u32,
    _em_asm: u64,
    _caller: u64,
    _size: u32,
    _args: u64,
    _ctx: u64,
    _ctx_args: u64,
) -> f64 {
    // Only the JS glue proxies calls to the main thread; these imports don't.
    panic!("unexpected proxied call to the main thread (function {func})");
}

// ---- time ------------------------------------------------------------------------

pub unsafe extern "C" fn clock_time_get(env: *mut Env, clock: u32, _precision: u64, out: u64) -> u32 {
    let ms = match clock {
        0 => time::date_now(),
        1..=3 => rt().clock.now_ms(),
        _ => return errno::INVAL as u32,
    };
    mem(env).set_u64(out, (ms * 1e6).round() as u64);
    0
}

pub unsafe extern "C" fn _tzset_js(env: *mut Env, timezone: u64, daylight: u64, std_name: u64, dst_name: u64) {
    time::tzset(mem(env), timezone, daylight, std_name, dst_name)
}

pub unsafe extern "C" fn _localtime_js(env: *mut Env, t: u64, tm: u64) -> u32 {
    time::tm_from_time(mem(env), t as i64, tm, true)
}

pub unsafe extern "C" fn _gmtime_js(env: *mut Env, t: u64, tm: u64) -> u32 {
    time::tm_from_time(mem(env), t as i64, tm, false)
}

pub unsafe extern "C" fn _mktime_js(env: *mut Env, tm: u64) -> u64 {
    time::mktime(mem(env), tm) as u64
}

// ---- environment ----------------------------------------------------------------

fn env_strings() -> Vec<String> {
    vec![
        "USER=web_user".into(),
        "LOGNAME=web_user".into(),
        "PATH=/".into(),
        "PWD=/".into(),
        "HOME=/home/web_user".into(),
        format!("LANG={}.UTF-8", language().replace('-', "_")),
        "_=./this.program".into(),
    ]
}

pub unsafe extern "C" fn environ_sizes_get(env: *mut Env, count: u64, size: u64) -> u32 {
    let strings = env_strings();
    mem(env).set_u64(count, strings.len() as u64);
    mem(env).set_u64(size, strings.iter().map(|s| s.len() as u64 + 1).sum());
    0
}

pub unsafe extern "C" fn environ_get(env: *mut Env, environ: u64, buf: u64) -> u32 {
    let m = mem(env);
    let mut at = buf;
    for (i, s) in env_strings().iter().enumerate() {
        m.set_u64(environ + i as u64 * 8, at);
        m.write_str(at, s, s.len() + 1);
        at += s.len() as u64 + 1;
    }
    0
}

// ---- files: WASI -------------------------------------------------------------

/// iovec (wasm64): { u64 ptr, u64 len }.
fn iovecs(m: Mem, iov: u64, count: u64) -> impl Iterator<Item = (u64, usize)> {
    (0..count).map(move |i| (m.u64(iov + i * 16), m.u64(iov + i * 16 + 8) as usize))
}

fn readv(env: *mut Env, fd: u32, iov: u64, count: u64, mut offset: Option<u64>) -> Result<u64, Errno> {
    let m = mem(env);
    let mut total = 0;
    for (ptr, len) in iovecs(m, iov, count) {
        let n = rt().vfs.read(fd as i32, m.bytes_mut(ptr, len), offset)?;
        total += n as u64;
        if let Some(off) = offset.as_mut() {
            *off += n as u64;
        }
        if n < len {
            break;
        }
    }
    Ok(total)
}

fn writev(env: *mut Env, fd: u32, iov: u64, count: u64, mut offset: Option<u64>) -> Result<u64, Errno> {
    let m = mem(env);
    let mut total = 0;
    for (ptr, len) in iovecs(m, iov, count) {
        let n = rt().vfs.write(fd as i32, m.bytes(ptr, len), offset)?;
        total += n as u64;
        if let Some(off) = offset.as_mut() {
            *off += n as u64;
        }
        if n < len {
            break;
        }
    }
    Ok(total)
}

pub unsafe extern "C" fn fd_read(env: *mut Env, fd: u32, iov: u64, count: u64, nread: u64) -> u32 {
    wasi(readv(env, fd, iov, count, None).map(|n| mem(env).set_u64(nread, n)))
}

pub unsafe extern "C" fn fd_write(env: *mut Env, fd: u32, iov: u64, count: u64, nwritten: u64) -> u32 {
    wasi(writev(env, fd, iov, count, None).map(|n| mem(env).set_u64(nwritten, n)))
}

pub unsafe extern "C" fn fd_pread(env: *mut Env, fd: u32, iov: u64, count: u64, offset: u64, nread: u64) -> u32 {
    wasi(readv(env, fd, iov, count, Some(offset)).map(|n| mem(env).set_u64(nread, n)))
}

pub unsafe extern "C" fn fd_pwrite(env: *mut Env, fd: u32, iov: u64, count: u64, offset: u64, nwritten: u64) -> u32 {
    wasi(writev(env, fd, iov, count, Some(offset)).map(|n| mem(env).set_u64(nwritten, n)))
}

pub unsafe extern "C" fn fd_seek(env: *mut Env, fd: u32, offset: u64, whence: u32, new_offset: u64) -> u32 {
    wasi(rt().vfs.seek(fd as i32, offset as i64, whence).map(|pos| mem(env).set_u64(new_offset, pos)))
}

pub unsafe extern "C" fn fd_close(_: *mut Env, fd: u32) -> u32 {
    wasi(rt().vfs.close(fd as i32))
}

// ---- files: syscalls ------------------------------------------------------------

/// `struct stat` (wasm64).
fn write_stat(m: Mem, buf: u64, st: &vfs::Stat) {
    m.bytes_mut(buf, 104).fill(0);
    m.set_u32(buf, st.dev);
    m.set_u32(buf + 4, st.mode);
    m.set_u64(buf + 8, 1); // nlink
    m.set_u64(buf + 32, st.size);
    m.set_i32(buf + 40, 4096);
    m.set_i32(buf + 44, st.size.div_ceil(512) as i32);
    for (at, t) in [(48, st.atime), (64, st.mtime), (80, st.ctime)] {
        let (s, ns) = vfs::epoch(t);
        m.set_i64(buf + at, s);
        m.set_u64(buf + at + 8, ns);
    }
    m.set_u64(buf + 96, st.ino);
}

pub unsafe extern "C" fn __syscall_openat(env: *mut Env, dirfd: u32, path: u64, flags: u32, _varargs: u64) -> u32 {
    sys(path_at(env, dirfd, path).and_then(|p| rt().vfs.open(&p, flags)))
}

pub unsafe extern "C" fn __syscall_fstat64(env: *mut Env, fd: u32, buf: u64) -> u32 {
    sys(rt().vfs.fstat(fd as i32).map(|st| write_stat(mem(env), buf, &st)).map(|()| 0))
}

pub unsafe extern "C" fn __syscall_stat64(env: *mut Env, path: u64, buf: u64) -> u32 {
    sys(rt().vfs.stat(&mem(env).str(path), true).map(|st| write_stat(mem(env), buf, &st)).map(|()| 0))
}

pub unsafe extern "C" fn __syscall_lstat64(env: *mut Env, path: u64, buf: u64) -> u32 {
    sys(rt().vfs.stat(&mem(env).str(path), false).map(|st| write_stat(mem(env), buf, &st)).map(|()| 0))
}

pub unsafe extern "C" fn __syscall_newfstatat(env: *mut Env, dirfd: u32, path: u64, buf: u64, flags: u32) -> u32 {
    const AT_SYMLINK_NOFOLLOW: u32 = 256;
    const AT_EMPTY_PATH: u32 = 4096;
    let p = mem(env).str(path);
    let st = if p.is_empty() && flags & AT_EMPTY_PATH != 0 {
        rt().vfs.fstat(dirfd as i32)
    } else {
        rt().vfs.at(dirfd as i32, &p).and_then(|p| rt().vfs.stat(&p, flags & AT_SYMLINK_NOFOLLOW == 0))
    };
    sys(st.map(|st| write_stat(mem(env), buf, &st)).map(|()| 0))
}

pub unsafe extern "C" fn __syscall_ftruncate64(_: *mut Env, fd: u32, len: u64) -> u32 {
    sys(rt().vfs.truncate(fd as i32, len).map(|()| 0))
}

pub unsafe extern "C" fn __syscall_fcntl64(env: *mut Env, fd: u32, cmd: u32, varargs: u64) -> u32 {
    let flags = match rt().vfs.flags(fd as i32) {
        Ok(f) => f,
        Err(e) => return sys::<i32>(Err(e)),
    };
    match cmd {
        1 | 2 | 4 | 6 | 7 => 0, // F_GETFD F_SETFD F_SETFL F_SETLK F_SETLKW
        3 => flags,             // F_GETFL
        5 => {
            // F_GETLK: report F_UNLCK.
            let lock = mem(env).u64(varargs);
            mem(env).set_u16(lock, 2);
            0
        }
        _ => sys::<i32>(Err(errno::INVAL)),
    }
}

pub unsafe extern "C" fn __syscall_ioctl(env: *mut Env, fd: u32, op: u32, varargs: u64) -> u32 {
    const TCGETS: u32 = 21505;
    const TCSETS: u32 = 21506;
    const TIOCGWINSZ: u32 = 21523;
    match rt().vfs.is_tty(fd as i32) {
        Err(e) => sys::<i32>(Err(e)),
        Ok(false) => sys::<i32>(Err(errno::NOTTY)),
        Ok(true) => match op {
            TCGETS | TCSETS..=21508 => 0,
            TIOCGWINSZ => {
                let ws = mem(env).u64(varargs);
                mem(env).set_u16(ws, 24);
                mem(env).set_u16(ws + 2, 80);
                0
            }
            _ => sys::<i32>(Err(errno::INVAL)),
        },
    }
}

pub unsafe extern "C" fn __syscall_getcwd(env: *mut Env, buf: u64, size: u64) -> u32 {
    match size {
        0 => sys::<i32>(Err(errno::INVAL)),
        1 => sys::<i32>(Err(errno::RANGE)),
        _ => {
            mem(env).write_str(buf, "/", 2);
            2
        }
    }
}

pub unsafe extern "C" fn __syscall_mkdirat(env: *mut Env, dirfd: u32, path: u64, _mode: u32) -> u32 {
    sys(path_at(env, dirfd, path).and_then(|p| rt().vfs.mkdir(&p)).map(|()| 0))
}

pub unsafe extern "C" fn __syscall_unlinkat(env: *mut Env, dirfd: u32, path: u64, flags: u32) -> u32 {
    const AT_REMOVEDIR: u32 = 512;
    sys(path_at(env, dirfd, path)
        .and_then(|p| match flags {
            0 => rt().vfs.unlink(&p),
            AT_REMOVEDIR => rt().vfs.rmdir(&p),
            _ => Err(errno::INVAL),
        })
        .map(|()| 0))
}

pub unsafe extern "C" fn __syscall_rmdir(env: *mut Env, path: u64) -> u32 {
    sys(rt().vfs.rmdir(&mem(env).str(path)).map(|()| 0))
}

pub unsafe extern "C" fn __syscall_renameat(env: *mut Env, olddir: u32, old: u64, newdir: u32, new: u64) -> u32 {
    let paths = path_at(env, olddir, old).and_then(|o| Ok((o, path_at(env, newdir, new)?)));
    sys(paths.and_then(|(o, n)| rt().vfs.rename(&o, &n)).map(|()| 0))
}

pub unsafe extern "C" fn __syscall_getdents64(env: *mut Env, fd: u32, dirp: u64, count: u64) -> u32 {
    let buf = mem(env).bytes_mut(dirp, count as usize);
    sys(rt().vfs.getdents(fd as i32, buf).map(|n| n as i64))
}

pub unsafe extern "C" fn __syscall_poll(env: *mut Env, fds: u64, nfds: u32, _timeout: u32) -> u32 {
    // Files are always ready. struct pollfd { i32 fd; i16 events; i16 revents; }
    let m = mem(env);
    let mut ready = 0;
    for i in 0..nfds as u64 {
        let at = fds + i * 8;
        let events = m.u16(at + 4);
        let revents = events & (0x1 | 0x4); // POLLIN | POLLOUT
        m.set_u16(at + 6, revents);
        ready += (revents != 0) as u32;
    }
    ready
}

pub unsafe extern "C" fn __syscall_statfs64(env: *mut Env, _path: u64, _size: u64, buf: u64) -> u32 {
    let m = mem(env);
    m.bytes_mut(buf, 88).fill(0);
    m.set_u32(buf + 8, 4096);
    m.set_u32(buf + 72, 4096);
    for (at, v) in [(16, 1u64 << 24), (24, 1 << 23), (32, 1 << 23), (40, 1 << 20), (48, 1 << 19)] {
        m.set_u64(buf + at, v);
    }
    m.set_u32(buf + 56, 42);
    m.set_u32(buf + 64, 255);
    0
}

// ---- sockets: the network is off (-nonetwork) ------------------------------------

macro_rules! no_sockets {
    ($($name:ident($($arg:ident: $ty:ty),*);)*) => {$(
        pub unsafe extern "C" fn $name(_: *mut Env, $(_: $ty),*) -> u32 {
            sys::<i32>(Err(errno::NOSYS))
        }
    )*};
}

no_sockets! {
    __syscall_accept4(fd: u32, addr: u64, len: u64, flags: u32, d1: u32, d2: u32);
    __syscall_connect(fd: u32, addr: u64, len: u32, d1: u32, d2: u32, d3: u32);
    __syscall_getsockname(fd: u32, addr: u64, len: u64, d1: u32, d2: u32, d3: u32);
    __syscall_getsockopt(fd: u32, level: u32, name: u32, val: u64, len: u64, d1: u32);
    __syscall_recvfrom(fd: u32, buf: u64, len: u64, flags: u32, addr: u64, alen: u64);
    __syscall_sendto(fd: u32, buf: u64, len: u64, flags: u32, addr: u64, alen: u32);
    __syscall_setsockopt(fd: u32, level: u32, name: u32, val: u64, len: u32, d1: u32);
    __syscall_shutdown(fd: u32, how: u32, d1: u32, d2: u32, d3: u32, d4: u32);
    __syscall_socket(domain: u32, kind: u32, proto: u32, d1: u32, d2: u32, d3: u32);
}
