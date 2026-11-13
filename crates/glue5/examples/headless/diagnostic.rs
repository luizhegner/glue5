//! Opt-in localhost diagnostic control for the offscreen GLUE5 harness.
//! A tiny, explicit allowlist. The socket thread never touches Game or GPU state;
//! requests execute on the harness loop and return versioned JSON responses.
use std::io::{BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, Sender, SyncSender};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use glue5_runtime::Game;
use glue5_runtime::input::button;
use serde_json::{Value, json};

use super::Key;

const VERSION: u64 = 1;
const MAX_REQUEST: usize = 8192;

pub(super) struct Config {
    pub port: u16,
    pub shot_dir: PathBuf,
    pub session_id: String,
    pub size: (u32, u32),
    pub fps_limit: u32,
    pub low_memory: bool,
    pub touch: bool,
    pub sandbox: bool,
    pub has_gpu: bool,
}

struct Request {
    message: Value,
    reply: SyncSender<Value>,
}

pub(super) struct Server {
    pending: Receiver<Request>,
    config: Config,
    session_id: String,
    started: Instant,
    last_metrics: Instant,
    capture_index: u64,
}

#[derive(Default)]
pub(super) struct Axes {
    left: Option<([f32; 2], Instant)>,
    right: Option<([f32; 2], Instant)>,
}

impl Axes {
    pub fn current(&self, spin: [f32; 2]) -> ([f32; 2], [f32; 2]) {
        let now = Instant::now();
        let left = self.left.filter(|(_, until)| *until > now).map_or([0.0; 2], |v| v.0);
        let right = self.right.filter(|(_, until)| *until > now).map_or(spin, |v| v.0);
        (left, right)
    }

    fn set(&mut self, stick: &str, x: f32, y: f32, duration_ms: u64) -> std::result::Result<(), String> {
        if !x.is_finite() || !y.is_finite() || !(-1.0..=1.0).contains(&x) || !(-1.0..=1.0).contains(&y) {
            return Err("axis values must be finite and between -1 and 1".into());
        }
        if !(10..=10_000).contains(&duration_ms) {
            return Err("duration_ms must be 10..10000".into());
        }
        let axis = Some(([x, y], Instant::now() + Duration::from_millis(duration_ms)));
        match stick {
            "left" => self.left = axis,
            "right" => self.right = axis,
            _ => return Err("stick must be left or right".into()),
        }
        Ok(())
    }
}

impl Server {
    pub fn start(config: Config) -> Result<Self> {
        let token = std::env::var("GLUE5_DIAGNOSTIC_TOKEN")
            .context("set GLUE5_DIAGNOSTIC_TOKEN (at least 16 ASCII characters) before enabling diagnostics")?;
        if token.len() < 16 || token.len() > 256 || !token.bytes().all(|b| b.is_ascii_graphic()) {
            bail!("GLUE5_DIAGNOSTIC_TOKEN must be 16..256 printable ASCII characters");
        }
        let listener = TcpListener::bind(("127.0.0.1", config.port))
            .with_context(|| format!("binding diagnostic listener on 127.0.0.1:{}", config.port))?;
        let (tx, pending) = mpsc::channel();
        std::thread::Builder::new().name("glue5-diagnostic".into()).spawn(move || {
            // One client at a time; a stalled local client times out, rather
            // than allocating unbounded threads. Never listen on LAN.
            for connection in listener.incoming() {
                match connection {
                    Ok(stream) => {
                        if let Err(e) = serve(stream, &token, &tx) {
                            log::debug!("diagnostic client disconnected: {e}");
                        }
                    }
                    Err(e) => log::warn!("diagnostic accept: {e}"),
                }
            }
        })?;

        log::info!("diagnostic: localhost port {} enabled (session token required)", config.port);
        let session_id = config.session_id.clone();
        Ok(Self {
            pending,
            config,
            session_id,
            started: Instant::now(),
            last_metrics: Instant::now(),
            capture_index: 0,
        })
    }

    pub fn drain(
        &mut self,
        game: &Game,
        t: f64,
        presses: &mut Vec<(f64, u16, f64)>,
        _keys: &mut Vec<Key>,
        axes: &mut Axes,
    ) {
        // Bound work per tick so a client cannot starve gameplay.
        for _ in 0..32 {
            let Ok(request) = self.pending.try_recv() else { break };
            let id = request.message.get("id").cloned().unwrap_or(Value::Null);
            let answer = match self.execute(&request.message, game, t, presses, axes) {
                Ok(value) => json!({"v": VERSION, "id": id, "ok": true, "result": value}),
                Err(message) => json!({"v": VERSION, "id": id, "ok": false, "error": message}),
            };
            let _ = request.reply.send(answer);
        }
    }

    fn execute(
        &mut self,
        request: &Value,
        game: &Game,
        t: f64,
        presses: &mut Vec<(f64, u16, f64)>,
        axes: &mut Axes,
    ) -> std::result::Result<Value, String> {
        let op = request["op"].as_str().ok_or("missing op")?;
        let a = &request["args"];
        match op {
            "status" => Ok(json!({
                "session_id": self.session_id,
                "platform": std::env::consts::OS,
                "game_build": glue5_runtime::GAME_BUILD,
                "uptime_ms": self.started.elapsed().as_millis(),
                "runtime_input_ready": game.input().is_some(),
                "game_loaded": null,
                "gpu_enabled": self.config.has_gpu,
                "capture_available": self.config.has_gpu,
                "transport": "tcp-loopback",
                "capabilities": ["status", "get_settings", "get_metrics", "request_capture",
                                 "capture_status", "input_press", "input_axis"],
            })),
            "get_settings" => Ok(json!({
                "source": "headless startup configuration", "mutable": false,
                "width": self.config.size.0, "height": self.config.size.1,
                "fps_limit": self.config.fps_limit,
                "low_memory": self.config.low_memory, "touch_mode": self.config.touch,
                "sandbox": self.config.sandbox, "gpu_enabled": self.config.has_gpu,
            })),
            "get_metrics" => {
                let gpu = glue5_gpu::perf::diagnostics_snapshot();
                let audio = glue5_runtime::audio::take_stats();
                let elapsed = self.last_metrics.elapsed();
                self.last_metrics = Instant::now();
                let avg = (!gpu.frame_ms.is_empty())
                    .then(|| gpu.frame_ms.iter().copied().sum::<f32>() / gpu.frame_ms.len() as f32);
                Ok(json!({
                    "timestamp_ms": self.started.elapsed().as_millis(),
                    "gpu": {
                        "last_frame_ms": gpu.frame_ms.last(), "average_frame_ms": avg,
                        "fps_estimate": avg.filter(|ms| *ms > 0.0).map(|ms| 1000.0 / ms),
                        "last_render_ms": gpu.render_ms.last(), "last_gpu_ms": gpu.gpu_ms.last(),
                        "draws": gpu.draws, "passes": gpu.passes, "submits": gpu.submits,
                        "allocated_bytes": gpu.gpu_bytes,
                        "frame_samples": gpu.frame_ms.len(),
                    },
                    "audio_since_previous_query": {
                        "interval_ms": elapsed.as_millis(),
                        "underruns": audio.underruns,
                        "silence_ms": audio.silence.as_secs_f64() * 1000.0,
                        "update_waits": audio.update_waits,
                        "update_waited_ms": audio.update_waited.as_secs_f64() * 1000.0,
                        "update_longest_ms": audio.update_longest.as_secs_f64() * 1000.0,
                    },
                }))
            }
            "input_press" => {
                if game.input().is_none() {
                    return Err("runtime input not ready".into());
                }
                let control = a["control"].as_str().ok_or("missing control")?;
                let bits = named_button(control)?;
                let ms = a["duration_ms"].as_u64().unwrap_or(100);
                if !(10..=5000).contains(&ms) {
                    return Err("duration_ms must be 10..5000".into());
                }
                presses.push((t, bits, ms as f64 / 1000.0));
                Ok(json!({"accepted": true, "control": control, "duration_ms": ms}))
            }
            "input_axis" => {
                if game.input().is_none() {
                    return Err("runtime input not ready".into());
                }
                let stick = a["stick"].as_str().ok_or("missing stick")?;
                let x = a["x"].as_f64().ok_or("missing x")? as f32;
                let y = a["y"].as_f64().ok_or("missing y")? as f32;
                let ms = a["duration_ms"].as_u64().unwrap_or(500);
                axes.set(stick, x, y, ms)?;
                Ok(json!({"accepted": true, "stick": stick, "x": x, "y": y, "duration_ms": ms}))
            }
            "request_capture" => {
                if !self.config.has_gpu {
                    return Err("GPU capture unavailable with --no-gpu".into());
                }
                let capture_id = self.capture_index + 1;
                if !glue5_gpu::request_diagnostic_capture(&capture_id.to_string()) {
                    return Err("renderer already has a pending capture".into());
                }
                self.capture_index = capture_id;
                Ok(json!({
                    "capture_id": capture_id, "state": "queued", "format": "png",
                    "note": "poll capture_status; only next rendered frame can fulfill this request",
                }))
            }
            "capture_status" => {
                let id = a["capture_id"].as_u64().ok_or("missing capture_id")?;
                if id == 0 || id > self.capture_index {
                    return Err("unknown capture_id".into());
                }
                let path = self.config.shot_dir.join(format!("diag-{id}.png"));
                match std::fs::metadata(&path) {
                    Ok(meta) if meta.is_file() => Ok(json!({
                        "capture_id": id, "ready": true, "path": path,
                        "size_bytes": meta.len(), "format": "png",
                    })),
                    _ => Ok(json!({"capture_id": id, "ready": false})),
                }
            }
            _ => Err(format!("operation not allowed: {op}")),
        }
    }
}

fn named_button(name: &str) -> std::result::Result<u16, String> {
    let bit = match name {
        "INPUT_FRONTEND_ACCEPT" | "INPUT_JUMP" => button::SOUTH,
        "INPUT_FRONTEND_CANCEL" => button::EAST,
        "INPUT_FRONTEND_PAUSE" => button::START,
        _ => match name.to_ascii_lowercase().as_str() {
            "start" => button::START,
            "select" => button::SELECT,
            "south" | "a" => button::SOUTH,
            "east" | "b" => button::EAST,
            "west" | "x" => button::WEST,
            "north" | "y" => button::NORTH,
            "l1" => button::L1,
            "r1" => button::R1,
            "l2" => button::L2,
            "r2" => button::R2,
            "l3" => button::L3,
            "r3" => button::R3,
            "up" => button::UP,
            "down" => button::DOWN,
            "left" => button::LEFT,
            "right" => button::RIGHT,
            _ => return Err(format!("control not in allowlist: {name}")),
        },
    };
    Ok(bit)
}

fn serve(mut stream: TcpStream, token: &str, tx: &Sender<Request>) -> Result<()> {
    stream.set_read_timeout(Some(Duration::from_secs(3)))?;
    stream.set_write_timeout(Some(Duration::from_secs(3)))?;
    let mut reader = BufReader::new(stream.try_clone()?);
    let mut input = Vec::new();
    let mut end = false;
    // A bounded single JSON object per connection; no streaming control.
    for _ in 0..MAX_REQUEST {
        let mut b = [0];
        reader.read_exact(&mut b)?;
        if b[0] == b'\n' {
            end = true;
            break;
        }
        input.push(b[0]);
    }
    let reply = if !end {
        json!({"v": VERSION, "ok": false, "error": "request too large"})
    } else {
        match serde_json::from_slice::<Value>(&input) {
            Ok(msg)
                if msg["v"].as_u64() == Some(VERSION) && msg["token"].as_str() == Some(token) && msg.is_object() =>
            {
                let (reply_tx, reply_rx) = mpsc::sync_channel(1);
                tx.send(Request { message: msg, reply: reply_tx })?;
                reply_rx.recv_timeout(Duration::from_secs(2))?
            }
            Ok(msg) => {
                json!({"v": VERSION, "id": msg.get("id"), "ok": false, "error": "unauthorized or incompatible protocol"})
            }
            Err(_) => json!({"v": VERSION, "ok": false, "error": "invalid JSON"}),
        }
    };
    stream.write_all(serde_json::to_string(&reply)?.as_bytes())?;
    stream.write_all(b"\n")?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn control_is_allowlisted() {
        assert_eq!(named_button("INPUT_FRONTEND_ACCEPT").unwrap(), button::SOUTH);
        assert_eq!(named_button("INPUT_FRONTEND_CANCEL").unwrap(), button::EAST);
        assert!(named_button("INPUT_ARBITRARY").is_err());
    }
    #[test]
    fn axes_are_bounded_and_expire() {
        let mut a = Axes::default();
        assert!(a.set("left", 1.2, 0.0, 100).is_err());
        assert!(a.set("left", 0.2, -0.5, 500).is_ok());
        assert_eq!(a.current([0.0; 2]).0, [0.2, -0.5]);
    }
}
