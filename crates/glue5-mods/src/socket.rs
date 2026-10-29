//! `net.socket(url)`: WebSockets, `ws://` and `wss://`, in the shape of
//! Lune's (`socket:send`, `socket:next`, `socket:close`,
//! `socket.closeCode`). tungstenite (the Studio's) over rustls with ring
//! and webpki's roots, the TLS of `net.request`. Each socket has a host
//! thread: it connects, then reads with a short timeout and sends what the
//! mod queued in between; the mod's coroutine waits for the connection and
//! for messages as `wait()` does. The guards are the app's: [`SOCKETS`] open
//! at once a mod, messages of [`MESSAGE`] at most, reading pauses while
//! [`QUEUED`] wait for the mod. A socket the mod drops (or a mod that stops)
//! is closed by its thread.

use std::collections::VecDeque;
use std::io::{ErrorKind, Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::{Receiver, Sender, TryRecvError};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use mlua::{Function, Lua, Table, UserData, UserDataMethods, Value};
use rustls::pki_types::ServerName;
use rustls::{ClientConfig, ClientConnection, RootCertStore, StreamOwned};
use tungstenite::client::IntoClientRequest;
use tungstenite::protocol::frame::coding::CloseCode;
use tungstenite::protocol::{CloseFrame, WebSocketConfig};
use tungstenite::{Message, WebSocket};

/// Sockets a mod may have open at once.
const SOCKETS: usize = 4;
/// The largest message, either way.
const MESSAGE: usize = 16 << 20;
/// Messages received and not taken yet: past this, reading waits.
const QUEUED: usize = 16 << 20;
/// How long a read waits before the thread looks at what the mod sent.
const TICK: Duration = Duration::from_millis(20);
/// Connecting and the handshake.
const CONNECT: Duration = Duration::from_secs(30);
/// "No status received" and "closed abnormally" (RFC 6455).
const NO_STATUS: u16 = 1005;
const ABNORMAL: u16 = 1006;

/// The connection under the WebSocket.
enum Conn {
    Plain(TcpStream),
    Tls(Box<StreamOwned<ClientConnection, TcpStream>>),
}

impl Conn {
    fn tcp(&self) -> &TcpStream {
        match self {
            Conn::Plain(s) => s,
            Conn::Tls(s) => &s.sock,
        }
    }
}

impl Read for Conn {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        match self {
            Conn::Plain(s) => s.read(buf),
            Conn::Tls(s) => s.read(buf),
        }
    }
}

impl Write for Conn {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        match self {
            Conn::Plain(s) => s.write(buf),
            Conn::Tls(s) => s.write(buf),
        }
    }

    fn flush(&mut self) -> std::io::Result<()> {
        match self {
            Conn::Plain(s) => s.flush(),
            Conn::Tls(s) => s.flush(),
        }
    }
}

/// What the thread and the mod share.
#[derive(Default)]
struct State {
    connected: bool,
    /// Why it could not connect.
    error: Option<String>,
    incoming: VecDeque<Vec<u8>>,
    queued: usize,
    closed: bool,
    close_code: Option<u16>,
}

enum Out {
    Message(Message),
    Close(u16),
}

/// The mod's end of a socket.
struct Socket {
    url: String,
    state: Arc<Mutex<State>>,
    out: Sender<Out>,
}

impl Socket {
    fn send(&self, out: Out) -> mlua::Result<()> {
        if crate::lock(&self.state).closed {
            return Err(mlua::Error::runtime(format!("net.socket({}): closed", self.url)));
        }
        // A thread that just ended: closed too, as the next call says.
        let _ = self.out.send(out);
        Ok(())
    }
}

impl UserData for Socket {
    fn add_methods<M: UserDataMethods<Self>>(m: &mut M) {
        // Whether it is open; an error when it could not be.
        m.add_method("connected", |_, s, ()| {
            let state = crate::lock(&s.state);
            match &state.error {
                Some(e) => Err(mlua::Error::runtime(format!("net.socket({}): {e}", s.url))),
                None => Ok(state.connected),
            }
        });
        // The next message, and whether it is closed (no more to come).
        m.add_method("recv", |lua, s, ()| {
            let mut state = crate::lock(&s.state);
            match state.incoming.pop_front() {
                Some(message) => {
                    state.queued -= message.len();
                    Ok((Some(lua.create_string(message)?), false))
                }
                None => Ok((None, state.closed)),
            }
        });
        m.add_method("send", |_, s, (message, binary): (Value, Option<bool>)| {
            let message = match message {
                Value::Nil => Vec::new(),
                Value::String(t) => t.as_bytes().to_vec(),
                Value::Buffer(b) => b.to_vec(),
                _ => return Err(mlua::Error::runtime("socket:send: a string or a buffer")),
            };
            if message.len() > MESSAGE {
                return Err(mlua::Error::runtime(format!("socket:send: messages of {} MB at most", MESSAGE >> 20)));
            }
            let message = if binary == Some(true) {
                Message::Binary(message.into())
            } else {
                let text = String::from_utf8(message)
                    .map_err(|_| mlua::Error::runtime("socket:send: text is UTF-8 (or send it as binary)"))?;
                Message::Text(text.into())
            };
            s.send(Out::Message(message))
        });
        m.add_method("close", |_, s, code: Option<u16>| {
            if crate::lock(&s.state).closed {
                return Ok(());
            }
            s.send(Out::Close(code.unwrap_or(1000)))
        });
        m.add_method("closeCode", |_, s, ()| Ok(crate::lock(&s.state).close_code));
    }
}

fn tls() -> Arc<ClientConfig> {
    static CONFIG: OnceLock<Arc<ClientConfig>> = OnceLock::new();
    CONFIG
        .get_or_init(|| {
            let roots = RootCertStore { roots: webpki_roots::TLS_SERVER_ROOTS.to_vec() };
            let config = ClientConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
                .with_safe_default_protocol_versions()
                .expect("ring has the default versions")
                .with_root_certificates(roots)
                .with_no_client_auth();
            Arc::new(config)
        })
        .clone()
}

fn connect(url: &str) -> Result<WebSocket<Conn>, String> {
    let request = url.into_client_request().map_err(|e| e.to_string())?;
    let uri = request.uri();
    let secure = match uri.scheme_str() {
        Some("wss") => true,
        Some("ws") => false,
        _ => return Err("a ws:// or wss:// URL".into()),
    };
    let host = uri.host().ok_or("no host")?.trim_start_matches('[').trim_end_matches(']').to_string();
    let port = uri.port_u16().unwrap_or(if secure { 443 } else { 80 });
    let mut last = None;
    let mut tcp = None;
    for addr in (host.as_str(), port).to_socket_addrs().map_err(|e| e.to_string())? {
        match TcpStream::connect_timeout(&addr, CONNECT) {
            Ok(s) => {
                tcp = Some(s);
                break;
            }
            Err(e) => last = Some(e),
        }
    }
    let tcp = tcp.ok_or_else(|| last.map_or("no address".into(), |e| e.to_string()))?;
    tcp.set_nodelay(true).map_err(|e| e.to_string())?;
    tcp.set_read_timeout(Some(CONNECT)).map_err(|e| e.to_string())?;
    let conn = if secure {
        let name = ServerName::try_from(host).map_err(|e| e.to_string())?;
        let tls = ClientConnection::new(tls(), name).map_err(|e| e.to_string())?;
        Conn::Tls(Box::new(StreamOwned::new(tls, tcp)))
    } else {
        Conn::Plain(tcp)
    };
    let config = WebSocketConfig::default().max_message_size(Some(MESSAGE)).max_frame_size(Some(MESSAGE));
    let (ws, _) = tungstenite::client::client_with_config(request, conn, Some(config)).map_err(|e| match e {
        tungstenite::HandshakeError::Failure(e) => e.to_string(),
        tungstenite::HandshakeError::Interrupted(_) => "no answer".into(),
    })?;
    ws.get_ref().tcp().set_read_timeout(Some(TICK)).map_err(|e| e.to_string())?;
    Ok(ws)
}

/// The socket's thread: connects, then sends what the mod queued and reads.
fn run(url: &str, state: &Mutex<State>, out: &Receiver<Out>) {
    let mut ws = match connect(url) {
        Ok(ws) => ws,
        Err(e) => {
            let mut s = crate::lock(state);
            s.error = Some(e);
            s.closed = true;
            return;
        }
    };
    crate::lock(state).connected = true;
    // When the mod let it go (nobody reads any more), and the code it closed
    // with.
    let mut dropped: Option<Instant> = None;
    let mut closing = None;
    loop {
        if dropped.is_some_and(|t| t.elapsed() > CONNECT) {
            return;
        }
        loop {
            let sent = match out.try_recv() {
                Ok(Out::Message(m)) => ws.write(m),
                Ok(Out::Close(code)) => {
                    closing = Some(code);
                    ws.close(Some(CloseFrame { code: CloseCode::from(code), reason: "".into() }))
                }
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) if dropped.is_none() => {
                    dropped = Some(Instant::now());
                    ws.close(None)
                }
                Err(TryRecvError::Disconnected) => break,
            };
            if sent.is_err() {
                break;
            }
        }
        // Written out now (a read sends only the replies to pings and
        // closes); an error shows at the read.
        let _ = ws.flush();
        let full = crate::lock(state).queued > QUEUED;
        if full && dropped.is_none() {
            std::thread::sleep(TICK);
            continue;
        }
        let message = match ws.read() {
            Ok(Message::Text(t)) => t.as_bytes().to_vec(),
            Ok(Message::Binary(b)) => b.to_vec(),
            Ok(Message::Close(frame)) => {
                crate::lock(state).close_code = Some(frame.map_or(NO_STATUS, |f| f.code.into()));
                continue;
            }
            Ok(_) => continue,
            Err(tungstenite::Error::Io(e)) if matches!(e.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut) => {
                continue;
            }
            Err(e) => {
                let mut s = crate::lock(state);
                s.closed = true;
                s.close_code.get_or_insert(match e {
                    tungstenite::Error::ConnectionClosed | tungstenite::Error::AlreadyClosed => {
                        closing.unwrap_or(NO_STATUS)
                    }
                    _ => ABNORMAL,
                });
                return;
            }
        };
        if dropped.is_none() {
            let mut s = crate::lock(state);
            s.queued += message.len();
            s.incoming.push_back(message);
        }
    }
}

/// Adds `socket` to the `net` table.
pub fn install(lua: &Lua, net: &Table) -> mlua::Result<()> {
    let open = Arc::new(AtomicUsize::new(0));
    let start = lua.create_function(move |_, url: String| {
        if open.load(Ordering::Relaxed) >= SOCKETS {
            return Err(mlua::Error::runtime(format!("net.socket({url}): a mod has {SOCKETS} sockets open at most")));
        }
        let state = Arc::new(Mutex::new(State::default()));
        let (tx, rx) = std::sync::mpsc::channel();
        let (s, u, o) = (state.clone(), url.clone(), open.clone());
        open.fetch_add(1, Ordering::Relaxed);
        glue5_runtime::qos::spawn("mod socket", glue5_runtime::qos::Qos::Utility, move || {
            let _ = std::panic::catch_unwind(|| run(&u, &s, &rx));
            let mut state = crate::lock(&s);
            state.closed = true;
            if state.connected {
                state.close_code.get_or_insert(ABNORMAL);
            } else {
                state.error.get_or_insert_with(|| "the socket failed".into());
            }
            drop(state);
            o.fetch_sub(1, Ordering::Relaxed);
        })
        .map_err(|e| {
            open.fetch_sub(1, Ordering::Relaxed);
            mlua::Error::runtime(format!("net.socket({url}): no thread for it: {e}"))
        })?;
        Ok(Socket { url, state, out: tx })
    })?;
    let socket: Function = lua
        .load(
            "local start, yield, setmetatable = ...\n\
             return function(url)\n\
             local s = start(url)\n\
             while not s:connected() do yield(0) end\n\
             local socket = {}\n\
             function socket.send(_, message, binary) s:send(message, binary) end\n\
             function socket.close(_, code) s:close(code) end\n\
             function socket.next(_)\n\
             while true do\n\
             local message, closed = s:recv()\n\
             if message then return message end\n\
             if closed then return nil end\n\
             yield(0)\n\
             end end\n\
             return setmetatable(socket, { __index = function(_, key)\n\
             if key == 'closeCode' then return s:closeCode() end\n\
             return nil end })\n\
             end",
        )
        .set_name("=net.socket")
        .call((
            start,
            lua.globals().get::<Table>("coroutine")?.get::<Function>("yield")?,
            lua.globals().get::<Function>("setmetatable")?,
        ))?;
    net.set("socket", socket)
}

#[cfg(test)]
mod tests {
    use std::net::TcpListener;

    use super::*;

    fn run_mod(lua: &Lua, code: &str) -> mlua::Result<Value> {
        let thread = lua.create_thread(lua.load(code).into_function()?)?;
        let start = std::time::Instant::now();
        loop {
            let v: Value = thread.resume(())?;
            if !thread.is_resumable() {
                return Ok(v);
            }
            assert!(start.elapsed() < Duration::from_secs(20), "no answer");
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    /// A WebSocket server here that echoes, then closes with 4000 after
    /// "bye".
    fn echo() -> String {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        std::thread::spawn(move || {
            for stream in listener.incoming().take(2) {
                let mut ws = tungstenite::accept(stream.unwrap()).unwrap();
                loop {
                    match ws.read() {
                        Ok(Message::Text(t)) if t.as_str() == "bye" => {
                            ws.close(Some(CloseFrame { code: CloseCode::from(4000), reason: "".into() })).unwrap();
                        }
                        Ok(m @ (Message::Text(_) | Message::Binary(_))) => ws.send(m).unwrap(),
                        Ok(_) => {}
                        Err(_) => break,
                    }
                }
            }
        });
        format!("ws://{address}")
    }

    fn lua() -> Lua {
        let lua = Lua::new();
        let net = lua.create_table().unwrap();
        install(&lua, &net).unwrap();
        lua.globals().set("net", net).unwrap();
        lua
    }

    #[test]
    fn echoes_and_closes() {
        let lua = lua();
        lua.globals().set("url", echo()).unwrap();
        let v = run_mod(
            &lua,
            r#"
            local s = net.socket(url)
            assert(s.closeCode == nil)
            s:send("hello")
            s:send(buffer.fromstring("\0\1"), true)
            assert(s:next() == "hello")
            assert(s:next() == "\0\1")
            s:send("bye")
            assert(s:next() == nil)
            local t = net.socket(url)
            t:close()
            assert(t:next() == nil and t.closeCode == 1000, t.closeCode)
            return tostring(s.closeCode)
            "#,
        )
        .unwrap();
        assert_eq!(v.as_string().unwrap().to_str().unwrap(), "4000");
    }

    #[test]
    fn failures_are_errors() {
        let lua = lua();
        let port = TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
        let e = run_mod(&lua, &format!("return net.socket('ws://127.0.0.1:{port}/')")).unwrap_err().to_string();
        assert!(e.contains(&format!("net.socket(ws://127.0.0.1:{port}/)")), "{e}");
        let e = run_mod(&lua, "return net.socket('http://example.com/')").unwrap_err().to_string();
        assert!(e.contains("ws:// or wss://"), "{e}");
    }

    /// wss:// to the internet (`--ignored`: it needs the network).
    #[test]
    #[ignore]
    fn wss() {
        let lua = lua();
        let v = run_mod(&lua, "local s = net.socket('wss://ws.postman-echo.com/raw') s:send('glue5') local m = s:next() s:close() return m")
            .unwrap();
        assert_eq!(v.as_string().unwrap().to_str().unwrap(), "glue5");
    }
}
