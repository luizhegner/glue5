//! A channel between two players: a Noise handshake, then the transport,
//! over the player's UDP socket. Datagrams may be lost, repeated or come
//! out of order: each carries its nonce, and a window drops those already
//! seen (and the ones too old to tell). Each side names its end of a
//! channel by a random index, which the other's datagrams carry.
//!
//! Datagrams, after p2p.rs's magic:
//! - `HS1` pattern, sender's index, Noise message 1, MAC (of all before
//!   it: see `crypto::first_mac`)
//! - `HS2` pattern, sender's index, receiver's index, Noise message 2
//! - `HS3` pattern, sender's index, receiver's index, Noise message 3 (XX)
//! - `DATA` receiver's index, nonce, ciphertext
//! - `PROBE` the prober's id (8 bytes: its own probes come back to it),
//!   then tags (8 bytes each): "is someone with one of these here?"

use std::net::SocketAddrV4;
use std::time::Instant;

use snow::{Builder, HandshakeState, StatelessTransportState};

use super::crypto::{self, CODE_PATTERN, FRIEND_PATTERN, Identity, Key, MAC_LEN, PROLOGUE, Tag};
use crate::p2p::{FRIENDS, MAGIC};

pub(crate) const HS1: u8 = FRIENDS + 1;
pub(crate) const HS2: u8 = FRIENDS + 2;
pub(crate) const HS3: u8 = FRIENDS + 3;
pub(crate) const DATA: u8 = FRIENDS + 4;
pub(crate) const PROBE: u8 = FRIENDS + 5;

/// Which handshake: between friends (IK), or with a friend code (XXpsk0).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Pattern {
    Friend = 1,
    Code = 2,
}

impl Pattern {
    pub(crate) fn from_byte(b: u8) -> Option<Pattern> {
        match b {
            1 => Some(Pattern::Friend),
            2 => Some(Pattern::Code),
            _ => None,
        }
    }
}

/// The largest datagram sent: what the Internet carries whole.
pub(crate) const MAX_DATAGRAM: usize = 1200;
/// Room for a transport message's payload.
pub(crate) const MAX_PAYLOAD: usize = MAX_DATAGRAM - MAGIC.len() - 1 - 4 - 8 - 16;

fn header(kind: u8) -> Vec<u8> {
    let mut d = Vec::with_capacity(MAX_DATAGRAM);
    d.extend_from_slice(&MAGIC);
    d.push(kind);
    d
}

/// The addresses one handshake's first message goes to, at most.
const ADDRESSES: usize = 8;

/// A handshake under way.
pub(crate) struct Handshake {
    state: HandshakeState,
    pub pattern: Pattern,
    /// This side's index, and the other's once known.
    pub index: u32,
    pub remote_index: u32,
    pub to: SocketAddrV4,
    pub started: Instant,
    /// The initiator's message 1, and the addresses it went to.
    first: Vec<u8>,
    pub tried: Vec<SocketAddrV4>,
}

fn builder<'a>(pattern: &str, identity: &'a Identity) -> Builder<'a> {
    Builder::new(crypto::params(pattern))
        .prologue(PROLOGUE)
        .and_then(|b| b.local_private_key(identity.private()))
        .expect("valid Noise parameters")
}

fn new_index() -> u32 {
    u32::from_be_bytes(crypto::random()).max(1)
}

impl Handshake {
    /// To a friend, whose key is known: message 1 carries `payload` (the
    /// time, which the friend keeps to refuse it again).
    pub(crate) fn friend(identity: &Identity, remote: &Key, payload: &[u8], to: SocketAddrV4, now: Instant) -> Option<(Handshake, Vec<u8>)> {
        let state = builder(FRIEND_PATTERN, identity).remote_public_key(remote).ok()?.build_initiator().ok()?;
        Handshake::first(state, Pattern::Friend, payload, &crypto::friend_mac_key(remote), to, now)
    }

    /// To a friend code's owner: message 1 proves the code.
    pub(crate) fn code(identity: &Identity, code: &str, to: SocketAddrV4, now: Instant) -> Option<(Handshake, Vec<u8>)> {
        let psk = crypto::code_psk(code);
        let state = builder(CODE_PATTERN, identity).psk(0, &psk).ok()?.build_initiator().ok()?;
        Handshake::first(state, Pattern::Code, &[], &crypto::code_mac_key(code), to, now)
    }

    fn first(
        mut state: HandshakeState,
        pattern: Pattern,
        payload: &[u8],
        mac_key: &Key,
        to: SocketAddrV4,
        now: Instant,
    ) -> Option<(Handshake, Vec<u8>)> {
        let index = new_index();
        let mut d = header(HS1);
        d.push(pattern as u8);
        d.extend_from_slice(&index.to_be_bytes());
        let mut buf = [0; 512];
        let len = state.write_message(payload, &mut buf).ok()?;
        d.extend_from_slice(&buf[..len]);
        let mac = crypto::first_mac(mac_key, &d);
        d.extend_from_slice(&mac);
        let h = Handshake { state, pattern, index, remote_index: 0, to, started: now, first: d.clone(), tried: vec![to] };
        Some((h, d))
    }

    /// Message 1 again, for another address the other side may be at (a
    /// few at most): it answers the first copy that reaches it.
    pub(crate) fn first_to(&mut self, to: SocketAddrV4) -> Option<Vec<u8>> {
        if self.first.is_empty() || self.tried.contains(&to) || self.tried.len() >= ADDRESSES {
            return None;
        }
        self.tried.push(to);
        Some(self.first.clone())
    }

    /// Answers message 1 (`hs1`: after the pattern and the sender's index):
    /// the handshake, message 1's payload, and the initiator's key when the
    /// pattern gives it (IK). Nothing for a message that does not open (not
    /// for this player's key, or not with the code).
    pub(crate) fn answer(
        identity: &Identity,
        pattern: Pattern,
        psk: Option<&Key>,
        remote_index: u32,
        hs1: &[u8],
        to: SocketAddrV4,
        now: Instant,
    ) -> Option<(Handshake, Vec<u8>)> {
        let state = match pattern {
            Pattern::Friend => builder(FRIEND_PATTERN, identity).build_responder().ok()?,
            Pattern::Code => builder(CODE_PATTERN, identity).psk(0, psk?).ok()?.build_responder().ok()?,
        };
        let mut hs =
            Handshake { state, pattern, index: new_index(), remote_index, to, started: now, first: Vec::new(), tried: Vec::new() };
        let mut payload = [0; 512];
        let len = hs.state.read_message(hs1, &mut payload).ok()?;
        Some((hs, payload[..len].to_vec()))
    }

    /// The other side's key, once the handshake has it.
    pub(crate) fn remote_key(&self) -> Option<Key> {
        self.state.get_remote_static()?.try_into().ok()
    }

    /// The next message this side sends (2 or 3).
    pub(crate) fn write(&mut self, payload: &[u8]) -> Option<Vec<u8>> {
        let kind = if self.state.is_initiator() { HS3 } else { HS2 };
        let mut d = header(kind);
        d.push(self.pattern as u8);
        d.extend_from_slice(&self.index.to_be_bytes());
        d.extend_from_slice(&self.remote_index.to_be_bytes());
        let mut buf = [0; 512];
        let len = self.state.write_message(payload, &mut buf).ok()?;
        d.extend_from_slice(&buf[..len]);
        Some(d)
    }

    /// Reads message 2 or 3: its payload.
    pub(crate) fn read(&mut self, message: &[u8]) -> Option<Vec<u8>> {
        let mut payload = [0; 512];
        let len = self.state.read_message(message, &mut payload).ok()?;
        Some(payload[..len].to_vec())
    }

    pub(crate) fn finished(&self) -> bool {
        self.state.is_handshake_finished()
    }

    pub(crate) fn into_transport(self, now: Instant) -> Option<Transport> {
        let remote_key = self.remote_key()?;
        Some(Transport {
            state: self.state.into_stateless_transport_mode().ok()?,
            index: self.index,
            remote_index: self.remote_index,
            remote_key,
            next_nonce: 0,
            window: Window::default(),
            established: now,
            heard: now,
            sent: now,
            addr: self.to,
        })
    }
}

/// Message 1 (after the kind) without its MAC, when the MAC is right under
/// `key`.
pub(crate) fn checked<'a>(hs1: &'a [u8], key: &Key) -> Option<&'a [u8]> {
    let (body, mac) = hs1.split_at_checked(hs1.len().checked_sub(MAC_LEN)?)?;
    let mut d = header(HS1);
    d.extend_from_slice(body);
    (crypto::first_mac(key, &d) == mac).then_some(body)
}

/// An open channel.
pub(crate) struct Transport {
    state: StatelessTransportState,
    pub index: u32,
    pub remote_index: u32,
    pub remote_key: Key,
    next_nonce: u64,
    window: Window,
    pub established: Instant,
    /// When the other side was last heard, and last sent to.
    pub heard: Instant,
    pub sent: Instant,
    /// Where the other side is (it follows the datagrams that open).
    pub addr: SocketAddrV4,
}

impl Transport {
    /// A datagram carrying `payload`.
    pub(crate) fn seal(&mut self, payload: &[u8], now: Instant) -> Option<Vec<u8>> {
        if payload.len() > MAX_PAYLOAD {
            return None;
        }
        let nonce = self.next_nonce;
        self.next_nonce += 1;
        let mut d = header(DATA);
        d.extend_from_slice(&self.remote_index.to_be_bytes());
        d.extend_from_slice(&nonce.to_be_bytes());
        let at = d.len();
        d.resize(at + payload.len() + 16, 0);
        let len = self.state.write_message(nonce, payload, &mut d[at..]).ok()?;
        d.truncate(at + len);
        self.sent = now;
        Some(d)
    }

    /// The payload of a datagram for this channel (after the index), once:
    /// a repeated or forged one gives nothing.
    pub(crate) fn open(&mut self, d: &[u8], from: SocketAddrV4, now: Instant) -> Option<Vec<u8>> {
        let nonce = u64::from_be_bytes(d.get(..8)?.try_into().unwrap());
        if !self.window.fresh(nonce) {
            return None;
        }
        let mut payload = vec![0; d.len().saturating_sub(8)];
        let len = self.state.read_message(nonce, &d[8..], &mut payload).ok()?;
        payload.truncate(len);
        self.window.mark(nonce);
        self.heard = now;
        self.addr = from;
        Some(payload)
    }
}

/// The nonces seen: the highest, and the 128 below it.
#[derive(Default)]
struct Window {
    /// The highest nonce seen + 1 (0: none yet).
    top: u64,
    bits: u128,
}

impl Window {
    fn fresh(&self, n: u64) -> bool {
        if n >= self.top {
            return true;
        }
        let back = self.top - 1 - n;
        back < 128 && self.bits >> back & 1 == 0
    }

    fn mark(&mut self, n: u64) {
        if n >= self.top {
            let shift = n + 1 - self.top;
            self.bits = if shift >= 128 { 0 } else { self.bits << shift };
            self.bits |= 1;
            self.top = n + 1;
        } else {
            self.bits |= 1 << (self.top - 1 - n);
        }
    }
}

/// The tags a probe carries at most (its id among them).
pub(crate) const MAX_TAGS: usize = (MAX_DATAGRAM - MAGIC.len() - 1) / 8;

/// A probe for `tags`.
pub(crate) fn probe(tags: &[Tag]) -> Vec<u8> {
    let mut d = header(PROBE);
    for t in tags.iter().take(MAX_TAGS) {
        d.extend_from_slice(t);
    }
    d
}

#[cfg(test)]
mod tests {
    use std::net::Ipv4Addr;

    use super::*;

    fn addr(port: u16) -> SocketAddrV4 {
        SocketAddrV4::new(Ipv4Addr::LOCALHOST, port)
    }

    /// The handshake's fields after the kind byte (message 1 without its
    /// MAC).
    fn split(d: &[u8], second: bool) -> (Pattern, u32, u32, &[u8]) {
        let d = &d[MAGIC.len() + 1..];
        let d = if second { d } else { &d[..d.len() - MAC_LEN] };
        let pattern = Pattern::from_byte(d[0]).unwrap();
        let sender = u32::from_be_bytes(d[1..5].try_into().unwrap());
        if second {
            (pattern, sender, u32::from_be_bytes(d[5..9].try_into().unwrap()), &d[9..])
        } else {
            (pattern, sender, 0, &d[5..])
        }
    }

    #[test]
    fn friends_open_a_channel() {
        let now = Instant::now();
        let (a, b, c) = (Identity::generate(), Identity::generate(), Identity::generate());
        let (mut ha, d1) = Handshake::friend(&a, b.public(), b"t=1", addr(2), now).unwrap();
        let (pattern, ia, _, m1) = split(&d1, false);
        // Only the friend's key opens it.
        assert!(Handshake::answer(&c, pattern, None, ia, m1, addr(1), now).is_none());
        let (mut hb, payload) = Handshake::answer(&b, pattern, None, ia, m1, addr(1), now).unwrap();
        assert_eq!(payload, b"t=1");
        assert_eq!(hb.remote_key(), Some(*a.public()));
        let d2 = hb.write(&[]).unwrap();
        assert!(hb.finished());
        let (_, ib, to_a, m2) = split(&d2, true);
        assert_eq!(to_a, ha.index);
        ha.remote_index = ib;
        ha.read(m2).unwrap();
        assert!(ha.finished());
        let mut ta = ha.into_transport(now).unwrap();
        let mut tb = hb.into_transport(now).unwrap();
        assert_eq!(ta.remote_key, *b.public());
        // Messages both ways; a repeated one is dropped, an old one still
        // opens once.
        let first = ta.seal(b"one", now).unwrap();
        let second = ta.seal(b"two", now).unwrap();
        let body = |d: &[u8]| d[MAGIC.len() + 1 + 4..].to_vec();
        assert_eq!(tb.open(&body(&second), addr(1), now).as_deref(), Some(&b"two"[..]));
        assert_eq!(tb.open(&body(&second), addr(1), now), None);
        assert_eq!(tb.open(&body(&first), addr(1), now).as_deref(), Some(&b"one"[..]));
        assert_eq!(tb.open(&body(&first), addr(1), now), None);
        let back = tb.seal(b"back", now).unwrap();
        assert_eq!(ta.open(&body(&back), addr(2), now).as_deref(), Some(&b"back"[..]));
        // Tampered: nothing.
        let mut forged = body(&ta.seal(b"x", now).unwrap());
        *forged.last_mut().unwrap() ^= 1;
        assert_eq!(tb.open(&forged, addr(1), now), None);
    }

    #[test]
    fn a_code_opens_a_channel_only_with_the_code() {
        let now = Instant::now();
        let (owner, asker) = (Identity::generate(), Identity::generate());
        let code = crypto::code_of(owner.public(), &crypto::random::<16>()).unwrap();
        let (mut hq, d1) = Handshake::code(&asker, &code, addr(1), now).unwrap();
        let (pattern, iq, _, m1) = split(&d1, false);
        let wrong = crypto::code_psk(&crypto::code_of(owner.public(), &crypto::random::<16>()).unwrap());
        assert!(Handshake::answer(&owner, pattern, Some(&wrong), iq, m1, addr(2), now).is_none());
        let psk = crypto::code_psk(&code);
        let (mut ho, _) = Handshake::answer(&owner, pattern, Some(&psk), iq, m1, addr(2), now).unwrap();
        let d2 = ho.write(&[]).unwrap();
        let (_, io, _, m2) = split(&d2, true);
        hq.remote_index = io;
        hq.read(m2).unwrap();
        assert_eq!(hq.remote_key(), Some(*owner.public()));
        let d3 = hq.write(&[]).unwrap();
        let (_, _, _, m3) = split(&d3, true);
        ho.read(m3).unwrap();
        assert_eq!(ho.remote_key(), Some(*asker.public()));
        assert!(ho.finished() && hq.finished());
        assert!(ho.into_transport(now).is_some() && hq.into_transport(now).is_some());
    }

    #[test]
    fn the_window_keeps_128_nonces() {
        let mut w = Window::default();
        for n in [0, 5, 3, 200, 100, 199] {
            assert!(w.fresh(n), "{n}");
            w.mark(n);
            assert!(!w.fresh(n));
        }
        assert!(!w.fresh(5), "too old to tell");
        assert!(w.fresh(150));
    }
}
