//! The engine's mixer output to the default playback device (SDL).

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
const MENU_FADE_IN: Duration = Duration::from_millis(1800);
const MENU_FADE_OUT: Duration = Duration::from_millis(1200);

#[derive(Clone, Copy)]
struct GainFade {
    from: f32,
    to: f32,
    started: Instant,
    duration: Duration,
}

/// Music on the GLUE 5 start page. The track is stopped while settings,
/// gameplay, or a backgrounded app is active, and fades in again on return.
///
/// Field order is intentional: SDL_mixer's track must be destroyed before its
/// mixer, its audio before the memory stream it reads from, and all of them
/// before SDL's audio subsystem is released.
pub struct MenuMusic {
    track: sdl3::mixer::Track<'static>,
    _audio: sdl3::mixer::Audio,
    _source: sdl3::iostream::IOStream<'static>,
    _mixer: Box<sdl3::mixer::Mixer>,
    enabled: bool,
    playing: bool,
    gain: f32,
    fade: Option<GainFade>,
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

        Ok(MenuMusic {
            track,
            _audio: audio,
            _source: source,
            _mixer: mixer,
            enabled: true,
            playing: true,
            gain: 0.0,
            fade: Some(GainFade { from: 0.0, to: MENU_MUSIC_GAIN, started: Instant::now(), duration: MENU_FADE_IN }),
        })
    }

    pub fn is_fading(&self) -> bool {
        self.fade.is_some()
    }

    /// Called from the app loop; gain interpolation runs only during a fade.
    pub fn update(&mut self, enabled: bool) -> Result<()> {
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
                    duration: MENU_FADE_OUT,
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
