//! The players' network, with no server of ours: one UDP socket per player,
//! carrying the engine's datagrams between the players of a lobby.
//!
//! A lobby is a virtual LAN (see vnet.rs): each player has an address on
//! it, and the game finds and joins sessions on it with its own LAN
//! matchmaking. The public lobby is everyone playing; a private one is
//! whoever knows its code.
//!
//! Players find each other
//! - on the same machine, at the lobby ports;
//! - on the same network, by broadcasting to them and by greeting the
//!   network's addresses one by one (iOS lets broadcasts through only with
//!   an entitlement Apple must grant; NATs seldom carry datagrams from one
//!   of their own players to another's public address);
//! - on the Internet, through the BitTorrent DHT (dht.rs), which gives each
//!   the others' public addresses (the ones their NATs opened).
//!
//! Two players behind NATs both send to each other until the NATs let the
//! datagrams through ("hole punching"); each new player is introduced to
//! the others by the first player it reaches, and while two players cannot
//! reach each other directly, that player relays between them.
//!
//! Mobile networks' NATs give a new port to each address a phone sends to
//! (one after the other), so the port the DHT hands out is not the one its
//! datagrams to another player come from, and a home router lets these in
//! only from where it sent to. A player whose NAT keeps one port for all
//! (the nodes agree on its address) and who does not hear from a far
//! player sends to the ports around each of the ones handed out
//! (`spray_ports`): its router then lets in the far player's greetings,
//! whichever port its NAT picked.
//!
//! The game looks for a session only for a moment (about a second) and
//! hosts one when it finds none, so it waits for the others first
//! (`may_enter`): until the lobby's players are known, and while another
//! player who started at the same time goes first and hosts.
//!
//! A player greets back, keeps or introduces to the others only someone
//! who proved it receives at the address its greeting came from
//! (`Cookies`): a greeting with someone else's address as its source gets
//! that address a datagram no larger than the greeting, nothing more.
//!
//! The socket is the player's from the application's start: friends meet
//! on it too (friends/), before and besides any lobby. A player can be in
//! more than one lobby at a time: its game's, and a friend's it was
//! invited to or joins (`visit_lobby`), whose players the friend
//! introduces; the virtual LAN spans them.

use std::collections::{HashMap, HashSet};
use std::hash::{BuildHasher, Hasher};
use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4, UdpSocket};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicU16, AtomicU32, Ordering};
use std::sync::{Mutex, OnceLock, RwLock, RwLockReadGuard};
use std::time::{Duration, Instant};

use sha2::{Digest, Sha256};

use crate::Locked;
use crate::dht::{self, Dht};
use crate::exit;

/// The lobby ports, also the ports looked at for players on the same
/// network: several players on one machine take the next free one.
pub(crate) const PORTS: std::ops::Range<u16> = 47000..47016;
/// The virtual LAN: 10.77.0.0/16.
pub const NET: u32 = 10 << 24 | 77 << 16;
pub(crate) const MAGIC: [u8; 3] = [b'G', b'5', 1];
const HELLO: u8 = 1;
const PEERS: u8 = 2;
const DATA: u8 = 3;
/// A datagram that only opens this player's NAT to where it goes (nobody
/// answers it).
const OPEN: u8 = 4;
/// The answer to a greeting without the cookie of its address: the cookie,
/// to greet with again (see `Cookies`).
const COOKIE: u8 = 5;
/// Kinds of datagram from here on are the friends' (friends/).
pub(crate) const FRIENDS: u8 = 10;
/// HELLO flags: please answer; the player is about to look for a session;
/// it went looking (and is in one, or hosts it).
const ASK: u8 = 1;
const WAITING: u8 = 2;
const PLAYING: u8 = 4;
/// A greeting answering a cookie: never answered by a cookie (two players
/// would send each other cookies for ever).
const ANSWER: u8 = 8;
/// A greeting after the kind: the lobby, the player's id, address and
/// flags, the cookie the player greeted gave for the address it comes from
/// (zero when none yet), the cookie of the address it goes to; then the
/// player's card. A cookie's datagram is as large as the smallest
/// greeting: the greeting it gets back is not larger.
const HELLO_LEN: usize = 8 + 8 + 4 + 1 + 8 + 8;
const HELLO_SIZE: usize = MAGIC.len() + 1 + HELLO_LEN;
/// How often players greet each other (keeping the NATs' mappings open),
/// and when a silent one is gone.
/// The network thread's clock: the DHT's and the friends' ticks (nothing
/// is due sooner; a datagram ends the wait at once).
const TICK: Duration = Duration::from_millis(250);
const KEEPALIVE: Duration = Duration::from_secs(2);
const GONE: Duration = Duration::from_secs(20);
/// How long an address is tried before giving up on it.
const PUNCH: Duration = Duration::from_secs(30);
/// How often the players reached are introduced to each other again.
const INTRODUCE: Duration = Duration::from_secs(8);
/// How often the local network's addresses are greeted.
const SWEEP: Duration = Duration::from_secs(5);
/// Before looking for a session: how long the DHT may take to find the
/// lobby's players (from the start), how long a player who went first
/// takes to host, and how long to wait at most.
const DISCOVERY: Duration = Duration::from_secs(40);
const HOSTING: Duration = Duration::from_secs(6);
const PATIENCE: Duration = Duration::from_secs(30);
/// Lobbies at most at a time: the game's and friends' visited.
const LOBBIES: usize = 4;
/// Players known per lobby, and addresses being reached, at most: bounds
/// for floods of strangers' greetings, well above what lobbies hold (the
/// public one has every player looking for a session at once).
const PLAYERS: usize = 512;
const TRYING: usize = 1024;
/// Addresses kept per player it greeted from, and of this player's own.
const SEEN: usize = 4;
const OWN: usize = 16;
/// How long a cookie holds: one to two of these.
const COOKIE_EPOCH: Duration = Duration::from_secs(60);
/// How often a lobby's players are asked for on the DHT (a player who
/// cannot greet this one is reached only once this one hears of it).
const LOBBY_REFRESH: Duration = Duration::from_secs(10);
/// Errors in a row from the socket after which it is made again.
const REBIND_AFTER: u32 = 20;
/// Toward a far player not heard from (`Far`): how long plain greetings
/// get, how often the ports around its own are opened again, how many,
/// and for how many players at a time.
const SPRAY_AFTER: Duration = Duration::from_secs(4);
const SPRAY_EVERY: Duration = Duration::from_secs(10);
const SPRAY_PORTS: usize = 1536;
/// Below and above each port handed out: the NAT hands out the next ones
/// meanwhile (hundreds in seconds while the far player starts and looks
/// up its keys).
const SPRAY_BELOW: u16 = 64;
const SPRAY_ABOVE: u16 = 640;
/// Ports handed out for a far player kept, the latest.
const FAR_PORTS: usize = 16;
const SPRAYING: usize = 4;

/// A lobby's id in the players' datagrams, and its key on the DHT.
pub(crate) type LobbyId = [u8; 8];

struct Lobby {
    /// What the players typed ("" for everyone's).
    name: String,
    id: LobbyId,
    key: dht::Id,
    joined: Instant,
}

impl Lobby {
    fn new(name: &str, now: Instant) -> Lobby {
        // 2: greetings with cookies (players of the builds before are in
        // lobbies of their own: they would greet without).
        let key: [u8; 32] = Sha256::digest(format!("glue5 lobby 2 {} {name}", crate::GAME_BUILD)).into();
        Lobby { name: name.to_owned(), id: key[..8].try_into().unwrap(), key: key[..20].try_into().unwrap(), joined: now }
    }
}

/// What a player tells the other players of its lobbies about itself, in
/// its greetings, for friends' sake (friends/): its gamer id, name and
/// friends' key, which a player it meets asks to be friends with.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Card {
    pub gamer: u64,
    pub key: [u8; 32],
    pub name: String,
}

const CARD_NAME: usize = 32;

static CARD: Mutex<Option<Card>> = Mutex::new(None);

/// This player's card, from now on.
pub(crate) fn set_card(card: Card) {
    *CARD.locked() = Some(card);
}

impl Card {
    fn write(&self, d: &mut Vec<u8>) {
        d.extend_from_slice(&self.gamer.to_be_bytes());
        d.extend_from_slice(&self.key);
        let mut n = self.name.len().min(CARD_NAME);
        while !self.name.is_char_boundary(n) {
            n -= 1;
        }
        d.push(n as u8);
        d.extend_from_slice(&self.name.as_bytes()[..n]);
    }

    fn read(d: &[u8]) -> Option<Card> {
        let gamer = u64::from_be_bytes(d.get(..8)?.try_into().unwrap());
        let key = d.get(8..40)?.try_into().unwrap();
        let n = usize::from(*d.get(40)?).min(CARD_NAME);
        let name = String::from_utf8_lossy(d.get(41..41 + n)?).into_owned();
        Some(Card { gamer, key, name })
    }
}

struct Peer {
    vip: u32,
    /// The lobby it was met in.
    lobby: LobbyId,
    /// What it says about itself.
    card: Option<Card>,
    /// Where its datagrams come from, when they do, and the last few
    /// addresses it greeted from (a LAN's and the Internet's).
    addr: Option<SocketAddrV4>,
    seen: Vec<SocketAddrV4>,
    /// When it was last heard from directly, or introduced.
    heard: Instant,
    introduced: Instant,
    /// Who introduced it: the relay while it cannot be reached directly.
    via: Option<SocketAddrV4>,
    /// Its WAITING and PLAYING flags, and since when it plays (seen).
    flags: u8,
    playing: Option<Instant>,
}

impl Peer {
    fn new(vip: u32, lobby: LobbyId, now: Instant) -> Peer {
        let heard = now.checked_sub(GONE).unwrap_or(now);
        Peer { vip, lobby, card: None, addr: None, seen: Vec::new(), heard, introduced: now, via: None, flags: 0, playing: None }
    }

    fn direct(&self, now: Instant) -> bool {
        self.addr.is_some() && now - self.heard < KEEPALIVE * 3
    }
}

/// A player on the Internet whose NAT may give its datagrams here another
/// port than the DHT's: the ports handed out for its address (the latest
/// last), since when.
struct Far {
    ports: Vec<u16>,
    since: Instant,
    sprayed: Option<Instant>,
}

/// The ports a far player's NAT may send from to this player, given the
/// ones the DHT handed out: around each, mostly above (its NAT hands them
/// out in order, and greets this player after announcing itself), the
/// latest first while there is room. The DHT also keeps the ones of
/// the player's earlier runs, and the NAT may have moved on elsewhere.
fn spray_ports(ports: &[u16]) -> Vec<std::ops::RangeInclusive<u16>> {
    let mut ranges: Vec<std::ops::RangeInclusive<u16>> = Vec::new();
    let mut room = SPRAY_PORTS;
    for &port in ports.iter().rev() {
        if room == 0 {
            break;
        }
        let start = port.saturating_sub(SPRAY_BELOW).max(1024);
        let end = port.saturating_add(SPRAY_ABOVE).max(start);
        room = room.saturating_sub(usize::from(end - start) + 1);
        ranges.push(start..=end);
    }
    ranges.sort_by_key(|r| *r.start());
    let mut merged: Vec<std::ops::RangeInclusive<u16>> = Vec::new();
    for r in ranges {
        match merged.last_mut() {
            Some(last) if *r.start() <= last.end().saturating_add(1) => *last = *last.start()..=*last.end().max(r.end()),
            _ => merged.push(r),
        }
    }
    merged
}

/// How close an address is: this machine, this network, the Internet.
fn nearness(addr: SocketAddrV4) -> u8 {
    let ip = addr.ip();
    if ip.is_loopback() {
        0
    } else if ip.is_private() || ip.is_link_local() {
        1
    } else {
        2
    }
}

/// Return routability, without keeping anything for strangers: a player
/// answers a greeting in full, and keeps its sender, only when it carries
/// the cookie of the address it comes from, a MAC of that address and the
/// time under a secret of this process. Else it answers with the cookie
/// alone: who greets again with it receives at that address. Each
/// greeting also gives the cookie of the address it goes to, which the
/// player there greets back with: once one side has the other's cookie, a
/// round trip proves both addresses (two players behind NATs, both
/// greeting first, get there too: each answers the other's cookie). A
/// broadcast's cookie is no address's: the players on the network answer
/// it with theirs.
struct Cookies {
    secret: [u8; 32],
    born: Instant,
}

impl Cookies {
    fn new(now: Instant) -> Cookies {
        let mut secret = [0; 32];
        getrandom::fill(&mut secret).expect("the system's random numbers");
        Cookies { secret, born: now }
    }

    fn epoch(&self, now: Instant) -> u64 {
        now.saturating_duration_since(self.born).as_secs() / COOKIE_EPOCH.as_secs()
    }

    fn at(&self, addr: SocketAddrV4, epoch: u64) -> [u8; 8] {
        use blake2::digest::Mac;
        let mut mac = blake2::Blake2sMac::<blake2::digest::consts::U8>::new_from_slice(&self.secret).unwrap();
        mac.update(&epoch.to_be_bytes());
        mac.update(&addr.ip().octets());
        mac.update(&addr.port().to_be_bytes());
        mac.finalize().into_bytes().into()
    }

    /// The cookie of `addr` now.
    fn of(&self, addr: SocketAddrV4, now: Instant) -> [u8; 8] {
        self.at(addr, self.epoch(now))
    }

    /// Whether `cookie` is `addr`'s, of now or of the epoch before.
    fn check(&self, addr: SocketAddrV4, cookie: &[u8; 8], now: Instant) -> bool {
        let epoch = self.epoch(now);
        *cookie == self.at(addr, epoch) || epoch > 0 && *cookie == self.at(addr, epoch - 1)
    }
}

/// A greeting, read.
struct Hello {
    lobby: LobbyId,
    id: u64,
    vip: u32,
    flags: u8,
    /// The cookie it carries (of the address it comes from, if right), and
    /// the one it gives (of this player's address, to greet back with).
    cookie: [u8; 8],
    gives: [u8; 8],
    card: Option<Card>,
}

impl Hello {
    fn read(d: &[u8]) -> Option<Hello> {
        let h = d.get(..HELLO_LEN)?;
        Some(Hello {
            lobby: h[..8].try_into().unwrap(),
            id: u64::from_be_bytes(h[8..16].try_into().unwrap()),
            vip: u32::from_be_bytes(h[16..20].try_into().unwrap()),
            flags: h[20],
            cookie: h[21..29].try_into().unwrap(),
            gives: h[29..37].try_into().unwrap(),
            card: Card::read(&d[HELLO_LEN..]),
        })
    }
}

struct State {
    /// The game's lobby first, then the ones visited.
    lobbies: Vec<Lobby>,
    /// By peer id.
    peers: HashMap<u64, Peer>,
    /// Addresses tried, not answering yet: since when, for which lobby.
    trying: HashMap<SocketAddrV4, (Instant, LobbyId)>,
    /// This player's own addresses (the DHT hands them out too), proven
    /// as any player's.
    own: HashSet<SocketAddrV4>,
    /// The cookies the players at these addresses gave (`Cookies`), which
    /// the greetings there carry.
    given: HashMap<SocketAddrV4, [u8; 8]>,
    /// When players' addresses were last heard from, and the far ones not
    /// heard from yet.
    heard: HashMap<Ipv4Addr, Instant>,
    far: HashMap<Ipv4Addr, Far>,
    introduced: Instant,
    dht: Dht,
}

impl State {
    fn lobby(&self, id: &LobbyId) -> Option<&Lobby> {
        self.lobbies.iter().find(|l| l.id == *id)
    }

    /// Whether an engine datagram from `from` comes from the player at
    /// `src` on the virtual LAN: sent from one of its addresses (`Some(true)`),
    /// or relayed by another player while it is not reached directly
    /// (`Some(false)`). Never from this player's address (`own`) or a
    /// broadcast's.
    fn sender(&self, from: SocketAddrV4, src: u32, own: u32, now: Instant) -> Option<bool> {
        if src == own || !is_player_vip(src) {
            return None;
        }
        let at = |p: &Peer| p.addr == Some(from) || p.seen.contains(&from);
        let mut players = self.peers.values().filter(|p| p.vip == src);
        if players.clone().any(at) {
            return Some(true);
        }
        let relay = self.peers.values().any(|p| at(p) && p.vip != src);
        (relay && players.any(|p| !p.direct(now))).then_some(false)
    }

    /// A player's address from the DHT: a far one when its NAT changed the
    /// port (one keeping the lobby port keeps it for all) and it is not
    /// heard from.
    fn note_far(&mut self, addr: SocketAddrV4, now: Instant) {
        if nearness(addr) < 2 || PORTS.contains(&addr.port()) || self.heard.get(addr.ip()).is_some_and(|t| now - *t < GONE) {
            return;
        }
        let far = self.far.entry(*addr.ip()).or_insert(Far { ports: Vec::new(), since: now, sprayed: None });
        if now - far.since > PUNCH {
            *far = Far { ports: Vec::new(), since: now, sprayed: None };
        }
        far.ports.retain(|p| *p != addr.port());
        far.ports.push(addr.port());
        if far.ports.len() > FAR_PORTS {
            far.ports.remove(0);
        }
    }

    /// The far players whose ports around their own are due to be opened
    /// (only by a player whose NAT keeps one port for all: `easy`).
    fn sprays(&mut self, easy: bool, now: Instant) -> Vec<(Ipv4Addr, Vec<std::ops::RangeInclusive<u16>>)> {
        self.heard.retain(|_, t| now - *t < GONE);
        let heard = &self.heard;
        self.far.retain(|ip, f| now - f.since < PUNCH && heard.get(ip).is_none_or(|t| *t < f.since));
        if !easy {
            return Vec::new();
        }
        self.far
            .iter_mut()
            .filter(|(_, f)| now - f.since >= SPRAY_AFTER && f.sprayed.is_none_or(|t| now - t >= SPRAY_EVERY))
            .take(SPRAYING)
            .map(|(ip, f)| {
                f.sprayed = Some(now);
                (*ip, spray_ports(&f.ports))
            })
            .collect()
    }
}

pub struct P2p {
    /// Made again when it fails for good (`rebind`), on its port if it can.
    socket: RwLock<UdpSocket>,
    port: AtomicU16,
    id: u64,
    vip: AtomicU32,
    /// The engine took the address: it cannot change any more.
    vip_taken: AtomicBool,
    /// This player's WAITING and PLAYING flags, and since when it waits.
    flags: AtomicU8,
    waiting: Mutex<Option<Instant>>,
    state: Mutex<State>,
    cookies: Cookies,
}

static P2P: OnceLock<P2p> = OnceLock::new();

pub(crate) fn random() -> u64 {
    std::collections::hash_map::RandomState::new().build_hasher().finish()
}

/// A socket for the player, at the first of `ports` free (0: any).
fn bind(ports: impl IntoIterator<Item = u16>) -> std::io::Result<UdpSocket> {
    let mut error = std::io::Error::from(std::io::ErrorKind::AddrNotAvailable);
    for port in ports {
        match UdpSocket::bind(SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, port)) {
            Ok(socket) => {
                socket.set_broadcast(true)?;
                socket.set_read_timeout(Some(TICK))?;
                return Ok(socket);
            }
            Err(e) => error = e,
        }
    }
    Err(error)
}

/// The player's socket and the thread serving it, started once.
pub fn start() -> std::io::Result<&'static P2p> {
    static STARTING: Mutex<()> = Mutex::new(());
    let _one = STARTING.locked();
    if let Some(p2p) = P2P.get() {
        return Ok(p2p);
    }
    let socket = bind(PORTS.chain([0]))?;
    let port = socket.local_addr()?.port();
    let id = random();
    let dht_id: dht::Id = Sha256::digest(id.to_le_bytes()).as_slice()[..20].try_into().unwrap();
    let p2p = P2p {
        socket: RwLock::new(socket),
        port: AtomicU16::new(port),
        id,
        vip: AtomicU32::new(0),
        vip_taken: AtomicBool::new(false),
        flags: AtomicU8::new(0),
        waiting: Mutex::new(None),
        state: Mutex::new(State {
            lobbies: Vec::new(),
            peers: HashMap::new(),
            trying: HashMap::new(),
            own: Default::default(),
            given: HashMap::new(),
            heard: HashMap::new(),
            far: HashMap::new(),
            introduced: Instant::now(),
            dht: Dht::new(dht_id, port),
        }),
        cookies: Cookies::new(Instant::now()),
    };
    p2p.vip.store(free_vip(&[]), Ordering::Relaxed);
    let p2p = P2P.get_or_init(|| p2p);
    log::info!("p2p: port {port}, address {}", Ipv4Addr::from(p2p.vip.load(Ordering::Relaxed)));
    crate::qos::spawn("p2p", crate::qos::Qos::Initiated, move || p2p.run())?;
    Ok(p2p)
}

pub fn get() -> Option<&'static P2p> {
    P2P.get()
}

/// An address none of `taken` is: 10.77.a.b, b not 0 or 255.
fn free_vip(taken: &[u32]) -> u32 {
    loop {
        let r = random() as u32;
        let vip = NET | (r & 0xff00) | (r % 254 + 1);
        if !taken.contains(&vip) {
            return vip;
        }
    }
}

fn is_broadcast(ip: u32) -> bool {
    ip == u32::MAX || (ip & 0xffff_0000 == NET && ip & 0xff == 0xff)
}

/// Whether a player may have `vip` (see `free_vip`).
fn is_player_vip(vip: u32) -> bool {
    vip & 0xffff_0000 == NET && !matches!(vip & 0xff, 0 | 0xff)
}

/// This device's address on its local network (the one its Internet
/// traffic leaves from), when the network is a private one. Connecting a
/// UDP socket sends nothing. Not a phone's mobile data, whose private
/// addresses are the carrier's (its /24 holds strangers, not players).
pub(crate) fn lan_address() -> Option<Ipv4Addr> {
    let socket = UdpSocket::bind(SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, 0)).ok()?;
    socket.connect(SocketAddrV4::new(Ipv4Addr::new(192, 0, 2, 1), 9)).ok()?;
    match socket.local_addr().ok()? {
        SocketAddr::V4(a) if a.ip().is_private() && on_broadcast_network(*a.ip()) => Some(*a.ip()),
        _ => None,
    }
}

/// Whether `ip` is on a network with neighbours (Wi-Fi, Ethernet): its
/// interface broadcasts and is not point-to-point, as mobile data's are.
#[cfg(unix)]
fn on_broadcast_network(ip: Ipv4Addr) -> bool {
    let mut found = false;
    // SAFETY: the list is walked as getifaddrs made it, then freed.
    unsafe {
        let mut list: *mut libc::ifaddrs = std::ptr::null_mut();
        if libc::getifaddrs(&mut list) != 0 {
            return true;
        }
        let mut at = list;
        while !at.is_null() {
            let a = (*at).ifa_addr;
            if !a.is_null() && (*a).sa_family as i32 == libc::AF_INET {
                let sin = &*(a as *const libc::sockaddr_in);
                if Ipv4Addr::from(u32::from_be(sin.sin_addr.s_addr)) == ip {
                    let flags = (*at).ifa_flags;
                    found = flags & libc::IFF_BROADCAST as u32 != 0 && flags & libc::IFF_POINTOPOINT as u32 == 0;
                    break;
                }
            }
            at = (*at).ifa_next;
        }
        libc::freeifaddrs(list);
    }
    found
}

#[cfg(not(unix))]
fn on_broadcast_network(_: Ipv4Addr) -> bool {
    true
}

impl P2p {
    /// This player's address on the virtual LAN; the engine takes it.
    pub fn local_ip(&self) -> u32 {
        self.vip_taken.store(true, Ordering::Relaxed);
        self.vip.load(Ordering::Relaxed)
    }

    pub(crate) fn port(&self) -> u16 {
        self.port.load(Ordering::Relaxed)
    }

    fn socket(&self) -> RwLockReadGuard<'_, UdpSocket> {
        self.socket.read().unwrap()
    }

    /// Makes the socket again, the old one failing for good (iOS takes back
    /// a suspended app's sockets): on its port if it is free (the old one
    /// let go first, a stand-in meanwhile), else on any.
    fn rebind(&self) {
        let mut socket = self.socket.write().unwrap();
        let stand_in = match bind([0]) {
            Ok(s) => s,
            Err(e) => {
                log::warn!("p2p: the socket fails, and no new one: {e}");
                return;
            }
        };
        drop(std::mem::replace(&mut *socket, stand_in));
        if let Ok(again) = bind([self.port()]) {
            *socket = again;
        }
        let port = socket.local_addr().map_or(0, |a| a.port());
        self.port.store(port, Ordering::Relaxed);
        log::warn!("p2p: the socket failed: a new one, port {port}");
    }

    /// Joins the game's lobby: `name` is any text, "" for everyone's (for
    /// this game build).
    pub fn join_lobby(&self, name: &str) {
        let now = Instant::now();
        let mut state = self.state.locked();
        if state.lobbies.first().is_some_and(|l| l.name == name) {
            return;
        }
        let lobby = Lobby::new(name, now);
        log::info!("p2p: lobby {:02x?}", lobby.id);
        state.dht.add(lobby.key, dht::Options { announce: true, refresh: LOBBY_REFRESH });
        state.lobbies.retain(|l| l.name != name);
        state.lobbies.insert(0, lobby);
    }

    /// Moves the game to another lobby (a session of another kind: everyone's
    /// or a private one): the lobbies visited, and their players, are left.
    pub(crate) fn switch_lobby(&self, name: &str) {
        let now = Instant::now();
        let mut state = self.state.locked();
        if state.lobbies.len() == 1 && state.lobbies[0].name == name {
            return;
        }
        let state = &mut *state;
        for lobby in state.lobbies.drain(..) {
            state.dht.remove(&lobby.key);
        }
        state.peers.clear();
        state.trying.clear();
        let lobby = Lobby::new(name, now);
        log::info!("p2p: lobby {:02x?}", lobby.id);
        state.dht.add(lobby.key, dht::Options { announce: true, refresh: LOBBY_REFRESH });
        state.lobbies.push(lobby);
        // Everyone's: its players are waited for again before the game
        // looks for a session there (a private one has none to wait for).
        if name.is_empty() {
            self.flags.store(0, Ordering::Relaxed);
            *self.waiting.locked() = None;
        }
    }

    /// Leaves the lobbies and their players (the game is back in the story).
    pub(crate) fn leave_lobbies(&self) {
        let mut state = self.state.locked();
        let state = &mut *state;
        for lobby in state.lobbies.drain(..) {
            state.dht.remove(&lobby.key);
        }
        state.peers.clear();
        state.trying.clear();
        self.flags.store(0, Ordering::Relaxed);
        *self.waiting.locked() = None;
        log::info!("p2p: out of the lobbies");
    }

    /// Visits a friend's lobby (`name`), the friend at `via` introducing
    /// its players: the game can then join a session there. The oldest
    /// lobby visited makes room.
    pub(crate) fn visit_lobby(&self, name: &str, via: SocketAddrV4) {
        let now = Instant::now();
        let mut state = self.state.locked();
        let lobby = Lobby::new(name, now);
        let id = lobby.id;
        if state.lobby(&id).is_none() {
            log::info!("p2p: visiting lobby {id:02x?} through {via}");
            if state.lobbies.len() >= LOBBIES {
                let gone = state.lobbies.remove(1);
                state.dht.remove(&gone.key);
                state.peers.retain(|_, p| p.lobby != gone.id);
            }
            state.dht.add(lobby.key, dht::Options { announce: true, refresh: LOBBY_REFRESH });
            state.lobbies.push(lobby);
        }
        state.trying.insert(via, (now, id));
        drop(state);
        self.hello(via, ASK, &id);
    }

    /// The players met in the lobbies who say who they are, and where they
    /// answer.
    pub(crate) fn players(&self) -> Vec<(Card, SocketAddrV4)> {
        let now = Instant::now();
        let state = self.state.locked();
        state.peers.values().filter(|p| p.direct(now)).filter_map(|p| Some((p.card.clone()?, p.addr?))).collect()
    }

    /// The lobbies the player is in (their names), the game's first.
    pub(crate) fn lobbies(&self) -> Vec<String> {
        self.state.locked().lobbies.iter().map(|l| l.name.clone()).collect()
    }

    /// Whether the engine may look for a session now (it asks until it
    /// may): once the DHT had the time to find the lobby's players, the
    /// ones it found answered or were given up on, and none of those
    /// waiting with it goes first (the lowest id) or has just gone, so that
    /// it hosts and the others join it.
    pub fn may_enter(&self) -> bool {
        if self.flags.load(Ordering::Relaxed) & PLAYING != 0 {
            return true;
        }
        let now = Instant::now();
        let since = *self.waiting.locked().get_or_insert_with(|| {
            self.flags.store(WAITING, Ordering::Relaxed);
            log::info!("p2p: waiting for the lobby's players");
            now
        });
        let state = self.state.locked();
        let Some(lobby) = state.lobbies.first() else { return true };
        let discovered = state.dht.settled(&lobby.key) || now - lobby.joined > DISCOVERY;
        let reaching = state.trying.values().any(|(_, l)| *l == lobby.id);
        let first = state.peers.iter().filter(|(_, p)| p.direct(now)).all(|(id, p)| {
            let ahead = p.flags & WAITING != 0 && *id < self.id;
            !ahead && p.playing.is_none_or(|t| now - t > HOSTING)
        });
        let go = discovered && !reaching && first || now - since > PATIENCE;
        if go {
            self.flags.store(PLAYING, Ordering::Relaxed);
            log::info!("p2p: looking for a session, {} players known", state.peers.len());
        }
        go
    }

    /// Sends an engine datagram (from `src_port`) to a player's address, or
    /// to all of them (a broadcast).
    pub fn send(&self, src_port: u16, dst: u32, dst_port: u16, payload: &[u8]) {
        let vip = self.vip.load(Ordering::Relaxed);
        let mut d = Vec::with_capacity(MAGIC.len() + 13 + payload.len());
        d.extend_from_slice(&MAGIC);
        d.push(DATA);
        d.extend_from_slice(&vip.to_be_bytes());
        d.extend_from_slice(&src_port.to_be_bytes());
        d.extend_from_slice(&dst.to_be_bytes());
        d.extend_from_slice(&dst_port.to_be_bytes());
        d.extend_from_slice(payload);
        const DST: std::ops::Range<usize> = MAGIC.len() + 7..MAGIC.len() + 11;
        let now = Instant::now();
        let state = self.state.locked();
        let socket = self.socket();
        for peer in state.peers.values() {
            if peer.vip != dst && !is_broadcast(dst) {
                continue;
            }
            let Some(to) = Self::path(peer, now) else { continue };
            // Through a relay a broadcast goes to the player alone (the
            // relay would keep it).
            if is_broadcast(dst) && Some(to) != peer.addr {
                d[DST].copy_from_slice(&peer.vip.to_be_bytes());
                let _ = socket.send_to(&d, to);
                d[DST].copy_from_slice(&dst.to_be_bytes());
            } else {
                let _ = socket.send_to(&d, to);
            }
        }
    }

    /// A datagram of the friends' (friends/), as it is.
    pub(crate) fn send_to(&self, to: SocketAddrV4, d: &[u8]) {
        let _ = self.socket().send_to(d, to);
    }

    /// The DHT, for the friends' lookups.
    pub(crate) fn dht<R>(&self, f: impl FnOnce(&mut Dht) -> R) -> R {
        f(&mut self.state.locked().dht)
    }

    /// Directly while the player answers there, else through who
    /// introduced it.
    fn path(peer: &Peer, now: Instant) -> Option<SocketAddrV4> {
        if peer.direct(now) { peer.addr } else { peer.via.or(peer.addr) }
    }

    /// Greets `to` with the cookie it gave, if any. The card only then:
    /// before, the player there answers with a cookie alone.
    fn hello(&self, to: SocketAddrV4, flags: u8, lobby: &LobbyId) {
        let cookie = self.state.locked().given.get(&to).copied();
        self.hello_with(to, flags, lobby, cookie);
    }

    fn hello_with(&self, to: SocketAddrV4, flags: u8, lobby: &LobbyId, cookie: Option<[u8; 8]>) {
        let mut d = Vec::with_capacity(HELLO_SIZE + 41 + CARD_NAME);
        d.extend_from_slice(&MAGIC);
        d.push(HELLO);
        d.extend_from_slice(lobby);
        d.extend_from_slice(&self.id.to_be_bytes());
        d.extend_from_slice(&self.vip.load(Ordering::Relaxed).to_be_bytes());
        d.push(flags | self.flags.load(Ordering::Relaxed));
        d.extend_from_slice(&cookie.unwrap_or_default());
        d.extend_from_slice(&self.cookies.of(to, Instant::now()));
        if cookie.is_some()
            && flags & ANSWER == 0
            && let Some(card) = CARD.locked().as_ref()
        {
            card.write(&mut d);
        }
        let _ = self.socket().send_to(&d, to);
    }

    /// The answer to a greeting without the cookie of `to`: the cookie.
    fn cookie(&self, to: SocketAddrV4, lobby: &LobbyId, now: Instant) {
        let mut d = Vec::with_capacity(HELLO_SIZE);
        d.extend_from_slice(&MAGIC);
        d.push(COOKIE);
        d.extend_from_slice(lobby);
        d.extend_from_slice(&self.cookies.of(to, now));
        d.resize(HELLO_SIZE, 0);
        let _ = self.socket().send_to(&d, to);
    }

    /// Introduces players of `lobby`: `about` to `to`.
    fn peers(&self, to: SocketAddrV4, lobby: &LobbyId, about: &[(u64, u32, SocketAddrV4)]) {
        for chunk in about.chunks(32) {
            let mut d = Vec::with_capacity(16 + chunk.len() * 18);
            d.extend_from_slice(&MAGIC);
            d.push(PEERS);
            d.extend_from_slice(lobby);
            for (id, vip, addr) in chunk {
                d.extend_from_slice(&id.to_be_bytes());
                d.extend_from_slice(&vip.to_be_bytes());
                d.extend_from_slice(&addr.ip().octets());
                d.extend_from_slice(&addr.port().to_be_bytes());
            }
            let _ = self.socket().send_to(&d, to);
        }
    }

    fn run(&self) {
        let mut buf = [0u8; 2048];
        let mut ticked = Instant::now() - KEEPALIVE;
        let mut dht_ticked = ticked;
        let mut swept = ticked - SWEEP;
        let mut failing = 0;
        loop {
            let received = self.socket().recv_from(&mut buf);
            match received {
                Ok((len, SocketAddr::V4(from))) => {
                    failing = 0;
                    // Anyone can send one: a mistake in reading it drops
                    // the datagram, not the game.
                    let d = &buf[..len];
                    let read = exit::recoverable(|| catch_unwind(AssertUnwindSafe(|| self.receive(from, d))));
                    if read.is_err() {
                        log::warn!("p2p: a datagram from {from} could not be read");
                    }
                }
                Ok(_) => failing = 0,
                Err(e) if matches!(e.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut) => failing = 0,
                // ICMP port unreachable, from a player gone, and a datagram
                // larger than the buffer (Windows says so).
                Err(e) if e.kind() == std::io::ErrorKind::ConnectionReset => {}
                #[cfg(windows)]
                Err(e) if e.raw_os_error() == Some(10040) => {}
                // The socket fails (taken back by the system): again and
                // again, waiting longer each time, then it is made again.
                Err(e) => {
                    failing += 1;
                    if failing == 1 {
                        log::warn!("p2p: the socket fails: {e}");
                    }
                    std::thread::sleep(Duration::from_millis(10 * u64::from(failing)));
                    if failing >= REBIND_AFTER {
                        failing = 0;
                        self.rebind();
                    }
                }
            }
            let now = Instant::now();
            if now - dht_ticked >= TICK {
                dht_ticked = now;
                let mut out = dht::Output::default();
                self.state.locked().dht.tick(now, &mut out);
                self.dht_output(out);
                crate::friends::tick(self, now);
            }
            if now - ticked > KEEPALIVE {
                ticked = now;
                self.tick(now);
            }
            if now - swept > SWEEP {
                swept = now;
                self.sweep();
            }
        }
    }

    fn dht_output(&self, out: dht::Output) {
        let socket = self.socket();
        for (to, d) in &out.send {
            let _ = socket.send_to(d, to);
        }
        drop(socket);
        if out.peers.is_empty() {
            return;
        }
        let now = Instant::now();
        let mut state = self.state.locked();
        let mut new = Vec::new();
        let mut friends = Vec::new();
        // This player (its announces, under other ports when its NAT gives
        // each node another), or players behind its NAT too: met on the
        // local network.
        let public = state.dht.public_ip();
        for (key, addr) in out.peers {
            if state.own.contains(&addr) || public == Some(*addr.ip()) {
                continue;
            }
            state.note_far(addr, now);
            let Some(lobby) = state.lobbies.iter().find(|l| l.key == key).map(|l| l.id) else {
                friends.push((key, addr));
                continue;
            };
            if !state.trying.contains_key(&addr)
                && state.trying.len() < TRYING
                && state.peers.values().all(|p| p.addr != Some(addr))
            {
                log::debug!("p2p: trying {addr}, from the DHT");
                state.trying.insert(addr, (now, lobby));
                new.push((addr, lobby));
            }
        }
        drop(state);
        for (addr, lobby) in new {
            self.hello(addr, ASK, &lobby);
        }
        for (key, addr) in friends {
            crate::friends::found(self, key, addr);
        }
    }

    fn tick(&self, now: Instant) {
        let mut state = self.state.locked();
        let lobbies: Vec<LobbyId> = state.lobbies.iter().map(|l| l.id).collect();
        state.peers.retain(|id, p| {
            let alive = (now - p.heard < GONE || now - p.introduced < GONE) && lobbies.contains(&p.lobby);
            if !alive {
                log::info!("p2p: player {id:016x} ({}) gone", Ipv4Addr::from(p.vip));
            }
            alive
        });
        state.trying.retain(|_, (since, lobby)| now - *since < PUNCH && lobbies.contains(lobby));
        // The cookies of the addresses still greeted.
        let greeted: HashSet<SocketAddrV4> =
            state.peers.values().flat_map(|p| p.seen.iter().copied().chain(p.addr)).chain(state.trying.keys().copied()).collect();
        state.given.retain(|a, _| greeted.contains(a));
        let greet: Vec<(SocketAddrV4, LobbyId)> = state
            .peers
            .values()
            .filter_map(|p| Some((p.addr?, p.lobby)))
            .chain(state.trying.iter().map(|(a, (_, l))| (*a, *l)))
            .collect();
        // Introductions again now and then: they keep the players reached
        // only through others, and retry the ones not reached yet.
        let reached: Vec<(u64, LobbyId, u32, SocketAddrV4)> = state
            .peers
            .iter()
            .filter(|(_, p)| p.direct(now))
            .map(|(i, p)| (*i, p.lobby, p.vip, p.addr.unwrap()))
            .collect();
        let introduce = now.duration_since(state.introduced) > INTRODUCE;
        if introduce {
            state.introduced = now;
        }
        let easy = state.dht.external().is_some();
        let sprays = state.sprays(easy, now);
        drop(state);
        for (ip, ranges) in sprays {
            log::debug!("p2p: opening the way from {ip}: {ranges:?}");
            let socket = self.socket();
            let mut d = MAGIC.to_vec();
            d.push(OPEN);
            for port in ranges.into_iter().flatten() {
                let _ = socket.send_to(&d, SocketAddrV4::new(ip, port));
            }
        }
        // Players on this machine or network (the ones on other machines
        // mostly at the first port, as this one).
        for lobby in &lobbies {
            for port in PORTS {
                self.hello(SocketAddrV4::new(Ipv4Addr::BROADCAST, port), ASK, lobby);
                if port != self.port() {
                    self.hello(SocketAddrV4::new(Ipv4Addr::LOCALHOST, port), ASK, lobby);
                }
            }
        }
        for (addr, lobby) in greet {
            self.hello(addr, ASK, &lobby);
        }
        if introduce {
            for (id, lobby, _, addr) in &reached {
                let others: Vec<_> = reached
                    .iter()
                    .filter(|(i, l, ..)| i != id && l == lobby)
                    .map(|(i, _, vip, a)| (*i, *vip, *a))
                    .collect();
                self.peers(*addr, lobby, &others);
            }
        }
    }

    /// Greets the local network's addresses (its /24) at the first lobby
    /// port, where broadcasts do not get through; the players there are
    /// introduced to the others by the one reached.
    fn sweep(&self) {
        let lobbies: Vec<LobbyId> = self.state.locked().lobbies.iter().map(|l| l.id).collect();
        if lobbies.is_empty() {
            return;
        }
        let Some(own) = lan_address() else { return };
        let [a, b, c, _] = own.octets();
        for d in 1..255 {
            let ip = Ipv4Addr::new(a, b, c, d);
            if ip != own {
                for lobby in &lobbies {
                    self.hello(SocketAddrV4::new(ip, PORTS.start), ASK, lobby);
                }
            }
        }
    }

    fn receive(&self, from: SocketAddrV4, d: &[u8]) {
        if Dht::is_dht(d) {
            let mut out = dht::Output::default();
            self.state.locked().dht.receive(from, d, &mut out);
            self.dht_output(out);
            return;
        }
        if d.len() < MAGIC.len() + 1 || d[..MAGIC.len()] != MAGIC {
            return;
        }
        let kind = d[MAGIC.len()];
        if kind >= FRIENDS {
            // A far player's (`Far`) gets through: no more opening the way.
            let mut state = self.state.locked();
            if state.far.contains_key(from.ip()) {
                state.heard.insert(*from.ip(), Instant::now());
            }
            drop(state);
            crate::friends::receive(self, from, kind, &d[MAGIC.len() + 1..]);
            return;
        }
        let d = &d[MAGIC.len() + 1..];
        match kind {
            HELLO => {
                if let Some(hello) = Hello::read(d) {
                    self.on_hello(from, hello);
                }
            }
            // As large as the smallest greeting (see `HELLO_SIZE`).
            COOKIE if d.len() >= HELLO_LEN => {
                let lobby: LobbyId = d[..8].try_into().unwrap();
                if self.state.locked().lobby(&lobby).is_some() {
                    self.hello_with(from, ASK | ANSWER, &lobby, Some(d[8..16].try_into().unwrap()));
                }
            }
            PEERS if d.len() >= 8 => {
                let lobby: LobbyId = d[..8].try_into().unwrap();
                let about: Vec<(u64, u32, SocketAddrV4)> = d[8..]
                    .as_chunks::<18>()
                    .0
                    .iter()
                    .map(|e| {
                        let id = u64::from_be_bytes(e[..8].try_into().unwrap());
                        let vip = u32::from_be_bytes(e[8..12].try_into().unwrap());
                        let ip = Ipv4Addr::new(e[12], e[13], e[14], e[15]);
                        (id, vip, SocketAddrV4::new(ip, u16::from_be_bytes([e[16], e[17]])))
                    })
                    .collect();
                self.on_peers(from, lobby, &about);
            }
            DATA if d.len() >= 12 => {
                let src = u32::from_be_bytes(d[..4].try_into().unwrap());
                let src_port = u16::from_be_bytes(d[4..6].try_into().unwrap());
                let dst = u32::from_be_bytes(d[6..10].try_into().unwrap());
                let dst_port = u16::from_be_bytes(d[10..12].try_into().unwrap());
                let vip = self.vip.load(Ordering::Relaxed);
                let state = self.state.locked();
                // Only from players of the lobbies, each in its own name: from
                // where it is heard or greeted from (another of its
                // addresses), or through a relay while it is not reached.
                let Some(direct) = state.sender(from, src, vip, Instant::now()) else { return };
                if dst == vip || is_broadcast(dst) {
                    drop(state);
                    crate::vnet::deliver(src, src_port, dst_port, &d[12..]);
                } else if direct && let Some(to) = state.peers.values().find(|p| p.vip == dst).and_then(|p| p.addr) {
                    // A relay for two players who cannot reach each other
                    // (their own datagrams only: not relayed again).
                    let mut relayed = Vec::with_capacity(MAGIC.len() + 1 + d.len());
                    relayed.extend_from_slice(&MAGIC);
                    relayed.push(DATA);
                    relayed.extend_from_slice(d);
                    let _ = self.socket().send_to(&relayed, to);
                }
            }
            _ => {}
        }
    }

    fn on_hello(&self, from: SocketAddrV4, hello: Hello) {
        let Hello { lobby, id, vip, flags, cookie, gives, card } = hello;
        let now = Instant::now();
        let mut state = self.state.locked();
        if state.lobby(&lobby).is_none() || id == self.id && state.own.contains(&from) || id != self.id && !is_player_vip(vip) {
            return;
        }
        if !self.cookies.check(from, &cookie, now) {
            drop(state);
            if flags & ANSWER == 0 {
                self.cookie(from, &lobby, now);
            }
            return;
        }
        state.trying.remove(&from);
        state.heard.insert(*from.ip(), now);
        if id == self.id {
            if state.own.len() < OWN {
                state.own.insert(from);
            }
            return;
        }
        let known = state.peers.contains_key(&id);
        if !known && state.peers.values().filter(|p| p.lobby == lobby).count() >= PLAYERS {
            return;
        }
        state.given.insert(from, gives);
        let heard_before = state.peers.get(&id).is_some_and(|p| p.addr.is_some());
        // Two players with one address: the one whose address the engine
        // has not taken yet takes another.
        if vip == self.vip.load(Ordering::Relaxed) {
            if self.vip_taken.load(Ordering::Relaxed) {
                log::warn!("p2p: player {id:016x} has this player's address {}", Ipv4Addr::from(vip));
            } else {
                let taken: Vec<u32> = state.peers.values().map(|p| p.vip).chain([vip]).collect();
                self.vip.store(free_vip(&taken), Ordering::Relaxed);
            }
        }
        let peer = state.peers.entry(id).or_insert(Peer::new(vip, lobby, now));
        peer.vip = vip;
        peer.lobby = lobby;
        if card.is_some() {
            peer.card = card;
        }
        // The nearest of its addresses that answer: one heard lately is
        // given up only for a nearer one (a greeting with its id from
        // elsewhere would take its traffic), or once it has been quiet for
        // more than a keepalive (the player moved: Wi-Fi to mobile data, a
        // NAT that remapped; a spoofer cannot hold off its greetings).
        let quiet = now - peer.heard > KEEPALIVE + KEEPALIVE / 2;
        if !peer.direct(now) || quiet || peer.addr.is_none_or(|a| nearness(from) < nearness(a)) {
            peer.addr = Some(from);
        }
        if peer.addr == Some(from) {
            peer.heard = now;
        }
        if !peer.seen.contains(&from) {
            if peer.seen.len() >= SEEN {
                peer.seen.remove(0);
            }
            peer.seen.push(from);
        }
        // A player already playing when first heard has had its time.
        if flags & PLAYING != 0 && peer.playing.is_none() {
            let long_ago = now.checked_sub(HOSTING).unwrap_or(now);
            peer.playing = Some(if heard_before { now } else { long_ago });
        }
        peer.flags = flags & (WAITING | PLAYING);
        if !known {
            log::info!("p2p: player {id:016x} ({}) at {from}", Ipv4Addr::from(vip));
            // Introductions: the newcomer to everyone of its lobby, them to it.
            let others: Vec<(u64, u32, SocketAddrV4)> = state
                .peers
                .iter()
                .filter(|(i, p)| **i != id && p.lobby == lobby)
                .filter_map(|(i, p)| Some((*i, p.vip, p.addr?)))
                .collect();
            drop(state);
            self.peers(from, &lobby, &others);
            for (_, _, addr) in &others {
                self.peers(*addr, &lobby, &[(id, vip, from)]);
            }
        } else {
            drop(state);
        }
        if flags & ASK != 0 {
            self.hello(from, 0, &lobby);
        }
    }

    fn on_peers(&self, from: SocketAddrV4, lobby: LobbyId, about: &[(u64, u32, SocketAddrV4)]) {
        let now = Instant::now();
        let mut state = self.state.locked();
        // Only a player of the lobby introduces others.
        if state.lobby(&lobby).is_none() || !state.peers.values().any(|p| p.addr == Some(from) && p.lobby == lobby) {
            return;
        }
        let mut punch = Vec::new();
        for &(id, vip, addr) in about {
            if id == self.id || !is_player_vip(vip) {
                continue;
            }
            if !state.peers.contains_key(&id) && state.peers.values().filter(|p| p.lobby == lobby).count() >= PLAYERS {
                continue;
            }
            let own = state.own.contains(&addr);
            let peer = state.peers.entry(id).or_insert(Peer::new(vip, lobby, now));
            // What a player says of itself wins over what others say of it.
            if !peer.direct(now) {
                peer.vip = vip;
            }
            peer.via = Some(from);
            peer.introduced = now;
            if !peer.direct(now) && !own {
                punch.push(addr);
            }
        }
        punch.truncate(TRYING.saturating_sub(state.trying.len()));
        // (Introduced again and again, an address keeps the time it was
        // first tried, and expires.)
        for addr in &punch {
            state.trying.entry(*addr).or_insert((now, lobby));
        }
        drop(state);
        for addr in punch {
            self.hello(addr, ASK, &lobby);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state() -> State {
        State {
            lobbies: Vec::new(),
            peers: HashMap::new(),
            trying: HashMap::new(),
            own: HashSet::new(),
            given: HashMap::new(),
            heard: HashMap::new(),
            far: HashMap::new(),
            introduced: Instant::now(),
            dht: Dht::new([0; 20], 47000),
        }
    }

    /// A player on this machine, without its thread: the tests hand it the
    /// datagrams its socket gets (`exchange`).
    fn player() -> (P2p, SocketAddrV4) {
        let (socket, addr) = socket();
        let p = P2p {
            socket: RwLock::new(socket),
            port: AtomicU16::new(addr.port()),
            id: random(),
            vip: AtomicU32::new(free_vip(&[])),
            vip_taken: AtomicBool::new(false),
            flags: AtomicU8::new(0),
            waiting: Mutex::new(None),
            state: Mutex::new(state()),
            cookies: Cookies::new(Instant::now()),
        };
        p.join_lobby("test");
        (p, addr)
    }

    fn socket() -> (UdpSocket, SocketAddrV4) {
        let socket = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        socket.set_read_timeout(Some(Duration::from_millis(30))).unwrap();
        let SocketAddr::V4(addr) = socket.local_addr().unwrap() else { unreachable!() };
        (socket, addr)
    }

    fn received(socket: &UdpSocket) -> Vec<Vec<u8>> {
        let mut buf = [0; 2048];
        std::iter::from_fn(|| socket.recv(&mut buf).ok().map(|len| buf[..len].to_vec())).collect()
    }

    /// Hands the players what they receive, until nothing comes.
    fn exchange(players: &[&P2p]) {
        let mut buf = [0; 2048];
        loop {
            let mut any = false;
            for p in players {
                loop {
                    let r = p.socket().recv_from(&mut buf);
                    let Ok((len, SocketAddr::V4(from))) = r else { break };
                    p.receive(from, &buf[..len]);
                    any = true;
                }
            }
            if !any {
                return;
            }
        }
    }

    fn lobby(p: &P2p) -> LobbyId {
        p.state.locked().lobbies[0].id
    }

    /// A greeting as anyone may send it, without a card: the smallest.
    fn greeting(lobby: &LobbyId, id: u64, flags: u8, cookie: [u8; 8]) -> Vec<u8> {
        let mut d = MAGIC.to_vec();
        d.push(HELLO);
        d.extend_from_slice(lobby);
        d.extend_from_slice(&id.to_be_bytes());
        d.extend_from_slice(&(NET | 0x0105).to_be_bytes());
        d.push(flags);
        d.extend_from_slice(&cookie);
        d.extend_from_slice(&[7; 8]);
        d
    }

    #[test]
    fn a_greeting_from_an_address_not_proven_gets_a_cookie_and_nothing_more() {
        let (b, _) = player();
        let (x, xa) = socket();
        let (y, ya) = socket();
        let lobby = lobby(&b);
        // Someone else's address as the source: a cookie no larger than
        // the greeting goes there, nothing is kept or introduced.
        let hello = greeting(&lobby, 42, ASK, [0; 8]);
        b.receive(xa, &hello);
        let got = received(&x);
        assert_eq!(got.len(), 1);
        assert_eq!(got[0][MAGIC.len()], COOKIE);
        assert!(got[0].len() <= hello.len());
        {
            let s = b.state.locked();
            assert!(s.peers.is_empty() && s.trying.is_empty() && s.given.is_empty() && s.heard.is_empty());
        }
        let cookie: [u8; 8] = got[0][MAGIC.len() + 9..MAGIC.len() + 17].try_into().unwrap();
        // The cookie is the address's: from elsewhere it is not taken.
        b.receive(ya, &greeting(&lobby, 42, ASK, cookie));
        assert_eq!(received(&y).len(), 1);
        assert!(b.state.locked().peers.is_empty());
        // An answer to a cookie with a wrong one: nothing back (no cookies
        // going back and forth between two players for ever).
        b.receive(xa, &greeting(&lobby, 42, ASK | ANSWER, [1; 8]));
        assert!(received(&x).is_empty());
        // Greeted again with it: a player, greeted back with its own.
        b.receive(xa, &greeting(&lobby, 42, ASK | ANSWER, cookie));
        assert_eq!(b.state.locked().peers.get(&42).and_then(|p| p.addr), Some(xa));
        let back = received(&x);
        let hello = back.iter().find(|d| d[MAGIC.len()] == HELLO).unwrap();
        assert_eq!(hello[MAGIC.len() + 22..MAGIC.len() + 30], [7; 8]);
    }

    #[test]
    fn engine_datagrams_come_in_their_sender_s_name() {
        let now = Instant::now();
        let mut s = state();
        let own = NET | 0x0101;
        let at = |port| SocketAddrV4::new(Ipv4Addr::new(203, 0, 113, 7), port);
        let (host, relay, far) = (NET | 0x0202, NET | 0x0303, NET | 0x0404);
        for (id, vip, addr) in [(1, host, Some(at(1))), (2, relay, Some(at(2))), (3, far, None)] {
            let mut p = Peer::new(vip, [0; 8], now);
            p.addr = addr;
            p.seen.extend(addr);
            p.heard = now;
            s.peers.insert(id, p);
        }
        assert_eq!(s.sender(at(1), host, own, now), Some(true));
        // Another player in the host's name, or in this player's own, or a
        // broadcast's: no.
        assert_eq!(s.sender(at(2), host, own, now), None);
        assert_eq!(s.sender(at(1), own, own, now), None);
        assert_eq!(s.sender(at(1), u32::MAX, own, now), None);
        assert_eq!(s.sender(at(1), NET | 0x02ff, own, now), None);
        // A player not reached directly, through another.
        assert_eq!(s.sender(at(2), far, own, now), Some(false));
        // Not from a player of the lobbies.
        assert_eq!(s.sender(at(9), far, own, now), None);
        assert_eq!(s.sender(at(9), NET | 0x0909, own, now), None);
    }

    #[test]
    fn cookies_hold_an_epoch_or_two_for_their_address_only() {
        let now = Instant::now();
        let c = Cookies::new(now);
        let addr = SocketAddrV4::new(Ipv4Addr::new(203, 0, 113, 7), 31329);
        let cookie = c.of(addr, now);
        assert!(c.check(addr, &cookie, now));
        assert!(c.check(addr, &cookie, now + COOKIE_EPOCH));
        assert!(!c.check(addr, &cookie, now + COOKIE_EPOCH * 2));
        assert!(!c.check(SocketAddrV4::new(*addr.ip(), 31330), &cookie, now));
        assert!(!c.check(SocketAddrV4::new(Ipv4Addr::new(203, 0, 113, 8), 31329), &cookie, now));
        // Another run's secret.
        assert!(!Cookies::new(now).check(addr, &cookie, now));
    }

    #[test]
    fn two_players_greeting_each_other_first_meet() {
        let ((a, aa), (b, ba)) = (player(), player());
        let lobby = lobby(&a);
        // As two players behind NATs do, both at once.
        a.hello(ba, ASK, &lobby);
        b.hello(aa, ASK, &lobby);
        exchange(&[&a, &b]);
        for (p, other, at) in [(&a, &b, ba), (&b, &a, aa)] {
            let s = p.state.locked();
            assert_eq!(s.peers.get(&other.id).and_then(|q| q.addr), Some(at));
            assert!(s.given.contains_key(&at));
        }
        // From then on their greetings carry the cookies: answered in full.
        a.hello(ba, ASK, &lobby);
        for d in received(&b.socket()) {
            b.receive(aa, &d);
        }
        let kinds: Vec<u8> = received(&a.socket()).iter().map(|d| d[MAGIC.len()]).collect();
        assert_eq!(kinds, [HELLO]);
    }

    #[test]
    fn a_player_s_own_greeting_tells_it_its_address() {
        let (a, aa) = player();
        a.hello(aa, ASK, &lobby(&a));
        exchange(&[&a]);
        let s = a.state.locked();
        assert!(s.own.contains(&aa) && s.peers.is_empty());
    }

    #[test]
    fn the_ports_opened_follow_the_ones_handed_out() {
        // Measured on a mobile network: announced at 31329, its greetings
        // came from 31435; announced at 25179-25362 while starting, its
        // greetings came from 25864.
        assert_eq!(spray_ports(&[31329]), [31265..=31969]);
        assert_eq!(spray_ports(&[25179, 25362, 25349]), [25115..=26002]);
        // Announces of the player's earlier runs (31441...) and of this
        // one (3106x, the NAT moved on): around each, close ones merged.
        assert_eq!(spray_ports(&[31441, 31874, 31457, 31900, 31063, 31067]), [30999..=31707, 31836..=32540]);
        // No room for more: the latest count.
        assert_eq!(spray_ports(&[10000, 20000, 30000, 40000, 50000]), [29936..=30640, 39936..=40640, 49936..=50640]);
        assert_eq!(spray_ports(&[1050]), [1024..=1690]);
        assert_eq!(spray_ports(&[65500]), [65436..=65535]);
    }

    #[test]
    fn a_far_player_not_heard_from_gets_its_ports_opened() {
        let mut s = state();
        let now = Instant::now();
        let ip = Ipv4Addr::new(203, 0, 113, 7);
        s.note_far(SocketAddrV4::new(ip, 31329), now);
        s.note_far(SocketAddrV4::new(ip, 31340), now);
        // Not on the lobby port, not on this network.
        s.note_far(SocketAddrV4::new(Ipv4Addr::new(198, 51, 100, 20), 47000), now);
        s.note_far(SocketAddrV4::new(Ipv4Addr::new(10, 0, 0, 7), 50000), now);
        assert_eq!(s.far.len(), 1);
        // Plain greetings first; never from behind a NAT changing ports.
        assert!(s.sprays(true, now + Duration::from_secs(1)).is_empty());
        assert!(s.sprays(false, now + SPRAY_AFTER).is_empty());
        assert_eq!(s.sprays(true, now + SPRAY_AFTER), vec![(ip, spray_ports(&[31329, 31340]))]);
        assert!(s.sprays(true, now + SPRAY_AFTER + Duration::from_secs(2)).is_empty());
        assert_eq!(s.sprays(true, now + SPRAY_AFTER + SPRAY_EVERY).len(), 1);
        // Heard from: done, and not noted again while it talks.
        let later = now + SPRAY_AFTER + SPRAY_EVERY + Duration::from_secs(1);
        s.heard.insert(ip, later);
        assert!(s.sprays(true, later).is_empty());
        assert!(s.far.is_empty());
        s.note_far(SocketAddrV4::new(ip, 31400), later);
        assert!(s.far.is_empty());
    }

    #[test]
    fn spraying_stops_when_punching_does() {
        let mut s = state();
        let now = Instant::now();
        s.note_far(SocketAddrV4::new(Ipv4Addr::new(203, 0, 113, 7), 31329), now);
        assert!(s.sprays(true, now + PUNCH).is_empty());
        assert!(s.far.is_empty());
    }
}
