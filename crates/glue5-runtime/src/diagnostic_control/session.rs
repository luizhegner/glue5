//! Exclusive input ownership: explicit local consent, expiring lease, bounded sequences.
//! Pure policy, also tested by the lightweight CI harness without the game.
use std::time::{Duration, Instant};
#[path = "media.rs"]
pub mod media;
pub const LEASE: Duration = Duration::from_secs(5);
pub const CONSENT_WAIT: Duration = Duration::from_secs(30);
pub const MAX_ACTION_MS: u64 = 1000;
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Pad {
    pub buttons: u16,
    pub left: [f32; 2],
    pub right: [f32; 2],
    pub l2: f32,
    pub r2: f32,
}
impl Pad {
    pub fn valid(self) -> bool {
        self.left.into_iter().chain(self.right).all(|v| v.is_finite() && (-1.0..=1.0).contains(&v))
            && [self.l2, self.r2].into_iter().all(|v| v.is_finite() && (0.0..=1.0).contains(&v))
    }
}
#[derive(Clone, Debug)]
pub struct Request {
    pub id: u64,
    pub owner: String,
    pub until: Instant,
}
#[derive(Clone, Copy, Debug)]
pub struct Step {
    pub pad: Pad,
    pub ms: u64,
}
pub const MAX_STEPS: usize = 16;
pub const MAX_SEQUENCE_MS: u64 = 4000;
pub const MAX_HOLD_MS: u64 = 3000;
struct Hold {
    pad: Pad,
    start: Option<u32>,
    ms: u32,
    deadline: Instant,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Condition {
    TaxiNear,
    Passenger,
    Vehicle,
}
struct Reaction {
    condition: Condition,
    pad: Pad,
    ms: u64,
    until: Instant,
}
struct Active {
    request: Request,
    command: Pad,
    command_until: Instant,
    sequence: Vec<(Instant, Pad)>,
    prompt: Option<(u32, Instant)>,
    hold: Option<Hold>,
}
#[derive(Clone, Copy, Debug, Default)]
pub struct InputReceipt {
    pub revision: u64,
    pub applied_frames: u64,
    pub non_neutral_frames: u64,
    pub last_buttons: u16,
}
#[derive(Default)]
pub struct Control {
    pub enabled: bool,
    pub ready: bool,
    pub pending: Option<Request>,
    active: Option<Active>,
    pub generation: u64,
    pub pause_pending: bool,
    pub media: media::Media,
    clock: Option<(u32, Instant)>,
    pub receipt: InputReceipt,
    reaction: Option<Reaction>,
    pub reaction_state: &'static str,
}
impl Control {
    /// Local opt-in lasts only for this app run; it is not consent to control.
    pub fn enable(&mut self, enabled: bool) {
        self.enabled = enabled;
        if !enabled {
            self.stop();
        }
    }
    pub fn tick(&mut self, now: Instant) {
        self.media.expire(now);
        if self.pending.as_ref().is_some_and(|p| now >= p.until) {
            self.pending = None;
        }
        if self.active.as_ref().is_some_and(|a| now >= a.request.until) {
            self.stop();
        }
    }
    pub fn availability(&mut self, ready: bool, now: Instant) {
        self.tick(now);
        self.ready = ready;
        if !ready {
            self.stop();
        }
    }
    pub fn request(&mut self, id: u64, owner: &str, now: Instant) -> Result<(), String> {
        self.tick(now);
        if !self.enabled {
            return Err("enable AI diagnostics on the phone first".into());
        }
        if !self.ready {
            return Err("game must be running in the foreground without another menu".into());
        }
        if self.pending.is_some() || self.active.is_some() {
            return Err("another request or control session exists".into());
        }
        self.pending = Some(Request { id, owner: owner.to_owned(), until: now + CONSENT_WAIT });
        Ok(())
    }
    pub fn approve(&mut self, id: u64, now: Instant) -> bool {
        self.tick(now);
        if !self.enabled || !self.ready || self.pending.as_ref().is_none_or(|r| r.id != id) {
            return false;
        }
        let mut request = self.pending.take().unwrap();
        request.until = now + LEASE;
        self.active = Some(Active {
            request,
            command: Pad::default(),
            command_until: now,
            sequence: Vec::new(),
            prompt: None,
            hold: None,
        });
        self.clock = None;
        self.receipt = InputReceipt::default();
        self.generation = self.generation.wrapping_add(1);
        true
    }
    pub fn reject(&mut self, id: u64) {
        if self.pending.as_ref().is_some_and(|r| r.id == id) {
            self.pending = None;
        }
    }
    pub fn heartbeat(&mut self, id: u64, owner: &str, now: Instant) -> Result<(), String> {
        self.tick(now);
        let a = self
            .active
            .as_mut()
            .filter(|a| a.request.id == id && a.request.owner == owner)
            .ok_or("no approved session for this client")?;
        a.request.until = now + LEASE;
        Ok(())
    }
    pub fn command(&mut self, id: u64, owner: &str, pad: Pad, ms: u64, now: Instant) -> Result<(), String> {
        self.tick(now);
        if !pad.valid() || ms == 0 || ms > MAX_ACTION_MS {
            return Err("invalid pad state or duration (1..1000 ms)".into());
        }
        let a = self
            .active
            .as_mut()
            .filter(|a| a.request.id == id && a.request.owner == owner)
            .ok_or("no approved session for this client")?;
        a.hold = None;
        a.prompt = None;
        a.sequence.clear();
        a.command = pad;
        a.command_until = now + Duration::from_millis(ms);
        self.new_input();
        Ok(())
    }
    pub fn authorize(&mut self, id: u64, owner: &str, now: Instant) -> Result<(), String> {
        self.tick(now);
        if self.active.as_ref().is_some_and(|a| a.request.id == id && a.request.owner == owner) {
            Ok(())
        } else {
            Err("no approved session for this client".into())
        }
    }
    /// Replaces, never appends. No catch-up replay of steps missed during a stall.
    pub fn sequence(&mut self, id: u64, owner: &str, steps: &[Step], now: Instant) -> Result<(), String> {
        self.authorize(id, owner, now)?;
        if steps.is_empty()
            || steps.len() > MAX_STEPS
            || steps.iter().any(|s| !s.pad.valid() || s.ms == 0 || s.ms > MAX_ACTION_MS)
            || steps.iter().map(|s| s.ms).sum::<u64>() > MAX_SEQUENCE_MS
        {
            return Err("sequence requires 1..16 valid steps, each 1..1000ms, total <=4000ms".into());
        }
        let a = self.active.as_mut().unwrap();
        let mut until = now;
        a.sequence = steps
            .iter()
            .map(|s| {
                until += Duration::from_millis(s.ms);
                (until, s.pad)
            })
            .collect();
        a.hold = None;
        a.prompt = None;
        a.command = Pad::default();
        a.command_until = now;
        self.new_input();
        Ok(())
    }
    pub fn prompt(&mut self, id: u64, owner: &str, input: u32, ms: u64, now: Instant) -> Result<(), String> {
        self.authorize(id, owner, now)?;
        if input >= 363 || ms == 0 || ms > MAX_ACTION_MS {
            return Err("invalid visible input or duration".into());
        }
        let a = self.active.as_mut().unwrap();
        a.command = Pad::default();
        a.command_until = now;
        a.sequence.clear();
        a.hold = None;
        a.prompt = Some((input, now + Duration::from_millis(ms)));
        self.new_input();
        Ok(())
    }
    fn new_input(&mut self) {
        if self.reaction.take().is_some() {
            self.reaction_state = "cancelled_by_new_input";
        }
        self.receipt = InputReceipt { revision: self.receipt.revision.wrapping_add(1), ..Default::default() };
    }
    pub fn arm(
        &mut self,
        id: u64,
        owner: &str,
        condition: Condition,
        pad: Pad,
        ms: u64,
        timeout: u64,
        now: Instant,
    ) -> Result<(), String> {
        self.authorize(id, owner, now)?;
        if !pad.valid() || ms == 0 || ms > MAX_HOLD_MS || timeout == 0 || timeout > 15000 {
            return Err("reaction requires hold 1..3000ms, timeout 1..15000ms".into());
        }
        self.command(id, owner, Pad::default(), 1, now)?;
        self.reaction = Some(Reaction { condition, pad, ms, until: now + Duration::from_millis(timeout) });
        self.reaction_state = "armed";
        Ok(())
    }
    pub fn react(&mut self, condition: Condition, matched: bool, now: Instant) {
        self.tick(now);
        let Some(r) = self.reaction.as_ref() else {
            return;
        };
        if now >= r.until {
            self.reaction = None;
            self.reaction_state = "timed_out";
            return;
        }
        if !matched || r.condition != condition {
            return;
        }
        if self.clock.is_none_or(|(_, at)| now.saturating_duration_since(at) >= Duration::from_millis(500)) {
            return;
        }
        let r = self.reaction.take().unwrap();
        let Some(a) = self.active.as_mut() else {
            return;
        };
        a.hold = Some(Hold { pad: r.pad, start: None, ms: r.ms as u32, deadline: now + Duration::from_secs(4) });
        self.new_input();
        self.reaction_state = "fired_once";
    }
    pub fn game_clock(&mut self, ms: u32, now: Instant) {
        self.tick(now);
        if self.active.is_some() {
            self.clock = Some((ms, now));
        }
    }
    pub fn hold(&mut self, id: u64, owner: &str, pad: Pad, ms: u64, now: Instant) -> Result<(), String> {
        self.authorize(id, owner, now)?;
        if !pad.valid() || ms == 0 || ms > MAX_HOLD_MS {
            return Err("hold requires a valid pad and 1..3000 game ms".into());
        }
        self.clock
            .filter(|(_, at)| now.saturating_duration_since(*at) < Duration::from_millis(500))
            .ok_or("game clock unavailable or stale")?;
        let a = self.active.as_mut().unwrap();
        a.sequence.clear();
        a.prompt = None;
        a.command = Pad::default();
        a.command_until = now;
        a.hold = Some(Hold { pad, start: None, ms: ms as u32, deadline: now + Duration::from_secs(4) });
        self.new_input();
        Ok(())
    }
    /// Called only when pad 0 really reads the state, never by status/active polling.
    pub fn consume(&mut self, now: Instant) -> Option<Pad> {
        // Start game duration only when the game actually reads this hold.
        // HTTP/status/active probes must not spend a hold before the first read.
        if let (Some((ms, _)), Some(a)) = (self.clock, self.active.as_mut()) {
            if let Some(h) = a.hold.as_mut() {
                h.start.get_or_insert(ms);
            }
        }
        let p = self.frame(now)?;
        self.receipt.applied_frames = self.receipt.applied_frames.saturating_add(1);
        if p != Pad::default() {
            self.receipt.non_neutral_frames = self.receipt.non_neutral_frames.saturating_add(1);
        }
        self.receipt.last_buttons = p.buttons;
        Some(p)
    }
    pub fn prompt_input(&mut self, visible: &[u32], now: Instant) -> Option<u32> {
        self.tick(now);
        let a = self.active.as_mut()?;
        let (input, until) = a.prompt?;
        if now >= until || !visible.contains(&input) {
            a.prompt = None;
            return None;
        }
        Some(input)
    }
    pub fn frame(&mut self, now: Instant) -> Option<Pad> {
        self.tick(now);
        self.active.as_ref().map(|a| {
            if let Some(h) = &a.hold {
                return if now < h.deadline
                    && h.start.is_none_or(|start| self.clock.is_some_and(|(ms, _)| ms.wrapping_sub(start) < h.ms))
                {
                    h.pad
                } else {
                    Pad::default()
                };
            }
            a.sequence
                .iter()
                .find(|(until, _)| now < *until)
                .map(|(_, p)| *p)
                .unwrap_or_else(|| if now < a.command_until { a.command } else { Pad::default() })
        })
    }
    pub fn owner(&self) -> Option<&str> {
        self.active
            .as_ref()
            .map(|a| a.request.owner.as_str())
            .or_else(|| self.pending.as_ref().map(|p| p.owner.as_str()))
    }
    pub fn active_id(&self) -> Option<u64> {
        self.active.as_ref().map(|a| a.request.id)
    }
    pub fn stop_owned(&mut self, id: u64, owner: &str) -> Result<(), String> {
        if self.active.as_ref().is_some_and(|a| a.request.id == id && a.request.owner == owner)
            || self.pending.as_ref().is_some_and(|p| p.id == id && p.owner == owner)
        {
            self.stop();
            Ok(())
        } else {
            Err("session does not belong to this client".into())
        }
    }
    pub fn stop(&mut self) {
        self.pending = None;
        self.reaction = None;
        self.reaction_state = "stopped";
        self.clock = None;
        self.media.clear();
        if self.active.take().is_some() {
            self.generation = self.generation.wrapping_add(1);
            self.pause_pending = true;
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    fn ready(t: Instant) -> Control {
        let mut c = Control::default();
        c.enable(true);
        c.availability(true, t);
        c
    }
    fn active(t: Instant) -> Control {
        let mut c = ready(t);
        c.request(7, "agent", t).unwrap();
        assert!(c.approve(7, t));
        c
    }
    #[test]
    fn sequences_replace_cancel_and_do_not_replay_missed_steps() {
        let t = Instant::now();
        let mut c = active(t);
        let a = Pad { buttons: 64, ..Default::default() };
        let steps = [Step { pad: a, ms: 100 }, Step { pad: Pad::default(), ms: 100 }, Step { pad: a, ms: 100 }];
        c.sequence(7, "agent", &steps, t).unwrap();
        assert_eq!(c.frame(t), Some(a));
        assert_eq!(c.frame(t + Duration::from_millis(150)), Some(Pad::default()));
        assert_eq!(c.frame(t + Duration::from_millis(350)), Some(Pad::default()));
        c.sequence(7, "agent", &steps, t).unwrap();
        c.command(7, "agent", Pad::default(), 1, t).unwrap();
        assert_eq!(c.frame(t + Duration::from_millis(250)), Some(Pad::default()));
        assert!(c.sequence(7, "other", &steps, t).is_err());
        for bad in
            [vec![], vec![Step { pad: a, ms: 1000 }; 5], vec![Step { pad: a, ms: 1 }; 17], vec![Step { pad: a, ms: 0 }]]
        {
            assert!(c.sequence(7, "agent", &bad, t).is_err());
        }
        c.sequence(7, "agent", &steps, t).unwrap();
        c.stop();
        assert_eq!(c.frame(t), None);
    }
    #[test]
    fn stopping_or_losing_lease_discards_media_and_old_owner_cannot_read() {
        for expire in [false, true] {
            let t = Instant::now();
            let mut c = active(t);
            let ticket = c.media.request(false, t).unwrap();
            c.media.take_frame(t);
            c.media.request(true, t).unwrap();
            assert!(c.authorize(7, "other", t).is_err());
            if expire {
                c.tick(t + LEASE);
            } else {
                c.stop();
            }
            c.media.finish_frame(ticket, Ok(vec![1]), t + LEASE);
            assert!(c.media.frame.is_none());
            assert!(c.media.audio.is_none());
            assert!(c.authorize(7, "agent", t + LEASE).is_err());
        }
    }
    #[test]
    fn contextual_input_cancels_on_visibility_loss_and_neutral_pad() {
        let t = Instant::now();
        let mut c = active(t);
        c.prompt(7, "agent", 179, 100, t).unwrap();
        assert_eq!(c.prompt_input(&[179], t), Some(179));
        assert_eq!(c.prompt_input(&[], t), None);
        assert_eq!(c.prompt_input(&[179], t), None);
        c.prompt(7, "agent", 179, 100, t).unwrap();
        assert_eq!(c.prompt_input(&[179], t + Duration::from_millis(100)), None);
        c.prompt(7, "agent", 179, 100, t).unwrap();
        c.command(7, "agent", Pad::default(), 1, t).unwrap();
        assert_eq!(c.prompt_input(&[179], t), None);
        assert!(c.prompt(7, "other", 179, 1, t).is_err());
        assert!(c.prompt(7, "agent", 363, 1, t).is_err());
    }
    #[test]
    fn game_hold_uses_game_time_has_wall_deadline_and_real_read_receipts() {
        let t = Instant::now();
        let mut c = active(t);
        let p = Pad { buttons: 16, ..Default::default() };
        assert!(c.hold(7, "agent", p, 2000, t).is_err());
        c.game_clock(100, t);
        c.hold(7, "agent", p, 2000, t).unwrap();
        assert_eq!(c.frame(t + Duration::from_secs(1)), Some(p));
        assert_eq!(c.receipt.applied_frames, 0);
        assert_eq!(c.consume(t + Duration::from_secs(1)), Some(p));
        assert_eq!(c.receipt.applied_frames, 1);
        c.game_clock(2099, t + Duration::from_secs(2));
        assert_eq!(c.frame(t + Duration::from_secs(2)), Some(p));
        c.game_clock(2100, t + Duration::from_secs(2));
        assert_eq!(c.frame(t + Duration::from_secs(2)), Some(Pad::default()));
        c.hold(7, "agent", p, 3000, t + Duration::from_secs(2)).unwrap();
        c.heartbeat(7, "agent", t + Duration::from_secs(4)).unwrap();
        assert_eq!(c.frame(t + Duration::from_secs(6)), Some(Pad::default()));
        c.stop();
        assert_eq!(c.frame(t + Duration::from_secs(6)), None);
    }
    #[test]
    fn reactions_are_one_shot_cancelled_by_input_stop_and_timeout() {
        let t = Instant::now();
        let mut c = active(t);
        c.game_clock(10, t);
        let p = Pad { buttons: 16, ..Default::default() };
        c.arm(7, "agent", Condition::TaxiNear, p, 1000, 2000, t).unwrap();
        c.react(Condition::TaxiNear, false, t);
        assert_eq!(c.reaction_state, "armed");
        c.react(Condition::TaxiNear, true, t);
        assert_eq!(c.consume(t), Some(p));
        assert_eq!(c.reaction_state, "fired_once");
        c.game_clock(1010, t);
        c.react(Condition::TaxiNear, true, t);
        assert_eq!(c.frame(t), Some(Pad::default()));
        c.arm(7, "agent", Condition::TaxiNear, p, 1000, 1, t).unwrap();
        c.react(Condition::TaxiNear, true, t + Duration::from_millis(2));
        assert_eq!(c.reaction_state, "timed_out");
        c.arm(7, "agent", Condition::TaxiNear, p, 1000, 2000, t).unwrap();
        c.command(7, "agent", Pad::default(), 1, t).unwrap();
        assert_eq!(c.reaction_state, "cancelled_by_new_input");
        c.arm(7, "agent", Condition::TaxiNear, p, 1000, 2000, t).unwrap();
        c.stop();
        c.react(Condition::TaxiNear, true, t);
        assert_eq!(c.frame(t), None);
    }
    #[test]
    fn pairing_request_is_not_consent() {
        let t = Instant::now();
        let mut c = ready(t);
        c.request(7, "agent", t).unwrap();
        assert_eq!(c.frame(t), None);
        assert!(c.heartbeat(7, "agent", t).is_err());
        assert!(!c.approve(8, t));
        assert!(c.approve(7, t));
        assert_eq!(c.frame(t), Some(Pad::default()));
    }
    #[test]
    fn reject_and_expired_consent_never_acquire() {
        let t = Instant::now();
        let mut c = ready(t);
        c.request(7, "agent", t).unwrap();
        c.reject(7);
        assert!(!c.approve(7, t));
        c.request(8, "agent", t).unwrap();
        assert!(!c.approve(8, t + CONSENT_WAIT));
        assert_eq!(c.frame(t + CONSENT_WAIT), None);
    }
    #[test]
    fn action_expires_even_with_healthy_connection() {
        let t = Instant::now();
        let mut c = active(t);
        let p = Pad { r2: 1.0, left: [1.0, 0.0], ..Default::default() };
        c.command(7, "agent", p, 100, t).unwrap();
        assert_eq!(c.frame(t), Some(p));
        c.heartbeat(7, "agent", t + Duration::from_secs(1)).unwrap();
        assert_eq!(c.frame(t + Duration::from_secs(1)), Some(Pad::default()));
    }
    #[test]
    fn lost_lease_releases_and_requires_fresh_consent() {
        let t = Instant::now();
        let mut c = active(t);
        assert_eq!(c.frame(t + LEASE), None);
        assert!(c.pause_pending);
        assert!(c.heartbeat(7, "agent", t + LEASE).is_err());
        assert!(c.command(7, "agent", Pad::default(), 1, t + LEASE).is_err());
        assert!(!c.approve(7, t + LEASE));
    }
    #[test]
    fn stop_and_background_clear_actions_and_request_pause() {
        for background in [false, true] {
            let t = Instant::now();
            let mut c = active(t);
            c.command(7, "agent", Pad { buttons: 0xffff, ..Default::default() }, 1000, t).unwrap();
            if background {
                c.availability(false, t);
            } else {
                c.stop();
            }
            assert_eq!(c.frame(t), None);
            assert!(c.pause_pending);
            assert!(c.pending.is_none());
        }
    }
    #[test]
    fn foreign_owner_and_invalid_commands_cannot_take_over() {
        let t = Instant::now();
        let mut c = active(t);
        assert!(c.request(9, "other", t).is_err());
        assert!(c.command(7, "other", Pad::default(), 1, t).is_err());
        assert!(c.stop_owned(7, "other").is_err());
        assert!(c.heartbeat(8, "agent", t).is_err());
        for ms in [0, 1001] {
            assert!(c.command(7, "agent", Pad::default(), ms, t).is_err());
        }
        assert!(c.command(7, "agent", Pad { l2: f32::NAN, ..Default::default() }, 1, t).is_err());
    }

    #[test]
    fn losing_foreground_invalidates_pending_consent_too() {
        let t = Instant::now();
        let mut c = ready(t);
        c.request(7, "agent", t).unwrap();
        c.availability(false, t);
        c.availability(true, t);
        assert!(!c.approve(7, t));
        assert_eq!(c.frame(t), None);
        assert!(!c.pause_pending); // No ownership was granted.
    }

    #[test]
    fn previous_session_commands_cannot_affect_a_new_grant() {
        let t = Instant::now();
        let mut c = active(t);
        c.command(7, "agent", Pad { r2: 1.0, ..Default::default() }, 1000, t).unwrap();
        c.stop();
        c.request(8, "agent", t).unwrap();
        assert!(c.approve(8, t));
        assert!(c.command(7, "agent", Pad::default(), 1000, t).is_err());
        assert!(c.heartbeat(7, "agent", t).is_err());
        assert!(c.stop_owned(7, "agent").is_err());
        assert_eq!(c.active_id(), Some(8));
        assert_eq!(c.frame(t), Some(Pad::default()));
    }

    #[test]
    fn opt_in_defaults_off_and_disabling_cancels_ownership() {
        let t = Instant::now();
        let mut c = Control::default();
        c.availability(true, t);
        assert!(!c.enabled);
        assert!(c.request(7, "agent", t).is_err());
        c.enable(true);
        c.request(7, "agent", t).unwrap();
        assert_eq!(c.frame(t), None); // Enabling is never consent.
        assert!(c.approve(7, t));
        c.enable(false);
        assert_eq!(c.frame(t), None);
        assert!(c.pause_pending);
        c.enable(true);
        assert!(!c.approve(7, t));
        assert!(c.heartbeat(7, "agent", t).is_err());
    }
}
