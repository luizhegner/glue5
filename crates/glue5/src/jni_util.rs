//! Android JNI helpers. Local references created for an SDL native call must
//! not accumulate across frames; each invocation therefore uses a local frame.

use std::sync::OnceLock;

use jni::objects::{GlobalRef, JClass};
use jni::{JNIEnv, errors::Result as JniResult};

static ACTIVITY: OnceLock<GlobalRef> = OnceLock::new();

/// Invoke a static method on Glue5Activity with a cached global class reference
/// and a bounded local-reference frame. Values returned from `call` must not
/// contain local JNI references, because the frame is popped before returning.
pub fn with_activity<R>(
    call: impl FnOnce(&mut JNIEnv<'_>, JClass<'_>) -> anyhow::Result<R>,
) -> anyhow::Result<R> {
    unsafe extern "C" {
        fn SDL_GetAndroidJNIEnv() -> *mut jni::sys::JNIEnv;
    }

    let raw = unsafe { SDL_GetAndroidJNIEnv() };
    if raw.is_null() {
        anyhow::bail!("Android did not provide a JNI environment");
    }
    let mut env = unsafe { JNIEnv::from_raw(raw) }.map_err(anyhow::Error::from)?;
    let result = env.with_local_frame(16, |env| {
        let global = activity_class(env)?;
        // A GlobalRef is not a local ref. Make a local reference inside this
        // frame before wrapping it as a JClass.
        let local = env.new_local_ref(global.as_obj())?;
        call(env, JClass::from(local))
    });
    if result.is_err() {
        let _ = env.exception_clear();
    }
    result
}

/// The single lookup for the activity class used by all Android JNI callers.
fn activity_class(env: &mut JNIEnv<'_>) -> JniResult<&'static GlobalRef> {
    if let Some(class) = ACTIVITY.get() {
        return Ok(class);
    }
    let local = env.find_class("local/glue5/Glue5Activity")?;
    let global = env.new_global_ref(&local)?;
    env.delete_local_ref(local)?;
    Ok(ACTIVITY.get_or_init(|| global))
}
