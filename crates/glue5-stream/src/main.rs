//! `glue5-serve`: serves a game folder to Glue 5 apps on other devices.

use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use anyhow::{Context, Result, bail};
use glue5_stream::server::{self, Step};

const USAGE: &str = "\
usage: glue5-serve [options] [game folder]

The game folder holds data/ and b/<build>/shaders/ (as for the app);
GLUE5_GAME_DIR if none is given.

options:
  --port <n>       port to listen on (default 47100)
  --bind <ip>      address to listen on (default all)
  --build <id>     the game build to serve, if b/ holds several
  --cache <dir>    where the packs are kept (default: the user's cache folder)
  --rate <MB/s>    send at most this much, all clients together
";

fn main() {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();
    if let Err(e) = run() {
        eprintln!("glue5-serve: {e:#}");
        std::process::exit(1);
    }
}

fn run() -> Result<()> {
    let mut args = std::env::args().skip(1);
    let (mut port, mut bind, mut build, mut cache, mut rate, mut game) =
        (glue5_stream::PORT, IpAddr::V4(Ipv4Addr::UNSPECIFIED), None, None, None, None);
    while let Some(a) = args.next() {
        let mut value = || args.next().with_context(|| format!("{a} needs a value"));
        match a.as_str() {
            "--port" => port = value()?.parse().context("--port")?,
            "--bind" => bind = value()?.parse().context("--bind")?,
            "--build" => build = Some(value()?),
            "--cache" => cache = Some(PathBuf::from(value()?)),
            "--rate" => rate = Some((value()?.parse::<f64>().context("--rate")? * 1e6) as u64),
            "-h" | "--help" => {
                print!("{USAGE}");
                return Ok(());
            }
            _ if a.starts_with('-') => bail!("unknown option {a}\n\n{USAGE}"),
            _ => game = Some(PathBuf::from(a)),
        }
    }
    let Some(game) = game.or_else(|| std::env::var_os("GLUE5_GAME_DIR").map(PathBuf::from)) else {
        bail!("no game folder\n\n{USAGE}")
    };
    let build = match build {
        Some(b) => b,
        None => only_build(&game)?,
    };
    let cache = cache.unwrap_or_else(default_cache);
    let listener = server::Server::bind(SocketAddr::new(bind, port))?;
    // A line per tenth.
    let shown = std::cell::Cell::new(None);
    let progress = |step: Step| {
        if let Step::Packing(name, done, total) = step {
            let tenth = done * 10 / total.max(1);
            if shown.replace(Some((name, tenth))) != Some((name, tenth)) {
                log::info!("packing {name}: {} %", tenth * 10);
            }
        }
    };
    let prepared = server::prepare(server::Options { game, build, cache, rate }, &progress)?;
    let server = server::Server::start_bound(&prepared, listener)?;
    let port = server.addr().port();
    for ip in glue5_stream::lan_addresses() {
        log::info!("on this network: {ip}:{port}");
    }
    // Devices on the network find it (it announces itself), until Ctrl-C
    // or a request to stop: then it says goodbye, and they forget it.
    let advert = glue5_stream::discovery::advertise(port);
    on_stop_requests();
    while !STOP.load(Ordering::Acquire) {
        std::thread::park_timeout(Duration::from_millis(200));
    }
    drop(advert);
    drop(server);
    log::info!("stopped");
    Ok(())
}

/// Ctrl-C, SIGTERM (or the console closing on Windows) asked to stop.
static STOP: AtomicBool = AtomicBool::new(false);

#[cfg(unix)]
fn on_stop_requests() {
    extern "C" fn stop(_: libc::c_int) {
        STOP.store(true, Ordering::Release);
    }
    unsafe {
        libc::signal(libc::SIGINT, stop as *const () as libc::sighandler_t);
        libc::signal(libc::SIGTERM, stop as *const () as libc::sighandler_t);
    }
}

#[cfg(windows)]
fn on_stop_requests() {
    unsafe extern "system" fn stop(_: u32) -> windows_sys::core::BOOL {
        STOP.store(true, Ordering::Release);
        // Handled: the main thread ends the process (closing the console
        // leaves it a few seconds for that).
        1
    }
    unsafe { windows_sys::Win32::System::Console::SetConsoleCtrlHandler(Some(stop), 1) };
}

/// The one build in `<game>/b/`.
fn only_build(game: &std::path::Path) -> Result<String> {
    let dir = game.join("b");
    let builds: Vec<String> = std::fs::read_dir(&dir)
        .with_context(|| format!("listing {}", dir.display()))?
        .flatten()
        .filter(|e| e.path().join("shaders").join("index.json").is_file())
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();
    match &builds[..] {
        [one] => Ok(one.clone()),
        [] => bail!("{} holds no build with shaders", dir.display()),
        many => bail!("{} holds several builds ({}): choose one with --build", dir.display(), many.join(", ")),
    }
}

fn default_cache() -> PathBuf {
    let home = std::env::var_os("HOME").map(PathBuf::from);
    let base = if cfg!(target_os = "macos") {
        home.map(|h| h.join("Library/Caches"))
    } else if cfg!(windows) {
        std::env::var_os("LOCALAPPDATA").map(PathBuf::from)
    } else {
        std::env::var_os("XDG_CACHE_HOME").map(PathBuf::from).or_else(|| home.map(|h| h.join(".cache")))
    };
    base.unwrap_or_else(std::env::temp_dir).join("glue5-serve")
}
