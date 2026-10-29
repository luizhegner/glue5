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
//! bigger one changes what the program computes — and how often the target
//! set changes, an indicator of attachment/pass boundaries, not a bandwidth
//! measurement or proof of a physical pass end at `SCENE_DONE`.
//!
//! This module measures that instead of guessing. It is **passive**: it reads
//! what the commands already say and changes nothing about rendering, so a
//! frame is recorded exactly as it would have been drawn.
//!
//! Aggregates per frame rather than per draw and keeps only a small bounded
//! tail of frame JSON (plus the latest violating frames) for the support
//! report. That keeps the cost and the output small enough to leave on for a
//! whole session on a phone.
//!
//! Turn it on with `GLUE5_CENSUS`: a path appends one JSON line per frame
//! there, `log` writes the same lines to the log. Off (the default) costs one
//! relaxed atomic load per draw.

use std::collections::{BTreeMap, BTreeSet, VecDeque};
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

/// Recording now.
static ON: AtomicBool = AtomicBool::new(false);
/// `GLUE5_CENSUS` asked for it, which the application's own on/off does not
/// override: a run can then be recorded to a file without the overlay, and
/// turning the overlay off cannot silently stop it.
static ENV: AtomicBool = AtomicBool::new(false);
/// `GLUE5_CENSUS` has been read, so `ENV` will not change again.
static READ: AtomicBool = AtomicBool::new(false);
static LIVE: LazyLock<Mutex<State>> = LazyLock::new(|| Mutex::new(State::default()));

#[derive(Default)]
struct State {
    recorder: Recorder,
    /// What the frames recorded since the census came on add up to.
    agg: Agg,
    /// Bounded frame traces for a support report, independent of the summary
    /// queried by the overlay every 250 ms.
    recent_frames: VecDeque<String>,
    incident_frames: VecDeque<String>,
    /// Last Full-overlay session, retained after the overlay stops so the
    /// support report can be copied from Settings after leaving the game.
    last_diagnostics: Option<DiagnosticsSnapshot>,
    sink: Option<Sink>,
    /// Set once the environment has been read.
    read_env: bool,
}

/// A render target a draw writes, as the command named it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Target {
    pub id: u32,
    /// The dimensions of the attached mip, not the texture's base mip.
    pub width: u32,
    pub height: u32,
    /// The engine's format code (see [`crate::format`]).
    pub format: u32,
    pub mip: u32,
    pub slice: u32,
    pub samples: u32,
}

/// A draw's pixel program, and what matters of it here.
#[derive(Clone, Copy, Debug)]
pub struct Ps<'a> {
    /// Stable within a device lifetime; labels are resolved once per program,
    /// not cloned and ordered by name on every draw.
    pub id: u32,
    pub label: &'a str,
    /// It reads D3D's `SV_Position` (WGSL's `@builtin(position)` as a fragment
    /// input), so its result depends on the target's size in pixels.
    pub reads_position: bool,
}

/// A graphics-stage SRV view read by the draw.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Read {
    pub id: u32,
    pub first_mip: u32,
    pub mip_count: u32,
    pub first_slice: u32,
    pub slice_count: u32,
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
    /// Graphics-stage shader-resource views read by this draw.
    pub reads: &'a [Read],
    /// It reads the back buffer through a shader resource view.
    pub reads_bb: bool,
    /// Its blend state's id.
    pub blend: u32,
}

/// A pixel program the HUD phase drew with.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HudProgram {
    pub id: u32,
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
    pub format: u32,
    pub mip: u32,
    pub slice: u32,
    pub samples: u32,
    /// It is the back buffer.
    pub bb: bool,
    pub draws: u32,
    pub first_draw: u32,
    pub last_draw: u32,
    /// It was also written before the logical `SCENE_DONE` marker.
    pub written_before_scene_done: bool,
    /// HUD draws writing the back buffer that read this texture.
    pub reads_into_bb_draws: u32,
    /// Pixel programs that wrote this target during the HUD phase.
    pub pixel_program_ids: Vec<u32>,
    pub pixel_programs: Vec<String>,
}

/// A HUD target with the scene back buffer's dimensions, which breaks the
/// redirect premise and needs to be explained before any redirect is tried.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SceneSizedTargetIssue {
    pub id: u32,
    pub width: u32,
    pub height: u32,
    pub format: u32,
    pub mip: u32,
    pub slice: u32,
    pub samples: u32,
    pub frames: u64,
    pub draws: u64,
    pub first_draw: u32,
    pub last_draw: u32,
    pub written_before_scene_done_frames: u64,
    pub reads_into_bb_draws: u64,
    pub pixel_program_ids: Vec<u32>,
    pub pixel_programs: Vec<String>,
}

/// A target written before `SCENE_DONE` and later sampled by a HUD draw that
/// writes the back buffer. This is evidence of resource flow, not proof that
/// the physical render pass ended at the marker.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SceneTargetRead {
    pub id: u32,
    pub width: u32,
    pub height: u32,
    pub format: u32,
    pub mip: u32,
    pub slice: u32,
    pub samples: u32,
    pub scene_writes: u32,
    pub bb_read_draws: u32,
    pub bb_reader_draw_indices: Vec<u32>,
    pub scene_pixel_program_ids: Vec<u32>,
    pub scene_pixel_programs: Vec<String>,
    pub bb_reader_program_ids: Vec<u32>,
    pub bb_reader_programs: Vec<String>,
}

/// A scene subresource sampled by HUD draws that write the back buffer,
/// aggregated over the frames shown in the performance overlay.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SceneTargetReadSummary {
    pub id: u32,
    pub width: u32,
    pub height: u32,
    pub format: u32,
    pub mip: u32,
    pub slice: u32,
    pub samples: u32,
    pub frames: u64,
    pub bb_read_draws: u64,
    pub bb_reader_program_ids: Vec<u32>,
    pub bb_reader_programs: Vec<String>,
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
    /// Times the HUD phase's target set changed. This is not the number of
    /// draws writing the back buffer, nor proof the pass ended at the marker.
    pub hud_target_set_changes: u32,
    /// HUD target-set segments that attach the back buffer. `SCENE_DONE` can
    /// split a logical phase inside a still-open physical render pass.
    pub hud_bb_passes: u32,
    pub hud_programs: Vec<HudProgram>,
    pub hud_targets: Vec<HudTarget>,
    /// Scene targets that HUD draws writing the back buffer subsequently read.
    pub scene_targets_read_by_hud: Vec<SceneTargetRead>,
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
            .map(|p| serde_json::json!({ "id": p.id, "label": p.label, "reads_position": p.reads_position, "draws": p.draws }))
            .collect();
        let scene_targets: Vec<_> = self
            .scene_targets_read_by_hud
            .iter()
            .map(|t| serde_json::json!({
                "id": t.id, "width": t.width, "height": t.height,
                "format": t.format, "mip": t.mip, "slice": t.slice,
                "samples": t.samples, "scene_writes": t.scene_writes,
                "bb_read_draws": t.bb_read_draws,
                "bb_reader_draw_indices": t.bb_reader_draw_indices,
                "scene_pixel_program_ids": t.scene_pixel_program_ids,
                "scene_pixel_programs": t.scene_pixel_programs,
                "bb_reader_program_ids": t.bb_reader_program_ids,
                "bb_reader_programs": t.bb_reader_programs,
            }))
            .collect();
        let scene_sized_targets: Vec<_> = self
            .hud_targets
            .iter()
            .filter(|t| !t.bb && t.width == self.bb_width && t.height == self.bb_height)
            .map(|t| serde_json::json!({
                "id": t.id, "width": t.width, "height": t.height,
                "format": t.format, "mip": t.mip, "slice": t.slice,
                "samples": t.samples, "draws": t.draws,
                "first_draw": t.first_draw, "last_draw": t.last_draw,
                "written_before_scene_done": t.written_before_scene_done,
                "reads_into_bb_draws": t.reads_into_bb_draws,
                "pixel_program_ids": t.pixel_program_ids,
                "pixel_programs": t.pixel_programs,
            }))
            .collect();
        let targets: Vec<_> = self
            .hud_targets
            .iter()
            .map(|t| {
                serde_json::json!({
                    "id": t.id, "width": t.width, "height": t.height,
                    "format": t.format, "mip": t.mip, "slice": t.slice,
                    "samples": t.samples, "back_buffer": t.bb, "draws": t.draws,
                    "first_draw": t.first_draw, "last_draw": t.last_draw,
                    "written_before_scene_done": t.written_before_scene_done,
                    "reads_into_bb_draws": t.reads_into_bb_draws,
                    "pixel_program_ids": t.pixel_program_ids,
                    "pixel_programs": t.pixel_programs,
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
            "hud_target_set_changes": self.hud_target_set_changes,
            "hud_bb_passes": self.hud_bb_passes,
            "hud_programs": programs,
            "hud_targets": targets,
            "scene_sized_targets": scene_sized_targets,
            "scene_targets_read_by_hud": scene_targets,
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
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct TargetKey {
    id: u32,
    mip: u32,
    slice: u32,
    format: u32,
}

impl From<Target> for TargetKey {
    fn from(t: Target) -> Self {
        Self { id: t.id, mip: t.mip, slice: t.slice, format: t.format }
    }
}

#[derive(Default)]
struct SceneTargetRecord {
    target: Option<Target>,
    writes: u32,
    pixel_programs: BTreeSet<u32>,
    bb_read_draws: u32,
    last_bb_read_draw: u32,
    bb_reader_draw_indices: BTreeSet<u32>,
    bb_readers: BTreeSet<u32>,
}

#[derive(Default)]
struct TargetRecord {
    target: Option<Target>,
    bb: bool,
    draws: u32,
    first_draw: u32,
    last_draw: u32,
    pixel_programs: BTreeSet<u32>,
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
    target_set_changes: u32,
    bb_passes: u32,
    /// The HUD phase's last target set, to count its changes.
    last: Option<Vec<TargetKey>>,
    /// Cache labels by stable shader-program id; no per-draw label clone.
    programs: BTreeMap<u32, (String, bool)>,
    /// HUD-only usage counts; scene shaders do not pollute HUD diagnostics.
    hud_programs: BTreeMap<u32, (bool, u32)>,
    targets: BTreeMap<TargetKey, TargetRecord>,
    /// Targets written before the logical marker. This is not proof that the
    /// GPU render pass ended at `SCENE_DONE`.
    scene_target_keys: BTreeSet<TargetKey>,
    scene_target_ids: BTreeSet<u32>,
    scene_targets: BTreeMap<TargetKey, SceneTargetRecord>,
    /// BB-writing HUD draws that read an earlier scene target, by subresource.
    reads_scene_into_bb: BTreeMap<TargetKey, u32>,
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
            for t in d.targets.iter().chain(d.ds.iter()) {
                let key = TargetKey::from(*t);
                self.scene_target_keys.insert(key);
                self.scene_target_ids.insert(t.id);
                let record = self.scene_targets.entry(key).or_default();
                record.target = Some(*t);
                record.writes += 1;
                if let Some(ps) = d.ps {
                    record.pixel_programs.insert(ps.id);
                    let program = self.programs.entry(ps.id).or_insert_with(|| (ps.label.to_owned(), ps.reads_position));
                    program.1 |= ps.reads_position;
                }
            }
            return;
        }
        self.draws_hud += 1;
        let draw_index = self.draws_hud;

        // A change of attachment set is a render-target switch. `SCENE_DONE`
        // is only a logical boundary: this does not assert a physical pass end.
        let keys: Vec<TargetKey> = d.targets.iter().map(|t| TargetKey::from(*t)).collect();
        if self.last.as_ref() != Some(&keys) {
            if self.last.is_some() {
                self.target_set_changes += 1;
            }
            if d.targets.iter().any(|t| t.id == d.bb) {
                self.bb_passes += 1;
            }
            self.last = Some(keys);
        }

        if let Some(ps) = d.ps {
            let e = self.programs.entry(ps.id).or_insert_with(|| (ps.label.to_owned(), ps.reads_position));
            e.1 |= ps.reads_position;
            let hud = self.hud_programs.entry(ps.id).or_insert((ps.reads_position, 0));
            hud.0 |= ps.reads_position;
            hud.1 += 1;
        }
        for t in d.targets {
            let key = TargetKey::from(*t);
            let record = self.targets.entry(key).or_default();
            record.target = Some(*t);
            record.bb |= t.id == d.bb;
            record.draws += 1;
            if record.first_draw == 0 {
                record.first_draw = draw_index;
            }
            record.last_draw = draw_index;
            if let Some(ps) = d.ps {
                record.pixel_programs.insert(ps.id);
            }
        }
        // A resource can be bound in multiple SRV slots; the draw-site
        // collector deduplicates identical views. Overlapping views still map
        // to each matching scene subresource only once per draw.
        if d.targets.iter().any(|t| t.id == d.bb) {
            for read in d.reads {
                if self.scene_target_ids.contains(&read.id) {
                    for (key, record) in self.scene_targets.iter_mut().filter(|(key, _)| {
                        key.id == read.id
                            && key.mip >= read.first_mip
                            && key.mip.saturating_sub(read.first_mip) < read.mip_count.max(1)
                            && key.slice >= read.first_slice
                            && key.slice.saturating_sub(read.first_slice) < read.slice_count.max(1)
                    }) {
                        if record.last_bb_read_draw != draw_index {
                            *self.reads_scene_into_bb.entry(*key).or_default() += 1;
                            record.bb_read_draws += 1;
                            record.last_bb_read_draw = draw_index;
                            record.bb_reader_draw_indices.insert(draw_index);
                            if let Some(ps) = d.ps {
                                record.bb_readers.insert(ps.id);
                            }
                        }
                    }
                }
            }
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
        let hud_programs: Vec<_> = self
            .hud_programs
            .iter()
            .filter_map(|(&id, (pos, n))| self.programs.get(&id).map(|(label, _)| HudProgram {
                id,
                label: label.clone(),
                reads_position: *pos,
                draws: *n,
            }))
            .collect();
        let program_labels: BTreeMap<_, _> = self.programs.iter().map(|(&id, (label, _))| (id, label.as_str())).collect();
        let scene_targets_read_by_hud = self
            .scene_targets
            .iter()
            .filter_map(|(_key, record)| {
                let t = record.target?;
                (record.bb_read_draws > 0).then(|| SceneTargetRead {
                    id: t.id,
                    width: t.width,
                    height: t.height,
                    format: t.format,
                    mip: t.mip,
                    slice: t.slice,
                    samples: t.samples,
                    scene_writes: record.writes,
                    bb_read_draws: record.bb_read_draws,
                    bb_reader_draw_indices: record.bb_reader_draw_indices.iter().copied().collect(),
                    scene_pixel_program_ids: record.pixel_programs.iter().copied().collect(),
                    scene_pixel_programs: record.pixel_programs.iter().filter_map(|id| program_labels.get(id).map(|s| (*s).to_owned())).collect(),
                    bb_reader_program_ids: record.bb_readers.iter().copied().collect(),
                    bb_reader_programs: record.bb_readers.iter().filter_map(|id| program_labels.get(id).map(|s| (*s).to_owned())).collect(),
                })
            })
            .collect();
        let hud_targets = self
            .targets
            .iter()
            .filter_map(|(key, record)| {
                let t = record.target?;
                Some(HudTarget {
                    id: t.id,
                    width: t.width,
                    height: t.height,
                    format: t.format,
                    mip: t.mip,
                    slice: t.slice,
                    samples: t.samples,
                    bb: record.bb,
                    draws: record.draws,
                    first_draw: record.first_draw,
                    last_draw: record.last_draw,
                    written_before_scene_done: self.scene_target_keys.contains(key),
                    reads_into_bb_draws: *self.reads_scene_into_bb.get(key).unwrap_or(&0),
                    pixel_program_ids: record.pixel_programs.iter().copied().collect(),
                    pixel_programs: record
                        .pixel_programs
                        .iter()
                        .filter_map(|id| program_labels.get(id).map(|s| (*s).to_owned()))
                        .collect(),
                })
            })
            .collect();
        let mut frame = Frame {
            frame: self.frame,
            bb,
            bb_width: w,
            bb_height: h,
            scene_done: self.scene_done,
            draws_scene: self.draws_scene,
            draws_hud: self.draws_hud,
            hud_target_set_changes: self.target_set_changes,
            hud_bb_passes: self.bb_passes,
            hud_programs,
            hud_targets,
            scene_targets_read_by_hud,
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
                frame.violations.push(format!(
                    "HUD draw into a scene-sized target {} via {}",
                    t.id,
                    if t.pixel_programs.is_empty() {
                        "unknown".to_owned()
                    } else {
                        t.pixel_programs.join(",")
                    },
                ));
            }
        }
        if frame.hud_out_of_bounds > 0 {
            frame.violations.push(format!("{} HUD draws outside the back buffer", frame.hud_out_of_bounds));
        }
    } else if frame.draws_scene == 0 {
        frame.violations.push("a frame without draws".to_owned());
    }
}

/// How many distinct names a summary keeps, so a long session cannot grow it
/// without bound: past this, the count still rises and the new name does not
/// appear.
const CAP: usize = 24;

#[derive(Default)]
struct SceneTargetReadAgg {
    target: Option<Target>,
    frames: u64,
    bb_read_draws: u64,
    bb_reader_program_ids: BTreeSet<u32>,
    bb_reader_programs: BTreeSet<String>,
}

#[derive(Default)]
struct SceneSizedTargetAgg {
    target: Option<Target>,
    frames: u64,
    draws: u64,
    first_draw: u32,
    last_draw: u32,
    written_before_scene_done_frames: u64,
    reads_into_bb_draws: u64,
    pixel_program_ids: BTreeSet<u32>,
    pixel_programs: BTreeSet<String>,
}

/// What the frames recorded since the census came on add up to.
#[derive(Default)]
struct Agg {
    frames: u64,
    with_hud: u64,
    hud_draws: u64,
    hud_draws_min: u32,
    hud_draws_max: u32,
    target_set_changes: u64,
    target_set_changes_max: u32,
    bb_passes: u64,
    bb_passes_min: u32,
    bb_passes_max: u32,
    reads_position: BTreeSet<String>,
    frames_reading_bb: u64,
    frames_with_ds: u64,
    sizes: BTreeSet<(u32, u32)>,
    scene_sized_targets: BTreeMap<TargetKey, SceneSizedTargetAgg>,
    scene_targets_read_by_hud: BTreeMap<TargetKey, SceneTargetReadAgg>,
    violations: BTreeMap<String, u64>,
}

impl Agg {
    fn add(&mut self, f: &Frame) {
        self.frames += 1;
        // A frame without the marker has no HUD phase, so it says nothing
        // about one; it is only counted, as the difference is the point.
        if !f.scene_done {
            return;
        }
        self.with_hud += 1;
        self.hud_draws += f.draws_hud as u64;
        self.hud_draws_min = if self.with_hud == 1 { f.draws_hud } else { self.hud_draws_min.min(f.draws_hud) };
        self.hud_draws_max = self.hud_draws_max.max(f.draws_hud);
        self.target_set_changes += f.hud_target_set_changes as u64;
        self.target_set_changes_max = self.target_set_changes_max.max(f.hud_target_set_changes);
        self.bb_passes += f.hud_bb_passes as u64;
        self.bb_passes_min = if self.with_hud == 1 { f.hud_bb_passes } else { self.bb_passes_min.min(f.hud_bb_passes) };
        self.bb_passes_max = self.bb_passes_max.max(f.hud_bb_passes);
        for p in &f.hud_programs {
            if p.reads_position && self.reads_position.len() < CAP {
                self.reads_position.insert(p.label.clone());
            }
        }
        self.frames_reading_bb += (f.hud_reads_bb > 0) as u64;
        self.frames_with_ds += (f.hud_with_ds > 0) as u64;
        for t in &f.hud_targets {
            if self.sizes.len() < CAP {
                self.sizes.insert((t.width, t.height));
            }
        }
        for source in &f.scene_targets_read_by_hud {
            let key = TargetKey { id: source.id, mip: source.mip, slice: source.slice, format: source.format };
            if self.scene_targets_read_by_hud.len() >= CAP && !self.scene_targets_read_by_hud.contains_key(&key) {
                continue;
            }
            let read = self.scene_targets_read_by_hud.entry(key).or_default();
            read.target = Some(Target {
                id: source.id,
                width: source.width,
                height: source.height,
                format: source.format,
                mip: source.mip,
                slice: source.slice,
                samples: source.samples,
            });
            read.frames += 1;
            read.bb_read_draws += source.bb_read_draws as u64;
            for &id in &source.bb_reader_program_ids {
                if read.bb_reader_program_ids.len() < CAP {
                    read.bb_reader_program_ids.insert(id);
                }
            }
            for program in &source.bb_reader_programs {
                if read.bb_reader_programs.len() < CAP {
                    read.bb_reader_programs.insert(program.clone());
                }
            }
        }
        for t in &f.hud_targets {
            if t.bb || t.width != f.bb_width || t.height != f.bb_height {
                continue;
            }
            let key = TargetKey { id: t.id, mip: t.mip, slice: t.slice, format: t.format };
            if self.scene_sized_targets.len() >= CAP && !self.scene_sized_targets.contains_key(&key) {
                continue;
            }
            let issue = self.scene_sized_targets.entry(key).or_default();
            issue.target = Some(Target {
                id: t.id,
                width: t.width,
                height: t.height,
                format: t.format,
                mip: t.mip,
                slice: t.slice,
                samples: t.samples,
            });
            issue.frames += 1;
            issue.draws += t.draws as u64;
            issue.first_draw = if issue.frames == 1 { t.first_draw } else { issue.first_draw.min(t.first_draw) };
            issue.last_draw = issue.last_draw.max(t.last_draw);
            issue.written_before_scene_done_frames += t.written_before_scene_done as u64;
            issue.reads_into_bb_draws += t.reads_into_bb_draws as u64;
            for &id in &t.pixel_program_ids {
                if issue.pixel_program_ids.len() < CAP {
                    issue.pixel_program_ids.insert(id);
                }
            }
            for p in &t.pixel_programs {
                if issue.pixel_programs.len() < CAP {
                    issue.pixel_programs.insert(p.clone());
                }
            }
        }
        for v in &f.violations {
            // Target warnings are stored structurally above, keyed by a
            // bounded subresource id, not as an unbounded id+shader string.
            if v.starts_with("HUD draw into a scene-sized target ") {
                continue;
            }
            *self.violations.entry(v.clone()).or_insert(0) += 1;
        }
    }
}

/// What the frames since the census came on add up to, for the overlay.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Summary {
    /// Frames recorded, and of them how many had a HUD phase at all
    /// (`SCENE_DONE` came). The rest are a menu over everything, which is the
    /// whole-frame bicubic path and has nothing to redirect.
    pub frames: u64,
    pub with_hud: u64,
    /// HUD draws per frame.
    pub hud_draws_min: u32,
    pub hud_draws_max: u32,
    pub hud_draws_mean: f32,
    /// Maximum/mean target-set changes per HUD frame. These are not a count
    /// of BB-writing passes and are not a bandwidth measurement.
    pub target_set_changes_max: u32,
    pub target_set_changes_mean: f32,
    /// HUD target-set segments attaching the back buffer, not bandwidth.
    pub bb_passes_min: u32,
    pub bb_passes_max: u32,
    pub bb_passes_mean: f32,
    /// Pixel programs the HUD phase drew with that read D3D's `SV_Position`,
    /// so a draw redirected to a differently sized target would compute
    /// something else. Empty is what makes the redirect mostly mechanical.
    pub reads_position: Vec<String>,
    /// Frames where a HUD draw read the back buffer through an SRV (a blur
    /// behind a menu), and where one had a depth-stencil attached.
    pub frames_reading_bb: u64,
    pub frames_with_ds: u64,
    /// Distinct sizes the HUD phase drew into: the back buffer's, and any HUD
    /// target of its own (the minimap's mask).
    pub sizes: Vec<(u32, u32)>,
    /// Scene-sized attachments, with enough metadata to explain `BROKE`.
    pub scene_sized_targets: Vec<SceneSizedTargetIssue>,
    /// Scene targets that BB-writing HUD draws sampled.
    pub scene_targets_read_by_hud: Vec<SceneTargetReadSummary>,
    /// Other violations, and in how many frames.
    pub violations: Vec<(String, u64)>,
}

/// A shareable bounded snapshot: aggregate census, recent frames, and the
/// most recent frames that contained a census violation.
#[derive(Clone, Debug, Default)]
pub struct DiagnosticsSnapshot {
    pub summary: Summary,
    /// Recent per-frame JSON lines, oldest first.
    pub recent_frames: Vec<String>,
    /// Recent per-frame JSON lines containing one or more violations.
    pub incident_frames: Vec<String>,
}

/// What has been recorded since the census came on.
pub fn summary() -> Summary {
    let st = state();
    summarize(&st.agg)
}

/// The active census, or the last completed Full-overlay session for a
/// support report copied after leaving the game.
pub fn diagnostics_snapshot() -> Option<DiagnosticsSnapshot> {
    let st = state();
    if ON.load(Ordering::Relaxed) {
        Some(snapshot(&st))
    } else {
        st.last_diagnostics.clone()
    }
}

/// Aggregate-only form for consumers that do not need the bounded frame log.
pub fn diagnostics_summary() -> Option<Summary> {
    diagnostics_snapshot().map(|snapshot| snapshot.summary)
}

fn snapshot(st: &State) -> DiagnosticsSnapshot {
    DiagnosticsSnapshot {
        summary: summarize(&st.agg),
        recent_frames: st.recent_frames.iter().cloned().collect(),
        incident_frames: st.incident_frames.iter().cloned().collect(),
    }
}

fn summarize(a: &Agg) -> Summary {
    let n = a.with_hud.max(1) as f32;
    Summary {
        frames: a.frames,
        with_hud: a.with_hud,
        hud_draws_min: a.hud_draws_min,
        hud_draws_max: a.hud_draws_max,
        hud_draws_mean: a.hud_draws as f32 / n,
        target_set_changes_max: a.target_set_changes_max,
        target_set_changes_mean: a.target_set_changes as f32 / n,
        bb_passes_min: a.bb_passes_min,
        bb_passes_max: a.bb_passes_max,
        bb_passes_mean: a.bb_passes as f32 / n,
        reads_position: a.reads_position.iter().cloned().collect(),
        frames_reading_bb: a.frames_reading_bb,
        frames_with_ds: a.frames_with_ds,
        sizes: a.sizes.iter().copied().collect(),
        scene_sized_targets: a.scene_sized_targets.iter().filter_map(|(_, issue)| {
            let t = issue.target?;
            Some(SceneSizedTargetIssue {
                id: t.id,
                width: t.width,
                height: t.height,
                format: t.format,
                mip: t.mip,
                slice: t.slice,
                samples: t.samples,
                frames: issue.frames,
                draws: issue.draws,
                first_draw: issue.first_draw,
                last_draw: issue.last_draw,
                written_before_scene_done_frames: issue.written_before_scene_done_frames,
                reads_into_bb_draws: issue.reads_into_bb_draws,
                pixel_program_ids: issue.pixel_program_ids.iter().copied().collect(),
                pixel_programs: issue.pixel_programs.iter().cloned().collect(),
            })
        }).collect(),
        scene_targets_read_by_hud: a.scene_targets_read_by_hud.iter().filter_map(|(_, read)| {
            let t = read.target?;
            Some(SceneTargetReadSummary {
                id: t.id,
                width: t.width,
                height: t.height,
                format: t.format,
                mip: t.mip,
                slice: t.slice,
                samples: t.samples,
                frames: read.frames,
                bb_read_draws: read.bb_read_draws,
                bb_reader_program_ids: read.bb_reader_program_ids.iter().copied().collect(),
                bb_reader_programs: read.bb_reader_programs.iter().cloned().collect(),
            })
        }).collect(),
        violations: a.violations.iter().map(|(v, n)| (v.clone(), *n)).collect(),
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

/// Turns the census on or off, as the application does with its performance
/// overlay (the census is cheap enough to leave on while it is shown, and its
/// summary is part of what the overlay is for). `GLUE5_CENSUS` wins either way.
pub fn set_on(on: bool) {
    if !READ.load(Ordering::Relaxed) {
        read_env();
    }
    if ENV.load(Ordering::Relaxed) {
        return;
    }
    // Keep a snapshot for Import diagnostics after the game closes. The next
    // Full-overlay session starts fresh, matching the overlay's live figures.
    let was_on = ON.swap(on, Ordering::Relaxed);
    if was_on == on {
        return;
    }
    let mut st = state();
    if on {
        st.recorder = Recorder::default();
        st.agg = Agg::default();
        st.recent_frames.clear();
        st.incident_frames.clear();
        st.last_diagnostics = None;
    } else {
        st.last_diagnostics = Some(snapshot(&st));
        st.recorder = Recorder::default();
        st.agg = Agg::default();
        st.recent_frames.clear();
        st.incident_frames.clear();
    }
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
    ENV.store(true, Ordering::Relaxed);
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
    draw_enabled(d);
}

/// Records a draw after the caller has checked `on()` before gathering its
/// target and SRV metadata, avoiding a second atomic load in the hot path.
pub(crate) fn draw_enabled(d: &Draw<'_>) {
    state().recorder.draw(d);
}

const REPORT_FRAME_LIMIT: usize = 12;
const REPORT_FRAME_MAX_BYTES: usize = 32 * 1024;

fn retain_frame(queue: &mut VecDeque<String>, line: &str) {
    if line.len() > REPORT_FRAME_MAX_BYTES {
        return;
    }
    if queue.len() == REPORT_FRAME_LIMIT {
        queue.pop_front();
    }
    queue.push_back(line.to_owned());
}

/// The frame was presented: its line is written, a bounded sample is kept
/// for support reports, and the next frame starts.
pub fn present(bb: u32, size: Option<(u32, u32)>) {
    if !on() {
        return;
    }
    let mut st = state();
    let frame = st.recorder.present(bb, size);
    st.agg.add(&frame);
    let line = frame.to_json();
    retain_frame(&mut st.recent_frames, &line);
    if !frame.violations.is_empty() {
        retain_frame(&mut st.incident_frames, &line);
    }
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
    const BB_TARGETS: [Target; 1] = [Target { id: BB, width: 1280, height: 720, format: 0, mip: 0, slice: 0, samples: 1 }];
    const SCENE_TARGETS: [Target; 1] = [Target { id: SCENE, width: 1280, height: 720, format: 0, mip: 0, slice: 0, samples: 1 }];
    const MASK_TARGETS: [Target; 1] = [Target { id: 11, width: 256, height: 256, format: 0, mip: 0, slice: 0, samples: 1 }];

    fn draw<'a>(ps: Option<Ps<'a>>, reads_bb: bool) -> Draw<'a> {
        Draw {
            bb: BB,
            targets: &[],
            ds: None,
            viewport: None,
            scissor: None,
            vs: None,
            ps,
            reads: &[],
            reads_bb,
            blend: 1,
        }
    }

    /// A frame with a scene and a HUD phase, as the commands describe them.
    #[test]
    fn a_hud_phase_is_counted_apart_from_the_scene() {
        let mut r = Recorder::default();
        let mut d = draw(Some(Ps { id: 1, label: "im::PS_Blit", reads_position: false }), false);
        d.targets = &SCENE_TARGETS;
        r.draw(&d);
        r.draw(&d);
        r.scene_done();
        d.targets = &BB_TARGETS;
        r.draw(&d);
        let frame = r.present(BB, Some((1280, 720)));

        assert!(frame.scene_done);
        assert_eq!(frame.draws_scene, 2);
        assert_eq!(frame.draws_hud, 1);
        assert_eq!(
            frame.hud_targets,
            vec![HudTarget {
                id: BB,
                width: 1280,
                height: 720,
                format: 0,
                mip: 0,
                slice: 0,
                samples: 1,
                bb: true,
                draws: 1,
                first_draw: 1,
                last_draw: 1,
                written_before_scene_done: false,
                reads_into_bb_draws: 0,
                pixel_program_ids: vec![1],
                pixel_programs: vec!["im::PS_Blit".to_owned()],
            }]
        );
        assert!(frame.violations.is_empty(), "{:?}", frame.violations);
    }

    /// A frame without `SCENE_DONE` (a menu over everything) has no HUD phase
    /// to redirect: that is the whole-frame bicubic path.
    #[test]
    fn a_frame_without_the_marker_is_all_scene() {
        let mut r = Recorder::default();
        let mut d = draw(None, false);
        d.targets = &BB_TARGETS;
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
        let mut d = draw(Some(Ps { id: 2, label: "im::PS_Dither", reads_position: true }), false);
        d.targets = &BB_TARGETS;
        r.draw(&d);
        r.draw(&d);
        d.ps = Some(Ps { id: 1, label: "im::PS_Blit", reads_position: false });
        r.draw(&d);
        let frame = r.present(BB, Some((1280, 720)));

        assert_eq!(
            frame.hud_programs,
            vec![
                HudProgram { id: 1, label: "im::PS_Blit".to_owned(), reads_position: false, draws: 1 },
                HudProgram { id: 2, label: "im::PS_Dither".to_owned(), reads_position: true, draws: 2 },
            ]
        );
    }

    /// Target-set changes are counted separately from target-set segments
    /// that attach the back buffer; neither is a bandwidth measurement.
    #[test]
    fn alternating_targets_are_counted_as_target_set_changes() {
        let mut r = Recorder::default();
        r.scene_done();
        let mut d = draw(None, false);
        d.targets = &BB_TARGETS;
        r.draw(&d);
        d.targets = &MASK_TARGETS;
        r.draw(&d);
        d.targets = &BB_TARGETS;
        r.draw(&d);
        let frame = r.present(BB, Some((1280, 720)));

        assert_eq!(frame.hud_target_set_changes, 2);
        assert_eq!(frame.hud_bb_passes, 2, "BB, mask, BB: two BB target-set segments");
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
        d.targets = &SCENE_TARGETS;
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
        d.targets = &BB_TARGETS;
        d.ds = Some(Target { id: 12, width: 1280, height: 720, format: 1, mip: 0, slice: 0, samples: 1 });
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
        d.targets = &BB_TARGETS;
        r.draw(&d);
        let frame = r.present(BB, Some((1280, 720)));

        assert_eq!(frame.hud_reads_bb, 1);
    }

    #[test]
    fn a_viewport_outside_the_back_buffer_is_a_violation() {
        let mut r = Recorder::default();
        r.scene_done();
        let mut d = draw(None, false);
        d.targets = &BB_TARGETS;
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
        d.targets = &BB_TARGETS;
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
        let mut d = draw(Some(Ps { id: 1, label: "im::PS_Blit", reads_position: true }), true);
        d.targets = &BB_TARGETS;
        r.draw(&d);
        let line = r.present(BB, Some((1280, 720))).to_json();

        assert!(!line.contains('\n'), "one line per frame: {line}");
        let v: serde_json::Value = serde_json::from_str(&line).unwrap();
        assert_eq!(v["bb"], BB);
        assert_eq!(v["scene_done"], true);
        assert_eq!(v["hud_reads_bb"], 1);
        assert_eq!(v["hud_programs"][0]["id"], 1);
        assert_eq!(v["hud_programs"][0]["label"], "im::PS_Blit");
        assert_eq!(v["hud_programs"][0]["reads_position"], true);
        assert_eq!(v["hud_targets"][0]["back_buffer"], true);
        assert_eq!(v["hud_targets"][0]["pixel_program_ids"][0], 1);
        assert_eq!(v["hud_targets"][0]["pixel_programs"][0], "im::PS_Blit");
    }

    /// Two frames of a HUD phase, added up: the range and the mean of its
    /// draws, and the most target-set changes in one of them, which is what a
    /// tile-based GPU (Mali, Adreno) would reload a native-sized target for.
    #[test]
    fn frames_add_up() {
        let mut r = Recorder::default();
        let mut d = draw(None, false);
        d.targets = &BB_TARGETS;

        r.scene_done();
        r.draw(&d);
        r.draw(&d);
        let first = r.present(BB, Some((1280, 720)));

        r.scene_done();
        let mut e = draw(None, false);
        e.targets = &MASK_TARGETS;
        r.draw(&e);
        r.draw(&d);
        r.draw(&d);
        r.draw(&d);
        let second = r.present(BB, Some((1280, 720)));

        let mut agg = Agg::default();
        agg.add(&first);
        agg.add(&second);
        assert_eq!(agg.frames, 2);
        assert_eq!(agg.with_hud, 2);
        assert_eq!(agg.hud_draws_min, 2);
        assert_eq!(agg.hud_draws_max, 4);
        assert_eq!(agg.target_set_changes_max, 1, "the mask-to-back-buffer target change");
        assert_eq!(agg.sizes.iter().copied().collect::<Vec<_>>(), vec![(256, 256), (1280, 720)]);
    }

    /// A frame without the marker (a menu over everything) is counted, but
    /// says nothing about a HUD phase, so it does not move its figures: the
    /// difference between the two counts is the point.
    #[test]
    fn a_frame_without_a_hud_phase_is_counted_apart() {
        let mut r = Recorder::default();
        let mut d = draw(None, false);
        d.targets = &BB_TARGETS;

        r.scene_done();
        r.draw(&d);
        r.draw(&d);
        let with = r.present(BB, Some((1280, 720)));
        r.draw(&d);
        let without = r.present(BB, Some((1280, 720)));

        let mut agg = Agg::default();
        agg.add(&with);
        agg.add(&without);
        assert_eq!(agg.frames, 2);
        assert_eq!(agg.with_hud, 1);
        // The menu frame's single draw is the scene's, not a HUD phase's.
        assert_eq!(agg.hud_draws_max, 2);
        assert_eq!(agg.hud_draws, 2);
    }

    /// The programs reading D3D's `SV_Position` are named once each, however
    /// many frames draw with them: the list is what a redirect has to rescale.
    #[test]
    fn programs_reading_the_position_are_named_once_each() {
        let mut r = Recorder::default();
        let mut d = draw(Some(Ps { id: 2, label: "im::PS_Dither", reads_position: true }), false);
        d.targets = &BB_TARGETS;
        let mut plain = draw(Some(Ps { id: 1, label: "im::PS_Blit", reads_position: false }), false);
        plain.targets = &BB_TARGETS;

        let mut agg = Agg::default();
        for _ in 0..3 {
            r.scene_done();
            r.draw(&d);
            r.draw(&plain);
            agg.add(&r.present(BB, Some((1280, 720))));
        }
        assert_eq!(agg.reads_position.iter().collect::<Vec<_>>(), vec!["im::PS_Dither"]);
        assert_eq!(agg.frames, 3);
    }

    /// What broke, and in how many frames, so a change that breaks the premise
    /// of "everything after the marker is HUD" is not quietly averaged away.
    #[test]
    fn violations_are_counted_by_target_and_program() {
        let mut r = Recorder::default();
        let mut d = draw(Some(Ps { id: 1, label: "im::PS_Blit", reads_position: false }), false);
        // A scene-sized target that is not the back buffer.
        d.targets = &SCENE_TARGETS;

        let mut agg = Agg::default();
        for _ in 0..2 {
            r.scene_done();
            r.draw(&d);
            agg.add(&r.present(BB, Some((1280, 720))));
        }
        assert!(agg.violations.is_empty(), "scene-sized targets have structured issues");
        assert_eq!(agg.scene_sized_targets.len(), 1);
        let issue = agg.scene_sized_targets.values().next().unwrap();
        assert_eq!(issue.frames, 2);
        assert_eq!(issue.pixel_programs.iter().next().unwrap(), "im::PS_Blit");
    }

    /// The names a summary keeps are capped, so a long session cannot grow it
    /// without bound; the count still rises past the cap.
    #[test]
    fn the_names_a_summary_keeps_are_capped() {
        // The cap is on what `add` keeps, so drive it through frames rather
        // than filling the set directly.
        let mut agg = Agg::default();
        let mut r = Recorder::default();
        for i in 0..(CAP as u32 + 5) {
            let label = format!("im::PS_{i}");
            let mut d = draw(None, false);
            d.targets = &BB_TARGETS;
            let ps = Ps { id: i + 100, label: &label, reads_position: true };
            d.ps = Some(ps);
            r.scene_done();
            r.draw(&d);
            agg.add(&r.present(BB, Some((1280, 720))));
        }
        assert_eq!(agg.reads_position.len(), CAP);
        assert_eq!(agg.frames, CAP as u64 + 5);
    }
    /// The censo links a pre-marker render target to the pixel shader that
    /// later samples it while writing the back buffer, with exact subresource
    /// metadata. The marker remains a logical boundary only.
    #[test]
    fn scene_target_sampled_by_bb_hud_is_linked() {
        const MIP0: [Target; 1] = [Target { id: 31, width: 1280, height: 720, format: 5, mip: 0, slice: 0, samples: 1 }];
        const MIP1: [Target; 1] = [Target { id: 31, width: 640, height: 360, format: 5, mip: 1, slice: 0, samples: 1 }];
        const READ: [Read; 1] = [Read { id: 31, first_mip: 1, mip_count: 1, first_slice: 0, slice_count: 1 }];

        let mut r = Recorder::default();
        let mut scene = draw(Some(Ps { id: 10, label: "im::PS_Scene", reads_position: false }), false);
        scene.targets = &MIP0;
        r.draw(&scene);
        scene.targets = &MIP1;
        r.draw(&scene);
        r.scene_done();

        let mut hud = draw(Some(Ps { id: 20, label: "im::PS_Blit", reads_position: false }), false);
        hud.targets = &BB_TARGETS;
        hud.reads = &READ;
        r.draw(&hud);
        let frame = r.present(BB, Some((1280, 720)));

        assert_eq!(frame.scene_targets_read_by_hud, vec![SceneTargetRead {
            id: 31,
            width: 640,
            height: 360,
            format: 5,
            mip: 1,
            slice: 0,
            samples: 1,
            scene_writes: 1,
            bb_read_draws: 1,
            bb_reader_draw_indices: vec![1],
            scene_pixel_program_ids: vec![10],
            scene_pixel_programs: vec!["im::PS_Scene".to_owned()],
            bb_reader_program_ids: vec![20],
            bb_reader_programs: vec!["im::PS_Blit".to_owned()],
        }]);
        assert!(frame.hud_targets[0].bb);
        let mut agg = Agg::default();
        agg.add(&frame);
        let read = agg.scene_targets_read_by_hud.values().next().unwrap();
        assert_eq!(read.frames, 1);
        assert_eq!(read.bb_read_draws, 1);
        assert_eq!(read.bb_reader_programs.iter().next().unwrap(), "im::PS_Blit");
        let json: serde_json::Value = serde_json::from_str(&frame.to_json()).unwrap();
        assert_eq!(json["scene_targets_read_by_hud"][0]["mip"], 1);
        assert_eq!(json["scene_targets_read_by_hud"][0]["bb_reader_programs"][0], "im::PS_Blit");
    }

    /// A warning is an identified attachment, not just a count: the summary
    /// retains its format, subresource, samples, draw range, shader, and
    /// whether the same attachment appeared before the marker.
    #[test]
    fn scene_sized_warning_has_structured_summary() {
        let mut r = Recorder::default();
        let mut d = draw(Some(Ps { id: 1, label: "im::PS_Blit", reads_position: false }), false);
        d.targets = &SCENE_TARGETS;
        r.draw(&d);
        r.scene_done();
        r.draw(&d);
        r.draw(&d);
        let frame = r.present(BB, Some((1280, 720)));
        let mut agg = Agg::default();
        agg.add(&frame);

        assert_eq!(agg.scene_sized_targets.len(), 1);
        let issue = agg.scene_sized_targets.values().next().unwrap();
        assert_eq!(issue.frames, 1);
        assert_eq!(issue.draws, 2);
        assert_eq!(issue.first_draw, 1);
        assert_eq!(issue.last_draw, 2);
        assert_eq!(issue.written_before_scene_done_frames, 1);
        assert_eq!(issue.target.unwrap().id, SCENE);
        assert_eq!(issue.pixel_programs.iter().next().unwrap(), "im::PS_Blit");
        let json: serde_json::Value = serde_json::from_str(&frame.to_json()).unwrap();
        assert_eq!(json["scene_sized_targets"][0]["written_before_scene_done"], true);
        assert_eq!(json["scene_sized_targets"][0]["pixel_program_ids"][0], 1);
    }

    #[test]
    fn report_frame_tail_is_bounded_and_keeps_the_latest() {
        let mut frames = VecDeque::new();
        for frame in 0..=REPORT_FRAME_LIMIT {
            retain_frame(&mut frames, &format!("{{\"frame\":{frame}}}"));
        }
        assert_eq!(frames.len(), REPORT_FRAME_LIMIT);
        assert_eq!(frames.front().unwrap(), "{\"frame\":1}");
        assert_eq!(frames.back().unwrap(), &format!("{{\"frame\":{REPORT_FRAME_LIMIT}}}"));
        retain_frame(&mut frames, &"x".repeat(REPORT_FRAME_MAX_BYTES + 1));
        assert_eq!(frames.len(), REPORT_FRAME_LIMIT);
    }
}
