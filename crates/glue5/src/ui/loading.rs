//! Loading feedback shared by startup and resource preparation.
//! Delays are presentation-only: never invent progress or infer success/failure
//! from elapsed time. A quiet producer may still be doing useful work.
use std::time::{Duration, Instant};

const APPEAR_AFTER: Duration = Duration::from_millis(400);
const DETAILS_AFTER: Duration = Duration::from_secs(3);
const QUIET_AFTER: Duration = Duration::from_secs(15);

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) struct Presentation {
    pub visible: bool,
    pub details: bool,
    pub quiet: bool,
}

#[derive(Default)]
pub(super) struct Feedback {
    since: Option<Instant>,
    last_change: Option<Instant>,
    phase: String,
    progress: Option<f32>,
}

impl Feedback {
    pub fn reset(&mut self) {
        *self = Self::default();
    }

    /// None means indeterminate work. Do not treat it as 0 or 100%.
    pub fn observe(&mut self, active: bool, phase: &str, progress: Option<f32>, now: Instant) -> Presentation {
        if !active {
            self.reset();
            return Presentation::default();
        }
        let progress = progress.filter(|value| value.is_finite()).map(|value| value.clamp(0.0, 1.0));
        if self.since.is_none() {
            self.since = Some(now);
            self.last_change = Some(now);
            self.phase = phase.to_owned();
            self.progress = progress;
        } else {
            let changed = self.phase != phase || match (self.progress, progress) {
                (Some(old), Some(new)) => (old - new).abs() >= 0.005,
                (None, None) => false,
                _ => true,
            };
            if changed {
                self.phase = phase.to_owned();
                self.progress = progress;
                self.last_change = Some(now);
            }
        }
        let elapsed = now.saturating_duration_since(self.since.expect("active work has a start"));
        let silent = now.saturating_duration_since(self.last_change.expect("active work has a heartbeat"));
        Presentation {
            visible: elapsed >= APPEAR_AFTER,
            details: elapsed >= DETAILS_AFTER,
            quiet: elapsed >= QUIET_AFTER && silent >= QUIET_AFTER,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn suppresses_short_spinner_and_never_fakes_progress() {
        let now = Instant::now();
        let mut f = Feedback::default();
        assert!(!f.observe(true, "start", None, now).visible);
        assert!(!f.observe(true, "start", None, now + Duration::from_millis(399)).visible);
        assert!(f.observe(true, "start", None, now + Duration::from_millis(400)).visible);
        assert!(f.observe(true, "start", None, now + Duration::from_secs(4)).details);
        assert!(f.observe(true, "start", None, now + Duration::from_secs(16)).quiet);
        assert_eq!(f.progress, None);
        assert_eq!(f.observe(false, "", None, now + Duration::from_secs(17)), Presentation::default());
        assert!(!f.observe(true, "restart", None, now + Duration::from_secs(18)).visible);
    }

    #[test]
    fn real_stage_or_byte_progress_clears_quiet_indicator() {
        let now = Instant::now();
        let mut f = Feedback::default();
        f.observe(true, "download", Some(0.1), now);
        assert!(f.observe(true, "download", Some(0.1), now + Duration::from_secs(16)).quiet);
        assert!(!f.observe(true, "download", Some(0.35), now + Duration::from_secs(17)).quiet);
        assert!(!f.observe(true, "verify", None, now + Duration::from_secs(35)).quiet);
    }

    #[test]
    fn invalid_progress_is_indeterminate_not_percent() {
        let now = Instant::now();
        let mut f = Feedback::default();
        f.observe(true, "start", Some(f32::NAN), now);
        assert_eq!(f.progress, None);
        f.observe(true, "start", Some(1.4), now + Duration::from_secs(1));
        assert_eq!(f.progress, Some(1.0));
    }
}
