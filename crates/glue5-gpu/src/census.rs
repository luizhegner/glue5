//! What a frame's HUD phase consists of, for making the UI's resolution
//! independent of the scene's.
//!
//! The engine draws the scene and the HUD into one back buffer, at
//! `window × render_scale`, so with a low scale the HUD is low-resolution too.
//! The host's hook on `PostFX::ProcessNonDepthFX` marks where the scene is
//! finished ([`glue5_runtime::gpu::op::SCENE_DONE`]); everything after it is
//! HUD, which is where a native-resolution target could take over.
//!
//! Redirecting those draws is only safe if what they actually do is known:
//! which targets they write, whether any reads the back buffer back (a blur
//! behind a menu, the minimap's mask), whether a depth-stencil is attached
//! (wgpu rejects a pass whose attachments differ in size), whether the pixel
//! program reads D3D's `SV_Position` — which is in pixels of the target, so a
//! bigger one changes what the program computes — and how often the targets
//! change, since every change reloads a tile-based GPU's (Mali, Adreno) tiles.
//!
//! This module measures that instead of guessing. It is **passive**: it reads
//! what the commands already say and changes nothing about rendering, so a
//! frame is recorded exactly as it would have been drawn.
//!
//! Records aggregate per frame rather than per draw: a frame's HUD phase is
//! tens of draws, and the questions above are about the *set* of things it
//! does, not their order. That keeps the cost and the output small enough to
//! leave on for a whole session on a phone.
//!
//! Turn it on with `GLUE5_CENSUS`: a path appends one JSON line per frame
//! there, `log` writes the same lines to the log. Off (the default) costs one
//! relaxed atomic load per draw.

use std::collections::BTreeMap;
use std::fs::OpenOptions;
use std::io::Write;
use std::sync::LazyLock;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, MutexGuard};

/// Where the lines go: `None` until the first frame is recorded.
enum Sink {
    Log,
    File(std::fs::File),
}

static ON: AtomicBool = AtomicBool::new(false);
/// `GLUE5_CENSUS` has been read, so `ON` will not change again.
static READ: AtomicBool = AtomicBool::new(false);
static LIVE: LazyLock<Mutex<State>> = LazyLock::new(|| Mutex::new(State::default()));

#[derive(Default)]
struct State {
    recorder: Recorder,
    sink: Option<Sink>,
    /// Set once the environment has been read.
    read_env: bool,
}

/// A render target a draw writes, as the command named it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Target {
    pub id: u32,
    pub width: u32,
    pub height: u32,
    /// The engine's format code (see [`crate::format`]).
    pub format: u32,
}

/// A draw's pixel program, and what matters of it here.
#[derive(Clone, Copy, Debug)]
pub struct Ps<'a> {
    pub label: &'a str,
    /// It reads D3D's `SV_Position` (WGSL's `@builtin(position)` as a fragment
    /// input), so its result depends on the target's size in pixels.
    pub reads_position: bool,
}

/// One draw, as the census records it.
#[derive(Clone, Copy, Debug)]
pub struct Draw<'a> {
    /// The back buffer's id this frame (the `PRESENT` command's argument).
    pub bb: u32,
    /// Its colour targets, in slot order, without the empty ones.
    pub targets: &'a [Target],
    /// Its depth-stencil target, when one is attached.
    pub ds: Option<Target>,
    pub viewport: Option<[f32; 6]>,
    pub scissor: Option<[u32; 4]>,
    pub vs: Option<&'a str>,
    pub ps: Option<Ps<'a>>,
    /// It reads the back buffer through a shader resource view.
    pub reads_bb: bool,
    /// Its blend state's id.
    pub blend: u32,
}

/// A pixel program the HUD phase drew with.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HudProgram {
    pub label: String,
    pub reads_position: bool,
    pub draws: u32,
}

/// A target the HUD phase drew into.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HudTarget {
    pub id: u32,
    pub width: u32,
    pub height: u32,
    /// It is the back buffer.
    pub bb: bool,
    pub draws: u32,
}

/// One frame, as the census saw it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Frame {
    pub frame: u64,
    /// The back buffer's id and size, as `PRESENT` named them.
    pub bb: u32,
    pub bb_width: u32,
    pub bb_height: u32,
    /// `SCENE_DONE` came, so the frame has a HUD phase at all. Frames without
    /// it (a menu over everything) are the whole-frame bicubic path.
    pub scene_done: bool,
    /// Draws before `SCENE_DONE`, and after it.
    pub draws_scene: u32,
    pub draws_hud: u32,
    /// Times the HUD phase's target set changed: each is a render pass the GPU
    /// has to begin, and on a tile-based one, reload.
    pub hud_pass_switches: u32,
    pub hud_programs: Vec<HudProgram>,
    pub hud_targets: Vec<HudTarget>,
    /// HUD draws reading the back buffer through an SRV.
    pub hud_reads_bb: u32,
    /// HUD draws with a depth-stencil target attached.
    pub hud_with_ds: u32,
    /// Blend states the HUD phase used.
    pub hud_blends: Vec<u32>,
    /// HUD draws whose viewport or scissor reaches outside the back buffer.
    pub hud_out_of_bounds: u32,
    /// What the frame broke (see [`check`]).
    pub violations: Vec<String>,
}

impl Frame {
    /// Its JSON line.
    pub fn to_json(&self) -> String {
        let programs: Vec<_> = self
            .hud_programs
            .iter()
            .map(|p| serde_json::json!({ "label": p.label, "reads_position": p.reads_position, "draws": p.draws }))
            .collect();
        let targets: Vec<_> = self
            .hud_targets
            .iter()
            .map(|t| {
                serde_json::json!({
                    "id": t.id, "width": t.width, "height": t.height,
                    "back_buffer": t.bb, "draws": t.draws,
                })
            })
            .collect();
        serde_json::json!({
            "frame": self.frame,
            "bb": self.bb,
            "bb_width": self.bb_width,
            "bb_height": self.bb_height,
            "scene_done": self.scene_done,
            "draws_scene": self.draws_scene,
            "draws_hud": self.draws_hud,
            "hud_pass_switches": self.hud_pass_switches,
            "hud_programs": programs,
            "hud_targets": targets,
            "hud_reads_bb": self.hud_reads_bb,
            "hud_with_ds": self.hud_with_ds,
            "hud_blends": self.hud_blends,
            "hud_out_of_bounds": self.hud_out_of_bounds,
            "violations": self.violations,
        })
        .to_string()
    }
}

/// Accumulates a frame's draws; [`Recorder::present`] finishes it.
///
/// Plain data, no globals: the process-wide census is a thin wrapper, so this
/// can be driven directly (and tested) without touching it.
#[derive(Default)]
pub struct Recorder {
    frame: u64,
    scene_done: bool,
    draws_scene: u32,
    draws_hud: u32,
    switches: u32,
    /// The HUD phase's last target set, to count its changes.
    last: Option<Vec<u32>>,
    programs: BTreeMap<String, (bool, u32)>,
    targets: BTreeMap<u32, (u32, u32, bool, u32)>,
    reads_bb: u32,
    with_ds: u32,
    blends: BTreeMap<u32, ()>,
    out_of_bounds: u32,
}

impl Recorder {
    /// The scene is finished in the back buffer; the HUD follows.
    pub fn scene_done(&mut self) {
        self.scene_done = true;
    }

    pub fn draw(&mut self, d: &Draw<'_>) {
        if !self.scene_done {
            self.draws_scene += 1;
            return;
        }
        self.draws_hud += 1;

        // A change of target set is a render pass the GPU begins again.
        let ids: Vec<u32> = d.targets.iter().map(|t| t.id).collect();
        if self.last.as_ref() != Some(&ids) {
            if self.last.is_some() {
                self.switches += 1;
            }
            self.last = Some(ids);
        }

        for t in d.targets {
            let e = self.targets.entry(t.id).or_insert((t.width, t.height, t.id == d.bb, 0));
            e.3 += 1;
        }
        if let Some(ps) = d.ps {
            let e = self.programs.entry(ps.label.to_owned()).or_insert((ps.reads_position, 0));
            // A label seen both ways would be a contradiction worth knowing.
            e.0 |= ps.reads_position;
            e.1 += 1;
        }
        if d.reads_bb {
            self.reads_bb += 1;
        }
        if d.ds.is_some() {
            self.with_ds += 1;
        }
        self.blends.insert(d.blend, ());
        if outside(d.viewport, d.scissor, d.bb_width(), d.bb_height()) {
            self.out_of_bounds += 1;
        }
    }

    /// The frame was presented: what it was, and the next one starts.
    pub fn present(&mut self, bb: u32, size: Option<(u32, u32)>) -> Frame {
        let (w, h) = size.unwrap_or((0, 0));
        let mut frame = Frame {
            frame: self.frame,
            bb,
            bb_width: w,
            bb_height: h,
            scene_done: self.scene_done,
            draws_scene: self.draws_scene,
            draws_hud: self.draws_hud,
            hud_pass_switches: self.switches,
            hud_programs: self
                .programs
                .iter()
                .map(|(label, (pos, n))| HudProgram { label: label.clone(), reads_position: *pos, draws: *n })
                .collect(),
            hud_targets: self
                .targets
                .iter()
                .map(|(id, (w, h, bb, n))| HudTarget { id: *id, width: *w, height: *h, bb: *bb, draws: *n })
                .collect(),
            hud_reads_bb: self.reads_bb,
            hud_with_ds: self.with_ds,
            hud_blends: self.blends.keys().copied().collect(),
            hud_out_of_bounds: self.out_of_bounds,
            violations: Vec::new(),
        };
        check(&mut frame);
        *self = Recorder::default();
        self.frame = frame.frame + 1;
        frame
    }
}

impl<'a> Draw<'a> {
    /// The back buffer's size, from whichever of its targets is it.
    fn bb_width(&self) -> u32 {
        self.bb_size().0
    }
    fn bb_height(&self) -> u32 {
        self.bb_size().1
    }
    fn bb_size(&self) -> (u32, u32) {
        self.targets
            .iter()
            .find(|t| t.id == self.bb)
            .map_or((0, 0), |t| (t.width, t.height))
    }
}

/// Whether a viewport or scissor reaches outside the back buffer it draws
/// into. A draw into another target is not the HUD's business here, so only
/// draws into the back buffer are looked at; a viewport is
/// `(x, y, w, h, min_depth, max_depth)`, a scissor `(left, top, right,
/// bottom)`, both in the target's pixels.
fn outside(viewport: Option<[f32; 6]>, scissor: Option<[u32; 4]>, width: u32, height: u32) -> bool {
    if width == 0 || height == 0 {
        return false;
    }
    let (w, h) = (width as f32, height as f32);
    if let Some(v) = viewport
        && (v[0] < -0.5 || v[1] < -0.5 || v[0] + v[2] > w + 0.5 || v[1] + v[3] > h + 0.5)
    {
        return true;
    }
    if let Some(s) = scissor
        && (s[2] as f32 > w + 0.5 || s[3] as f32 > h + 0.5)
    {
        return true;
    }
    false
}

/// What a frame must satisfy for the HUD phase to be redirectable, so a
/// change that breaks one of them says so instead of drawing wrongly.
///
/// Not checked here, because they need more than one frame or more than the
/// commands: that `SCENE_DONE` comes only in gameplay (needs the game's own
/// state), and that the shaders reading `SV_Position` are all rescaled (needs
/// the rescale itself).
fn check(frame: &mut Frame) {
    if frame.scene_done {
        if frame.draws_hud == 0 {
            frame.violations.push("SCENE_DONE without a HUD phase".to_owned());
        }
        // Everything after the scene is HUD, so it draws into the back buffer
        // or into a HUD target of its own; a scene-sized target there means
        // the phase's premise is wrong.
        for t in &frame.hud_targets {
            if !t.bb && t.width == frame.bb_width && t.height == frame.bb_height {
                frame.violations.push(format!("HUD draw into a scene-sized target {}", t.id));
            }
        }
        if frame.hud_out_of_bounds > 0 {
            frame.violations.push(format!("{} HUD draws outside the back buffer", frame.hud_out_of_bounds));
        }
    } else if frame.draws_scene == 0 {
        frame.violations.push("a frame without draws".to_owned());
    }
}

/// Whether the census is on. Two relaxed loads once `GLUE5_CENSUS` has been
/// read, one before that; the common case (off) never takes the lock, so the
/// callers can be left in the draw path unconditionally.
#[inline]
pub fn on() -> bool {
    if ON.load(Ordering::Relaxed) {
        return true;
    }
    if READ.load(Ordering::Relaxed) {
        return false;
    }
    read_env();
    ON.load(Ordering::Relaxed)
}

/// Reads `GLUE5_CENSUS` once, on the first call: a path to append one JSON
/// line per frame to, `log` for the log, anything else (or nothing) leaves the
/// census off.
#[cold]
fn read_env() {
    let mut st = LIVE.lock().unwrap_or_else(|e| e.into_inner());
    if st.read_env {
        return;
    }
    st.read_env = true;
    READ.store(true, Ordering::Relaxed);
    let want = std::env::var("GLUE5_CENSUS").unwrap_or_default();
    let want = want.trim();
    if want.is_empty() {
        return;
    }
    let sink = if want.eq_ignore_ascii_case("log") {
        Sink::Log
    } else {
        match OpenOptions::new().create(true).append(true).open(want) {
            Ok(file) => Sink::File(file),
            Err(e) => {
                log::error!("census: cannot write {want}: {e}");
                return;
            }
        }
    };
    st.sink = Some(sink);
    ON.store(true, Ordering::Relaxed);
    log::info!("census: recording the HUD phase of every frame");
}

fn state() -> MutexGuard<'static, State> {
    LIVE.lock().unwrap_or_else(|e| e.into_inner())
}

/// The scene is finished; the HUD follows.
pub fn scene_done() {
    if !on() {
        return;
    }
    state().recorder.scene_done();
}

/// A draw, when the census is on.
pub fn draw(d: &Draw<'_>) {
    if !on() {
        return;
    }
    state().recorder.draw(d);
}

/// The frame was presented: its line is written, and the next frame starts.
pub fn present(bb: u32, size: Option<(u32, u32)>) {
    if !on() {
        return;
    }
    let mut st = state();
    let frame = st.recorder.present(bb, size);
    let line = frame.to_json();
    match &mut st.sink {
        Some(Sink::Log) => log::info!("census: {line}"),
        Some(Sink::File(file)) => {
            if let Err(e) = writeln!(file, "{line}") {
                log::error!("census: {e}");
            }
        }
        None => {}
    }
}

/// The frames recorded so far, for an overlay to show (nothing is kept: each
/// frame's line is written and forgotten).
#[allow(dead_code)] // The overlay that shows it comes with the redirect itself.
pub fn frames() -> u64 {
    state().recorder.frame
}

#[cfg(test)]
mod tests {
    use super::*;

    const BB: u32 = 7;
    const SCENE: u32 = 3;

    fn bb_target() -> Target {
        Target { id: BB, width: 1280, height: 720, format: 0 }
    }

    fn draw(ps: Option<Ps<'static>>, reads_bb: bool) -> Draw<'static> {
        Draw {
            bb: BB,
            targets: &[],
            ds: None,
            viewport: None,
            scissor: None,
            vs: None,
            ps,
            reads_bb,
            blend: 1,
        }
    }

    /// A frame with a scene and a HUD phase, as the commands describe them.
    #[test]
    fn a_hud_phase_is_counted_apart_from_the_scene() {
        let mut r = Recorder::default();
        let mut d = draw(Some(Ps { label: "im::PS_Blit", reads_position: false }), false);
        d.targets = &[Target { id: SCENE, width: 1280, height: 720, format: 0 }];
        r.draw(&d);
        r.draw(&d);
        r.scene_done();
        d.targets = &[bb_target()];
        r.draw(&d);
        let frame = r.present(BB, Some((1280, 720)));

        assert!(frame.scene_done);
        assert_eq!(frame.draws_scene, 2);
        assert_eq!(frame.draws_hud, 1);
        assert_eq!(frame.hud_targets, vec![HudTarget { id: BB, width: 1280, height: 720, bb: true, draws: 1 }]);
        assert!(frame.violations.is_empty(), "{:?}", frame.violations);
    }

    /// A frame without `SCENE_DONE` (a menu over everything) has no HUD phase
    /// to redirect: that is the whole-frame bicubic path.
    #[test]
    fn a_frame_without_the_marker_is_all_scene() {
        let mut r = Recorder::default();
        let mut d = draw(None, false);
        d.targets = &[bb_target()];
        r.draw(&d);
        let frame = r.present(BB, Some((1280, 720)));

        assert!(!frame.scene_done);
        assert_eq!(frame.draws_scene, 1);
        assert_eq!(frame.draws_hud, 0);
        assert!(frame.hud_targets.is_empty());
        assert!(frame.violations.is_empty(), "{:?}", frame.violations);
    }

    /// A pixel program that reads D3D's `SV_Position` is what a redirected
    /// draw would compute differently, so the frame names it.
    #[test]
    fn a_program_reading_the_position_is_named() {
        let mut r = Recorder::default();
        r.scene_done();
        let mut d = draw(Some(Ps { label: "im::PS_Dither", reads_position: true }), false);
        d.targets = &[bb_target()];
        r.draw(&d);
        r.draw(&d);
        d.ps = Some(Ps { label: "im::PS_Blit", reads_position: false });
        r.draw(&d);
        let frame = r.present(BB, Some((1280, 720)));

        assert_eq!(
            frame.hud_programs,
            vec![
                HudProgram { label: "im::PS_Blit".to_owned(), reads_position: false, draws: 1 },
                HudProgram { label: "im::PS_Dither".to_owned(), reads_position: true, draws: 2 },
            ]
        );
    }

    /// Every change of target set in the HUD phase is a render pass the GPU
    /// begins again, which on a tile-based one reloads its tiles.
    #[test]
    fn alternating_targets_are_counted_as_pass_switches() {
        let mut r = Recorder::default();
        r.scene_done();
        let mask = Target { id: 11, width: 256, height: 256, format: 0 };
        let mut d = draw(None, false);
        d.targets = &[bb_target()];
        r.draw(&d);
        d.targets = &[mask];
        r.draw(&d);
        d.targets = &[bb_target()];
        r.draw(&d);
        let frame = r.present(BB, Some((1280, 720)));

        assert_eq!(frame.hud_pass_switches, 2);
        assert_eq!(frame.hud_targets.len(), 2);
        // The mask is a HUD target of its own, not a scene-sized one.
        assert!(frame.violations.is_empty(), "{:?}", frame.violations);
    }

    /// A HUD draw writing into a scene-sized target that is not the back
    /// buffer means "everything after the marker is HUD" does not hold.
    #[test]
    fn a_scene_sized_hud_target_is_a_violation() {
        let mut r = Recorder::default();
        r.scene_done();
        let mut d = draw(None, false);
        d.targets = &[Target { id: SCENE, width: 1280, height: 720, format: 0 }];
        r.draw(&d);
        let frame = r.present(BB, Some((1280, 720)));

        assert!(frame.violations.iter().any(|v| v.contains("scene-sized")), "{:?}", frame.violations);
    }

    /// A depth-stencil attached to a HUD draw is what a redirected draw would
    /// have to match in size, or wgpu rejects the pass.
    #[test]
    fn a_hud_draw_with_depth_is_counted() {
        let mut r = Recorder::default();
        r.scene_done();
        let mut d = draw(None, false);
        d.targets = &[bb_target()];
        d.ds = Some(Target { id: 12, width: 1280, height: 720, format: 1 });
        r.draw(&d);
        let frame = r.present(BB, Some((1280, 720)));

        assert_eq!(frame.hud_with_ds, 1);
    }

    /// A HUD draw reading the back buffer through an SRV (a blur behind a
    /// menu) would read the redirected target instead, so it is counted.
    #[test]
    fn a_hud_draw_reading_the_back_buffer_is_counted() {
        let mut r = Recorder::default();
        r.scene_done();
        let mut d = draw(None, true);
        d.targets = &[bb_target()];
        r.draw(&d);
        let frame = r.present(BB, Some((1280, 720)));

        assert_eq!(frame.hud_reads_bb, 1);
    }

    #[test]
    fn a_viewport_outside_the_back_buffer_is_a_violation() {
        let mut r = Recorder::default();
        r.scene_done();
        let mut d = draw(None, false);
        d.targets = &[bb_target()];
        d.viewport = Some([0.0, 0.0, 2560.0, 720.0, 0.0, 1.0]);
        r.draw(&d);
        let frame = r.present(BB, Some((1280, 720)));

        assert_eq!(frame.hud_out_of_bounds, 1);
        assert!(frame.violations.iter().any(|v| v.contains("outside")), "{:?}", frame.violations);
    }

    /// A viewport inside the back buffer, however close to its edges, is not.
    #[test]
    fn a_viewport_inside_the_back_buffer_is_not() {
        assert!(!outside(Some([0.0, 0.0, 1280.0, 720.0, 0.0, 1.0]), None, 1280, 720));
        assert!(!outside(Some([10.0, 10.0, 1270.0, 710.0, 0.0, 1.0]), None, 1280, 720));
        assert!(!outside(None, Some([0, 0, 1280, 720]), 1280, 720));
        assert!(outside(None, Some([0, 0, 1281, 720]), 1280, 720));
        // Without a known size there is nothing to be outside of.
        assert!(!outside(Some([0.0, 0.0, 9999.0, 9999.0, 0.0, 1.0]), None, 0, 0));
    }

    /// Frames are numbered from the frames recorded, and each starts clean.
    #[test]
    fn frames_are_numbered_and_start_empty() {
        let mut r = Recorder::default();
        let mut d = draw(None, false);
        d.targets = &[bb_target()];
        r.scene_done();
        r.draw(&d);
        let first = r.present(BB, Some((1280, 720)));
        assert_eq!(first.frame, 0);
        assert_eq!(first.draws_hud, 1);

        // The next frame has no HUD phase of its own until the marker comes.
        r.draw(&d);
        let second = r.present(BB, Some((1280, 720)));
        assert_eq!(second.frame, 1);
        assert!(!second.scene_done);
        assert_eq!(second.draws_hud, 0);
        assert_eq!(second.draws_scene, 1);
    }

    /// The line a frame becomes, for the file or the log.
    #[test]
    fn a_frame_becomes_one_json_line() {
        let mut r = Recorder::default();
        r.scene_done();
        let mut d = draw(Some(Ps { label: "im::PS_Blit", reads_position: true }), true);
        d.targets = &[bb_target()];
        r.draw(&d);
        let line = r.present(BB, Some((1280, 720))).to_json();

        assert!(!line.contains('\n'), "one line per frame: {line}");
        let v: serde_json::Value = serde_json::from_str(&line).unwrap();
        assert_eq!(v["bb"], BB);
        assert_eq!(v["scene_done"], true);
        assert_eq!(v["hud_reads_bb"], 1);
        assert_eq!(v["hud_programs"][0]["label"], "im::PS_Blit");
        assert_eq!(v["hud_programs"][0]["reads_position"], true);
        assert_eq!(v["hud_targets"][0]["back_buffer"], true);
    }
}
