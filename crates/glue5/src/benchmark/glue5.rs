//! JSONL capture format for GLUE 5's own controlled benchmarks.
//!
//! This is deliberately separate from [`super`], which reads GTA V's
//! built-in fly-through results. A caller starts a run, appends one flushed
//! frame record at a time, and closes it with an outcome. The scenario runner
//! and UI can be wired to this recorder independently of the phone controls.

use std::collections::BTreeMap;
use std::io::{self, Write};

use serde::Serialize;

/// Increment when the JSONL record shape changes incompatibly.
pub const SCHEMA_VERSION: u32 = 1;

/// Repeatable GLUE 5 benchmark scenarios; these are not GTA V's benchmark.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Scenario {
    /// Compare GLUE 5's visual-effects path under an explicitly recorded setup.
    VisualEffects,
    /// Compare GLUE 5's map-rendering path under an explicitly recorded setup.
    MapRendering,
}

/// Conditions that must accompany a result so runs can be compared fairly.
#[derive(Clone, Debug, Serialize)]
pub struct Conditions {
    pub device: String,
    pub gpu: String,
    pub os: String,
    pub width: u32,
    pub height: u32,
    pub render_scale: f32,
    pub upscaler: String,
    pub vsync: bool,
    /// 0 means the application's frame cap is disabled.
    pub fps_limit: u32,
    /// Scenario-specific switches, kept in stable key order in the file.
    pub settings: BTreeMap<String, String>,
}

/// Metadata written before any samples. `build_id` should be the GLUE 5 build
/// label, not the GTA version or the benchmark's run number.
#[derive(Clone, Debug, Serialize)]
pub struct Run {
    pub id: String,
    pub scenario: Scenario,
    pub build_id: String,
    pub started_unix_ms: u64,
    pub conditions: Conditions,
}

/// One frame's GLUE 5 measurements. Missing GPU timestamps are `None`, not 0.
#[derive(Clone, Debug, Serialize)]
pub struct FrameSample {
    /// Assigned by [`Recorder`] starting at zero; callers leave this at zero.
    pub index: u64,
    pub frame_ms: f32,
    pub render_ms: f32,
    pub gpu_ms: Option<f32>,
    pub draws: u32,
    pub passes: u32,
    pub submits: u32,
    pub gpu_bytes: u64,
}

impl FrameSample {
    fn validate(&self) -> io::Result<()> {
        let valid = |value: f32| value.is_finite() && value >= 0.0;
        if !valid(self.frame_ms) || !valid(self.render_ms) || self.gpu_ms.is_some_and(|v| !valid(v)) {
            return Err(io::Error::new(io::ErrorKind::InvalidInput, "benchmark timings must be finite and non-negative"));
        }
        Ok(())
    }
}

/// Why a controlled run ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Outcome {
    Completed,
    Cancelled,
    Failed,
}

#[derive(Serialize)]
#[serde(tag = "record", rename_all = "snake_case")]
enum Line<'a> {
    Start { schema: u32, run: &'a Run },
    Frame { sample: &'a FrameSample },
    End { frames: u64, elapsed_ms: u64, ended_unix_ms: u64, outcome: Outcome },
}

/// Append-only benchmark logger. Every record is flushed before returning,
/// so a crash preserves all complete frames captured so far.
pub struct Recorder<W: Write> {
    writer: W,
    started_unix_ms: u64,
    frames: u64,
}

impl<W: Write> Recorder<W> {
    /// Start a run and immediately flush its conditions to the log.
    pub fn begin(mut writer: W, run: &Run) -> io::Result<Self> {
        write_line(&mut writer, &Line::Start { schema: SCHEMA_VERSION, run })?;
        Ok(Self { writer, started_unix_ms: run.started_unix_ms, frames: 0 })
    }

    /// Append and flush one measured frame. Invalid timings are rejected
    /// before they can contaminate a run's data.
    pub fn frame(&mut self, mut sample: FrameSample) -> io::Result<()> {
        sample.validate()?;
        sample.index = self.frames;
        write_line(&mut self.writer, &Line::Frame { sample: &sample })?;
        self.frames += 1;
        Ok(())
    }

    /// Finish the log and return its writer, allowing a caller to close or
    /// inspect it. `ended_unix_ms` is supplied by the scenario runner.
    pub fn finish(mut self, ended_unix_ms: u64, outcome: Outcome) -> io::Result<W> {
        write_line(
            &mut self.writer,
            &Line::End {
                frames: self.frames,
                elapsed_ms: ended_unix_ms.saturating_sub(self.started_unix_ms),
                ended_unix_ms,
                outcome,
            },
        )?;
        Ok(self.writer)
    }
}

fn write_line(writer: &mut impl Write, line: &impl Serialize) -> io::Result<()> {
    let bytes = serde_json::to_vec(line).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
    writer.write_all(&bytes)?;
    writer.write_all(b"\n")?;
    writer.flush()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Default)]
    struct FlushWriter {
        bytes: Vec<u8>,
        flushes: usize,
    }

    impl Write for FlushWriter {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            self.bytes.extend_from_slice(bytes);
            Ok(bytes.len())
        }

        fn flush(&mut self) -> io::Result<()> {
            self.flushes += 1;
            Ok(())
        }
    }

    fn run() -> Run {
        Run {
            id: "run-1".into(),
            scenario: Scenario::MapRendering,
            build_id: "ALPHA · 0.1.0 willow · test".into(),
            started_unix_ms: 1_000,
            conditions: Conditions {
                device: "test device".into(),
                gpu: "test GPU".into(),
                os: "Android".into(),
                width: 1920,
                height: 1080,
                render_scale: 0.75,
                upscaler: "bicubic".into(),
                vsync: true,
                fps_limit: 60,
                settings: BTreeMap::from([("shadows".into(), "high".into())]),
            },
        }
    }

    fn sample() -> FrameSample {
        FrameSample {
            index: 99,
            frame_ms: 16.0,
            render_ms: 5.0,
            gpu_ms: None,
            draws: 300,
            passes: 12,
            submits: 2,
            gpu_bytes: 64 * 1024 * 1024,
        }
    }

    #[test]
    fn writes_start_frame_and_end_as_flushed_json_lines() {
        let mut recorder = Recorder::begin(FlushWriter::default(), &run()).unwrap();
        recorder.frame(sample()).unwrap();
        let output = recorder.finish(1_250, Outcome::Completed).unwrap();
        let lines: Vec<serde_json::Value> = std::str::from_utf8(&output.bytes)
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();

        assert_eq!(output.flushes, 3, "start, each frame, and end are flushed");
        assert_eq!(lines.len(), 3);
        assert_eq!(lines[0]["record"], "start");
        assert_eq!(lines[0]["schema"], SCHEMA_VERSION);
        assert_eq!(lines[0]["run"]["scenario"], "map_rendering");
        assert_eq!(lines[0]["run"]["conditions"]["width"], 1920);
        assert_eq!(lines[1]["record"], "frame");
        assert_eq!(lines[1]["sample"]["index"], 0);
        assert!(lines[1]["sample"]["gpu_ms"].is_null(), "unsupported GPU timing is explicit");
        assert_eq!(lines[2]["record"], "end");
        assert_eq!(lines[2]["frames"], 1);
        assert_eq!(lines[2]["elapsed_ms"], 250);
    }

    #[test]
    fn rejects_invalid_timings_without_writing_a_frame() {
        let mut recorder = Recorder::begin(FlushWriter::default(), &run()).unwrap();
        let mut invalid = sample();
        invalid.gpu_ms = Some(f32::NAN);
        assert_eq!(recorder.frame(invalid).unwrap_err().kind(), io::ErrorKind::InvalidInput);
        let output = recorder.finish(1_100, Outcome::Cancelled).unwrap();
        let text = std::str::from_utf8(&output.bytes).unwrap();
        assert_eq!(text.lines().count(), 2, "only start and end were recorded");
        assert_eq!(output.flushes, 2);
    }
}
