//! A GPU driver the user added, on Android with an Adreno GPU: Mesa's
//! Turnip or another build of Qualcomm's, as emulators take them (an
//! Adreno Tools package: its `meta.json` and libraries). libadrenotools
//! opens a copy of the system's Vulkan loader of the app's own, in a linker
//! namespace where the loader's driver is the package's library in place
//! of the phone's; wgpu's Vulkan instance is made on that loader.

use std::ffi::{CStr, CString, c_char, c_int, c_void};
use std::path::Path;
use std::sync::{Mutex, PoisonError};

use anyhow::{Context, Result, bail};

/// `ADRENOTOOLS_DRIVER_CUSTOM` (adrenotools/priv.h).
const DRIVER_CUSTOM: c_int = 1;

type OpenLibvulkan = unsafe extern "C" fn(
    dlopen_mode: c_int,
    feature_flags: c_int,
    tmp_lib_dir: *const c_char,
    hook_lib_dir: *const c_char,
    custom_driver_dir: *const c_char,
    custom_driver_name: *const c_char,
    file_redirect_dir: *const c_char,
    user_mapping_handle: *mut *mut c_void,
) -> *mut c_void;

/// The driver package in use (its folder's name), once [`instance`] made
/// an instance on it: the pipeline cache is kept per driver. The system's
/// instance ([`crate::instance`]) clears it.
static IN_USE: Mutex<Option<String>> = Mutex::new(None);

pub(crate) fn in_use() -> std::sync::MutexGuard<'static, Option<String>> {
    IN_USE.lock().unwrap_or_else(PoisonError::into_inner)
}

/// A Vulkan instance on the driver `library` (its file name) of the package
/// in `dir`, which must be in the app's internal storage (the linker loads
/// nothing from shared storage). The loader and the driver stay loaded
/// for the rest of the process.
pub fn instance(dir: &Path, library: &str) -> Result<wgpu::Instance> {
    // Opened only now: linkernsbypass changes the linker's pages as it loads.
    let tools = unsafe { libc::dlopen(c"libadrenotools.so".as_ptr(), libc::RTLD_NOW | libc::RTLD_LOCAL) };
    if tools.is_null() {
        bail!("libadrenotools: {}", dl_error());
    }
    let open = unsafe { libc::dlsym(tools, c"adrenotools_open_libvulkan".as_ptr()) };
    if open.is_null() {
        bail!("libadrenotools: {}", dl_error());
    }
    // Its hooks are next to it: in the APK (`base.apk!/lib/arm64-v8a`) or
    // in the app's library folder.
    let mut info: libc::Dl_info = unsafe { std::mem::zeroed() };
    if unsafe { libc::dladdr(open, &mut info) } == 0 || info.dli_fname.is_null() {
        bail!("libadrenotools: where it was loaded from is unknown");
    }
    let path = unsafe { CStr::from_ptr(info.dli_fname) }.to_bytes();
    let hooks = CString::new(&path[..path.iter().rposition(|&b| b == b'/').unwrap_or(0)])?;
    // A folder ending in '/' (the library's path is the two joined).
    let mut folder = dir.as_os_str().as_encoded_bytes().to_vec();
    folder.push(b'/');
    let (folder, name) = (CString::new(folder)?, CString::new(library)?);
    let open: OpenLibvulkan = unsafe { std::mem::transmute(open) };
    let loader = unsafe {
        open(
            libc::RTLD_NOW,
            DRIVER_CUSTOM,
            std::ptr::null(),
            hooks.as_ptr(),
            folder.as_ptr(),
            name.as_ptr(),
            std::ptr::null(),
            std::ptr::null_mut(),
        )
    };
    if loader.is_null() {
        bail!("libadrenotools could not open the Vulkan loader for {library}");
    }
    let gipa = unsafe { libc::dlsym(loader, c"vkGetInstanceProcAddr".as_ptr()) };
    if gipa.is_null() {
        bail!("the Vulkan loader has no vkGetInstanceProcAddr");
    }
    let gipa = unsafe { std::mem::transmute::<*mut c_void, ash::vk::PFN_vkGetInstanceProcAddr>(gipa) };
    let entry = unsafe { ash::Entry::from_static_fn(ash::StaticFn { get_instance_proc_addr: gipa }) };
    // Drivers made for emulators, some not conformant yet (Mesa's PanVK on
    // Mali's kernel driver): the user chose it, wgpu would hide it.
    let flags = wgpu::InstanceFlags::from_build_config().with_env()
        | wgpu::InstanceFlags::ALLOW_UNDERLYING_NONCOMPLIANT_ADAPTER;
    let desc = wgpu::hal::InstanceDescriptor {
        name: "wgpu",
        flags,
        memory_budget_thresholds: Default::default(),
        backend_options: wgpu::BackendOptions::from_env_or_default(),
        telemetry: None,
        display: None,
    };
    // SAFETY: the loader is never unloaded.
    let hal = unsafe { wgpu::hal::vulkan::Instance::init_with_entry(entry, &desc, None) }
        .context("a Vulkan instance on the driver")?;
    let instance = unsafe { wgpu::Instance::from_hal::<wgpu::hal::api::Vulkan>(hal) };
    // Where its hook fails, libadrenotools loads the phone's driver after
    // all, or finds no GPU. So does the linker when the phone's driver is
    // loaded already under the package's library name (Android's own UI
    // renders with Skia on Vulkan).
    let adapters = pollster::block_on(instance.enumerate_adapters(wgpu::Backends::VULKAN));
    let Some(adapter) = adapters.first() else { bail!("the driver found no GPU") };
    if !loaded_from(dir) {
        bail!("the phone's own was loaded in its place, its library having the same name ({library})");
    }
    let info = adapter.get_info();
    log::info!("gpu: driver {library} from {}: {} ({}, {})", dir.display(), info.name, info.driver, info.driver_info);
    *in_use() = Some(dir.file_name().unwrap_or_default().to_string_lossy().into_owned());
    Ok(instance)
}

/// Whether a library from `dir` is loaded.
fn loaded_from(dir: &Path) -> bool {
    // As given and as the linker has it (/data/user/0 is /data/data).
    let dirs: Vec<Vec<u8>> = [Some(dir.to_path_buf()), std::fs::canonicalize(dir).ok()]
        .into_iter()
        .flatten()
        .map(|d| {
            let mut d = d.into_os_string().into_encoded_bytes();
            d.push(b'/');
            d
        })
        .collect();
    unsafe extern "C" fn each(info: *mut libc::dl_phdr_info, _: usize, dirs: *mut c_void) -> c_int {
        let (name, dirs) = unsafe { ((*info).dlpi_name, &*(dirs as *const Vec<Vec<u8>>)) };
        if name.is_null() {
            return 0;
        }
        let name = unsafe { CStr::from_ptr(name) }.to_bytes();
        dirs.iter().any(|d| name.starts_with(d)).into()
    }
    unsafe { libc::dl_iterate_phdr(Some(each), (&raw const dirs).cast_mut().cast()) != 0 }
}

fn dl_error() -> String {
    let e = unsafe { libc::dlerror() };
    if e.is_null() { "unknown error".into() } else { unsafe { CStr::from_ptr(e) }.to_string_lossy().into_owned() }
}
