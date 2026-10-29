//! This computer serving its game folder to the app on other devices on the
//! network (Settings › Game › Share on the network): phones and TVs play it
//! without a copy of their own.

use std::net::{Ipv4Addr, SocketAddr};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use glue5_stream::server::{self, Server, Step};

#[derive(Clone, Debug, PartialEq)]
pub enum State {
    /// Listing the folder, or packing its small files (once): how far.
    Preparing(Option<f32>),
    /// Serving: where the other devices connect, and whether they find
    /// this computer by themselves (announced on the network; not when
    /// another program holds the port announcements use).
    On(Vec<String>, bool),
    Failed(String),
}

pub struct Sharing {
    dir: PathBuf,
    state: Arc<Mutex<State>>,
    /// The server, and its announcement to the devices looking (Bonjour).
    server: Arc<Mutex<Option<(Server, Option<glue5_stream::discovery::Advert>)>>>,
    stopped: Arc<AtomicBool>,
}

/// Ports tried after the usual one, if another program has it.
const MORE_PORTS: u16 = 4;

impl Sharing {
    /// Shares the game folder `dir`; its packs are kept in `cache`.
    pub fn start(dir: PathBuf, cache: PathBuf) -> Sharing {
        let state = Arc::new(Mutex::new(State::Preparing(None)));
        let server = Arc::new(Mutex::new(None));
        let stopped = Arc::new(AtomicBool::new(false));
        let (s, srv, stop, game) = (state.clone(), server.clone(), stopped.clone(), dir.clone());
        let spawned = glue5_runtime::qos::spawn("sharing", glue5_runtime::qos::Qos::Utility, move || {
            let progress = |step: Step| {
                let done = match step {
                    Step::Listing => None,
                    Step::Packing(_, done, total) => Some(done as f32 / total.max(1) as f32),
                };
                *s.lock().unwrap() = State::Preparing(done);
            };
            let options =
                server::Options { game, build: glue5_runtime::GAME_BUILD.into(), cache, rate: None };
            let result = server::prepare(options, &progress).and_then(|prepared| {
                // Turned off (or on again: a newer one starts) meanwhile: not
                // listening, not even for a moment on the port the newer
                // one wants.
                if stop.load(Ordering::Acquire) {
                    anyhow::bail!("stopped");
                }
                let mut last = None;
                for port in glue5_stream::PORT..=glue5_stream::PORT + MORE_PORTS {
                    match Server::start(&prepared, SocketAddr::from((Ipv4Addr::UNSPECIFIED, port))) {
                        Ok(server) => return Ok(server),
                        Err(e) => last = Some(e),
                    }
                }
                Err(last.unwrap())
            });
            match result {
                Ok(server) if !stop.load(Ordering::Acquire) => {
                    let port = server.addr().port();
                    let shown = |ip: Ipv4Addr| if port == glue5_stream::PORT { ip.to_string() } else { format!("{ip}:{port}") };
                    let addresses = glue5_stream::lan_addresses().into_iter().map(shown).collect();
                    let advert = glue5_stream::discovery::advertise(port);
                    let announced = advert.is_some();
                    *srv.lock().unwrap() = Some((server, advert));
                    *s.lock().unwrap() = State::On(addresses, announced);
                }
                Ok(_) => {}
                Err(_) if stop.load(Ordering::Acquire) => {}
                Err(e) => {
                    log::warn!("sharing: {e:#}");
                    *s.lock().unwrap() = State::Failed(format!("{e:#}"));
                }
            }
        });
        if let Err(e) = spawned {
            *state.lock().unwrap() = State::Failed(e.to_string());
        }
        Sharing { dir, state, server, stopped }
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    pub fn state(&self) -> State {
        self.state.lock().unwrap().clone()
    }
}

impl Drop for Sharing {
    fn drop(&mut self) {
        self.stopped.store(true, Ordering::Release);
        self.server.lock().unwrap().take();
    }
}
