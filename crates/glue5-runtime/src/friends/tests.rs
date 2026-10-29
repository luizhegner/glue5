//! Players in memory: their datagrams go straight to each other, the DHT's
//! answers are given by hand.

use std::net::{Ipv4Addr, SocketAddrV4};
use std::path::PathBuf;
use std::time::{Duration, Instant};

use super::*;

struct Player {
    s: Service,
    addr: SocketAddrV4,
    dir: PathBuf,
}

fn dir(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("glue5-friends-{}-{name}-{:x}", std::process::id(), crate::p2p::random()));
    std::fs::create_dir_all(&d).unwrap();
    d
}

impl Player {
    fn new(name: &str, port: u16) -> Player {
        let dir = dir(name);
        let mut s = Service::load(&dir, name, u64::from(port) * 1000);
        s.identity();
        Player { s, addr: SocketAddrV4::new(Ipv4Addr::LOCALHOST, port), dir }
    }

    /// The file written now.
    fn flush(&mut self) {
        if let Some(saving) = self.s.saving(Instant::now(), true) {
            saving.write();
        }
    }

    /// The same player started again (from its file).
    fn restarted(&mut self) -> Player {
        self.flush();
        let s = Service::load(&self.dir, &self.s.local.name, self.s.local.id);
        Player { s, addr: self.addr, dir: self.dir.clone() }
    }

    fn key(&self) -> Key {
        *self.s.identity.as_ref().unwrap().public()
    }

    fn code(&self) -> String {
        self.s.code.clone().unwrap()
    }

    fn connected_to(&self, other: &Player) -> bool {
        self.s.friends.get(&other.key()).is_some_and(|f| f.connected() && f.presence.is_some())
    }
}

/// Delivers datagrams between the players until none is left; `drop`
/// decides which are lost. The players' ticks run every quarter second.
fn run(players: &mut [&mut Player], first: Out, from: SocketAddrV4, now: &mut Instant, ticks: u32, drop: &dyn Fn(&[u8]) -> bool) {
    let mut queue: Vec<(SocketAddrV4, SocketAddrV4, Vec<u8>)> = first.send.into_iter().map(|(to, d)| (from, to, d)).collect();
    for _ in 0..ticks {
        let mut rounds = 0;
        while !queue.is_empty() {
            rounds += 1;
            assert!(rounds < 1000, "datagrams going round");
            for (from, to, d) in std::mem::take(&mut queue) {
                if drop(&d) {
                    continue;
                }
                let Some(p) = players.iter_mut().find(|p| p.addr == to) else { continue };
                let mut out = Out::default();
                assert_eq!(d[..3], crate::p2p::MAGIC);
                p.s.receive(from, d[3], &d[4..], *now, &mut out);
                queue.extend(out.send.into_iter().map(|(t, d)| (p.addr, t, d)));
            }
        }
        *now += Duration::from_millis(250);
        for p in players.iter_mut() {
            let mut out = Out::default();
            p.s.tick(*now, Vec::new(), &[], p.addr.port(), &mut out);
            queue.extend(out.send.into_iter().map(|(t, d)| (p.addr, t, d)));
        }
    }
}

fn keep(_: &[u8]) -> bool {
    false
}

/// `b` asks `a` with `a`'s code, found on the DHT; `a` answers.
fn befriend(a: &mut Player, b: &mut Player, yes: bool, now: &mut Instant) {
    b.s.asked.push(Ask::new(a.code(), None));
    let mut out = Out::default();
    b.s.found(crypto::code_key(&a.code()), a.addr, *now, &mut out);
    let from = b.addr;
    run(&mut [&mut *a, &mut *b], out, from, now, 4, &keep);
    let key = b.key();
    assert_eq!(a.s.requests.get(&key).map(|r| (r.asked, r.name.as_str())), Some((true, b.s.local.name.as_str())));
    let mut out = Out::default();
    a.s.answer(&key, yes, *now, &mut out);
    let from = a.addr;
    run(&mut [&mut *a, &mut *b], out, from, now, 4, &keep);
}

#[test]
fn a_code_makes_friends_who_then_meet_again() {
    let mut now = Instant::now();
    let (mut a, mut b) = (Player::new("Ann", 1), Player::new("Bob", 2));
    befriend(&mut a, &mut b, true, &mut now);
    assert!(a.connected_to(&b) && b.connected_to(&a));
    assert!(b.s.asked.is_empty());
    assert_eq!(a.s.friends[&b.key()].name, "Bob");
    assert_eq!(b.s.friends[&a.key()].id, 1000);
    assert_eq!(a.s.friends[&b.key()].presence.as_ref().map(|p| p.playing), Some(Playing::Menu));

    // Both start again: whoever finds the other first (the DHT, its
    // meeting key) reaches it, whichever key is lower.
    for finder in [0, 1] {
        let (mut a2, mut b2) = (a.restarted(), b.restarted());
        assert_eq!(a2.s.friends.len(), 1);
        let (finder, other) = if finder == 0 { (&mut a2, &mut b2) } else { (&mut b2, &mut a2) };
        let shared = finder.s.friends.values().next().unwrap().shared;
        let mut out = Out::default();
        finder.s.found(crypto::meeting_key(&shared, crypto::epoch(unix())), other.addr, now, &mut out);
        // The higher key probes: the other has to probe back.
        if finder.key() > other.key() {
            let mut back = Out::default();
            other.s.on_probe(finder.addr, &probe_body(&other.s, &shared), now, &mut back);
            let from = other.addr;
            run(&mut [finder, other], back, from, &mut now, 4, &keep);
        } else {
            let from = finder.addr;
            run(&mut [finder, other], out, from, &mut now, 4, &keep);
        }
        assert!(a2.connected_to(&b2) && b2.connected_to(&a2));
    }
}

/// A probe's body (after the kind) for one tag, from someone else.
fn probe_body(_s: &Service, shared: &Key) -> Vec<u8> {
    let mut d = vec![9; 8];
    d.extend_from_slice(&crypto::meeting_tag(shared, crypto::epoch(unix())));
    d
}

#[test]
fn a_no_is_kept() {
    let mut now = Instant::now();
    let (mut a, mut c) = (Player::new("Ann", 1), Player::new("Carl", 3));
    befriend(&mut a, &mut c, false, &mut now);
    assert!(a.s.friends.is_empty() && c.s.friends.is_empty());
    assert!(c.s.asked[0].declined);
    assert!(a.s.blocked.contains(&c.key()));
    // Asking again: not heard.
    c.s.asked[0].declined = false;
    c.s.asked[0].contacted.clear();
    let mut out = Out::default();
    c.s.found(crypto::code_key(&a.code()), a.addr, now, &mut out);
    let from = c.addr;
    run(&mut [&mut a, &mut c], out, from, &mut now, 4, &keep);
    assert!(a.s.requests.is_empty());
}

#[test]
fn strangers_and_replays_are_not_answered() {
    let mut now = Instant::now();
    let (mut a, mut b, mut d) = (Player::new("Ann", 1), Player::new("Bob", 2), Player::new("Dan", 4));
    befriend(&mut a, &mut b, true, &mut now);
    // Dan knows Bob's key but is no friend of his: Bob says nothing.
    let mut out = Out::default();
    let time = unix_ms().to_be_bytes();
    let (h, first) = Handshake::friend(d.s.identity.as_ref().unwrap(), &b.key(), &time, b.addr, now).unwrap();
    d.s.handshakes.insert(h.index, (h, Owner::Friend(b.key())));
    let mut answer = Out::default();
    b.s.receive(d.addr, first[3], &first[4..], now, &mut answer);
    assert!(answer.send.is_empty());
    // A friend's handshake seen before (replayed by someone in between):
    // not answered again.
    let (lower, higher) = if a.key() < b.key() { (&mut a, &mut b) } else { (&mut b, &mut a) };
    let key = higher.key();
    let shared = lower.s.friends[&key].shared;
    lower.s.friends.get_mut(&key).unwrap().contacted.clear();
    lower.s.found(crypto::meeting_key(&shared, crypto::epoch(unix())), higher.addr, now + REKEY * 2, &mut out);
    let hs1 = out.send.iter().find(|(_, d)| d[3] == session::HS1).unwrap().1.clone();
    let mut first_answer = Out::default();
    let before = higher.s.friends[&lower.key()].transport.as_ref().map(|t| t.index);
    higher.s.receive(lower.addr, hs1[3], &hs1[4..], now, &mut first_answer);
    // Message 2 only: the new channel is taken once the other end shows it
    // has it (a replayed message 1 never does).
    assert_eq!(first_answer.send.len(), 1, "message 2");
    assert_eq!(higher.s.friends[&lower.key()].transport.as_ref().map(|t| t.index), before);
    let mut replayed = Out::default();
    higher.s.receive(d.addr, hs1[3], &hs1[4..], now, &mut replayed);
    assert!(replayed.send.is_empty());
}

#[test]
fn game_events_arrive_once_through_losses() {
    let mut now = Instant::now();
    let (mut a, mut b) = (Player::new("Ann", 1), Player::new("Bob", 2));
    befriend(&mut a, &mut b, true, &mut now);
    // Not kept while Bob is not in GTA Online.
    let mut out = Out::default();
    assert!(a.s.send_game(2000, "{\"stale\":1}", now, &mut out));
    let from = a.addr;
    run(&mut [&mut a, &mut b], out, from, &mut now, 4, &keep);
    assert!(b.s.events.is_empty());
    b.s.local.playing = Playing::Online;
    let mut out = Out::default();
    assert!(a.s.send_game(2000, "{\"invite\":1}", now, &mut out));
    assert!(!a.s.send_game(7, "nobody", now, &mut out));
    // The first copy is lost; the next one arrives, its ack too.
    let lost = std::cell::Cell::new(0);
    let drop = |d: &[u8]| d[3] == session::DATA && lost.replace(lost.get() + 1) == 0;
    let from = a.addr;
    run(&mut [&mut a, &mut b], out, from, &mut now, 12, &drop);
    let events: Vec<_> = b.s.events.drain(..).collect();
    assert_eq!(events.len(), 1);
    assert_eq!((events[0].from, events[0].name.as_str(), events[0].text.as_str()), (1000, "Ann", "{\"invite\":1}"));
    assert!(a.s.friends[&b.key()].outbox.is_empty());
}

/// `finder` finds `other`, a friend, on the DHT: they connect.
fn reconnect(finder: &mut Player, other: &mut Player, now: &mut Instant) {
    let shared = finder.s.friends[&other.key()].shared;
    let mut out = Out::default();
    finder.s.found(crypto::meeting_key(&shared, crypto::epoch(unix())), other.addr, *now, &mut out);
    let from = finder.addr;
    run(&mut [&mut *finder, &mut *other], out, from, now, 4, &keep);
    assert!(finder.connected_to(other) && other.connected_to(finder));
}

#[test]
fn messages_after_a_restart_are_not_repeats() {
    let mut now = Instant::now();
    let (mut a, mut b) = (Player::new("Ann", 1), Player::new("Bob", 2));
    befriend(&mut a, &mut b, true, &mut now);
    b.s.local.playing = Playing::Online;
    // Ann starts again, Bob does not: her messages still reach him.
    for round in 0..2 {
        if round == 1 {
            a = a.restarted();
            reconnect(&mut a, &mut b, &mut now);
        }
        let mut out = Out::default();
        assert!(a.s.send_game(2000, &format!("event {round}"), now, &mut out));
        let from = a.addr;
        run(&mut [&mut a, &mut b], out, from, &mut now, 4, &keep);
        assert_eq!(b.s.events.drain(..).map(|e| e.text).collect::<Vec<_>>(), [format!("event {round}")]);
    }
}

#[test]
fn removing_a_friend_tells_it() {
    let mut now = Instant::now();
    let (mut a, mut b) = (Player::new("Ann", 1), Player::new("Bob", 2));
    befriend(&mut a, &mut b, true, &mut now);
    let mut out = Out::default();
    a.s.remove_friend(&b.key(), &mut out);
    let from = a.addr;
    run(&mut [&mut a, &mut b], out, from, &mut now, 2, &keep);
    assert!(a.s.friends.is_empty() && b.s.friends.is_empty());
    assert!(a.restarted().s.friends.is_empty());
}

#[test]
fn friends_go_offline_when_silent() {
    let mut now = Instant::now();
    let (mut a, mut b) = (Player::new("Ann", 1), Player::new("Bob", 2));
    befriend(&mut a, &mut b, true, &mut now);
    // Bob's datagrams stop.
    let mut out = Out::default();
    for _ in 0..(SILENCE.as_millis() / 250 + 4) {
        now += Duration::from_millis(250);
        a.s.tick(now, Vec::new(), &[], 1, &mut out);
    }
    assert!(!a.s.friends[&b.key()].connected());
}

#[test]
fn invites_come_as_notices_once() {
    let mut now = Instant::now();
    let (mut a, mut b) = (Player::new("Ann", 1), Player::new("Bob", 2));
    befriend(&mut a, &mut b, true, &mut now);
    b.s.notices.clear();
    // From a session for invited players only: its lobby and session go too.
    a.s.joinable = false;
    a.s.local.session = Some("SESSION".into());
    for _ in 0..2 {
        let mut out = Out::default();
        assert!(a.s.send_invite(2000, Some("LOBBY".into()), now, &mut out));
        let from = a.addr;
        run(&mut [&mut a, &mut b], out, from, &mut now, 2, &keep);
    }
    // The second one, so soon: not again.
    assert_eq!(b.s.notices.drain(..).collect::<Vec<_>>(), [Notice::Invite("Ann".into())]);
    assert_eq!(b.s.pending(), 1);
    let invited = b.s.friends[&a.key()].invited.as_ref().unwrap();
    assert_eq!((invited.lobby.as_deref(), invited.session.as_deref()), (Some("LOBBY"), Some("SESSION")));
    // Not a friend's id: nothing sent.
    assert!(!a.s.send_invite(7, None, now, &mut Out::default()));
}

#[test]
fn players_met_ask_each_other() {
    let mut now = Instant::now();
    let (mut a, mut c) = (Player::new("Ann", 1), Player::new("Carl", 3));
    let card = |p: &Player| crate::p2p::Card { gamer: p.s.local.id, key: p.key(), name: p.s.local.name.clone() };
    let (ca, cc) = (card(&a), card(&c));
    // They meet in a lobby: each one's card and address.
    let mut out = Out::default();
    a.s.meet(&[(cc.clone(), c.addr)], now, &mut out);
    c.s.meet(&[(ca.clone(), a.addr)], now, &mut out);
    assert_eq!(a.s.recent.len(), 1);
    // Ann asks Carl; Carl says yes.
    a.s.asked.push(Ask::new(crypto::hex(&c.key()), Some((c.key(), "Carl".into()))));
    let mut out = Out::default();
    a.s.meet(&[(cc, c.addr)], now, &mut out);
    let from = a.addr;
    run(&mut [&mut a, &mut c], out, from, &mut now, 4, &keep);
    assert_eq!(c.s.notices.drain(..).collect::<Vec<_>>(), [Notice::Request("Ann".into())]);
    assert_eq!(c.s.pending(), 1);
    let mut out = Out::default();
    (a.s.summarized, c.s.summarized) = (true, true);
    c.s.answer(&a.key(), true, now, &mut out);
    let from = c.addr;
    run(&mut [&mut a, &mut c], out, from, &mut now, 4, &keep);
    assert!(a.connected_to(&c) && c.connected_to(&a));
    // Said once: no "online" for a friend just made.
    assert_eq!(a.s.notices.drain(..).collect::<Vec<_>>(), [Notice::Accepted("Carl".into())]);
    assert!(c.s.notices.is_empty());
    // Someone not met (no card from that address): not answered.
    let mut d = Player::new("Dan", 4);
    let (h, first) = Handshake::friend(d.s.identity.as_ref().unwrap(), &c.key(), &unix_ms().to_be_bytes(), c.addr, now).unwrap();
    d.s.handshakes.insert(h.index, (h, Owner::Asking("x".into())));
    let mut answer = Out::default();
    c.s.receive(d.addr, first[3], &first[4..], now, &mut answer);
    assert!(answer.send.is_empty());
}

/// Not a player's: datagrams sent there are lost.
fn nowhere() -> SocketAddrV4 {
    SocketAddrV4::new(Ipv4Addr::new(192, 0, 2, 7), 40000)
}

#[test]
fn a_stale_address_first_does_not_hold_up_the_others() {
    let mut now = Instant::now();
    let (mut a, mut b) = (Player::new("Ann", 1), Player::new("Bob", 2));
    befriend(&mut a, &mut b, true, &mut now);
    let (mut a, mut b) = (a.restarted(), b.restarted());
    let (lower, higher) = if a.key() < b.key() { (&mut a, &mut b) } else { (&mut b, &mut a) };
    let shared = lower.s.friends[&higher.key()].shared;
    let key = crypto::meeting_key(&shared, crypto::epoch(unix()));
    let mut out = Out::default();
    lower.s.found(key, nowhere(), now, &mut out);
    lower.s.found(key, higher.addr, now, &mut out);
    let from = lower.addr;
    run(&mut [&mut *lower, &mut *higher], out, from, &mut now, 2, &keep);
    assert!(lower.connected_to(higher) && higher.connected_to(lower));
    // A code's owner too.
    let mut c = Player::new("Carl", 3);
    c.s.asked.push(Ask::new(a.code(), None));
    let mut out = Out::default();
    c.s.found(crypto::code_key(&a.code()), nowhere(), now, &mut out);
    c.s.found(crypto::code_key(&a.code()), a.addr, now, &mut out);
    let from = c.addr;
    run(&mut [&mut a, &mut c], out, from, &mut now, 2, &keep);
    assert!(a.s.requests.get(&c.key()).is_some_and(|r| r.asked));
}

#[test]
fn a_forged_answer_does_not_cancel_the_handshake() {
    let mut now = Instant::now();
    let (mut a, mut b) = (Player::new("Ann", 1), Player::new("Bob", 2));
    befriend(&mut a, &mut b, true, &mut now);
    let (mut a, mut b) = (a.restarted(), b.restarted());
    let (lower, higher) = if a.key() < b.key() { (&mut a, &mut b) } else { (&mut b, &mut a) };
    let shared = lower.s.friends[&higher.key()].shared;
    let mut out = Out::default();
    lower.s.found(crypto::meeting_key(&shared, crypto::epoch(unix())), higher.addr, now, &mut out);
    let hs1 = &out.send.iter().find(|(_, d)| d[3] == session::HS1).unwrap().1;
    // Someone who saw message 1 answers first.
    let mut forged = vec![1];
    forged.extend_from_slice(&999u32.to_be_bytes());
    forged.extend_from_slice(&hs1[5..9]);
    forged.extend_from_slice(&[0; 48]);
    lower.s.receive(nowhere(), session::HS2, &forged, now, &mut Out::default());
    let from = lower.addr;
    run(&mut [&mut *lower, &mut *higher], out, from, &mut now, 2, &keep);
    assert!(lower.connected_to(higher) && higher.connected_to(lower));
}

#[test]
fn only_a_code_s_owner_answers_for_it() {
    let mut now = Instant::now();
    let (mut a, mut b, mut m) = (Player::new("Ann", 1), Player::new("Bob", 2), Player::new("Mal", 3));
    // Mal knows Ann's code and answers for it, first.
    m.s.code = Some(a.code());
    b.s.asked.push(Ask::new(a.code(), None));
    let mut out = Out::default();
    b.s.found(crypto::code_key(&a.code()), m.addr, now, &mut out);
    b.s.found(crypto::code_key(&a.code()), a.addr, now, &mut out);
    let from = b.addr;
    run(&mut [&mut b, &mut m, &mut a], out, from, &mut now, 4, &keep);
    assert!(m.s.requests.is_empty());
    assert!(a.s.requests.get(&b.key()).is_some_and(|r| r.asked));
}

#[test]
fn a_code_from_before_is_replaced_by_one_from_the_key() {
    let a = Player::new("Ann", 1);
    // A file of before: the key and a code, no salt.
    let dir = dir("Before");
    let store = Store { key: Some(crypto::hex(a.s.identity.as_ref().unwrap().private())), code: a.s.code.clone(), ..Store::default() };
    std::fs::write(dir.join(FILE), serde_json::to_vec(&store).unwrap()).unwrap();
    let s = Service::load(&dir, "Ann", 1);
    assert!(s.code.is_some() && s.code != a.s.code);
    assert_eq!(s.code, crypto::code_of(&a.key(), &s.salt));
}

#[test]
fn one_address_does_not_use_up_the_handshakes_of_all() {
    let mut now = Instant::now();
    let (mut a, mut b) = (Player::new("Ann", 1), Player::new("Bob", 2));
    befriend(&mut a, &mut b, true, &mut now);
    let (lower, higher) = if a.key() < b.key() { (&mut a, &mut b) } else { (&mut b, &mut a) };
    // Handshakes from one address, more than are answered in all.
    let flood = SocketAddrV4::new(Ipv4Addr::new(198, 51, 100, 1), 5000);
    let mut junk = vec![1];
    junk.extend_from_slice(&7u32.to_be_bytes());
    junk.extend_from_slice(&[0; 48]);
    for _ in 0..RATE_ALL + 10 {
        higher.s.receive(flood, session::HS1, &junk, now, &mut Out::default());
    }
    // The friend's own is answered.
    let key = higher.key();
    let shared = lower.s.friends[&key].shared;
    let mut out = Out::default();
    lower.s.found(crypto::meeting_key(&shared, crypto::epoch(unix())), higher.addr, now + REKEY * 2, &mut out);
    let hs1 = out.send.iter().find(|(_, d)| d[3] == session::HS1).unwrap().1.clone();
    let mut answer = Out::default();
    higher.s.receive(lower.addr, hs1[3], &hs1[4..], now, &mut answer);
    assert!(!answer.send.is_empty());
}

#[test]
fn a_lobby_of_more_players_than_are_kept_changes_nothing_until_one_comes() {
    let now = Instant::now();
    let mut a = Player::new("Ann", 1);
    let player = |i: u64| {
        let card = crate::p2p::Card { gamer: 5000 + i, key: crypto::random(), name: format!("P{i}") };
        (card, SocketAddrV4::new(Ipv4Addr::new(198, 51, 100, i as u8), 47000))
    };
    let lobby: Vec<_> = (0..MAX_RECENT as u64 + 10).map(player).collect();
    a.s.meet(&lobby, now, &mut Out::default());
    assert_eq!(a.s.recent.len(), MAX_RECENT);
    a.flush();
    let generation = a.s.generation;
    for t in 1..10 {
        a.s.meet(&lobby, now + Duration::from_millis(250 * t), &mut Out::default());
    }
    assert_eq!(a.s.generation, generation);
    assert!(a.s.saving(now, true).is_none());
    // One not kept can be asked all the same.
    let other = lobby.iter().find(|(c, _)| a.s.recent.iter().all(|r| r.key != c.key)).unwrap();
    assert_eq!(a.s.met_gamer(other.0.gamer), Some(other.0.key));
    assert_eq!(a.s.met_name(&other.0.key).as_deref(), Some(other.0.name.as_str()));
    // One kept gone, a new one come: a player here takes the place.
    let mut next = lobby[1..].to_vec();
    next.push(player(200));
    let gone = lobby[0].0.key;
    assert!(a.s.recent.iter().any(|r| r.key == gone));
    a.s.meet(&next, now, &mut Out::default());
    assert_eq!(a.s.recent.len(), MAX_RECENT);
    assert!(a.s.recent.iter().all(|r| next.iter().any(|(c, _)| c.key == r.key)));
    assert_eq!(a.s.generation, generation + 1);
}

#[test]
fn a_probe_with_this_player_s_id_from_elsewhere_hides_nothing() {
    let now = Instant::now();
    let mut a = Player::new("Ann", 1);
    let probe = a.s.probe_id.to_vec();
    a.s.on_probe(nowhere(), &probe, now, &mut Out::default());
    assert!(a.s.own.is_empty());
    a.s.on_probe(a.addr, &probe, now, &mut Out::default());
    assert!(a.s.own.contains(&a.addr));
}

#[test]
fn junk_handshakes_from_everywhere_use_up_nothing() {
    let mut now = Instant::now();
    let (mut a, mut b) = (Player::new("Ann", 1), Player::new("Bob", 2));
    befriend(&mut a, &mut b, true, &mut now);
    let (lower, higher) = if a.key() < b.key() { (&mut a, &mut b) } else { (&mut b, &mut a) };
    // Message 1 of both patterns, from more addresses than handshakes are
    // answered in all, without the MAC of whom they ask.
    let (answered, all) = (higher.s.answered.len(), higher.s.answered_all.1);
    for i in 0..RATE_ALL * 2 {
        let from = SocketAddrV4::new(Ipv4Addr::new(198, 51, 100, i as u8), 5000);
        for pattern in [1, 2] {
            let mut junk = vec![pattern];
            junk.extend_from_slice(&7u32.to_be_bytes());
            junk.extend_from_slice(&[i as u8; 96 + crypto::MAC_LEN]);
            higher.s.receive(from, session::HS1, &junk, now, &mut Out::default());
        }
    }
    assert_eq!((higher.s.answered.len(), higher.s.answered_all.1), (answered, all));
    // The friend's own is answered.
    let key = higher.key();
    let shared = lower.s.friends[&key].shared;
    let mut out = Out::default();
    lower.s.found(crypto::meeting_key(&shared, crypto::epoch(unix())), higher.addr, now + REKEY * 2, &mut out);
    let hs1 = out.send.iter().find(|(_, d)| d[3] == session::HS1).unwrap().1.clone();
    let mut answer = Out::default();
    higher.s.receive(lower.addr, hs1[3], &hs1[4..], now, &mut answer);
    assert!(!answer.send.is_empty());
}

#[test]
fn probes_find_friends_by_this_epoch_s_tag_and_the_last_one_s() {
    let mut now = Instant::now();
    let (mut a, mut b) = (Player::new("Ann", 1), Player::new("Bob", 2));
    befriend(&mut a, &mut b, true, &mut now);
    let shared = a.s.friends[&b.key()].shared;
    let epoch = crypto::epoch(unix());
    for (e, found) in [(epoch, true), (epoch - 1, true), (epoch - 2, false)] {
        a.s.friends.get_mut(&b.key()).unwrap().contacted.clear();
        a.s.handshakes.clear();
        let mut d = vec![9; 8];
        d.extend_from_slice(&crypto::meeting_tag(&shared, e));
        let mut out = Out::default();
        a.s.on_probe(b.addr, &d, now, &mut out);
        assert_eq!(out.send.iter().any(|(to, _)| *to == b.addr), found, "{e}");
    }
}

#[test]
fn the_file_is_written_at_most_every_few_seconds() {
    let mut now = Instant::now();
    let (mut a, c) = (Player::new("Ann", 1), Player::new("Carl", 3));
    a.flush();
    // A player met changes its name at every greeting, for 10 s.
    let mut writes = 0;
    for i in 0..40 {
        let card = crate::p2p::Card { gamer: 3000, key: c.key(), name: format!("Carl {i}") };
        a.s.meet(&[(card, c.addr)], now, &mut Out::default());
        if let Some(saving) = a.s.saving(now, false) {
            saving.write();
            writes += 1;
        }
        now += Duration::from_millis(250);
    }
    assert!((1..=2).contains(&writes), "{writes}");
    assert_eq!(a.restarted().s.recent[0].name, "Carl 39");
}

#[test]
fn a_file_that_cannot_be_read_is_kept() {
    let dir = dir("Bad");
    std::fs::write(dir.join(FILE), b"{ not json").unwrap();
    let mut p = Player { s: Service::load(&dir, "Ann", 1), addr: SocketAddrV4::new(Ipv4Addr::LOCALHOST, 1), dir: dir.clone() };
    p.s.identity();
    p.flush();
    let kept: Vec<_> = std::fs::read_dir(&dir)
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.file_name().unwrap().to_str().unwrap().starts_with("friends.json.bad-"))
        .collect();
    assert_eq!(kept.len(), 1);
    assert_eq!(std::fs::read(&kept[0]).unwrap(), b"{ not json");
    assert!(serde_json::from_slice::<Store>(&std::fs::read(dir.join(FILE)).unwrap()).is_ok_and(|s| s.key.is_some()));
}

#[test]
fn a_friend_cannot_take_another_s_gamer_id() {
    let mut now = Instant::now();
    let (mut a, mut v, mut m) = (Player::new("Ann", 1), Player::new("Vic", 2), Player::new("Mal", 3));
    befriend(&mut a, &mut v, true, &mut now);
    befriend(&mut a, &mut m, true, &mut now);
    assert_eq!((a.s.friends[&v.key()].id, a.s.friends[&m.key()].id), (2000, 3000));
    // Mal says he has Vic's id: not taken.
    m.s.local.id = 2000;
    let from = m.addr;
    run(&mut [&mut a, &mut m], Out::default(), from, &mut now, 2, &keep);
    assert_eq!(a.s.friends[&m.key()].id, 3000);
    let mut out = Out::default();
    assert!(a.s.send_game(2000, "{}", now, &mut out));
    assert!(out.send.iter().all(|(to, _)| *to == v.addr));
    // Vic installed again, with a new id: taken.
    v.s.local.id = 2500;
    let from = v.addr;
    run(&mut [&mut a, &mut v], Out::default(), from, &mut now, 2, &keep);
    assert_eq!(a.s.friends[&v.key()].id, 2500);    // Two players met say they have one id: neither is asked by it.
    let card = |p: &Player, gamer| crate::p2p::Card { gamer, key: p.key(), name: p.s.local.name.clone() };
    let (c, d) = (Player::new("Carl", 4), Player::new("Dan", 5));
    a.s.meet(&[(card(&c, 4000), c.addr)], now, &mut Out::default());
    assert_eq!(a.s.met_gamer(4000), Some(c.key()));
    a.s.meet(&[(card(&c, 4000), c.addr), (card(&d, 4000), d.addr)], now, &mut Out::default());
    assert_eq!(a.s.met_gamer(4000), None);
}

#[test]
fn game_events_as_large_as_the_game_s_arrive() {
    let mut now = Instant::now();
    let (mut a, mut b) = (Player::new("Ann", 1), Player::new("Bob", 2));
    befriend(&mut a, &mut b, true, &mut now);
    b.s.local.playing = Playing::Online;
    let long = format!("{{\"x\":\"{}\"}}", "a".repeat(1000));
    let mut out = Out::default();
    assert!(a.s.send_game(2000, &long, now, &mut out));
    let from = a.addr;
    run(&mut [&mut a, &mut b], out, from, &mut now, 2, &keep);
    assert_eq!(b.s.events.drain(..).map(|e| e.text).collect::<Vec<_>>(), [long]);
    // One that a datagram cannot carry (its quotes escaped): not sent.
    assert!(!a.s.send_game(2000, &"\"".repeat(1000), now, &mut Out::default()));
}

#[test]
fn asking_each_other_at_once_leaves_no_request() {
    let mut now = Instant::now();
    let (mut a, mut b) = (Player::new("Ann", 1), Player::new("Bob", 2));
    let ask = |asker: &mut Player, owner: &mut Player, now: &mut Instant| {
        asker.s.asked.push(Ask::new(owner.code(), None));
        let mut out = Out::default();
        asker.s.found(crypto::code_key(&owner.code()), owner.addr, *now, &mut out);
        let from = asker.addr;
        run(&mut [&mut *asker, &mut *owner], out, from, now, 4, &keep);
        assert!(owner.s.requests.get(&asker.key()).is_some_and(|r| r.asked));
    };
    ask(&mut a, &mut b, &mut now);
    ask(&mut b, &mut a, &mut now);
    let mut out = Out::default();
    a.s.answer(&b.key(), true, now, &mut out);
    let from = a.addr;
    run(&mut [&mut a, &mut b], out, from, &mut now, 4, &keep);
    assert!(a.connected_to(&b) && b.connected_to(&a));
    assert!(a.s.requests.is_empty() && a.s.asked.is_empty());
    assert!(b.s.requests.is_empty() && b.s.asked.is_empty());
}

#[test]
fn a_friend_followed_is_waited_for_from_the_game_s_search() {
    let mut now = Instant::now();
    let (mut a, mut b) = (Player::new("Ann", 1), Player::new("Bob", 2));
    befriend(&mut a, &mut b, true, &mut now);
    a.s.follow = Some(Follow { key: b.key(), asked: now, since: None });
    // The game takes its time to start (preparing, loading): still followed.
    assert_eq!(a.s.following(now + FOLLOW * 2), Some(2000));
    // It looks for a session: the friend is waited for from then.
    a.s.looking_for_session(now + FOLLOW * 2);
    a.s.looking_for_session(now + FOLLOW * 3);
    assert_eq!(a.s.following(now + FOLLOW * 2 + FOLLOW / 2), Some(2000));
    assert_eq!(a.s.following(now + FOLLOW * 3 + Duration::from_secs(1)), None);
    // A game that never looks for one: not followed for ever.
    a.s.follow = Some(Follow { key: b.key(), asked: now, since: None });
    assert_eq!(a.s.following(now + FOLLOW_START + Duration::from_secs(1)), None);
}

#[test]
fn the_session_joined_is_the_friend_s_whose_session_it_is() {
    let mut now = Instant::now();
    let (mut a, mut b, mut c) = (Player::new("Ann", 1), Player::new("Bob", 2), Player::new("Carl", 3));
    befriend(&mut a, &mut b, true, &mut now);
    befriend(&mut a, &mut c, true, &mut now);
    for (p, lobby) in [(&b, "B"), (&c, "C")] {
        let f = a.s.friends.get_mut(&p.key()).unwrap();
        f.presence = Some(Presence { playing: Playing::Online, lobbies: vec![lobby.into()], ..Presence::default() });
    }
    // "Join Friends" lists both, Bob's session is the one joined.
    assert!(a.s.reach(2000, true, Some(""), now).is_some());
    assert!(a.s.reach(3000, true, Some(""), now).is_some());
    a.s.session_changed(Some(2000), now);
    assert_eq!(a.s.joined.as_deref(), Some("B"));
    assert!(!a.s.joinable);
    // A session none of them is in: not a friend's.
    a.s.joined = None;
    a.s.reach(3000, true, Some(""), now);
    a.s.session_changed(None, now);
    assert_eq!(a.s.joined, None);
}

#[test]
fn a_code_owner_out_of_reach_finds_the_asker_and_probes_it() {
    // The DHT hands out the owner at the port its NAT gave a node, where
    // nothing of the asker's gets in; the asker announces itself under the
    // code too, the owner finds it there, probes it, and is asked.
    let mut now = Instant::now();
    let (mut a, mut b) = (Player::new("Ann", 1), Player::new("Bob", 2));
    b.s.asked.push(Ask::new(a.code(), None));
    let code_key = crypto::code_key(&a.code());
    assert!(b.s.lookups(unix()).contains(&(code_key, dht::Options { announce: true, refresh: LOOKUP_CODE })));
    let mut out = Out::default();
    b.s.found(code_key, nowhere(), now, &mut out);
    let from = b.addr;
    run(&mut [&mut a, &mut b], out, from, &mut now, 4, &keep);
    assert!(a.s.requests.is_empty());
    let mut out = Out::default();
    a.s.found(code_key, b.addr, now, &mut out);
    let from = a.addr;
    run(&mut [&mut a, &mut b], out, from, &mut now, 4, &keep);
    assert_eq!(a.s.requests.get(&b.key()).map(|r| r.asked), Some(true));
    // Probed again for a while, then no more.
    run(&mut [&mut a, &mut b], Out::default(), from, &mut now, (CALLING.as_millis() / 250) as u32 + 4, &keep);
    assert!(a.s.callers.is_empty());
}
