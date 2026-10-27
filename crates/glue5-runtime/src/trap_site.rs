//! Where a trap occurred: the native call stack, logged from the trap itself
//! (patches/wabt/0005), before execution unwinds out of the module. Where no
//! debugger can attach (a release app on a phone) it still names the engine
//! function: `llvm-symbolizer --obj=<unstripped library> <offset>` turns
//! each `library+offset` into a `w2c_game_...` frame of the generated C;
//! an `unreachable` also gives its line there.

static LAST_TRAP: std::sync::OnceLock<String> = std::sync::OnceLock::new();

/// The first WebAssembly trap, including its generated-C line and
/// native frames, retained so the Android support report survives logcat
/// being unavailable to the app.
pub(crate) fn last_trap() -> Option<&'static str> {
    LAST_TRAP.get().map(String::as_str)
}

/// The trap hook: logs and retains the frames as `library+offset`, innermost
/// first, and for `unreachable` the line in the generated C file of the
/// function the second frame names (`target/glue5-tools/gen/<hash>/game_N.c`).
pub(crate) unsafe extern "C" fn record(code: u32, line: u32) {
    let frames = frames();
    let detail = format!(
        "wasm trap {code} (generated C line {line}), native frames: {}",
        frames.join(" ")
    );
    let _ = LAST_TRAP.set(detail.clone());
    log::error!("{detail}");
}

#[cfg(unix)]
fn frames() -> Vec<String> {
    use std::ffi::{CStr, c_int, c_void};

    const MAX: usize = 64;
    unsafe extern "C" {
        fn _Unwind_Backtrace(trace: unsafe extern "C" fn(*mut c_void, *mut c_void) -> c_int, arg: *mut c_void) -> c_int;
        fn _Unwind_GetIP(context: *mut c_void) -> usize;
    }
    unsafe extern "C" fn each(context: *mut c_void, arg: *mut c_void) -> c_int {
        // SAFETY: `arg` is the Vec below, for the duration of the walk.
        let ips = unsafe { &mut *(arg as *mut Vec<usize>) };
        ips.push(unsafe { _Unwind_GetIP(context) });
        // _URC_END_OF_STACK stops the walk, _URC_NO_REASON goes on.
        if ips.len() >= MAX { 5 } else { 0 }
    }
    let mut ips: Vec<usize> = Vec::with_capacity(MAX);
    unsafe { _Unwind_Backtrace(each, &mut ips as *mut Vec<usize> as *mut c_void) };
    ips.iter()
        .map(|&ip| {
            let mut info: libc::Dl_info = unsafe { std::mem::zeroed() };
            if unsafe { libc::dladdr(ip as *const c_void, &mut info) } == 0 || info.dli_fname.is_null() {
                return format!("{ip:#x}");
            }
            let path = unsafe { CStr::from_ptr(info.dli_fname) }.to_string_lossy();
            let name = path.rsplit('/').next().unwrap_or(&path).to_owned();
            format!("{name}+{:#x}", ip - info.dli_fbase as usize)
        })
        .collect()
}

#[cfg(windows)]
fn frames() -> Vec<String> {
    use std::ffi::c_void;

    use windows_sys::Win32::Foundation::HMODULE;
    use windows_sys::Win32::System::Diagnostics::Debug::RtlCaptureStackBackTrace;
    use windows_sys::Win32::System::LibraryLoader::{
        GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS, GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT, GetModuleFileNameW,
        GetModuleHandleExW,
    };

    const MAX: usize = 64;
    let mut ips = [std::ptr::null_mut::<c_void>(); MAX];
    let n = unsafe { RtlCaptureStackBackTrace(0, MAX as u32, ips.as_mut_ptr(), std::ptr::null_mut()) } as usize;
    ips[..n]
        .iter()
        .map(|&ip| {
            let mut module: HMODULE = std::ptr::null_mut();
            let flags = GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS | GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT;
            if unsafe { GetModuleHandleExW(flags, ip as *const u16, &mut module) } == 0 {
                return format!("{:#x}", ip as usize);
            }
            let mut path = [0u16; 260];
            let len = unsafe { GetModuleFileNameW(module, path.as_mut_ptr(), path.len() as u32) } as usize;
            let path = String::from_utf16_lossy(&path[..len]);
            let name = path.rsplit(['\\', '/']).next().unwrap_or(&path).to_owned();
            format!("{name}+{:#x}", ip as usize - module as usize)
        })
        .collect()
}

#[cfg(not(any(unix, windows)))]
fn frames() -> Vec<String> {
    vec!["(not recorded on this platform)".into()]
}
