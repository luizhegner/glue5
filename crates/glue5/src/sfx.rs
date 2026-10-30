//! The interface's own sounds: the ticks and clicks a console menu makes when
//! the selection moves, a value changes, a page opens or the game starts.
//!
//! The files are not shipped with GLUE 5 (see `docs/menu-sfx.md`): they are read
//! from `sfx/` in the app's folder or `glue5-sfx/` beside or inside the game's,
//! like the start page's scenes. A missing file means silence for that event
//! and nothing else; every sound that is found is decoded once, at start.
//!
//! The sounds play on their own mixer, so the music's muffling (see
//! [`crate::audio`]) never dulls them, and a tick never cuts a swell.
//!
//! The menus do not hold the mixer: they only say what happened
//! ([`want`]), and the application plays what was asked for once per frame
//! ([`Player::update`]). That keeps the pages free of audio plumbing and the
//! audio on the thread that owns it.

use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use anyhow::{Result, anyhow};
use sdl3::mixer::{Audio as Clip, Mixer, Track};

use crate::paths;

/// What happened in the interface.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Sound {
    /// The selection moved: another line, another tab.
    Focus,
    /// Something was accepted: Play, Open, Choose….
    Confirm,
    /// Back out of a page, cancel, close.
    Back,
    /// Another category or page.
    Tab,
    /// A value changed on the line the selection is on.
    Adjust,
    ToggleOn,
    ToggleOff,
    /// A menu appeared over the game.
    Open,
    /// A menu went away.
    Close,
    /// Something cannot happen.
    Error,
    /// The game is about to start: the one loud sound.
    Launch,
    /// A notification slid in.
    Notify,
}

impl Sound {
    /// How many there are.
    pub const COUNT: usize = 12;

    pub const ALL: [Sound; Sound::COUNT] = [
        Sound::Focus,
        Sound::Confirm,
        Sound::Back,
        Sound::Tab,
        Sound::Adjust,
        Sound::ToggleOn,
        Sound::ToggleOff,
        Sound::Open,
        Sound::Close,
        Sound::Error,
        Sound::Launch,
        Sound::Notify,
    ];

    /// The file's name, without its extension.
    pub const fn file(self) -> &'static str {
        match self {
            Sound::Focus => "focus",
            Sound::Confirm => "confirm",
            Sound::Back => "back",
            Sound::Tab => "tab",
            Sound::Adjust => "adjust",
            Sound::ToggleOn => "toggle_on",
            Sound::ToggleOff => "toggle_off",
            Sound::Open => "open",
            Sound::Close => "close",
            Sound::Error => "error",
            Sound::Launch => "launch",
            Sound::Notify => "notify",
        }
    }

    const fn index(self) -> usize {
        self as usize
    }

    /// How loud it is beside the others: the ticks stay under the swells.
    const fn gain(self) -> f32 {
        match self {
            Sound::Focus => 0.55,
            Sound::Adjust => 0.55,
            Sound::Tab => 0.65,
            Sound::Confirm => 0.8,
            Sound::Back => 0.8,
            Sound::ToggleOn | Sound::ToggleOff => 0.7,
            Sound::Open => 0.9,
            Sound::Close => 0.9,
            Sound::Error => 0.85,
            Sound::Launch => 1.0,
            Sound::Notify => 0.85,
        }
    }

    /// The shortest time between two of these: a stick held against an edge, or
    /// a slider dragged, must not buzz.
    const fn gap(self) -> Duration {
        match self {
            Sound::Focus | Sound::Adjust => Duration::from_millis(40),
            Sound::Tab => Duration::from_millis(120),
            _ => Duration::from_millis(200),
        }
    }
}

thread_local! {
    /// What the pages asked for since the last frame, and when each sound last
    /// played (to keep a held key from buzzing).
    static WHICH: RefCell<Vec<Sound>> = const { RefCell::new(Vec::new()) };
    static LAST: RefCell<[Option<Instant>; Sound::COUNT]> = const { RefCell::new([None; Sound::COUNT]) };
}

/// Says that something happened: the menus call this, the application plays it
/// at the end of the frame. Silent when the same sound played a moment ago.
pub fn want(sound: Sound) {
    let now = Instant::now();
    let allowed = LAST.with_borrow_mut(|last| {
        let slot = &mut last[sound.index()];
        if slot.is_some_and(|at| now.duration_since(at) < sound.gap()) {
            false
        } else {
            *slot = Some(now);
            true
        }
    });
    if allowed {
        WHICH.with_borrow_mut(|queue| queue.push(sound));
    }
}

/// What was asked for since the last call (and nothing more will be, until the
/// next frame at most).
fn take() -> Vec<Sound> {
    WHICH.with_borrow_mut(std::mem::take)
}

/// A sound's track: one per file, so a tick restarts itself rather than
/// stacking up.
struct Voice {
    sound: Sound,
    track: Track<'static>,
}

/// The interface's sounds, on a mixer of their own.
///
/// Field order is intentional: the tracks must be destroyed before the mixer
/// they belong to.
pub struct Player {
    voices: Vec<Voice>,
    mixer: Box<Mixer>,
    volume: f32,
    on: bool,
    /// The files were looked for and none was found: said once, in the log.
    warned: bool,
}

impl Player {
    /// Loads every sound found for this user and game folder. Never fails: a
    /// mixer that will not open, or a file that will not decode, leaves the
    /// menus silent.
    pub fn start(user_dir: &Path, game_dir: Option<&Path>) -> Option<Player> {
        let mixer = match Mixer::open_device(None) {
            Ok(mixer) => Box::new(mixer),
            Err(error) => {
                log::warn!("menu sounds unavailable (no mixer): {error}");
                return None;
            }
        };
        let dirs = candidates(user_dir, game_dir);
        let mut voices = Vec::new();
        for sound in Sound::ALL {
            let Some(path) = find(&dirs, sound) else { continue };
            match load(mixer.as_ref(), &path, sound) {
                Ok(track) => {
                    log::info!("menu sounds: {} from {}", sound.file(), path.display());
                    // SAFETY: the track is stored in a field declared before the
                    // mixer, so it is destroyed while the mixer still exists;
                    // the 'static lifetime never leaves this struct.
                    let track: Track<'static> = unsafe { std::mem::transmute(track) };
                    voices.push(Voice { sound, track });
                }
                Err(error) => log::warn!("menu sounds: {}: {error:#}", path.display()),
            }
        }
        if voices.is_empty() {
            log::info!("menu sounds: none in {} (optional; see docs/menu-sfx.md)", dirs[0].display());
        }
        Some(Player { voices, mixer, volume: -1.0, on: true, warned: false })
    }

    /// Plays what the menus asked for. `enabled` is the setting; `volume` is 0
    /// to 1 (the mixer's master gain).
    pub fn update(&mut self, enabled: bool, volume: f32) {
        let volume = volume.clamp(0.0, 1.0);
        if (volume - self.volume).abs() > 0.001 || enabled != self.on {
            self.volume = volume;
            self.on = enabled;
            // Off is silence, not a lower gain: 0 is silent either way, but
            // the mixer's gain stays where the slider is.
            let _ = self.mixer.set_gain(if enabled { volume } else { 0.0 });
        }
        let asked = take();
        if !self.on {
            return;
        }
        for sound in asked {
            match self.voices.iter().position(|voice| voice.sound == sound) {
                // Play restarts the track from its beginning: a tick that is
                // still sounding starts over instead of doubling.
                Some(index) => {
                    if let Err(error) = self.voices[index].track.play() {
                        log::warn!("menu sounds: cannot play {:?}: {error}", sound);
                    }
                }
                None => {
                    if !self.warned {
                        self.warned = true;
                        log::debug!("menu sounds: no file for {:?} (see docs/menu-sfx.md)", sound);
                    }
                }
            }
        }
    }
}

/// The folders a sound may be in, best first.
fn candidates(user_dir: &Path, game_dir: Option<&Path>) -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    if let Some(game) = game_dir {
        dirs.push(game.join(paths::SFX_FOLDER));
        if let Some(parent) = game.parent() {
            dirs.push(parent.join(paths::SFX_FOLDER));
            if let Some(grand) = parent.parent() {
                dirs.push(grand.join(paths::SFX_FOLDER));
            }
        }
    }
    dirs.push(paths::sfx_dir(user_dir));
    dirs
}

/// The first file for `sound` in those folders. Any format SDL_mixer decodes
/// will do; WAV and MP3 are what the list suggests.
fn find(dirs: &[PathBuf], sound: Sound) -> Option<PathBuf> {
    const EXTENSIONS: [&str; 4] = ["wav", "mp3", "ogg", "flac"];
    dirs.iter().find_map(|dir| {
        EXTENSIONS.iter().map(|extension| dir.join(format!("{}.{extension}", sound.file()))).find(|path| path.is_file())
    })
}

fn load<'a>(mixer: &'a Mixer, path: &Path, sound: Sound) -> Result<Track<'a>> {
    // Decoded once: the files are short, and decoding during a click would
    // drop the first milliseconds of it.
    let clip: Clip = mixer.load_audio(path, true).map_err(|e| anyhow!("{e}"))?;
    let track = mixer.create_track().map_err(|e| anyhow!("{e}"))?;
    track.set_audio(&clip).map_err(|e| anyhow!("{e}"))?;
    track.set_loops(0).map_err(|e| anyhow!("{e}"))?;
    track.set_gain(sound.gain()).map_err(|e| anyhow!("{e}"))?;
    Ok(track)
}

/// Whether any sound was found, for the settings page to say so.
pub fn installed(user_dir: &Path, game_dir: Option<&Path>) -> bool {
    let dirs = candidates(user_dir, game_dir);
    Sound::ALL.iter().any(|sound| find(&dirs, *sound).is_some())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_sound_has_a_name_of_its_own() {
        let mut names: Vec<&str> = Sound::ALL.iter().map(|s| s.file()).collect();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), Sound::ALL.len());
        assert_eq!(Sound::ALL.len(), Sound::COUNT);
    }

    #[test]
    fn a_held_key_does_not_buzz() {
        want(Sound::Focus);
        want(Sound::Focus);
        let asked = take();
        assert_eq!(asked.iter().filter(|s| **s == Sound::Focus).count(), 1);
    }

    #[test]
    fn files_are_looked_for_in_every_format() {
        let dir = std::env::temp_dir().join("glue5-sfx-test");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("confirm.ogg"), b"").unwrap();
        assert_eq!(find(&[dir.clone()], Sound::Confirm), Some(dir.join("confirm.ogg")));
        assert_eq!(find(&[dir.clone()], Sound::Back), None);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
