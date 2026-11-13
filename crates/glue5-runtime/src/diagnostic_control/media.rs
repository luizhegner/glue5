//! Bounded, volatile capture slots. No OS capture, filesystem or background recorder.
use std::time::{Duration, Instant};
pub const TTL: Duration = Duration::from_secs(3);
pub const MAX_PNG: usize = 1024 * 1024;
pub const AUDIO_SAMPLES: usize = 16_000;
pub enum Frame {
    Waiting,
    Encoding,
    Ready(Vec<u8>),
    Failed(String),
}
pub struct Capture<T> {
    pub ticket: u64,
    pub until: Instant,
    pub value: T,
}
#[derive(Default)]
pub struct Media {
    pub frame: Option<Capture<Frame>>,
    pub audio: Option<Capture<Vec<i16>>>,
    serial: u64,
    last_frame: Option<Instant>,
    last_audio: Option<Instant>,
    audio_phase: usize,
    audio_sum: f32,
    watch_until: Option<Instant>,
}
impl Media {
    pub fn clear(&mut self) {
        self.frame = None;
        self.audio = None;
        self.watch_until = None;
    }
    pub fn expire(&mut self, now: Instant) {
        if self.watch_until.is_some_and(|t| now >= t) {
            self.watch_until = None;
        }
        if self.frame.as_ref().is_some_and(|s| now >= s.until) {
            self.frame = None;
        }
        if self.audio.as_ref().is_some_and(|s| now >= s.until) {
            self.audio = None;
        }
    }
    pub fn request(&mut self, audio: bool, now: Instant) -> Result<u64, String> {
        self.expire(now);
        if !audio && self.watch_until.is_some() {
            return Err("watch already supplies latest frames; use observe".into());
        }
        let (busy, last) =
            if audio { (self.audio.is_some(), self.last_audio) } else { (self.frame.is_some(), self.last_frame) };
        if busy || last.is_some_and(|t| now.saturating_duration_since(t) < Duration::from_secs(1)) {
            return Err("capture pending or rate limited (one per second)".into());
        }
        self.serial = self.serial.wrapping_add(1);
        let ticket = self.serial;
        if audio {
            self.audio = Some(Capture { ticket, until: now + TTL, value: Vec::with_capacity(AUDIO_SAMPLES) });
            self.audio_phase = 0;
            self.audio_sum = 0.0;
            self.last_audio = Some(now);
        } else {
            self.frame = Some(Capture { ticket, until: now + TTL, value: Frame::Waiting });
            self.last_frame = Some(now);
        }
        Ok(ticket)
    }
    pub fn take_frame(&mut self, now: Instant) -> Option<u64> {
        self.expire(now);
        if self.watch_until.is_some()
            && self.last_frame.is_none_or(|t| now.saturating_duration_since(t) >= Duration::from_secs(1))
            && self.frame.as_ref().is_none_or(|s| !matches!(s.value, Frame::Encoding))
        {
            self.serial = self.serial.wrapping_add(1);
            self.last_frame = Some(now);
            self.frame = Some(Capture { ticket: self.serial, until: now + TTL, value: Frame::Waiting });
        }
        let s = self.frame.as_mut()?;
        if !matches!(s.value, Frame::Waiting) {
            return None;
        }
        s.value = Frame::Encoding;
        Some(s.ticket)
    }
    pub fn watch(&mut self, ms: u64, now: Instant) -> Result<(), String> {
        if ms == 0 || ms > 15000 {
            return Err("watch duration must be 1..15000ms".into());
        }
        self.watch_until = Some(now + Duration::from_millis(ms));
        Ok(())
    }
    pub fn wanted(&self) -> bool {
        self.watch_until.is_some() || self.frame.is_some() || self.audio.is_some()
    }
    pub fn finish_frame(&mut self, ticket: u64, result: Result<Vec<u8>, String>, now: Instant) {
        self.expire(now);
        if let Some(s) = self.frame.as_mut().filter(|s| s.ticket == ticket && matches!(s.value, Frame::Encoding)) {
            s.value = match result {
                Ok(v) if v.len() <= MAX_PNG => Frame::Ready(v),
                Ok(_) => Frame::Failed("encoded frame too large".into()),
                Err(e) => Frame::Failed(e),
            };
        }
    }
    /// SDL's game output only, 48kHz stereo -> 16kHz mono PCM. No allocations.
    pub fn push_audio(&mut self, samples: &[f32], now: Instant) {
        self.expire(now);
        let Some(s) = self.audio.as_mut() else {
            return;
        };
        for pair in samples.chunks_exact(2) {
            if s.value.len() == AUDIO_SAMPLES {
                break;
            }
            let x = (pair[0] + pair[1]) * 0.5;
            self.audio_sum += if x.is_finite() { x.clamp(-1.0, 1.0) } else { 0.0 };
            self.audio_phase += 1;
            if self.audio_phase == 3 {
                s.value.push((self.audio_sum / 3.0 * 32767.0) as i16);
                self.audio_sum = 0.0;
                self.audio_phase = 0;
            }
        }
    }
}
pub fn wav(samples: &[i16]) -> Vec<u8> {
    let len = (samples.len() * 2) as u32;
    let mut out = Vec::with_capacity(len as usize + 44);
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(36 + len).to_le_bytes());
    out.extend_from_slice(b"WAVEfmt ");
    out.extend_from_slice(&16u32.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&16000u32.to_le_bytes());
    out.extend_from_slice(&32000u32.to_le_bytes());
    out.extend_from_slice(&2u16.to_le_bytes());
    out.extend_from_slice(&16u16.to_le_bytes());
    out.extend_from_slice(b"data");
    out.extend_from_slice(&len.to_le_bytes());
    for s in samples {
        out.extend_from_slice(&s.to_le_bytes());
    }
    out
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn late_capture_cannot_repopulate_a_cancelled_slot() {
        let t = Instant::now();
        let mut m = Media::default();
        let id = m.request(false, t).unwrap();
        assert_eq!(m.take_frame(t), Some(id));
        m.clear();
        m.finish_frame(id, Ok(vec![1]), t);
        assert!(m.frame.is_none());
    }
    #[test]
    fn captures_are_bounded_expire_and_rate_limit() {
        let t = Instant::now();
        let mut m = Media::default();
        m.request(false, t).unwrap();
        assert!(m.request(false, t).is_err());
        m.take_frame(t);
        m.finish_frame(1, Ok(vec![0; MAX_PNG + 1]), t);
        assert!(matches!(m.frame.as_ref().unwrap().value, Frame::Failed(_)));
        m.expire(t + TTL);
        assert!(m.frame.is_none());
    }
    #[test]
    fn game_audio_is_mono_bounded_and_wav_is_valid() {
        let t = Instant::now();
        let mut m = Media::default();
        m.request(true, t).unwrap();
        m.push_audio(&vec![0.5; 96000], t);
        let a = &m.audio.as_ref().unwrap().value;
        assert_eq!(a.len(), 16000);
        assert_eq!(a[0], 16383);
        let w = wav(a);
        assert_eq!(&w[..4], b"RIFF");
        assert_eq!(w.len(), 32044);
        m.expire(t + TTL);
        assert!(m.audio.is_none());
    }
    #[test]
    fn watch_replaces_only_completed_frames_at_one_hz_and_stops() {
        let t = Instant::now();
        let mut m = Media::default();
        m.watch(2500, t).unwrap();
        let a = m.take_frame(t).unwrap();
        assert_eq!(m.take_frame(t + Duration::from_secs(1)), None); // encoder still busy
        m.finish_frame(a, Ok(vec![1]), t + Duration::from_secs(1));
        let b = m.take_frame(t + Duration::from_secs(1)).unwrap();
        assert_ne!(a, b);
        m.finish_frame(a, Ok(vec![2]), t + Duration::from_secs(1));
        assert!(matches!(m.frame.as_ref().unwrap().value, Frame::Encoding));
        m.clear();
        assert!(!m.wanted());
        assert!(m.take_frame(t + Duration::from_secs(2)).is_none());
        m.watch(1, t).unwrap();
        m.expire(t + Duration::from_millis(2));
        assert!(!m.wanted());
    }
}
