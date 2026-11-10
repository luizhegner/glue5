//! Capture availability follows the Android owner, never a successful JNI send.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum State { Unavailable, Idle, Consent, Starting, Recording, Saving, Snapshot }

impl State {
    pub(super) fn from_android(value: i32) -> Self {
        match value {
            0 => Self::Idle, 1 => Self::Consent, 2 => Self::Starting,
            3 => Self::Recording, 4 => Self::Saving, 5 => Self::Snapshot,
            _ => Self::Unavailable,
        }
    }
    pub(super) fn can_start(self) -> bool { self == Self::Idle }
    pub(super) fn can_stop(self) -> bool { self == Self::Recording }
    pub(super) fn label(self) -> &'static str {
        match self {
            Self::Unavailable => "Unavailable", Self::Idle => "Not recording",
            Self::Consent => "Waiting for confirmation", Self::Starting => "Starting / awaiting Shizuku",
            Self::Recording => "Recording", Self::Saving => "Finishing and saving",
            Self::Snapshot => "Capturing previous session",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn only_idle_can_start_and_only_recording_can_stop() {
        for value in -1..=6 {
            let state = State::from_android(value);
            assert_eq!(state.can_start(), value == 0);
            assert_eq!(state.can_stop(), value == 3);
            assert!(!state.label().is_empty());
        }
    }
}
