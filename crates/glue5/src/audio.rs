//! The engine's mixer output to the default playback device (SDL), and the
//! start page's music: one track, muffled while a menu is open.

use std::cell::UnsafeCell;
use std::ffi::c_void;
use std::os::raw::c_int;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use anyhow::{Result, anyhow};
use glue5_runtime::audio::{AudioOut, CHANNELS, SAMPLE_RATE};
use sdl3::audio::{AudioCallback, AudioFormat, AudioSpec, AudioStream, AudioStreamWithCallback};

/// Playing while alive.
pub struct Audio {
    stream: AudioStreamWithCallback<Pull>,
}

impl Drop for Audio {
    fn drop(&mut self) {
        // sdl3 frees the callback's state before it destroys the stream, and
        // SDL's audio thread can call it in between: at exit, a memset into
        // the freed buffer. Pausing takes the device's lock, so it waits for
        // a callback in progress, and no other comes.
        let _ = self.stream.pause();
    }
}

pub fn start(subsystem: &sdl3::AudioSubsystem, out: AudioOut) -> Result<Audio> {
    let spec = AudioSpec {
        freq: Some(SAMPLE_RATE as i32),
        channels: Some(CHANNELS as i32),
        format: Some(AudioFormat::f32_sys()),
    };
    // A small device period, so the mixer refills a little at a time. The
    // game's audio update runs at most a few frames ahead of the mixer, and
    // with SDL's default (about 1024 frames) light scenes such as the pause
    // menu wait in bursts: frames alternate between 4 and 30 ms.
    sdl3::hint::set("SDL_AUDIO_DEVICE_SAMPLE_FRAMES", "256");
    let pull = Pull { out, buf: Vec::new(), frames: 0, mixed: 0, calls: 0, peak: 0.0, last_log: Instant::now() };
    let stream = subsystem.open_playback_stream(&spec, pull).map_err(|e| anyhow!("SDL audio: {e}"))?;
    stream.resume().map_err(|e| anyhow!("SDL audio: {e}"))?;
    log::info!("audio: {SAMPLE_RATE} Hz stereo ({})", subsystem.current_audio_driver());
    Ok(Audio { stream })
}

/// SDL's audio thread asks for samples; they come straight from the ring.
struct Pull {
    out: AudioOut,
    buf: Vec<f32>,
    frames: u64,
    mixed: u64,
    calls: u64,
    peak: f32,
    last_log: Instant,
}

impl AudioCallback<f32> for Pull {
    fn callback(&mut self, stream: &mut AudioStream, requested: i32) {
        let samples = (requested.max(0) as usize).next_multiple_of(CHANNELS);
        self.buf.resize(samples, 0.0);
        self.mixed += self.out.read(&mut self.buf) as u64;
        self.frames += (samples / CHANNELS) as u64;
        self.calls += 1;
        let _ = stream.put_data_f32(&self.buf);

        if log::log_enabled!(log::Level::Debug) {
            self.peak = self.buf.iter().fold(self.peak, |p, s| p.max(s.abs()));
            if self.last_log.elapsed() >= Duration::from_secs(10) {
                self.last_log = Instant::now();
                log::debug!(
                    "audio: {} frames played ({} per request), {} silent (underrun or no mixer yet), peak {:.3}",
                    self.frames,
                    self.frames / self.calls.max(1),
                    self.frames - self.mixed,
                    std::mem::take(&mut self.peak)
                );
            }
        }
    }
}

/// The start-page soundtrack, streamed by SDL3_mixer from embedded MP3 bytes.
const MENU_MUSIC: &[u8] = include_bytes!("../assets/audio/pausemenu.mp3");
const MENU_MUSIC_GAIN: f32 = 0.20;
/// The first fade: from silence to the track, once the app is up.
const MENU_FADE_IN: Duration = Duration::from_millis(1800);
/// Before the track pauses (the game starting, the app in the background):
/// short, so that it never clicks off.
const MENU_FADE_PAUSE: Duration = Duration::from_millis(300);

// --- The muffling ---------------------------------------------------------
//
// Opening a menu does not stop or fade the track: the music keeps playing as it
// is, a wall away. SDL_mixer's post-mix callback hands over everything it has
// mixed, and a low-pass there turns the sound dull. Nothing of the file
// changes: same MP3, same gain, same loop, no extra asset.

/// The cutoff with no menu open (the music as recorded: the filter is bypassed)
/// and with one open.
const MUFFLE_OPEN_HZ: f32 = 20_000.0;
const MUFFLE_CLOSED_HZ: f32 = 750.0;
/// Muffled, and a little quieter: 0.70 is about -3 dB.
const MUFFLE_CLOSED_GAIN: f32 = 0.70;
/// The filter's resonance: Butterworth, no peak (above 1 the cutoff would ring).
const MUFFLE_Q: f32 = 0.707;
/// How much of the way to the target the filter moves per callback (about 5 ms
/// with 256-frame periods): ten callbacks, 50 ms, to close.
const MUFFLE_STEP: f32 = 0.10;
/// Filter state per channel, which covers every device SDL opens.
const MUFFLE_CHANNELS: usize = 8;

#[derive(Clone, Copy)]
struct GainFade {
    from: f32,
    to: f32,
    started: Instant,
    duration: Duration,
}

/// The filter, as the audio thread keeps it: where it is now, and one biquad's
/// state per channel.
struct Filter {
    cutoff: f32,
    gain: f32,
    rate: f32,
    channels: usize,
    /// The coefficients follow `cutoff`; false until it changes.
    tuned: bool,
    b0: f32,
    b1: f32,
    b2: f32,
    a1: f32,
    a2: f32,
    x1: [f32; MUFFLE_CHANNELS],
    x2: [f32; MUFFLE_CHANNELS],
    y1: [f32; MUFFLE_CHANNELS],
    y2: [f32; MUFFLE_CHANNELS],
}

impl Filter {
    fn new() -> Self {
        Filter {
            cutoff: MUFFLE_OPEN_HZ,
            gain: 1.0,
            rate: 48_000.0,
            channels: 2,
            tuned: false,
            b0: 1.0,
            b1: 0.0,
            b2: 0.0,
            a1: 0.0,
            a2: 0.0,
            x1: [0.0; MUFFLE_CHANNELS],
            x2: [0.0; MUFFLE_CHANNELS],
            y1: [0.0; MUFFLE_CHANNELS],
            y2: [0.0; MUFFLE_CHANNELS],
        }
    }

    /// The device may open at another rate or with another channel count than
    /// the one before: the filter starts over then.
    fn configure(&mut self, rate: f32, channels: usize) {
        let channels = channels.clamp(1, MUFFLE_CHANNELS);
        if (self.rate - rate).abs() > 0.5 || self.channels != channels {
            self.rate = rate.max(8_000.0);
            self.channels = channels;
            self.x1 = [0.0; MUFFLE_CHANNELS];
            self.x2 = [0.0; MUFFLE_CHANNELS];
            self.y1 = [0.0; MUFFLE_CHANNELS];
            self.y2 = [0.0; MUFFLE_CHANNELS];
            self.tuned = false;
        }
    }

    /// A step of the way toward where the menu wants the sound, once per
    /// callback: fast enough to feel like a hand over the speaker, slow enough
    /// that no step is audible.
    fn reach(&mut self, cutoff: f32, gain: f32) {
        let move_to = |from: f32, to: f32| if (to - from).abs() < 1.0 { to } else { from + (to - from) * MUFFLE_STEP };
        let cutoff = move_to(self.cutoff, cutoff).clamp(80.0, self.rate * 0.45);
        let gain = move_to(self.gain, gain);
        if (cutoff - self.cutoff).abs() > 0.5 || gain != self.gain {
            self.tuned = false;
        }
        self.cutoff = cutoff;
        self.gain = gain;
        if !self.tuned {
            self.tune();
        }
    }

    /// Robert Bristow-Johnson's low-pass, normalized by a0.
    fn tune(&mut self) {
        let w0 = 2.0 * std::f32::consts::PI * self.cutoff / self.rate;
        let (sin, cos) = w0.sin_cos();
        let alpha = sin / (2.0 * MUFFLE_Q);
        let a0 = 1.0 + alpha;
        self.b0 = ((1.0 - cos) * 0.5) / a0;
        self.b1 = (1.0 - cos) / a0;
        self.b2 = self.b0;
        self.a1 = (-2.0 * cos) / a0;
        self.a2 = (1.0 - alpha) / a0;
        self.tuned = true;
    }

    /// Whether the sound is already as it should be: then nothing is touched at
    /// all (no cost per sample, and the output stays bit for bit the track).
    fn open(&self) -> bool {
        self.cutoff >= MUFFLE_OPEN_HZ * 0.95 && self.gain >= 0.999
    }

    fn process(&mut self, pcm: &mut [f32]) {
        if self.open() {
            return;
        }
        let channels = self.channels;
        let (gain, b0, b1, b2, a1, a2) = (self.gain, self.b0, self.b1, self.b2, self.a1, self.a2);
        for frame in pcm.chunks_mut(channels) {
            for (c, sample) in frame.iter_mut().enumerate() {
                let x = *sample;
                let y = b0 * x + b1 * self.x1[c] + b2 * self.x2[c] - a1 * self.y1[c] - a2 * self.y2[c];
                self.x2[c] = self.x1[c];
                self.x1[c] = x;
                self.y2[c] = self.y1[c];
                self.y1[c] = y;
                *sample = y * gain;
            }
        }
    }
}

/// What the UI tells the audio thread, and the filter the thread keeps. The
/// target packs the cutoff and the gain in one atomic, so a callback either
/// sees all of a change or none of it, without a lock on the audio thread.
struct Muffle {
    target: AtomicU64,
    filter: UnsafeCell<Filter>,
}

// SAFETY: `target` is atomic, and `filter` is only reached from the mixer's
// callback, which SDL calls from one thread at a time; `MenuMusic` only sets
// `target`.
unsafe impl Send for Muffle {}
unsafe impl Sync for Muffle {}

impl Muffle {
    fn new() -> Self {
        let mut filter = Filter::new();
        filter.reach(MUFFLE_OPEN_HZ, 1.0);
        Muffle { target: AtomicU64::new(Self::pack(MUFFLE_OPEN_HZ, 1.0)), filter: UnsafeCell::new(filter) }
    }

    fn pack(cutoff: f32, gain: f32) -> u64 {
        (u64::from(cutoff.to_bits()) << 32) | u64::from(gain.to_bits())
    }

    /// What a menu asks for: dull behind it, or as recorded.
    fn set(&self, muffled: bool) {
        let (cutoff, gain) = if muffled {
            (MUFFLE_CLOSED_HZ, MUFFLE_CLOSED_GAIN)
        } else {
            (MUFFLE_OPEN_HZ, 1.0)
        };
        self.target.store(Self::pack(cutoff, gain), Ordering::Relaxed);
    }
}

/// SDL_mixer's post-mix callback: every sample it has mixed, before the device
/// plays it (always float32, whatever the device's format).
///
/// # Safety
///
/// `userdata` is the `Muffle` that `MenuMusic` owns, which outlives the mixer.
unsafe extern "C" fn muffled(
    userdata: *mut c_void,
    _mixer: *mut sdl3::mixer::sys::MIX_Mixer,
    spec: *const sdl3::sys::audio::SDL_AudioSpec,
    pcm: *mut f32,
    samples: c_int,
) {
    if userdata.is_null() || pcm.is_null() || samples <= 0 {
        return;
    }
    // SAFETY: the caller is SDL_mixer's mix thread, with the `Muffle` that
    // `MenuMusic` keeps alive for as long as the mixer exists; only that thread
    // touches `filter`.
    let muffle = unsafe { &*(userdata.cast::<Muffle>()) };
    let target = muffle.target.load(Ordering::Relaxed);
    let (cutoff, gain) = (f32::from_bits((target >> 32) as u32), f32::from_bits(target as u32));
    let (rate, channels) = if spec.is_null() {
        (48_000.0, 2)
    } else {
        // SAFETY: SDL passes the mixer's own format here, for this call.
        let spec = unsafe { &*spec };
        (spec.freq as f32, spec.channels as usize)
    };
    // SAFETY: one audio thread at a time reaches the filter.
    let filter = unsafe { &mut *muffle.filter.get() };
    filter.configure(rate, channels.max(1));
    filter.reach(cutoff, gain);
    // SAFETY: SDL says `pcm` points at `samples` floats, and this callback may
    // change them.
    let pcm = unsafe { std::slice::from_raw_parts_mut(pcm, samples as usize) };
    filter.process(pcm);
}

/// Music on the GLUE 5 start page, muffled while a menu is open over it. The
/// track still stops (with a short fade) when the game starts or the app goes
/// to the background.
///
/// Field order is intentional: SDL_mixer's track must be destroyed before its
/// mixer, its audio before the memory stream it reads from, and all of them
/// before SDL's audio subsystem is released. The muffling's state comes last:
/// the mixer that calls it is destroyed before it.
pub struct MenuMusic {
    track: sdl3::mixer::Track<'static>,
    _audio: sdl3::mixer::Audio,
    _source: sdl3::iostream::IOStream<'static>,
    _mixer: Box<sdl3::mixer::Mixer>,
    muffle: Box<Muffle>,
    enabled: bool,
    playing: bool,
    gain: f32,
    fade: Option<GainFade>,
}

impl Drop for MenuMusic {
    fn drop(&mut self) {
        // The callback goes before the state it reads does.
        // SAFETY: the mixer is alive until this struct's fields are dropped.
        unsafe {
            sdl3::mixer::sys::MIX_SetPostMixCallback(self._mixer.raw(), None, std::ptr::null_mut());
        }
    }
}

impl MenuMusic {
    pub fn start(_audio: &sdl3::AudioSubsystem) -> Result<MenuMusic> {
        let mixer = Box::new(sdl3::mixer::Mixer::open_device(None).map_err(|e| anyhow!("SDL mixer: {e}"))?);
        let source = sdl3::iostream::IOStream::from_bytes(MENU_MUSIC).map_err(|e| anyhow!("menu music stream: {e}"))?;
        // Keep the IO stream alive for the lifetime of the decoder: the track
        // is streamed, not expanded to hundreds of megabytes of PCM at launch.
        let audio = mixer.load_audio_io(&source, false).map_err(|e| anyhow!("menu music decode: {e}"))?;
        let track = mixer.create_track().map_err(|e| anyhow!("menu music track: {e}"))?;
        track.set_audio(&audio).map_err(|e| anyhow!("menu music source: {e}"))?;
        track.set_loops(-1).map_err(|e| anyhow!("menu music loop: {e}"))?;
        track.set_gain(0.0).map_err(|e| anyhow!("menu music volume: {e}"))?;

        // SAFETY: Track contains only SDL_mixer's raw track pointer plus a
        // phantom borrow. The mixer is owned by this struct and kept in a Box;
        // `track` is the first field, so it is destroyed before the mixer.
        // The extended lifetime cannot escape MenuMusic.
        let track: sdl3::mixer::Track<'static> = unsafe { std::mem::transmute(track) };
        track.play().map_err(|e| anyhow!("menu music playback: {e}"))?;

        let muffle = Box::new(Muffle::new());
        // SAFETY: the callback reads the `Muffle` this struct owns, which is
        // dropped only after the mixer that calls it (Drop turns it off first).
        let installed = unsafe {
            sdl3::mixer::sys::MIX_SetPostMixCallback(
                mixer.raw(),
                Some(muffled),
                (&*muffle as *const Muffle).cast_mut().cast::<c_void>(),
            )
        };
        if !installed {
            log::warn!("menu music: no muffling (SDL_mixer refused the post-mix callback)");
        }

        Ok(MenuMusic {
            track,
            _audio: audio,
            _source: source,
            _mixer: mixer,
            muffle,
            enabled: true,
            playing: true,
            gain: 0.0,
            fade: Some(GainFade { from: 0.0, to: MENU_MUSIC_GAIN, started: Instant::now(), duration: MENU_FADE_IN }),
        })
    }

    pub fn is_fading(&self) -> bool {
        self.fade.is_some()
    }

    /// Called from the app loop. `enabled`: the music plays at all (before the
    /// game, in the foreground). `muffled`: a menu is open over the start page,
    /// and the track goes dull behind it, without stopping.
    pub fn update(&mut self, enabled: bool, muffled: bool) -> Result<()> {
        // What the audio thread should be doing, every frame: a menu opened or
        // closed (or the app resumed) is noticed at the next callback.
        self.muffle.set(muffled && enabled);

        if enabled != self.enabled {
            self.enabled = enabled;
            if enabled {
                if !self.playing {
                    self.track.set_gain(0.0).map_err(|e| anyhow!("menu music volume: {e}"))?;
                    self.track.resume().map_err(|e| anyhow!("resume menu music: {e}"))?;
                    self.playing = true;
                    self.gain = 0.0;
                }
                self.fade = Some(GainFade {
                    from: self.gain,
                    to: MENU_MUSIC_GAIN,
                    started: Instant::now(),
                    duration: MENU_FADE_IN,
                });
            } else if self.playing {
                self.fade = Some(GainFade {
                    from: self.gain,
                    to: 0.0,
                    started: Instant::now(),
                    duration: MENU_FADE_PAUSE,
                });
            } else {
                self.fade = None;
            }
        }

        if let Some(fade) = self.fade {
            let progress = (fade.started.elapsed().as_secs_f32() / fade.duration.as_secs_f32()).clamp(0.0, 1.0);
            self.gain = fade.from + (fade.to - fade.from) * progress;
            self.track.set_gain(self.gain).map_err(|e| anyhow!("menu music volume: {e}"))?;
            if progress >= 1.0 {
                self.fade = None;
                if !self.enabled && self.playing {
                    self.track.pause().map_err(|e| anyhow!("pause menu music: {e}"))?;
                    self.playing = false;
                }
            }
        }
        Ok(())
    }
}
