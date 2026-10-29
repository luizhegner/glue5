//! The game's benchmark results, for the start page: the engine writes
//! one file a run, `Benchmark-<date>.txt`, in `Documents/Rockstar
//! Games/GTA V/Benchmarks` of the user directory.
//!
//! GLUE 5's own controlled benchmark capture is separate: see [`glue5`].

pub mod glue5;

use std::path::Path;

/// A run's results over all its passes.
#[derive(Clone, Debug, PartialEq)]
pub struct Results {
    pub avg_fps: f32,
    pub min_fps: f32,
    /// Share of frames under 33 ms (30 fps), 0 to 1.
    pub smooth: f32,
}

/// The last run's results, if there was one.
pub fn last(user_dir: &Path) -> Option<Results> {
    let dir = user_dir.join("Documents/Rockstar Games/GTA V/Benchmarks");
    // The names hold the date (yy-mm-dd-hh-mm-ss): the greatest is the last.
    let last = std::fs::read_dir(dir).ok()?.flatten().map(|e| e.path()).filter(|p| is_results(p)).max()?;
    parse(&std::fs::read_to_string(last).ok()?)
}

fn is_results(path: &Path) -> bool {
    path.file_name().and_then(|n| n.to_str()).is_some_and(|n| n.starts_with("Benchmark-") && n.ends_with(".txt"))
}

/// The passes' "min, max, avg" fps, their average frame times and their
/// frames under 33 ms ("Pass n: under/all frames"), into one result.
fn parse(text: &str) -> Option<Results> {
    let mut sections = text.split("\n\n");
    let numbers = |line: &str| -> Vec<f32> { line.split(',').skip(1).filter_map(|v| v.trim().parse().ok()).collect() };
    let rows = |section: Option<&str>| -> Vec<Vec<f32>> {
        section.map_or(Vec::new(), |s| s.lines().filter(|l| l.starts_with("Pass ")).map(numbers).collect())
    };
    let fps = rows(sections.next());
    let ms = rows(sections.next());
    let _under_16 = sections.next();
    // "Pass 0: 796/807 frames (98.64%)": under 33 ms, of all.
    let frames: Vec<(f32, f32)> = sections
        .next()?
        .lines()
        .filter_map(|l| {
            let (under, all) = l.split_once(": ")?.1.split_whitespace().next()?.split_once('/')?;
            Some((under.parse().ok()?, all.parse().ok()?))
        })
        .collect();
    if fps.is_empty() || fps.len() != ms.len() || fps.len() != frames.len() {
        return None;
    }
    let all: f32 = frames.iter().map(|f| f.1).sum();
    // The average over the whole run: all frames over all the time.
    let time: f32 = ms.iter().zip(&frames).map(|(m, f)| m.get(2).copied().unwrap_or(0.0) * f.1).sum();
    Some(Results {
        avg_fps: all * 1000.0 / time,
        min_fps: fps.iter().filter_map(|r| r.first().copied()).fold(f32::INFINITY, f32::min),
        smooth: frames.iter().map(|f| f.0).sum::<f32>() / all,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_the_engines_file() {
        let text = "Frames Per Second (Higher is better) Min, Max, Avg\n\
            Pass 0, 6.0, 200.0, 100.0\nPass 1, 5.0, 247.0, 50.0\n\n\
            Time in milliseconds(ms). (Lower is better). Min, Max, Avg\n\
            Pass 0, 5.0, 163.0, 10.0\nPass 1, 4.0, 188.0, 20.0\n\n\
            Frames under 16ms (for 60fps): \nPass 0: 720/800 frames (90.00%)\nPass 1: 100/200 frames (50.00%)\n\n\
            Frames under 33ms (for 30fps): \nPass 0: 790/800 frames (98.75%)\nPass 1: 190/200 frames (95.00%)\n\n\
            Percentiles in ms for pass 0\n50%,\t10.00\n";
        let r = parse(text).unwrap();
        // 1000 frames in 8000 + 4000 ms.
        assert!((r.avg_fps - 1000.0 / 12.0).abs() < 0.01);
        assert_eq!(r.min_fps, 5.0);
        assert!((r.smooth - 0.98).abs() < 0.001);
    }
}
