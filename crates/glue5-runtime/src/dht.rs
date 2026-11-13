//! Finding other players on the Internet, with no server of ours: the
//! BitTorrent DHT (BEP 5), millions of public nodes that keep, for a 20-byte
//! key, the addresses announced under it. A lobby is a key; each player
//! announces itself under it and asks for the others. Friends meet the same
//! way, under keys only the two of them can work out (friends/).
//!
//! The DHT runs over the game's own UDP socket, and announces with the
//! port the nodes see (`implied_port`): the address it hands out is the
//! one the player's NAT opened for that socket, which the others can send
//! to.
//!
//! The client stays small: it never answers queries (it says so, BEP 43's
//! read-only flag) and keeps no routing table, only the nodes of each
//! lookup (the nodes closest to its key, which hold its announces); a new
//! lookup starts from the nodes the others know.
//!
//! Nodes that watch the DHT take ids right next to every key looked up:
//! announces sent to them are kept and never handed out, so two players
//! announcing there do not find each other. An honest node's id goes with
//! its address (BEP 42), which such ids cannot: lookups take only nodes
//! whose ids do, and one node per address.

use std::collections::{BTreeMap, HashMap};
use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4, ToSocketAddrs};
use std::time::{Duration, Instant};

/// Well-known entry points to the DHT.
const BOOTSTRAP: &[&str] = &[
    "router.bittorrent.com:6881",
    "dht.transmissionbt.com:6881",
    "router.utorrent.com:6881",
    "dht.libtorrent.org:25401",
];
/// Queries in flight at most, per lookup and in all, and how long one may
/// take.
const PARALLEL: usize = 8;
const IN_FLIGHT: usize = 48;
const QUERY_TIMEOUT: Duration = Duration::from_secs(2);
/// The nodes a lookup ends on (and announces to).
const CLOSEST: usize = 8;
/// Nodes kept as candidates.
const CANDIDATES: usize = 64;
/// Nodes another lookup's start borrows.
const SEEDS: usize = 16;
/// How often a lookup announces itself: announces expire after about 30
/// minutes.
const ANNOUNCE: Duration = Duration::from_secs(120);
/// A lookup starts over after this (the closest nodes change), or sooner
/// when no node answered.
const RESTART: Duration = Duration::from_secs(600);
const RETRY: Duration = Duration::from_secs(15);
const EMPTY_RETRY: Duration = Duration::from_secs(1);

pub type Id = [u8; 20];

struct Node {
    addr: SocketAddrV4,
    queried: bool,
    responded: bool,
    token: Option<Vec<u8>>,
    /// Its id goes with its address (BEP 42), or is not known yet.
    honest: bool,
}

impl Node {
    fn new(addr: SocketAddrV4, honest: bool) -> Node {
        Node { addr, queried: false, responded: false, token: None, honest }
    }
}

/// Whether a node's id goes with its address (BEP 42): its first 21 bits
/// are the CRC-32C of the address's masked bits and of the id's last byte.
/// Nodes on private networks may take any.
fn honest_id(id: &Id, ip: &Ipv4Addr) -> bool {
    if ip.is_private() || ip.is_loopback() || ip.is_link_local() {
        return true;
    }
    let r = id[19];
    let mut masked: [u8; 4] = std::array::from_fn(|i| ip.octets()[i] & [0x03, 0x0f, 0x3f, 0xff][i]);
    masked[0] |= (r & 7) << 5;
    let crc = crc32c(&masked).to_be_bytes();
    id[0] == crc[0] && id[1] == crc[1] && id[2] & 0xf8 == crc[2] & 0xf8
}

fn crc32c(data: &[u8]) -> u32 {
    let mut crc = !0u32;
    for &b in data {
        crc ^= u32::from(b);
        for _ in 0..8 {
            crc = (crc >> 1) ^ (0x82f6_3b78 & (crc & 1).wrapping_neg());
        }
    }
    !crc
}

/// What a lookup is for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Options {
    /// Whether this player announces itself under the key (or only looks).
    pub announce: bool,
    /// How often the closest nodes are asked again for new players.
    pub refresh: Duration,
}

struct Lookup {
    options: Options,
    /// By distance to the key.
    nodes: BTreeMap<Id, Node>,
    started: Option<Instant>,
    refreshed: Option<Instant>,
    announced: Option<Instant>,
}

impl Lookup {
    /// The closest honest nodes that answered, one per address.
    fn closest(&self) -> impl Iterator<Item = (&Id, &Node)> {
        let mut ips = std::collections::HashSet::new();
        self.nodes.iter().filter(move |(_, n)| n.responded && n.honest && ips.insert(*n.addr.ip())).take(CLOSEST)
    }
}

pub struct Dht {
    id: Id,
    port: u16,
    lookups: HashMap<Id, Lookup>,
    /// Queries in flight: transaction id -> lookup key, node (by distance),
    /// sent at.
    pending: HashMap<u16, (Id, Id, Instant)>,
    next_tid: u16,
    /// This player's address as the nodes see it (its NAT's, which the DHT
    /// hands out to the others), once two nodes agree on it; the last one
    /// told, and by whom.
    external: Option<SocketAddrV4>,
    told: Option<(SocketAddrV4, Ipv4Addr)>,
}

/// What the socket should send, and the players found (under which key).
#[derive(Default)]
pub struct Output {
    pub send: Vec<(SocketAddrV4, Vec<u8>)>,
    pub peers: Vec<(Id, SocketAddrV4)>,
}

fn distance(a: &Id, b: &Id) -> Id {
    std::array::from_fn(|i| a[i] ^ b[i])
}

impl Dht {
    /// A client announcing `port`, with no lookup yet.
    pub fn new(id: Id, port: u16) -> Dht {
        Dht { id, port, lookups: HashMap::new(), pending: HashMap::new(), next_tid: 0, external: None, told: None }
    }

    /// This player's address on the Internet, as the nodes see it, when
    /// known: the DHT gives it back among the others under the keys this
    /// player announces.
    pub fn external(&self) -> Option<SocketAddrV4> {
        self.external
    }

    /// This player's address on the Internet without the port, which the
    /// nodes agree on even when its NAT gives each of them another port.
    pub fn public_ip(&self) -> Option<Ipv4Addr> {
        self.external.or(self.told.map(|(a, _)| a)).map(|a| *a.ip())
    }

    /// Whether a datagram is the DHT's (a bencoded dictionary).
    pub fn is_dht(data: &[u8]) -> bool {
        data.first() == Some(&b'd')
    }

    /// Looks for `key`'s players from now on (again with other options).
    pub fn add(&mut self, key: Id, options: Options) {
        let lookup = self.lookups.entry(key).or_insert_with(|| Lookup {
            options,
            nodes: BTreeMap::new(),
            started: None,
            refreshed: None,
            announced: None,
        });
        lookup.options = options;
    }

    pub fn remove(&mut self, key: &Id) {
        self.lookups.remove(key);
        self.pending.retain(|_, (k, ..)| k != key);
    }

    /// Advances the lookups; call it a few times a second.
    pub fn tick(&mut self, now: Instant, out: &mut Output) {
        self.pending.retain(|_, (.., at)| now - *at < QUERY_TIMEOUT);
        let keys: Vec<Id> = self.lookups.keys().copied().collect();
        for key in keys {
            self.tick_lookup(key, now, out);
        }
    }

    fn tick_lookup(&mut self, key: Id, now: Instant, out: &mut Output) {
        let Some(lookup) = self.lookups.get(&key) else { return };
        // Started with no node to ask (the bootstrap nodes' addresses not
        // looked up yet): again in a moment.
        let restart = lookup.started.is_none_or(|s| {
            let age = now - s;
            age > RESTART
                || (age > RETRY && !lookup.nodes.values().any(|n| n.responded))
                || (age > EMPTY_RETRY && lookup.nodes.is_empty())
        });
        if restart {
            self.start(key, now);
        }
        // The lookup: the closest nodes not asked yet.
        let busy = self.pending.values().filter(|(k, ..)| *k == key).count();
        let room = PARALLEL.saturating_sub(busy).min(IN_FLIGHT.saturating_sub(self.pending.len()));
        let lookup = &self.lookups[&key];
        let ask: Vec<Id> =
            lookup.nodes.iter().take(CANDIDATES).filter(|(_, n)| !n.queried).take(room).map(|(d, _)| *d).collect();
        for d in ask {
            self.get_peers(key, d, now, out);
        }
        // Done when the closest nodes that answered were all asked: then
        // ask them again now and then, and announce.
        let lookup = &self.lookups[&key];
        let asking = self.pending.values().any(|(k, ..)| *k == key);
        let settled = !asking
            && !lookup.nodes.is_empty()
            && lookup.nodes.values().filter(|n| n.honest).take(CLOSEST).all(|n| n.queried);
        if !settled {
            return;
        }
        if lookup.refreshed.is_none_or(|r| now - r > lookup.options.refresh) {
            let closest: Vec<Id> = lookup.closest().map(|(d, _)| *d).collect();
            self.lookups.get_mut(&key).unwrap().refreshed = Some(now);
            for d in closest {
                self.get_peers(key, d, now, out);
            }
        }
        let lookup = &self.lookups[&key];
        if lookup.options.announce
            && lookup.announced.is_none_or(|a| now - a > ANNOUNCE)
            && lookup.closest().any(|(_, n)| n.token.is_some())
        {
            let to: Vec<(SocketAddrV4, Vec<u8>)> =
                lookup.closest().filter_map(|(_, n)| Some((n.addr, n.token.clone()?))).collect();
            self.lookups.get_mut(&key).unwrap().announced = Some(now);
            for (addr, token) in to {
                let tid = self.tid();
                let query = dict([
                    ("t", B(tid.to_be_bytes().to_vec())),
                    ("y", B(b"q".to_vec())),
                    ("q", B(b"announce_peer".to_vec())),
                    (
                        "a",
                        dict([
                            ("id", B(self.id.to_vec())),
                            ("implied_port", I(1)),
                            ("info_hash", B(key.to_vec())),
                            ("port", I(self.port.into())),
                            ("token", B(token)),
                        ]),
                    ),
                    ("ro", I(1)),
                ]);
                out.send.push((addr, query.encode()));
            }
        }
    }

    /// Whether this player was announced under `key` and its players asked
    /// for again since: the ones announced by then are known.
    pub fn settled(&self, key: &Id) -> bool {
        self.lookups.get(key).is_some_and(|l| matches!((l.announced, l.refreshed), (Some(a), Some(r)) if r > a))
    }

    /// (Re)starts a lookup from the bootstrap nodes and the nodes the other
    /// lookups found.
    fn start(&mut self, key: Id, now: Instant) {
        let seeds: Vec<SocketAddrV4> = self
            .lookups
            .iter()
            .filter(|(k, _)| **k != key)
            .flat_map(|(_, l)| l.nodes.values().filter(|n| n.responded && n.honest).map(|n| n.addr))
            .take(SEEDS)
            .collect();
        self.pending.retain(|_, (k, ..)| *k != key);
        let Some(lookup) = self.lookups.get_mut(&key) else { return };
        lookup.started = Some(now);
        lookup.refreshed = None;
        lookup.nodes.clear();
        // Nodes whose ids are unknown go in as the farthest.
        for (i, addr) in bootstrap().into_iter().chain(seeds).enumerate() {
            let mut d = [0xff; 20];
            d[18] = (i >> 8) as u8;
            d[19] = i as u8;
            if lookup.nodes.values().all(|n| n.addr != addr) {
                lookup.nodes.insert(d, Node::new(addr, true));
            }
        }
    }

    fn tid(&mut self) -> u16 {
        self.next_tid = self.next_tid.wrapping_add(1);
        self.next_tid
    }

    fn get_peers(&mut self, key: Id, d: Id, now: Instant, out: &mut Output) {
        let tid = self.tid();
        let Some(node) = self.lookups.get_mut(&key).and_then(|l| l.nodes.get_mut(&d)) else { return };
        node.queried = true;
        let query = dict([
            ("t", B(tid.to_be_bytes().to_vec())),
            ("y", B(b"q".to_vec())),
            ("q", B(b"get_peers".to_vec())),
            ("a", dict([("id", B(self.id.to_vec())), ("info_hash", B(key.to_vec()))])),
            ("ro", I(1)),
        ]);
        out.send.push((node.addr, query.encode()));
        self.pending.insert(tid, (key, d, now));
    }

    /// A datagram from a DHT node.
    pub fn receive(&mut self, from: SocketAddrV4, data: &[u8], out: &mut Output) {
        let Some(Value::Dict(msg)) = Value::decode(data) else { return };
        let Some(Value::Bytes(t)) = msg.get(&b"t"[..]) else { return };
        let Ok(t) = <[u8; 2]>::try_from(t.as_slice()) else { return };
        let tid = u16::from_be_bytes(t);
        let Some(&(key, d, _)) = self.pending.get(&tid) else { return };
        // A datagram from elsewhere with the query's (guessable) id leaves
        // it waiting for the node's answer.
        if self.lookups.get(&key).and_then(|l| l.nodes.get(&d)).is_some_and(|n| n.addr != from) {
            return;
        }
        self.pending.remove(&tid);
        let Some(Value::Dict(r)) = msg.get(&b"r"[..]) else { return };
        let Some(lookup) = self.lookups.get_mut(&key) else { return };
        // The node, under its real id now.
        let Some(mut node) = lookup.nodes.remove(&d) else { return };
        if node.addr != from {
            lookup.nodes.insert(d, node);
            return;
        }
        node.responded = true;
        // The address it sees this player at (BEP 42), taken once another
        // node said the same.
        if let Some(Value::Bytes(ip)) = msg.get(&b"ip"[..])
            && let Some(seen) = compact_addr(ip)
        {
            // Dropped once two in a row say otherwise: the player moved to
            // another network (Wi-Fi to mobile data), whose NAT may give
            // each node another address, so that none agree again.
            if let Some((a, by)) = self.told
                && by != *from.ip()
            {
                if a == seen {
                    self.external = Some(seen);
                } else if self.external.is_some_and(|e| e != seen && e != a) {
                    self.external = None;
                }
            }
            self.told = Some((seen, *from.ip()));
        }
        if let Some(Value::Bytes(token)) = r.get(&b"token"[..]) {
            node.token = Some(token.clone());
        }
        let at = match r.get(&b"id"[..]) {
            Some(Value::Bytes(id)) if id.len() == 20 => {
                let id = id.as_slice().try_into().unwrap();
                node.honest = honest_id(&id, from.ip());
                distance(&key, &id)
            }
            _ => d,
        };
        let honest = node.honest;
        lookup.nodes.insert(at, node);
        // The players a node watching the DHT hands out are made up (the
        // nodes it names are checked one by one).
        if honest && let Some(Value::List(values)) = r.get(&b"values"[..]) {
            for v in values {
                if let Value::Bytes(v) = v
                    && let Some(addr) = compact_addr(v)
                {
                    out.peers.push((key, addr));
                }
            }
        }
        if let Some(Value::Bytes(nodes)) = r.get(&b"nodes"[..]) {
            for n in nodes.as_chunks::<26>().0 {
                let (Ok(id), Some(addr)) = (<Id>::try_from(&n[..20]), compact_addr(&n[20..])) else { continue };
                let d = distance(&key, &id);
                if honest_id(&id, addr.ip())
                    && !lookup.nodes.contains_key(&d)
                    && lookup.nodes.values().all(|n| n.addr != addr)
                {
                    lookup.nodes.insert(d, Node::new(addr, true));
                }
            }
            // Only the closest candidates matter.
            while lookup.nodes.len() > CANDIDATES {
                let last = *lookup.nodes.keys().next_back().unwrap();
                lookup.nodes.remove(&last);
            }
        }
    }
}

/// The bootstrap nodes' addresses, looked up away from the network thread
/// (empty until then, or while there is no network).
fn bootstrap() -> Vec<SocketAddrV4> {
    use std::sync::Mutex;
    static RESOLVED: Mutex<(Vec<SocketAddrV4>, bool)> = Mutex::new((Vec::new(), false));
    let mut resolved = RESOLVED.lock().unwrap();
    if resolved.0.is_empty() && !resolved.1 {
        resolved.1 = true;
        std::thread::spawn(|| {
            let addrs: Vec<SocketAddrV4> = BOOTSTRAP
                .iter()
                .filter_map(|host| host.to_socket_addrs().ok())
                .flatten()
                .filter_map(|a| match a {
                    SocketAddr::V4(a) => Some(a),
                    SocketAddr::V6(_) => None,
                })
                .collect();
            log::debug!("dht: {} bootstrap nodes", addrs.len());
            *RESOLVED.lock().unwrap() = (addrs, false);
        });
    }
    resolved.0.clone()
}

/// An address in a reply: only one on the Internet (a node or player on
/// this machine, a local network or multicast would be made up, to have
/// this player send there).
fn compact_addr(b: &[u8]) -> Option<SocketAddrV4> {
    let b: [u8; 6] = b.try_into().ok()?;
    let ip = Ipv4Addr::new(b[0], b[1], b[2], b[3]);
    let port = u16::from_be_bytes([b[4], b[5]]);
    let local = ip.is_unspecified()
        || ip.is_loopback()
        || ip.is_private()
        || ip.is_link_local()
        || ip.is_multicast()
        // "This network", the carriers' shared space, reserved and broadcast.
        || b[0] == 0
        || (b[0] == 100 && b[1] & 0xc0 == 64)
        || b[0] >= 240;
    (port != 0 && !local).then_some(SocketAddrV4::new(ip, port))
}

// ---- bencode -------------------------------------------------------------------

#[derive(Debug, PartialEq)]
enum Value {
    Int(i64),
    Bytes(Vec<u8>),
    List(Vec<Value>),
    Dict(BTreeMap<Vec<u8>, Value>),
}
use Value::{Bytes as B, Int as I};

fn dict<const N: usize>(entries: [(&str, Value); N]) -> Value {
    Value::Dict(entries.into_iter().map(|(k, v)| (k.as_bytes().to_vec(), v)).collect())
}

impl Value {
    fn encode(&self) -> Vec<u8> {
        let mut out = Vec::new();
        self.write(&mut out);
        out
    }

    fn write(&self, out: &mut Vec<u8>) {
        match self {
            Value::Int(i) => out.extend_from_slice(format!("i{i}e").as_bytes()),
            Value::Bytes(b) => {
                out.extend_from_slice(format!("{}:", b.len()).as_bytes());
                out.extend_from_slice(b);
            }
            Value::List(l) => {
                out.push(b'l');
                l.iter().for_each(|v| v.write(out));
                out.push(b'e');
            }
            Value::Dict(d) => {
                out.push(b'd');
                for (k, v) in d {
                    Value::Bytes(k.clone()).write(out);
                    v.write(out);
                }
                out.push(b'e');
            }
        }
    }

    fn decode(data: &[u8]) -> Option<Value> {
        let mut at = 0;
        let v = Value::read(data, &mut at, 0)?;
        Some(v)
    }

    fn read(d: &[u8], at: &mut usize, depth: u32) -> Option<Value> {
        if depth > 8 {
            return None;
        }
        match *d.get(*at)? {
            b'i' => {
                let end = *at + d[*at..].iter().position(|&c| c == b'e')?;
                let i = std::str::from_utf8(&d[*at + 1..end]).ok()?.parse().ok()?;
                *at = end + 1;
                Some(Value::Int(i))
            }
            b'l' => {
                *at += 1;
                let mut l = Vec::new();
                while *d.get(*at)? != b'e' {
                    l.push(Value::read(d, at, depth + 1)?);
                }
                *at += 1;
                Some(Value::List(l))
            }
            b'd' => {
                *at += 1;
                let mut m = BTreeMap::new();
                while *d.get(*at)? != b'e' {
                    let Value::Bytes(k) = Value::read(d, at, depth + 1)? else { return None };
                    m.insert(k, Value::read(d, at, depth + 1)?);
                }
                *at += 1;
                Some(Value::Dict(m))
            }
            b'0'..=b'9' => {
                let colon = *at + d[*at..].iter().position(|&c| c == b':')?;
                let len: usize = std::str::from_utf8(&d[*at..colon]).ok()?.parse().ok()?;
                // (A length from the network: no overflow.)
                let end = (colon + 1).checked_add(len)?;
                let b = d.get(colon + 1..end)?.to_vec();
                *at = end;
                Some(Value::Bytes(b))
            }
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bencode_round_trip() {
        let v = dict([("a", I(-3)), ("b", B(b"xyz".to_vec())), ("c", Value::List(vec![I(1), B(vec![])]))]);
        let e = v.encode();
        assert_eq!(e, b"d1:ai-3e1:b3:xyz1:cli1e0:ee");
        assert_eq!(Value::decode(&e), Some(v));
        assert_eq!(Value::decode(b"d1:a"), None);
    }

    #[test]
    fn honest_ids_go_with_their_addresses() {
        // BEP 42's examples.
        for (ip, r, prefix) in [
            ([124, 31, 75, 21], 1, [0x5f, 0xbf, 0xbf]),
            ([21, 75, 31, 124], 86, [0x5a, 0x3c, 0xe9]),
            ([65, 23, 51, 170], 22, [0xa5, 0xd4, 0x32]),
            ([84, 124, 73, 14], 65, [0x1b, 0x03, 0x21]),
            ([43, 213, 53, 83], 90, [0xe5, 0x6f, 0x6c]),
        ] {
            let mut id = [0; 20];
            id[..3].copy_from_slice(&prefix);
            id[19] = r;
            assert!(honest_id(&id, &Ipv4Addr::from(ip)), "{ip:?}");
            id[0] ^= 1;
            assert!(!honest_id(&id, &Ipv4Addr::from(ip)), "{ip:?}");
        }
        assert!(honest_id(&[0; 20], &Ipv4Addr::new(10, 0, 0, 7)));
    }

    #[test]
    fn players_come_from_honest_nodes() {
        let key = [7; 20];
        let mut dht = Dht::new([1; 20], 47000);
        dht.add(key, Options { announce: false, refresh: Duration::from_secs(60) });
        let now = Instant::now();
        let player = SocketAddrV4::new(Ipv4Addr::new(203, 0, 113, 7), 31329);
        let honest = {
            let mut id = [0; 20];
            id[..3].copy_from_slice(&[0x5f, 0xbf, 0xbf]);
            id[19] = 1;
            id
        };
        // A node watching the DHT (an id right next to the key), then an
        // honest one far from it: only the honest one's players count, and
        // only it is among the closest.
        for (i, (ip, id)) in [([198, 51, 100, 9], key), ([124, 31, 75, 21], honest)].into_iter().enumerate() {
            let addr = SocketAddrV4::new(Ipv4Addr::from(ip), 6881);
            let mut d = [0xee; 20];
            d[19] = i as u8;
            dht.lookups.get_mut(&key).unwrap().nodes.insert(d, Node::new(addr, true));
            let mut out = Output::default();
            dht.get_peers(key, d, now, &mut out);
            let value = [player.ip().octets().as_slice(), &player.port().to_be_bytes()].concat();
            let response = dict([
                ("t", B(dht.next_tid.to_be_bytes().to_vec())),
                ("y", B(b"r".to_vec())),
                ("r", dict([("id", B(id.to_vec())), ("values", Value::List(vec![B(value)]))])),
            ]);
            dht.receive(addr, &response.encode(), &mut out);
            assert_eq!(out.peers, if i == 1 { vec![(key, player)] } else { vec![] }, "{i}");
        }
        let closest: Vec<_> = dht.lookups[&key].closest().map(|(_, n)| n.addr).collect();
        assert_eq!(closest, [SocketAddrV4::new(Ipv4Addr::new(124, 31, 75, 21), 6881)]);
    }

    #[test]
    fn the_address_nodes_see_counts_once_two_agree() {
        let key = [7; 20];
        let mut dht = Dht::new([1; 20], 47000);
        dht.add(key, Options { announce: false, refresh: Duration::from_secs(60) });
        let now = Instant::now();
        let seen = SocketAddrV4::new(Ipv4Addr::new(203, 0, 113, 5), 40123);
        let ip = [seen.ip().octets().as_slice(), &seen.port().to_be_bytes()].concat();
        // Told by one node twice, then by another.
        for (i, node) in [1, 1, 2].into_iter().enumerate() {
            let addr = SocketAddrV4::new(Ipv4Addr::new(198, 51, 100, node), 6881);
            let mut d = [0xee; 20];
            d[19] = i as u8;
            dht.lookups.get_mut(&key).unwrap().nodes.insert(d, Node::new(addr, true));
            let mut out = Output::default();
            dht.get_peers(key, d, now, &mut out);
            let response = dict([
                ("t", B(dht.next_tid.to_be_bytes().to_vec())),
                ("y", B(b"r".to_vec())),
                ("ip", B(ip.clone())),
                ("r", dict([("id", B(vec![i as u8; 20]))])),
            ]);
            dht.receive(addr, &response.encode(), &mut out);
            assert_eq!(dht.external(), (i == 2).then_some(seen), "{i}");
        }
    }
}
