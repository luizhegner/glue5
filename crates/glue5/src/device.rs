//! The device's name as its owner set it (the computer's or phone's name in
//! the system's settings), for what shows this device to others: the
//! Studio's title bar and its control page. `GLUE5_DEVICE_NAME` names it
//! instead; Android's activity sets it from the system's setting
//! (`platform/android`).

/// Whether this Android device is a Galaxy S21 Plus model (SM-G996 family).
/// Five Star touch controls are intentionally offered only on this handset.
pub fn is_galaxy_s21_plus() -> bool {
    #[cfg(target_os = "android")]
    {
        return is_galaxy_s21_plus_model(&std::env::var("GLUE5_DEVICE_MODEL").unwrap_or_default());
    }
    #[cfg(not(target_os = "android"))]
    false
}

#[cfg(any(target_os = "android", test))]
fn is_galaxy_s21_plus_model(model: &str) -> bool {
    let model = model.trim().to_ascii_uppercase();
    model.starts_with("SM-G996") || model.contains("GALAXY S21+") || model == "GALAXY S21 PLUS"
}

#[cfg(test)]
mod tests {
    use super::is_galaxy_s21_plus_model;

    #[test]
    fn five_star_support_is_limited_to_the_galaxy_s21_plus_model_family() {
        for model in ["SM-G996B", "SM-G996U1", " sm-g996b/ds "] {
            assert!(is_galaxy_s21_plus_model(model), "{model}");
        }
        for model in ["SM-G991B", "SM-G998B", "SM-G990B", "Pixel 8"] {
            assert!(!is_galaxy_s21_plus_model(model), "{model}");
        }
    }
}

/// The device's name, or what it is ("Mac", "iPhone") when it has none.
pub fn name() -> String {
    std::env::var("GLUE5_DEVICE_NAME")
        .ok()
        .or_else(own_name)
        .map(|n| n.trim().to_string())
        .filter(|n| !n.is_empty())
        .unwrap_or_else(|| kind().to_string())
}

/// What the device is.
fn kind() -> &'static str {
    match std::env::consts::OS {
        "macos" => "Mac",
        "ios" => "iPhone or iPad",
        "tvos" => "Apple TV",
        "android" => "Android",
        "windows" => "Windows PC",
        _ => "Linux PC",
    }
}

/// The Mac's name (System Settings › General › Sharing).
#[cfg(target_os = "macos")]
fn own_name() -> Option<String> {
    use std::ffi::{CStr, c_char, c_void};
    #[link(name = "SystemConfiguration", kind = "framework")]
    unsafe extern "C" {
        fn SCDynamicStoreCopyComputerName(store: *const c_void, encoding: *mut u32) -> *const c_void;
    }
    #[link(name = "CoreFoundation", kind = "framework")]
    unsafe extern "C" {
        fn CFStringGetCString(string: *const c_void, buffer: *mut c_char, size: isize, encoding: u32) -> u8;
        fn CFRelease(object: *const c_void);
    }
    const UTF8: u32 = 0x0800_0100;
    // SAFETY: a CFString the call owns, read into a buffer of its size and
    // released once.
    unsafe {
        let name = SCDynamicStoreCopyComputerName(std::ptr::null(), std::ptr::null_mut());
        if name.is_null() {
            return None;
        }
        let mut buffer = [0 as c_char; 256];
        let read = CFStringGetCString(name, buffer.as_mut_ptr(), buffer.len() as isize, UTF8) != 0;
        CFRelease(name);
        read.then(|| CStr::from_ptr(buffer.as_ptr()).to_string_lossy().into_owned())
    }
}

/// UIKit's name of the device: the one in Settings › General › About with
/// Apple's user-assigned-device-name entitlement, else "iPhone" or "iPad".
/// From the main thread, as UIKit wants.
#[cfg(target_os = "ios")]
fn own_name() -> Option<String> {
    use objc2::rc::Retained;
    use objc2::runtime::AnyObject;
    use objc2::{class, msg_send};
    use objc2_foundation::NSString;
    // SAFETY: UIDevice's class methods, on the main thread.
    unsafe {
        let device: Retained<AnyObject> = msg_send![class!(UIDevice), currentDevice];
        let name: Option<Retained<NSString>> = msg_send![&*device, name];
        name.map(|n| n.to_string())
    }
}

/// The computer's name as Settings › System › About shows it (its DNS host
/// name: `COMPUTERNAME` is the upper-case NetBIOS one).
#[cfg(windows)]
fn own_name() -> Option<String> {
    use windows_sys::Win32::System::SystemInformation::{ComputerNamePhysicalDnsHostname, GetComputerNameExW};
    let mut buffer = [0u16; 256];
    let mut len = buffer.len() as u32;
    // SAFETY: a buffer of `len` characters, which the call shortens to what
    // it wrote.
    let read = unsafe { GetComputerNameExW(ComputerNamePhysicalDnsHostname, buffer.as_mut_ptr(), &mut len) } != 0;
    read.then(|| String::from_utf16_lossy(&buffer[..len as usize]))
}

/// The pretty host name (`hostnamectl`), else the host name.
#[cfg(target_os = "linux")]
fn own_name() -> Option<String> {
    let pretty = std::fs::read_to_string("/etc/machine-info").ok().and_then(|info| {
        info.lines()
            .find_map(|l| l.strip_prefix("PRETTY_HOSTNAME="))
            .map(|v| v.trim_matches(|c| c == '"' || c == '\'').to_string())
    });
    pretty.filter(|p| !p.is_empty()).or_else(|| std::fs::read_to_string("/proc/sys/kernel/hostname").ok())
}

/// Android's comes as `GLUE5_DEVICE_NAME`; tvOS has none to read here.
#[cfg(not(any(target_os = "macos", target_os = "ios", windows, target_os = "linux")))]
fn own_name() -> Option<String> {
    None
}
