//! The on-screen keyboard: how much of the window's bottom it covers, so
//! that the app's pages end above it rather than under it.
//!
//! SDL tells when the system keyboard shows and hides, not where it is; the
//! platforms tell that through their own notifications, which land here.

use std::sync::atomic::{AtomicU32, Ordering};

use sdl3::video::Window;

/// The keyboard's overlap of the window's bottom edge, in SDL's window
/// units (an `f32`'s bits); 0 without one, or with one that floats.
static OVERLAP: AtomicU32 = AtomicU32::new(0);

#[cfg_attr(not(any(target_os = "ios", target_os = "android")), allow(dead_code))]
fn set(units: f32) {
    OVERLAP.store(units.max(0.0).to_bits(), Ordering::Relaxed);
}

/// The on-screen keyboard's height over the window's bottom edge, in
/// points: 0 when there is none, when it floats (an iPad's undocked one),
/// and on platforms without one.
pub fn overlap(window: &Window) -> f32 {
    f32::from_bits(OVERLAP.load(Ordering::Relaxed)) * crate::ui::points_per_window_unit(window)
}

/// Starts following the keyboard (once, on the main thread, with the
/// window made).
pub fn init(window: &Window) {
    #[cfg(target_os = "ios")]
    ios::observe(window);
    let _ = window;
}

/// Who keeps the text being typed in sight: `ours`, the app's pages (laid
/// out above the keyboard), or the system, for the game's text box, which
/// the game draws where it does. Android 11 and later pan the window for
/// the latter only (Glue5Activity); elsewhere nothing changes (SDL shifts
/// its view on iOS where the text area needs it). Before the text input
/// starts.
pub fn typing(ours: bool) {
    // SDLActivity's COMMAND_USER: Glue5Activity's COMMAND_KEYBOARD_PANS.
    #[cfg(target_os = "android")]
    unsafe {
        sdl3::sys::system::SDL_SendAndroidMessage(0x8000, i32::from(!ours));
    }
    let _ = ours;
}

/// iOS: UIKit tells where the keyboard goes as it starts moving (shown,
/// hidden, resized, undocked), in the screen's coordinates; UIKit's points
/// are SDL's window units there.
#[cfg(target_os = "ios")]
mod ios {
    use std::ptr::NonNull;

    use block2::RcBlock;
    use objc2::rc::Retained;
    use objc2::runtime::AnyObject;
    use objc2::{class, msg_send};
    use objc2_foundation::{NSRect, NSString};
    use sdl3::video::Window;

    #[link(name = "UIKit", kind = "framework")]
    unsafe extern "C" {
        static UIKeyboardWillChangeFrameNotification: &'static NSString;
        static UIKeyboardWillHideNotification: &'static NSString;
        static UIKeyboardFrameEndUserInfoKey: &'static NSString;
    }

    pub(super) fn observe(window: &Window) {
        use sdl3::sys::properties::SDL_GetPointerProperty;
        use sdl3::sys::video::{SDL_GetWindowProperties, SDL_PROP_WINDOW_UIKIT_WINDOW_POINTER};
        let pointer = unsafe {
            SDL_GetPointerProperty(SDL_GetWindowProperties(window.raw()), SDL_PROP_WINDOW_UIKIT_WINDOW_POINTER, std::ptr::null_mut())
        };
        // SDL's UIWindow, kept by the block for as long as the app runs.
        let Some(ui_window) = (unsafe { Retained::retain(pointer.cast::<AnyObject>()) }) else {
            log::warn!("keyboard: no UIWindow");
            return;
        };
        let moved = RcBlock::new(move |notification: NonNull<AnyObject>| {
            super::set(overlap(&ui_window, unsafe { notification.as_ref() }).unwrap_or(0.0));
        });
        let hidden = RcBlock::new(|_: NonNull<AnyObject>| super::set(0.0));
        unsafe {
            let center: Retained<AnyObject> = msg_send![class!(NSNotificationCenter), defaultCenter];
            for (name, block) in [(UIKeyboardWillChangeFrameNotification, &moved), (UIKeyboardWillHideNotification, &hidden)] {
                // The centre keeps the observer (and the block) until the app ends.
                let _: Retained<AnyObject> = msg_send![
                    &center,
                    addObserverForName: name,
                    object: None::<&AnyObject>,
                    queue: None::<&AnyObject>,
                    usingBlock: &**block
                ];
            }
        }
    }

    /// The keyboard's end frame from a notification, as the height it covers
    /// over the window's bottom edge; 0 when it does not reach that edge
    /// (floating, undocked, split).
    fn overlap(ui_window: &AnyObject, notification: &AnyObject) -> Option<f32> {
        unsafe {
            let info: Option<Retained<AnyObject>> = msg_send![notification, userInfo];
            let value: Option<Retained<AnyObject>> = msg_send![&*info?, objectForKey: UIKeyboardFrameEndUserInfoKey];
            let frame: NSRect = msg_send![&*value?, CGRectValue];
            let screen: Option<Retained<AnyObject>> = msg_send![ui_window, screen];
            let space: Retained<AnyObject> = msg_send![&*screen?, coordinateSpace];
            let frame: NSRect = msg_send![ui_window, convertRect: frame, fromCoordinateSpace: &*space];
            let bounds: NSRect = msg_send![ui_window, bounds];
            let bottom = bounds.origin.y + bounds.size.height;
            let reaches = frame.size.height > 0.0 && frame.origin.y + frame.size.height >= bottom - 1.0;
            Some(if reaches { (bottom - frame.origin.y).clamp(0.0, bounds.size.height) as f32 } else { 0.0 })
        }
    }
}

/// Android 11 and later: SDL's surface (platform/android, Glue5Surface)
/// reports the keyboard's bottom inset as the system lays it out, in pixels,
/// SDL's window units there; Android 10 reports nothing (the window pans).
#[cfg(target_os = "android")]
#[unsafe(no_mangle)]
pub extern "system" fn Java_local_glue5_Glue5Surface_onKeyboardInset(
    _env: *mut std::ffi::c_void,
    _class: *mut std::ffi::c_void,
    bottom: i32,
) {
    set(bottom as f32);
}
