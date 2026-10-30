//! Hold Android's Wi-Fi multicast lock only while GLUE 5 uses LAN discovery.

use std::sync::Once;

use jni::objects::JValue;

static WARNING: Once = Once::new();

/// Request or release the activity's multicast lock. False means the JNI call
/// failed or Android could not acquire the lock; callers may retry later.
pub fn hold(on: bool) -> bool {
    let result = crate::jni_util::with_activity(|env, class| {
        env.call_static_method(class, "lanDiscovery", "(Z)Z", &[JValue::Bool(u8::from(on))])?.z()
            .map_err(anyhow::Error::from)
    });
    match result {
        Ok(held) => held,
        Err(error) => {
            WARNING.call_once(|| log::warn!("Android LAN multicast lock unavailable: {error:#}"));
            false
        }
    }
}
