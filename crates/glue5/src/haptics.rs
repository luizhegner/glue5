//! Haptic feedback on iPhones and Android phones: a light tap where a thumb
//! presses a touch control, and the game's rumble. Other platforms do nothing.

/// Whether this device can give haptic feedback (and the setting is shown).
pub const SUPPORTED: bool = cfg!(any(target_os = "ios", target_os = "android"));

/// One light tap. Call it from the main thread (the event loop's).
pub fn tap() {
    imp::tap();
}

/// The game's rumble request (low and high frequency motors), called every
/// frame: a continuous vibration as strong as the stronger motor, sharper the
/// more the high-frequency one weighs. `(0, 0)` stops it.
pub fn rumble(motors: (u16, u16)) {
    imp::rumble(motors);
}

#[cfg(not(any(target_os = "ios", target_os = "android")))]
mod imp {
    pub fn tap() {}
    pub fn rumble(_: (u16, u16)) {}
}

#[cfg(target_os = "android")]
mod imp {
    use std::cell::Cell;
    use std::sync::Once;
    use std::time::{Duration, Instant};

    use jni::objects::JValue;
    use jni::JNIEnv;

    /// The rumble arrives once per rendered frame. The phone has one actuator,
    /// and a fresh vibration request per frame is both wasteful and clipped by
    /// Android; cap requests at 20 Hz and let each pulse bridge the interval.
    const RUMBLE_INTERVAL: Duration = Duration::from_millis(50);

    thread_local! {
        static LAST_RUMBLE: Cell<Option<Instant>> = const { Cell::new(None) };
        static RUMBLE_ACTIVE: Cell<bool> = const { Cell::new(false) };
    }

    static JNI_WARNING: Once = Once::new();

    unsafe extern "C" {
        fn SDL_GetAndroidJNIEnv() -> *mut jni::sys::JNIEnv;
    }

    pub fn tap() {
        with_env(|env| {
            let class = env.find_class("local/glue5/Glue5Activity")?;
            env.call_static_method(class, "hapticTap", "()V", &[])?;
            Ok(())
        });
    }

    pub fn rumble((low, high): (u16, u16)) {
        let active = low != 0 || high != 0;
        if !active {
            LAST_RUMBLE.with(|last| last.set(None));
            let was_active = RUMBLE_ACTIVE.with(|active| active.replace(false));
            if was_active {
                call_rumble(0, 0);
            }
            return;
        }

        let now = Instant::now();
        let due = LAST_RUMBLE.with(|last| {
            if last.get().is_some_and(|sent| now.duration_since(sent) < RUMBLE_INTERVAL) {
                false
            } else {
                last.set(Some(now));
                true
            }
        });
        if due {
            RUMBLE_ACTIVE.with(|active| active.set(true));
            call_rumble(i32::from(low), i32::from(high));
        }
    }

    fn call_rumble(low: i32, high: i32) {
        with_env(|env| {
            let class = env.find_class("local/glue5/Glue5Activity")?;
            env.call_static_method(
                class,
                "hapticRumble",
                "(II)V",
                &[JValue::Int(low), JValue::Int(high)],
            )?;
            Ok(())
        });
    }

    fn with_env(call: impl FnOnce(&mut JNIEnv<'_>) -> jni::errors::Result<()>) {
        let raw = unsafe { SDL_GetAndroidJNIEnv() };
        if raw.is_null() {
            warn_once("SDL did not provide an Android JNI environment");
            return;
        }
        let mut env = match unsafe { JNIEnv::from_raw(raw) } {
            Ok(env) => env,
            Err(error) => {
                warn_once(&format!("could not open Android's JNI environment: {error}"));
                return;
            }
        };
        if let Err(error) = call(&mut env) {
            let _ = env.exception_clear();
            warn_once(&format!("could not call Android haptics: {error}"));
        }
    }

    fn warn_once(message: &str) {
        JNI_WARNING.call_once(|| log::warn!("Android haptics unavailable: {message}"));
    }
}

#[cfg(target_os = "ios")]
mod imp {
    use std::cell::RefCell;
    use std::time::{Duration, Instant};

    use objc2::rc::Retained;
    use objc2::runtime::{AnyClass, AnyObject, ProtocolObject};
    use objc2::{AnyThread, class, msg_send};
    use objc2_core_haptics::{
        CHHapticDeviceCapability, CHHapticDynamicParameter, CHHapticDynamicParameterIDHapticIntensityControl,
        CHHapticDynamicParameterIDHapticSharpnessControl, CHHapticEngine, CHHapticEvent, CHHapticEventParameter,
        CHHapticEventParameterIDHapticIntensity, CHHapticEventParameterIDHapticSharpness,
        CHHapticEventTypeHapticContinuous, CHHapticPattern, CHHapticPatternPlayer,
    };
    use objc2_foundation::NSArray;

    /// A continuous event lasts at most 30 seconds: a new one before that.
    const EVENT_LENGTH: f64 = 30.0;
    const RENEW_AFTER: Duration = Duration::from_secs(25);
    /// Dynamic parameters change by less than this are not worth sending.
    const STEP: f32 = 0.02;

    type Player = Retained<ProtocolObject<dyn CHHapticPatternPlayer>>;

    /// The haptic engine, and the continuous vibration playing on it.
    struct Rumble {
        engine: Retained<CHHapticEngine>,
        playing: Option<Playing>,
    }

    struct Playing {
        player: Player,
        since: Instant,
        /// What was last sent: intensity, and sharpness around the event's.
        intensity: f32,
        sharpness: f32,
    }

    thread_local! {
        /// `None` until the first rumble, and after an error (it starts over).
        static RUMBLE: RefCell<Option<Rumble>> = const { RefCell::new(None) };
        /// No engine could be started (an iPad has none): when to try again.
        static RETRY: std::cell::Cell<Option<Instant>> = const { std::cell::Cell::new(None) };
        static TAP: RefCell<Option<Retained<AnyObject>>> = const { RefCell::new(None) };
    }

    pub fn tap() {
        TAP.with_borrow_mut(|generator| {
            let g = generator.get_or_insert_with(|| {
                let class: &AnyClass = class!(UIImpactFeedbackGenerator);
                // UIImpactFeedbackStyleMedium = 1.
                // SAFETY: UIKit, from the main thread's event loop.
                unsafe { msg_send![msg_send![class, alloc], initWithStyle: 1isize] }
            });
            // SAFETY: as above.
            unsafe {
                let _: () = msg_send![&**g, impactOccurredWithIntensity: 0.6f64];
                // Keeps the engine awake for the next tap (less latency).
                let _: () = msg_send![&**g, prepare];
            }
        });
    }

    pub fn rumble((low, high): (u16, u16)) {
        let (low, high) = (f32::from(low) / 65535.0, f32::from(high) / 65535.0);
        let intensity = low.max(high);
        // Sharpness is relative to the event's 0.5, in -0.5..=0.5.
        let sharpness = if intensity > 0.0 { high / (low + high) - 0.5 } else { 0.0 };
        RUMBLE.with_borrow_mut(|state| {
            if intensity == 0.0 {
                if let Some(r) = state.as_mut() {
                    if let Some(p) = r.playing.take() {
                        // SAFETY: CoreHaptics calls, from the main thread.
                        let _ = unsafe { p.player.stopAtTime_error(0.0) };
                    }
                }
                return;
            }
            let r = match state {
                Some(r) => r,
                None if RETRY.get().is_some_and(|t| Instant::now() < t) => return,
                None => match start() {
                    Some(r) => state.insert(r),
                    // Not at every frame of a rumble: once a second.
                    None => {
                        RETRY.set(Some(Instant::now() + Duration::from_secs(1)));
                        return;
                    }
                },
            };
            if update(r, intensity, sharpness).is_none() {
                // The engine was stopped (the app in the background, a call):
                // starts over with the next frame.
                *state = None;
            }
        });
    }

    fn start() -> Option<Rumble> {
        // SAFETY: CoreHaptics calls, from the main thread.
        unsafe {
            if !CHHapticEngine::capabilitiesForHardware().supportsHaptics() {
                return None;
            }
            let engine = CHHapticEngine::initAndReturnError(CHHapticEngine::alloc()).ok()?;
            engine.setPlaysHapticsOnly(true);
            engine.startAndReturnError().ok()?;
            Some(Rumble { engine, playing: None })
        }
    }

    /// Plays or renews the vibration and sets its strength and sharpness.
    fn update(r: &mut Rumble, intensity: f32, sharpness: f32) -> Option<()> {
        if r.playing.as_ref().is_some_and(|p| p.since.elapsed() > RENEW_AFTER) {
            let old = r.playing.take()?;
            // SAFETY: CoreHaptics calls, from the main thread.
            let _ = unsafe { old.player.stopAtTime_error(0.0) };
        }
        // SAFETY: CoreHaptics calls, from the main thread.
        unsafe {
            if r.playing.is_none() {
                let player = vibration(&r.engine)?;
                player.startAtTime_error(0.0).ok()?;
                // Starts at zero: the first update below sets its strength.
                r.playing = Some(Playing { player, since: Instant::now(), intensity: -1.0, sharpness: 0.0 });
            }
            let p = r.playing.as_mut()?;
            if (p.intensity - intensity).abs() >= STEP || (p.sharpness - sharpness).abs() >= STEP {
                let dynamic = |id, value| {
                    CHHapticDynamicParameter::initWithParameterID_value_relativeTime(
                        CHHapticDynamicParameter::alloc(),
                        id,
                        value,
                        0.0,
                    )
                };
                let parameters = NSArray::from_retained_slice(&[
                    dynamic(CHHapticDynamicParameterIDHapticIntensityControl, intensity),
                    dynamic(CHHapticDynamicParameterIDHapticSharpnessControl, sharpness),
                ]);
                p.player.sendParameters_atTime_error(&parameters, 0.0).ok()?;
                p.intensity = intensity;
                p.sharpness = sharpness;
            }
        }
        Some(())
    }

    /// A player for one long continuous event, at full intensity and middle
    /// sharpness: the dynamic parameters scale and shift them.
    unsafe fn vibration(engine: &CHHapticEngine) -> Option<Player> {
        unsafe {
            let event_parameter = |id, value| {
                CHHapticEventParameter::initWithParameterID_value(CHHapticEventParameter::alloc(), id, value)
            };
            let parameters = NSArray::from_retained_slice(&[
                event_parameter(CHHapticEventParameterIDHapticIntensity, 1.0),
                event_parameter(CHHapticEventParameterIDHapticSharpness, 0.5),
            ]);
            let event = CHHapticEvent::initWithEventType_parameters_relativeTime_duration(
                CHHapticEvent::alloc(),
                CHHapticEventTypeHapticContinuous,
                &parameters,
                0.0,
                EVENT_LENGTH,
            );
            let pattern = CHHapticPattern::initWithEvents_parameters_error(
                CHHapticPattern::alloc(),
                &NSArray::from_retained_slice(&[event]),
                &NSArray::new(),
            )
            .ok()?;
            engine.createPlayerWithPattern_error(&pattern).ok()
        }
    }
}
