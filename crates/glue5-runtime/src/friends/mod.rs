//! Friends, with no server of ours: players who agreed to be, who see when
//! the other plays, join each other's games and invite each other, over a
//! channel only the two of them can read.
//!
//! - Each player has a key pair, made on the device (crypto.rs), and a
//!   friend code to give out. Whoever has the code can find its owner and
//!   ask to be friends; the owner says yes or no. Nothing else is shared
//!   with a stranger.
//! - Friends find each other on the BitTorrent DHT and the local network
//!   under keys only the two of them can compute, which change every 6
//!   hours, then open a Noise channel (session.rs) authenticated by their
//!   keys: a player answers no one else, and nothing it says to a friend
//!   is readable or forgeable by anyone else.
//! - Over it they tell each other their name, what they play and, playing
//!   GTA Online, their lobby and session (which a friend joins with); and
//!   they invite each other: to a session (the friend's game asks it
//!   whether to join), and with the game's own invites to jobs (game.rs).
//!
//! What a friend learns is the player's address on the Internet, as any
//! player of a lobby does. The DHT's nodes see two addresses meeting under
//! a key, not who they are.
//!
//! The service is a state machine fed with datagrams and time
//! (`Service`); `start`, `tick`, `receive` and `found` run it on the
//! player's socket (p2p.rs), the application reads and changes it through
//! the functions at the end.

mod crypto;
pub(crate) mod game;
mod session;

use std::collections::{HashMap, HashSet, VecDeque};
use std::net::{Ipv4Addr, SocketAddrV4};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crypto::{Identity, Key, Salt, Tag};
use session::{Handshake, Pattern, Transport};

use crate::Locked;
use crate::dht;

/// The file of friends, the player's key and code, in the user directory.
const FILE: &str = "friends.json";
/// Changes are written to it at once, then at most this often (the
/// network's: a player met changing its name at every greeting).
const SAVE_EVERY: Duration = Duration::from_secs(5);
/// Friends at most, requests waiting at most.
const MAX_FRIENDS: usize = 200;
const MAX_REQUESTS: usize = 8;
/// A channel stays open while the other side is heard; this much silence
/// and the friend is offline.
const SILENCE: Duration = Duration::from_secs(25);
/// How often something is said on a quiet channel, and the presence again.
const PING: Duration = Duration::from_secs(5);
const PRESENCE: Duration = Duration::from_secs(15);
/// A channel is replaced by a new handshake after this (fresh keys).
const REKEY: Duration = Duration::from_secs(15 * 60);
/// A handshake not answered by then is given up.
const HANDSHAKE: Duration = Duration::from_secs(5);
/// One address is tried for one friend at most this often.
const CONTACT: Duration = Duration::from_secs(2);
/// How often a message waiting for its acknowledgment goes again, and how
/// many times.
const RESEND: Duration = Duration::from_secs(1);
const TRIES: u32 = 10;
/// How often a friend request goes again while unanswered.
const ASK_AGAIN: Duration = Duration::from_secs(3);
/// How long a player the DHT gave under this player's code is probed (its
/// NAT may let nothing in before this player sends to it).
const CALLING: Duration = Duration::from_secs(30);
/// How often the local network is asked for friends not connected.
const PROBE_EVERY: Duration = Duration::from_secs(5);
const SWEEP_EVERY: Duration = Duration::from_secs(15);
/// Handshakes answered per address, and in all, per this time.
const RATE_WINDOW: Duration = Duration::from_secs(10);
const RATE_PER_ADDRESS: u32 = 10;
const RATE_ALL: u32 = 60;
/// DHT lookups: friends not connected are asked for often.
const LOOKUP_ALONE: Duration = Duration::from_secs(30);
/// A friend code is looked up often while asked with: the one asking and
/// its owner find each other there.
const LOOKUP_CODE: Duration = Duration::from_secs(10);
const LOOKUP_CONNECTED: Duration = Duration::from_secs(300);
/// An invite from a friend is shown this long, and one more from it is not
/// heard before then.
const INVITED: Duration = Duration::from_secs(300);
const INVITE_AGAIN: Duration = Duration::from_secs(20);
/// Texts from a friend, at most.
const MAX_NAME: usize = 32;
const MAX_LOBBIES: usize = 4;
const MAX_TEXT: usize = 900;

/// What a player is doing, as its friends see it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Playing {
    /// The application is open, no game yet.
    #[default]
    Menu,
    Story,
    Online,
}

/// What a player tells its friends about itself.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub(crate) struct Presence {
    pub name: String,
    /// Its gamer id in GTA Online.
    pub id: u64,
    pub playing: Playing,
    /// The lobbies it plays in, which a friend can join ("" is everyone's).
    #[serde(default)]
    pub lobbies: Vec<String>,
    /// The game's session, as the engine writes its info (game.rs), and
    /// who may join it.
    #[serde(default)]
    pub session: Option<String>,
    #[serde(default)]
    pub kind: SessionKind,
}

/// GTA Online's sessions, as its pause menu offers them: who may join.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum SessionKind {
    /// Anyone (matchmaking).
    #[default]
    Public,
    /// Friends, without an invite.
    Friends,
    /// Invited players only (invite only, crew).
    Closed,
    /// The player alone.
    Solo,
}

impl SessionKind {
    /// Whether friends join it without an invite.
    pub fn joinable(self) -> bool {
        matches!(self, SessionKind::Public | SessionKind::Friends)
    }
}

impl Presence {
    /// Within bounds, whatever a friend sent.
    fn bounded(mut self) -> Presence {
        self.name = clean_name(&self.name);
        self.lobbies.truncate(MAX_LOBBIES);
        self.lobbies.retain(|l| l.len() <= MAX_NAME);
        if self.session.as_ref().is_some_and(|s| s.len() > MAX_TEXT) {
            self.session = None;
        }
        self
    }
}

/// A name as shown: printable, 32 characters at most.
fn clean_name(name: &str) -> String {
    name.chars().filter(|c| !c.is_control()).take(MAX_NAME).collect::<String>().trim().to_owned()
}

/// What friends say to each other on the channel.
#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "t")]
enum Message {
    Presence(Presence),
    /// On a friend code's channel: "let us be friends".
    Request { name: String, id: u64 },
    Accept { name: String, id: u64 },
    Decline,
    /// "We are friends no more."
    Removed,
    /// The game's event for this friend (an invite to a job, an answer to
    /// one), its sequence number.
    Game { seq: u32, text: String },
    /// "Join me": an invite to the sender's session in GTA Online (the
    /// platform's invites, which Social Club's overlay showed), with its
    /// lobby and session when its presence does not tell them (a session
    /// for invited players only).
    Invite {
        seq: u32,
        #[serde(default)]
        lobby: Option<String>,
        #[serde(default)]
        session: Option<String>,
    },
    Ack { seq: u32 },
    Ping,
}

// ---- the file --------------------------------------------------------------

#[derive(Default, Serialize, Deserialize)]
struct Store {
    /// The private key (hex).
    #[serde(default)]
    key: Option<String>,
    #[serde(default)]
    code: Option<String>,
    /// What the code is made with besides the key (hex).
    #[serde(default)]
    salt: Option<String>,
    #[serde(default)]
    friends: Vec<StoredFriend>,
    /// Keys whose requests were declined (hex): not heard again.
    #[serde(default)]
    blocked: Vec<String>,
    /// Friend codes this player asked with, not answered yet.
    #[serde(default)]
    asked: Vec<String>,
    /// Players met in lobbies.
    #[serde(default)]
    recent: Vec<StoredFriend>,
}

#[derive(Clone, Serialize, Deserialize)]
struct StoredFriend {
    key: String,
    name: String,
    #[serde(default)]
    id: u64,
    #[serde(default)]
    since: u64,
}

// ---- the service -------------------------------------------------------------

fn unix() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

fn unix_ms() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_millis() as u64)
}

struct Friend {
    key: Key,
    /// The secret the two share, which their meetings come from.
    shared: Key,
    name: String,
    id: u64,
    since: u64,
    transport: Option<Transport>,
    /// The channel before the last handshake, for datagrams still on the way.
    previous: Option<Transport>,
    /// A channel it opened (its handshake answered here), taken once a
    /// datagram comes on it: proof the other end has it (a handshake
    /// message 1 replayed, or a message 2 lost, would else take the
    /// friend's traffic to a channel no one reads).
    pending: Option<Transport>,
    presence: Option<Presence>,
    /// When it was last told the presence, and what.
    presence_sent: Option<(Instant, Presence)>,
    /// The newest handshake time from it (ms): an older one is a replay.
    last_time: u64,
    /// Messages waiting for their acknowledgment: seq, payload, sent, tries.
    outbox: Vec<(u32, Vec<u8>, Instant, u32)>,
    /// From a random number at each start: the friend keeps the last ones
    /// it had from this player, which a count from 1 again would repeat.
    next_seq: u32,
    /// Game messages from it already passed on (repeated ones are not).
    seen: VecDeque<u32>,
    /// Its invite to its session, the latest.
    invited: Option<Invited>,
    /// Addresses tried for it, when.
    contacted: HashMap<SocketAddrV4, Instant>,
}

/// An invite from a friend to its session.
struct Invited {
    at: Instant,
    /// Where to join it, when the friend's presence does not say.
    lobby: Option<String>,
    session: Option<String>,
}

impl Friend {
    fn connected(&self) -> bool {
        self.transport.is_some()
    }

    /// The session it plays in, as it says (when friends may join it) or
    /// as its invite does.
    fn session(&self) -> Option<String> {
        self.transport.as_ref()?;
        let told = self.presence.as_ref().filter(|p| p.playing == Playing::Online).and_then(|p| p.session.clone());
        told.or_else(|| self.invited.as_ref().filter(|i| i.at.elapsed() < INVITED)?.session.clone())
    }

    /// Sends a message that is acknowledged (sent again until it is):
    /// false if the friend is not connected, or if the message does not fit
    /// in a datagram.
    fn send_reliably(&mut self, message: impl FnOnce(u32) -> Message, now: Instant, out: &mut Out) -> bool {
        let Some(t) = self.transport.as_mut() else { return false };
        let seq = self.next_seq;
        self.next_seq = self.next_seq.wrapping_add(1).max(1);
        let Ok(d) = serde_json::to_vec(&message(seq)) else { return false };
        let Some(sealed) = t.seal(&d, now) else {
            log::warn!("friends: a message of {} bytes not sent: too long", d.len());
            return false;
        };
        out.send.push((t.addr, sealed));
        self.outbox.push((seq, d, now, 0));
        true
    }

    /// Acknowledges message `seq`: whether it is the first time it came.
    fn first_time(&mut self, seq: u32, out: &mut Out) -> bool {
        if let Some(t) = self.transport.as_mut() {
            out.push(t.addr, seal(t, &Message::Ack { seq }));
        }
        if self.seen.contains(&seq) {
            return false;
        }
        self.seen.push_back(seq);
        if self.seen.len() > 64 {
            self.seen.pop_front();
        }
        true
    }
}

/// A request to this player, waiting for its answer.
struct Request {
    name: String,
    id: u64,
    transport: Transport,
    /// Told (the Request message came).
    asked: bool,
}

/// A request this player made: with a friend code, or to a player met in a
/// lobby (whose key its card gave).
struct Ask {
    /// The code, or the player's key (hex).
    id: String,
    /// The player met: its key and name.
    player: Option<(Key, String)>,
    transport: Option<Transport>,
    asked: Option<Instant>,
    declined: bool,
    contacted: HashMap<SocketAddrV4, Instant>,
    refused: Vec<SocketAddrV4>,
}

impl Ask {
    fn new(id: String, player: Option<(Key, String)>) -> Ask {
        Ask { id, player, transport: None, asked: None, declined: false, contacted: HashMap::new(), refused: Vec::new() }
    }

    /// The friend code asked with.
    fn code(&self) -> Option<&str> {
        self.player.is_none().then_some(self.id.as_str())
    }

    /// Addresses whose answer was not from the code's owner: not asked again.
    fn refuse(&mut self, addr: SocketAddrV4) {
        if self.refused.len() >= 16 {
            self.refused.remove(0);
        }
        self.refused.push(addr);
    }
}

/// The friends by what finds them in an epoch, worked out once per epoch
/// and as friends come and go (a hash each, which a probe would otherwise
/// cost per tag and friend): their meeting keys on the DHT, and their tags
/// on the local network (this epoch's and the last one's: clocks differ).
#[derive(Default)]
struct Meetings {
    epoch: Option<u64>,
    /// Each friend's key and tag this epoch.
    of: HashMap<Key, (dht::Id, Tag)>,
    keys: HashMap<dht::Id, Key>,
    tags: HashMap<Tag, Key>,
}

/// A friend followed into its session: asked when, and since when the
/// game looks for a session, which the wait for the friend counts from
/// (the game may take minutes to get there: preparing, loading).
struct Follow {
    key: Key,
    asked: Instant,
    since: Option<Instant>,
}

/// A player met in a lobby.
struct Recent {
    key: Key,
    name: String,
    /// Its gamer id.
    id: u64,
    /// When it was last met (seconds since 1970).
    seen: u64,
}

impl Recent {
    fn from_stored(r: &StoredFriend) -> Option<Recent> {
        Some(Recent { key: crypto::unhex(&r.key)?, name: clean_name(&r.name), id: r.id, seen: r.since })
    }
}

/// When the start's summary is told.
const SUMMARY: Duration = Duration::from_secs(20);

/// How long the game takes to join a friend's session, at most.
const JOINING: Duration = Duration::from_secs(120);

/// How long a new friend is new (seconds): no "online" notice.
const JUST_MADE: u64 = 10;

/// Players met kept, at most.
const MAX_RECENT: usize = 50;

/// This player's own addresses kept, at most.
const OWN: usize = 16;

/// What the application tells the player, as Social Club's notifications
/// did: the friend's name.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Notice {
    /// At the start, once friends had the time to be found: how many are
    /// online, how many ask to be friends.
    Summary { online: usize, requests: usize },
    /// A friend started playing (not the ones there at the start).
    Online(String),
    /// A friend stopped.
    Offline(String),
    /// Someone asks to be friends.
    Request(String),
    /// A friend invites this player to its session.
    Invite(String),
    /// A player asked said yes.
    Accepted(String),
}

/// Who a channel or handshake is with.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
enum Owner {
    Friend(Key),
    /// Someone asking this player (its key once known).
    Asker(Option<Key>),
    /// Someone this player asks (`Ask::id`).
    Asking(String),
}

/// An event for the game: a friend's invite and the like.
#[derive(Clone, Debug)]
pub(crate) struct GameEvent {
    pub from: u64,
    pub name: String,
    pub text: String,
}

pub(crate) struct Service {
    dir: PathBuf,
    identity: Option<Identity>,
    /// The friend code, made from the key and the salt.
    code: Option<String>,
    salt: Salt,
    /// This player, as told to friends (the lobbies are added as sent).
    local: Presence,
    friends: HashMap<Key, Friend>,
    meetings: Meetings,
    blocked: HashSet<Key>,
    requests: HashMap<Key, Request>,
    asked: Vec<Ask>,
    handshakes: HashMap<u32, (Handshake, Owner)>,
    /// Open channels by this side's index.
    channels: HashMap<u32, Owner>,
    /// Handshakes answered recently: per address, in all.
    answered: HashMap<Ipv4Addr, (Instant, u32)>,
    answered_all: (Instant, u32),
    /// A random id in this player's probes: its own come back (`OWN` at
    /// most).
    probe_id: [u8; 8],
    own: HashSet<SocketAddrV4>,
    /// Players asking with this player's code (they announce themselves
    /// under it), since when, and when last probed.
    callers: HashMap<SocketAddrV4, (Instant, Instant)>,
    probed: Option<Instant>,
    swept: Option<Instant>,
    /// The game's events from friends, for the game thread.
    events: VecDeque<GameEvent>,
    /// A friend to join in its session (game.rs).
    follow: Option<Follow>,
    /// Friends whose sessions the player chose to join, for the game (gamer
    /// ids, when), kept while the game is between sessions.
    joins: VecDeque<(u64, Instant)>,
    /// Players met in lobbies, the latest first, and the ones in them now
    /// (their keys, addresses, gamer ids and names).
    recent: Vec<Recent>,
    met: Vec<(Key, SocketAddrV4, u64, String)>,
    /// The newest handshake time from each player met who asked.
    stranger_times: HashMap<Key, u64>,
    /// Whether friends may join this player's session without an invite (a
    /// public or friends' session), and the name its card was last given.
    joinable: bool,
    card: Option<String>,
    /// The friend's lobby whose session this player joined, where its
    /// invites lead (its own lobby otherwise); the friends whose sessions
    /// the game may be joining (gamer id, lobby, since when: "Join Friends"
    /// lists all those it can join); whether the game looks for a friend's
    /// session to join.
    joined: Option<String>,
    joining: Vec<(u64, String, Instant)>,
    finding: bool,
    /// What happened that the player should know, for the application, and
    /// whether the summary of the start was told.
    notices: VecDeque<Notice>,
    started: Instant,
    summarized: bool,
    /// Bumped at every change the application shows.
    generation: u64,
    /// Whether the file may be written (see `load`), whether there are
    /// changes to write to it, and when it was last; the writes in order
    /// (`Saving`).
    writable: bool,
    unsaved: bool,
    saved: Option<Instant>,
    version: u64,
    written: Arc<Mutex<u64>>,
}

/// The file's content, written outside the service's lock (the system may
/// take a while to write it through).
pub(crate) struct Saving {
    path: PathBuf,
    bytes: Vec<u8>,
    version: u64,
    written: Arc<Mutex<u64>>,
}

impl Saving {
    /// Writes the file, unless newer content was written already. When it
    /// could not (a full disk), the changes are saved again later: else they
    /// would wait for some other change, and be lost at exit.
    fn write(self) {
        let mut written = self.written.locked();
        if self.version <= *written {
            return;
        }
        let temp = self.path.with_file_name(format!("{FILE}.new"));
        match write_private(&temp, &self.bytes).and_then(|()| std::fs::rename(&temp, &self.path)) {
            Ok(()) => *written = self.version,
            Err(e) => {
                log::warn!("friends: saving {}: {e}", self.path.display());
                drop(written);
                with(|s, _| s.save());
            }
        }
    }
}

/// What the service wants sent.
#[derive(Default)]
pub(crate) struct Out {
    pub send: Vec<(SocketAddrV4, Vec<u8>)>,
}

impl Out {
    fn push(&mut self, to: SocketAddrV4, d: Option<Vec<u8>>) {
        if let Some(d) = d {
            self.send.push((to, d));
        }
    }
}

impl Service {
    fn load(dir: &Path, name: &str, id: u64) -> Service {
        let path = dir.join(FILE);
        // A file that cannot be read (or has no key that can) is not written
        // over, the friends in it lost: it is put aside; one there that
        // cannot be read now is left as it is, and nothing is written.
        let (store, writable) = match std::fs::read(&path) {
            Ok(b) => match serde_json::from_slice::<Store>(&b) {
                Ok(s) if s.key.as_deref().is_none_or(|k| crypto::unhex::<32>(k).is_some()) => (s, true),
                Ok(_) => (Store::default(), put_aside(&path, "no key")),
                Err(e) => (Store::default(), put_aside(&path, &e.to_string())),
            },
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => (Store::default(), true),
            Err(e) => {
                log::warn!("friends: {}: {e}; not written this time", path.display());
                (Store::default(), false)
            }
        };
        let identity = store.key.as_deref().and_then(crypto::unhex::<32>).map(Identity::from_private);
        // The code comes from the key and the salt; one from before (random,
        // which anyone who knew it could answer for) is replaced.
        let (salt, new_code) = match store.salt.as_deref().and_then(crypto::unhex::<16>) {
            Some(salt) => (salt, false),
            None => (crypto::random(), identity.is_some()),
        };
        let code = identity.as_ref().and_then(|i| crypto::code_of(i.public(), &salt));
        let mut service = Service {
            dir: dir.to_owned(),
            identity,
            code,
            salt,
            local: Presence { name: clean_name(name), id, ..Presence::default() },
            friends: HashMap::new(),
            meetings: Meetings::default(),
            blocked: store.blocked.iter().filter_map(|k| crypto::unhex(k)).collect(),
            requests: HashMap::new(),
            asked: Vec::new(),
            handshakes: HashMap::new(),
            channels: HashMap::new(),
            answered: HashMap::new(),
            answered_all: (Instant::now(), 0),
            probe_id: crypto::random(),
            own: HashSet::new(),
            callers: HashMap::new(),
            probed: None,
            swept: None,
            events: VecDeque::new(),
            follow: None,
            joins: VecDeque::new(),
            recent: store.recent.iter().filter_map(Recent::from_stored).collect(),
            met: Vec::new(),
            stranger_times: HashMap::new(),
            joinable: true,
            card: None,
            joined: None,
            joining: Vec::new(),
            finding: false,
            notices: VecDeque::new(),
            started: Instant::now(),
            summarized: false,
            generation: 1,
            writable,
            unsaved: false,
            saved: None,
            version: 0,
            written: Arc::new(Mutex::new(0)),
        };
        for f in store.friends {
            if let Some(key) = crypto::unhex(&f.key) {
                service.add_friend(key, &f.name, f.id, f.since);
            }
        }
        for code in store.asked {
            if let Some(code) = crypto::parse_code(&code) {
                service.asked.push(Ask::new(code, None));
            }
        }
        if new_code {
            log::info!("friends: a new friend code, made from the key");
            service.save();
        }
        service
    }

    /// There are changes to keep: the file is written soon (`saving`).
    fn save(&mut self) {
        self.unsaved = true;
    }

    /// The file's content to write now, when there are changes: at once,
    /// or at most every `SAVE_EVERY`.
    fn saving(&mut self, now: Instant, at_once: bool) -> Option<Saving> {
        if !self.unsaved || !at_once && self.saved.is_some_and(|t| now - t < SAVE_EVERY) {
            return None;
        }
        self.unsaved = false;
        if !self.writable {
            return None;
        }
        self.saved = Some(now);
        self.version += 1;
        let store = Store {
            key: self.identity.as_ref().map(|i| crypto::hex(i.private())),
            code: self.code.clone(),
            salt: Some(crypto::hex(&self.salt)),
            friends: self
                .friends
                .values()
                .map(|f| StoredFriend { key: crypto::hex(&f.key), name: f.name.clone(), id: f.id, since: f.since })
                .collect(),
            blocked: self.blocked.iter().map(|k| crypto::hex(k)).collect(),
            asked: self.asked.iter().filter(|a| !a.declined).filter_map(|a| Some(a.code()?.to_owned())).collect(),
            recent: self
                .recent
                .iter()
                .map(|r| StoredFriend { key: crypto::hex(&r.key), name: r.name.clone(), id: r.id, since: r.seen })
                .collect(),
        };
        let bytes = serde_json::to_vec_pretty(&store).map_err(|e| log::warn!("friends: saving: {e}")).ok()?;
        Some(Saving { path: self.dir.join(FILE), bytes, version: self.version, written: self.written.clone() })
    }

    /// Whether there is anything to do on the network.
    fn active(&self) -> bool {
        self.identity.is_some() && (self.code.is_some() || !self.friends.is_empty() || !self.asked.is_empty())
    }

    /// The player's key and code, made the first time.
    fn identity(&mut self) -> &Identity {
        if self.identity.is_none() {
            self.identity = Some(Identity::generate());
            self.new_code();
            log::info!("friends: a new key and friend code");
        }
        self.identity.as_ref().unwrap()
    }

    /// A new salt, and with it a new code.
    fn new_code(&mut self) {
        let Some(identity) = &self.identity else { return };
        self.salt = crypto::random();
        self.code = crypto::code_of(identity.public(), &self.salt);
        self.generation += 1;
        self.save();
    }

    fn add_friend(&mut self, key: Key, name: &str, id: u64, since: u64) -> bool {
        let Some(identity) = self.identity.as_ref() else { return false };
        let Some(shared) = identity.shared(&key) else { return false };
        if key == *identity.public() || self.friends.len() >= MAX_FRIENDS {
            return false;
        }
        let id = if self.id_taken(id, &key) { 0 } else { id };
        self.blocked.remove(&key);
        self.meetings = Meetings::default();
        // Its request and this player's to it are answered (both asked at
        // once, and one said yes).
        if let Some(r) = self.requests.remove(&key) {
            self.channels.remove(&r.transport.index);
        }
        let mut answered = Vec::new();
        self.asked.retain_mut(|a| {
            let theirs = a.player.as_ref().is_some_and(|(k, _)| *k == key) || a.transport.as_ref().is_some_and(|t| t.remote_key == key);
            if theirs {
                answered.extend(a.transport.take());
            }
            !theirs
        });
        for t in answered {
            self.channels.remove(&t.index);
        }
        self.friends.entry(key).or_insert(Friend {
            key,
            shared,
            name: clean_name(name),
            id,
            since,
            transport: None,
            previous: None,
            pending: None,
            presence: None,
            presence_sent: None,
            last_time: 0,
            outbox: Vec::new(),
            next_seq: u32::from_be_bytes(crypto::random()).max(1),
            seen: VecDeque::new(),
            invited: None,
            contacted: HashMap::new(),
        });
        self.generation += 1;
        true
    }

    /// Whether gamer id `id` is someone else's than friend `key`'s: another
    /// friend's, or this player's. A gamer id is what a player says it is,
    /// and the game knows friends by it (their events, invites, "is a
    /// friend"): a friend does not get one another has, while the other
    /// has it (an id changes when the game is installed again).
    fn id_taken(&self, id: u64, key: &Key) -> bool {
        id != 0 && (id == self.local.id || self.friends.values().any(|f| f.id == id && f.key != *key))
    }

    /// The friend with gamer id `id` (none for 0: not told yet).
    fn friend_of(&mut self, id: u64) -> Option<&mut Friend> {
        self.friends.values_mut().find(|f| f.id == id && id != 0)
    }

    fn drop_channel(&mut self, transport: Option<Transport>) {
        if let Some(t) = transport {
            self.channels.remove(&t.index);
        }
    }

    fn remove_friend(&mut self, key: &Key, out: &mut Out) {
        let Some(mut f) = self.friends.remove(key) else { return };
        self.meetings = Meetings::default();
        if let Some(t) = f.transport.as_mut() {
            out.push(t.addr, seal(t, &Message::Removed));
        }
        self.drop_channel(f.transport);
        self.drop_channel(f.previous);
        self.drop_channel(f.pending);
        self.handshakes.retain(|_, (_, o)| *o != Owner::Friend(*key));
        self.generation += 1;
        self.save();
    }

    // ---- what the player tells ----

    /// This player as told to friends: where to join it only when they may
    /// without an invite.
    fn presence_for_friends(&self, lobbies: Vec<String>) -> Presence {
        let mut p = self.local.clone();
        let open = p.playing == Playing::Online && self.joinable;
        // The game's lobby, not friends' visited (theirs to share).
        p.lobbies = if open { lobbies.into_iter().take(1).collect() } else { Vec::new() };
        if !open {
            p.session = None;
        }
        p
    }

    // ---- time ----

    /// The friends' meeting keys and tags for the epoch of `unix`.
    fn update_meetings(&mut self, unix: u64) {
        let epoch = crypto::epoch(unix);
        if self.meetings.epoch == Some(epoch) {
            return;
        }
        let mut m = Meetings { epoch: Some(epoch), ..Meetings::default() };
        for f in self.friends.values() {
            let (key, tag) = (crypto::meeting_key(&f.shared, epoch), crypto::meeting_tag(&f.shared, epoch));
            m.of.insert(f.key, (key, tag));
            m.keys.insert(key, f.key);
            m.tags.insert(tag, f.key);
            m.tags.insert(crypto::meeting_tag(&f.shared, epoch.saturating_sub(1)), f.key);
        }
        self.meetings = m;
    }

    /// The DHT lookups wanted now: key, options.
    fn lookups(&mut self, unix: u64) -> Vec<(dht::Id, dht::Options)> {
        let mut keys = Vec::new();
        if !self.active() {
            return keys;
        }
        self.update_meetings(unix);
        for (k, (key, _)) in &self.meetings.of {
            let refresh = if self.friends[k].connected() { LOOKUP_CONNECTED } else { LOOKUP_ALONE };
            keys.push((*key, dht::Options { announce: true, refresh }));
        }
        // The ones asking with a code announce themselves under it too: a
        // NAT giving each address another port lets nothing in from them
        // before its player sends to them (`callers`).
        if let Some(code) = &self.code {
            keys.push((crypto::code_key(code), dht::Options { announce: true, refresh: LOOKUP_CODE }));
        }
        for code in self.asked.iter().filter(|a| a.transport.is_none() && !a.declined).filter_map(Ask::code) {
            keys.push((crypto::code_key(code), dht::Options { announce: true, refresh: LOOKUP_CODE }));
        }
        keys
    }

    pub(crate) fn tick(
        &mut self,
        now: Instant,
        lobbies: Vec<String>,
        players: &[(crate::p2p::Card, SocketAddrV4)],
        port: u16,
        out: &mut Out,
    ) {
        if self.identity.is_none() {
            return;
        }
        // The card the players met see.
        if self.card.as_deref() != Some(self.local.name.as_str()) {
            let key = *self.identity.as_ref().unwrap().public();
            crate::p2p::set_card(crate::p2p::Card { gamer: self.local.id, key, name: self.local.name.clone() });
            self.card = Some(self.local.name.clone());
        }
        self.meet(players, now, out);
        // The friends found by then, and the requests.
        if !self.summarized && now - self.started > SUMMARY {
            self.summarized = true;
            let online = self.friends.values().filter(|f| f.connected()).count();
            let requests = self.requests.values().filter(|r| r.asked).count();
            if online + requests > 0 {
                self.notices.push_back(Notice::Summary { online, requests });
            }
        }
        // Players asking with this player's code, probed for a while.
        self.callers.retain(|_, (since, _)| now - *since < CALLING);
        if let Some(code) = self.code.as_deref() {
            let d = self.probe(&[crypto::code_tag(code)]);
            for (addr, (_, probed)) in &mut self.callers {
                if now - *probed >= ASK_AGAIN {
                    *probed = now;
                    out.send.push((*addr, d.clone()));
                }
            }
        }
        // Handshakes not answered, and the count of the ones answered.
        self.handshakes.retain(|_, (h, _)| now - h.started < HANDSHAKE);
        self.answered.retain(|_, (t, _)| now - *t < RATE_WINDOW);
        let presence = self.presence_for_friends(lobbies);
        let me = *self.identity.as_ref().unwrap().public();
        let keys: Vec<Key> = self.friends.keys().copied().collect();
        for key in keys {
            let f = self.friends.get_mut(&key).unwrap();
            if f.previous.as_ref().is_some_and(|t| now - t.heard > HANDSHAKE * 2) {
                let t = f.previous.take();
                self.drop_channel(t);
            }
            let f = self.friends.get_mut(&key).unwrap();
            if f.pending.as_ref().is_some_and(|t| now - t.heard > HANDSHAKE * 2) {
                let t = f.pending.take();
                self.drop_channel(t);
            }
            let f = self.friends.get_mut(&key).unwrap();
            let Some(t) = f.transport.as_mut() else { continue };
            if now - t.heard > SILENCE {
                log::info!("friends: {} is offline", f.name);
                self.notices.push_back(Notice::Offline(f.name.clone()));
                f.presence = None;
                f.presence_sent = None;
                let t = f.transport.take();
                self.drop_channel(t);
                self.generation += 1;
                continue;
            }
            // Messages not acknowledged go again, a few times.
            let mut resend = Vec::new();
            f.outbox.retain_mut(|(_, d, sent, tries)| {
                if now - *sent >= RESEND {
                    *sent = now;
                    *tries += 1;
                    resend.push(d.clone());
                }
                *tries <= TRIES
            });
            for d in resend {
                out.push(t.addr, t.seal(&d, now));
            }
            let stale = f.presence_sent.as_ref().is_none_or(|(at, p)| *p != presence || now - *at > PRESENCE);
            if stale {
                f.presence_sent = Some((now, presence.clone()));
                out.push(t.addr, seal(t, &Message::Presence(presence.clone())));
            } else if now - t.sent > if t.heard == t.established { RESEND } else { PING } {
                // A channel nothing came on yet: often, until the other side
                // takes it (it waits for a datagram on it; see `pending`).
                out.push(t.addr, seal(t, &Message::Ping));
            }
            // Fresh keys now and then: the side that starts handshakes.
            let (addr, old) = (t.addr, now - t.established > REKEY);
            if old && me < key && !self.handshakes.values().any(|(_, o)| *o == Owner::Friend(key)) {
                self.contact(key, addr, false, now, out);
            }
        }
        // Requests this player made: again while unanswered.
        let request = Message::Request { name: self.local.name.clone(), id: self.local.id };
        for a in &mut self.asked {
            let Some(t) = a.transport.as_mut() else { continue };
            if now - t.heard > SILENCE {
                let t = a.transport.take().unwrap();
                self.channels.remove(&t.index);
                continue;
            }
            if a.asked.is_none_or(|at| now - at > ASK_AGAIN) {
                a.asked = Some(now);
                out.push(t.addr, seal(t, &request));
            }
        }
        // Requests to this player: their channels stay open while it decides.
        let mut gone = Vec::new();
        for (key, r) in &mut self.requests {
            if now - r.transport.heard > SILENCE {
                gone.push(*key);
            } else if now - r.transport.sent > PING {
                out.push(r.transport.addr, seal(&mut r.transport, &Message::Ping));
            }
        }
        for key in gone {
            if let Some(r) = self.requests.remove(&key) {
                self.channels.remove(&r.transport.index);
                self.generation += 1;
            }
        }
        // The local network: probes for friends and codes not connected.
        if self.probed.is_none_or(|t| now - t > PROBE_EVERY) {
            self.probed = Some(now);
            let tags = self.wanted_tags(unix());
            if !tags.is_empty() {
                let d = self.probe(&tags);
                for p in crate::p2p::PORTS {
                    out.send.push((SocketAddrV4::new(Ipv4Addr::BROADCAST, p), d.clone()));
                    if p != port {
                        out.send.push((SocketAddrV4::new(Ipv4Addr::LOCALHOST, p), d.clone()));
                    }
                }
                if self.swept.is_none_or(|t| now - t > SWEEP_EVERY) {
                    self.swept = Some(now);
                    if let Some(own) = crate::p2p::lan_address() {
                        let [a, b, c, _] = own.octets();
                        for x in 1..255 {
                            let ip = Ipv4Addr::new(a, b, c, x);
                            if ip != own {
                                out.send.push((SocketAddrV4::new(ip, crate::p2p::PORTS.start), d.clone()));
                            }
                        }
                    }
                }
            }
        }
    }

    /// The players met in the lobbies: kept as recent players, and reached
    /// when asked.
    fn meet(&mut self, players: &[(crate::p2p::Card, SocketAddrV4)], now: Instant, out: &mut Out) {
        let me = *self.identity.as_ref().unwrap().public();
        let today = unix();
        let mut changed = false;
        self.met = players.iter().map(|(c, a)| (c.key, *a, c.gamer, clean_name(&c.name))).collect();
        for (card, addr) in players {
            if card.key == me || self.friends.contains_key(&card.key) {
                continue;
            }
            let name = clean_name(&card.name);
            match self.recent.iter().position(|r| r.key == card.key) {
                Some(i) => {
                    let r = &mut self.recent[i];
                    // Kept again at most every few minutes.
                    if today.saturating_sub(r.seen) > 300 || r.name != name {
                        r.seen = today;
                        r.name = name;
                        r.id = card.gamer;
                        changed = true;
                    }
                }
                // Kept in place of the one met longest ago not here now: not
                // when all are (with more players here, they went in and out
                // at every tick, the file written every few seconds).
                None if self.recent.len() >= MAX_RECENT && !self.make_room() => {}
                None => {
                    self.recent.insert(0, Recent { key: card.key, name, id: card.gamer, seen: today });
                    changed = true;
                }
            }
            if let Some(i) = self.asked.iter().position(|a| a.player.as_ref().is_some_and(|(k, _)| *k == card.key)) {
                self.ask_at(i, *addr, now, out);
            }
        }
        if changed {
            self.recent.sort_by_key(|r| std::cmp::Reverse(r.seen));
            self.recent.truncate(MAX_RECENT);
            self.generation += 1;
            self.save();
        }
    }

    /// Drops the player met longest ago that is not in the lobbies now.
    fn make_room(&mut self) -> bool {
        let here = |key: &Key| self.met.iter().any(|(k, ..)| k == key);
        let gone = self.recent.iter().enumerate().filter(|(_, r)| !here(&r.key)).min_by_key(|(_, r)| r.seen).map(|(i, _)| i);
        gone.map(|i| self.recent.remove(i)).is_some()
    }

    /// The player met in a lobby now (not this one, not a friend) with gamer
    /// id `id`: none when another one met now says it has the same.
    fn met_gamer(&self, id: u64) -> Option<Key> {
        let mut keys = self.met.iter().filter(|(_, _, g, _)| *g == id && id != 0).map(|(k, ..)| *k);
        let key = keys.next()?;
        let me = self.identity.as_ref().map(|i| *i.public());
        (keys.all(|k| k == key) && Some(key) != me && !self.friends.contains_key(&key)).then_some(key)
    }

    /// The name of a player met, kept or in the lobbies now.
    fn met_name(&self, key: &Key) -> Option<String> {
        let kept = self.recent.iter().find(|r| r.key == *key).map(|r| &r.name);
        kept.or_else(|| self.met.iter().find(|(k, ..)| k == key).map(|(.., name)| name)).cloned()
    }

    /// The friend (gamer id) to join, while it is waited for.
    fn following(&mut self, now: Instant) -> Option<u64> {
        let f = self.follow.as_ref()?;
        let over = match f.since {
            Some(t) => now - t > FOLLOW,
            None => now - f.asked > FOLLOW_START,
        };
        let id = self.friends.get(&f.key).map(|f| f.id);
        if over || id.is_none() {
            self.follow = None;
            return None;
        }
        id.filter(|id| *id != 0)
    }

    /// The game looks for a session (it hosts one or looks for one, the
    /// first time since the friend was followed): the friend is waited for
    /// from now.
    fn looking_for_session(&mut self, now: Instant) {
        if let Some(f) = &mut self.follow
            && f.since.is_none()
        {
            f.since = Some(now);
        }
    }

    /// Friend `id`'s address and lobbies (its players introduced by it
    /// there). `joining`: the game joins its session, which, in the
    /// friend's lobby, will be the friend's to share (`session_changed`).
    fn reach(&mut self, id: u64, joining: bool, ours: Option<&str>, now: Instant) -> Option<(SocketAddrV4, Vec<String>)> {
        let f = self.friend_of(id)?;
        let addr = f.transport.as_ref()?.addr;
        let mut lobbies = f.presence.as_ref().filter(|p| p.playing == Playing::Online)?.lobbies.clone();
        lobbies.extend(f.invited.as_ref().and_then(|i| i.lobby.clone()));
        if joining && let Some(theirs) = lobbies.first().filter(|l| Some(l.as_str()) != ours) {
            self.joining.retain(|(i, _, since)| *i != id && now - *since < JOINING);
            self.joining.push((id, theirs.clone(), now));
        }
        Some((addr, lobbies))
    }

    /// The game is in a new session: friend `friend`'s, one being joined in
    /// its lobby, it is shared only through invites.
    fn session_changed(&mut self, friend: Option<u64>, now: Instant) {
        let joining = std::mem::take(&mut self.joining);
        let Some((_, lobby, _)) = joining.into_iter().find(|(id, _, since)| Some(*id) == friend && now - *since < JOINING)
        else {
            return;
        };
        log::info!("friends: in a friend's session");
        self.local.kind = SessionKind::Closed;
        self.joinable = false;
        self.joined = Some(lobby);
    }

    /// Requests to answer, and invites from friends connected.
    fn pending(&self) -> usize {
        let requests = self.requests.values().filter(|r| r.asked).count();
        let invites = self
            .friends
            .values()
            .filter(|f| f.transport.is_some() && f.presence.is_some())
            .filter(|f| f.invited.as_ref().is_some_and(|i| i.at.elapsed() < INVITED))
            .count();
        requests + invites
    }

    /// The tags this player looks for: friends not connected, codes asked.
    fn wanted_tags(&mut self, unix: u64) -> Vec<Tag> {
        if !self.active() {
            return Vec::new();
        }
        self.update_meetings(unix);
        let mut tags: Vec<Tag> =
            self.meetings.of.iter().filter(|(k, _)| !self.friends[*k].connected()).map(|(_, (_, tag))| *tag).collect();
        tags.extend(
            self.asked.iter().filter(|a| a.transport.is_none() && !a.declined).filter_map(Ask::code).map(crypto::code_tag),
        );
        tags
    }

    fn probe(&self, tags: &[Tag]) -> Vec<u8> {
        let mut all = vec![self.probe_id];
        all.extend_from_slice(tags);
        session::probe(&all)
    }

    /// An address the DHT gave for `key`.
    pub(crate) fn found(&mut self, key: dht::Id, addr: SocketAddrV4, now: Instant, out: &mut Out) {
        if self.own.contains(&addr) || !self.active() {
            return;
        }
        self.update_meetings(unix());
        if let Some(&k) = self.meetings.keys.get(&key) {
            self.contact(k, addr, false, now, out);
            return;
        }
        // Someone asking with this player's code: probed with its tag, it
        // asks where the probes come from.
        if let Some(code) = self.code.as_deref().filter(|c| crypto::code_key(c) == key) {
            if !self.callers.contains_key(&addr) {
                out.send.push((addr, self.probe(&[crypto::code_tag(code)])));
                self.callers.insert(addr, (now, now));
            }
            return;
        }
        if let Some(i) = self.asked.iter().position(|a| a.code().is_some_and(|c| crypto::code_key(c) == key)) {
            self.ask_at(i, addr, now, out);
        }
    }

    /// Reaches a friend at `addr`: the side with the lower key starts the
    /// handshake, the other probes there so that it does. `again`: even if
    /// connected (the friend probing says it is not: it started over).
    fn contact(&mut self, key: Key, addr: SocketAddrV4, again: bool, now: Instant, out: &mut Out) {
        let Some(identity) = self.identity.as_ref() else { return };
        let Some(f) = self.friends.get_mut(&key) else { return };
        let starts = identity.public() < &key;
        let due = f.transport.as_ref().is_none_or(|t| now - t.established > REKEY);
        if !due && !again || f.contacted.get(&addr).is_some_and(|t| now - *t < CONTACT) {
            return;
        }
        f.contacted.insert(addr, now);
        f.contacted.retain(|_, t| now - *t < CONTACT * 4);
        if !starts {
            let tag = crypto::meeting_tag(&f.shared, crypto::epoch(unix()));
            let d = self.probe(&[tag]);
            out.send.push((addr, d));
            return;
        }
        // One handshake at a time, its message 1 to every address found for
        // the friend meanwhile (the DHT keeps old ones for a while, which may
        // come first): the friend answers the first copy that reaches it and
        // not the others (their time is not newer).
        if let Some((h, _)) = self.handshakes.values_mut().find(|(_, o)| *o == Owner::Friend(key)) {
            out.push(addr, h.first_to(addr));
            return;
        }
        let time = unix_ms().to_be_bytes();
        if let Some((h, d)) = Handshake::friend(identity, &key, &time, addr, now) {
            self.handshakes.insert(h.index, (h, Owner::Friend(key)));
            out.send.push((addr, d));
        }
    }

    /// Asks a code's owner, or a player met, found at `addr`.
    fn ask_at(&mut self, i: usize, addr: SocketAddrV4, now: Instant, out: &mut Out) {
        let Some(identity) = self.identity.as_ref() else { return };
        let a = &mut self.asked[i];
        if a.transport.is_some()
            || a.declined
            || a.refused.contains(&addr)
            || a.contacted.get(&addr).is_some_and(|t| now - *t < CONTACT)
        {
            return;
        }
        a.contacted.insert(addr, now);
        a.contacted.retain(|_, t| now - *t < CONTACT * 4);
        // One handshake at a time, to every address found (see `contact`; a
        // code's owner reached at two answers twice, the second is not read).
        if let Some((h, _)) = self.handshakes.values_mut().find(|(_, o)| matches!(o, Owner::Asking(id) if *id == a.id)) {
            out.push(addr, h.first_to(addr));
            return;
        }
        let handshake = match &a.player {
            // Its key is known: the friends' handshake, which it answers
            // for a player of its lobbies.
            Some((key, _)) => Handshake::friend(identity, key, &unix_ms().to_be_bytes(), addr, now),
            None => Handshake::code(identity, &a.id, addr, now),
        };
        if let Some((h, d)) = handshake {
            self.handshakes.insert(h.index, (h, Owner::Asking(a.id.clone())));
            out.send.push((addr, d));
        }
    }

    // ---- datagrams ----

    pub(crate) fn receive(&mut self, from: SocketAddrV4, kind: u8, d: &[u8], now: Instant, out: &mut Out) {
        if self.identity.is_none() {
            return;
        }
        match kind {
            session::PROBE => self.on_probe(from, d, now, out),
            session::HS1 if d.len() > 5 => {
                let Some(pattern) = Pattern::from_byte(d[0]) else { return };
                // Its MAC first: made only by who knows whom it asks.
                let key = match (pattern, &self.code) {
                    (Pattern::Friend, _) => crypto::friend_mac_key(self.identity.as_ref().unwrap().public()),
                    (Pattern::Code, Some(code)) => crypto::code_mac_key(code),
                    (Pattern::Code, None) => return,
                };
                let Some(d) = session::checked(d, &key).filter(|d| d.len() > 5) else { return };
                let index = u32::from_be_bytes(d[1..5].try_into().unwrap());
                if self.allowed(from, now) {
                    self.on_first(from, pattern, index, &d[5..], now, out);
                }
            }
            session::HS2 | session::HS3 if d.len() > 9 => {
                let sender = u32::from_be_bytes(d[1..5].try_into().unwrap());
                let receiver = u32::from_be_bytes(d[5..9].try_into().unwrap());
                self.on_next(from, sender, receiver, &d[9..], now, out);
            }
            session::DATA if d.len() > 12 => {
                let index = u32::from_be_bytes(d[..4].try_into().unwrap());
                self.on_data(from, index, &d[4..], now, out);
            }
            _ => {}
        }
    }

    /// Whether a handshake from `from` is answered: at most so many per
    /// address and in all, the ones an address may not have not counted in
    /// all (one address does not hold up everyone's). The addresses kept
    /// are the ones answered (`RATE_ALL` per window), until `tick` lets
    /// them go.
    fn allowed(&mut self, from: SocketAddrV4, now: Instant) -> bool {
        if now - self.answered_all.0 > RATE_WINDOW {
            self.answered_all = (now, 0);
        }
        let ip = *from.ip();
        let n = self.answered.get(&ip).filter(|(t, _)| now - *t < RATE_WINDOW).map_or(0, |(_, n)| *n);
        if n >= RATE_PER_ADDRESS || self.answered_all.1 >= RATE_ALL {
            return false;
        }
        self.answered_all.1 += 1;
        let count = self.answered.entry(ip).or_insert((now, 0));
        if now - count.0 >= RATE_WINDOW {
            *count = (now, 0);
        }
        count.1 += 1;
        true
    }

    fn on_probe(&mut self, from: SocketAddrV4, d: &[u8], now: Instant, out: &mut Out) {
        // As many tags as a probe carries, at most.
        let all = d.as_chunks::<8>().0;
        let Some((id, tags)) = all[..all.len().min(session::MAX_TAGS)].split_first() else { return };
        // This player's own, back (a broadcast's): an address of its own,
        // only on this machine (anyone who got a probe knows the id: from a
        // friend's address, it would hide that address).
        if *id == self.probe_id {
            if self.own.len() < OWN && (from.ip().is_loopback() || crate::p2p::lan_address() == Some(*from.ip())) {
                self.own.insert(from);
            }
            return;
        }
        // A friend (this epoch's tag or the last one's).
        self.update_meetings(unix());
        let mut friends: Vec<Key> = tags.iter().filter_map(|t| self.meetings.tags.get(t).copied()).collect();
        friends.sort_unstable();
        friends.dedup();
        for key in friends {
            self.contact(key, from, true, now, out);
        }
        // Someone with this player's code: told where it is, it starts.
        if let Some(code) = &self.code {
            let tag = crypto::code_tag(code);
            if tags.contains(&tag) && self.allowed(from, now) {
                let d = self.probe(&[tag]);
                out.send.push((from, d));
            }
        }
        // The owner of a code asked.
        if let Some(i) = self.asked.iter().position(|a| a.code().is_some_and(|c| tags.contains(&crypto::code_tag(c)))) {
            self.ask_at(i, from, now, out);
        }
    }

    /// Message 1 of a handshake.
    fn on_first(&mut self, from: SocketAddrV4, pattern: Pattern, index: u32, msg: &[u8], now: Instant, out: &mut Out) {
        let identity = self.identity.as_ref().unwrap();
        match pattern {
            Pattern::Friend => {
                let Some((mut h, payload)) = Handshake::answer(identity, pattern, None, index, msg, from, now) else { return };
                let Some(key) = h.remote_key() else { return };
                let me = *identity.public();
                let Ok(time) = <[u8; 8]>::try_from(payload.as_slice()).map(u64::from_be_bytes) else { return };
                let Some(f) = self.friends.get_mut(&key) else {
                    self.stranger_asks(key, h, time, from, now, out);
                    return;
                };
                if time <= f.last_time {
                    return;
                }
                // Both started at once, the lower key's handshake goes on:
                // friends' handshakes are the lower key's to start, the
                // higher starts one only asking a player met, which has it
                // as a friend while it no longer has that player.
                if me < key && self.handshakes.values().any(|(_, o)| *o == Owner::Friend(key)) {
                    return;
                }
                f.last_time = time;
                let Some(d) = h.write(&[]) else { return };
                out.send.push((from, d));
                // Taken when the friend's first datagram on it comes (it
                // sends its presence at once): see `Friend::pending`.
                if let Some(t) = h.into_transport(now) {
                    self.channels.insert(t.index, Owner::Friend(key));
                    let older = f.pending.replace(t);
                    self.drop_channel(older);
                }
            }
            Pattern::Code => {
                let Some(code) = &self.code else { return };
                let psk = crypto::code_psk(code);
                if self.handshakes.values().filter(|(_, o)| matches!(o, Owner::Asker(_))).count() >= MAX_REQUESTS {
                    return;
                }
                let Some((mut h, _)) = Handshake::answer(identity, pattern, Some(&psk), index, msg, from, now) else { return };
                // The salt: the asker makes sure the code is this key's.
                let Some(d) = h.write(&self.salt) else { return };
                out.send.push((from, d));
                self.handshakes.insert(h.index, (h, Owner::Asker(None)));
            }
        }
    }

    /// The friends' handshake from someone who is no friend: a player met in
    /// one of this player's lobbies (its card has the key, its greetings
    /// come from that address) asking to be friends; anyone else is not
    /// answered.
    fn stranger_asks(&mut self, key: Key, mut h: Handshake, time: u64, from: SocketAddrV4, now: Instant, out: &mut Out) {
        let met = self.met.iter().any(|(k, a, ..)| *k == key && *a == from);
        if !met || self.blocked.contains(&key) || self.stranger_times.get(&key).is_some_and(|t| time <= *t) {
            return;
        }
        if !self.requests.contains_key(&key) && self.requests.len() >= MAX_REQUESTS {
            return;
        }
        self.stranger_times.insert(key, time);
        let Some(d) = h.write(&[]) else { return };
        out.send.push((from, d));
        let Some(t) = h.into_transport(now) else { return };
        self.channels.insert(t.index, Owner::Asker(Some(key)));
        let request = Request { name: String::new(), id: 0, transport: t, asked: false };
        if let Some(old) = self.requests.insert(key, request) {
            self.channels.remove(&old.transport.index);
        }
    }

    /// Message 2 or 3.
    fn on_next(&mut self, from: SocketAddrV4, sender: u32, receiver: u32, msg: &[u8], now: Instant, out: &mut Out) {
        // Read first: one that does not open (forged, by someone who saw
        // message 1) leaves the handshake as it was, for the real one.
        let Some((h, _)) = self.handshakes.get_mut(&receiver) else { return };
        if h.remote_index != 0 && h.remote_index != sender {
            return;
        }
        let Some(payload) = h.read(msg) else { return };
        let (mut h, owner) = self.handshakes.remove(&receiver).unwrap();
        h.remote_index = sender;
        h.to = from;
        let Some(key) = h.remote_key() else { return };
        // A code's owner sends the salt its code is made with: the code
        // comes from the key that answered, or someone else who knows it
        // answered first.
        if let Owner::Asking(id) = &owner
            && h.pattern == Pattern::Code
            && crypto::code_of(&key, &payload).as_deref() != Some(id.as_str())
        {
            log::warn!("friends: {from} answered for a code not its own");
            self.not_the_owner(id, from, std::mem::take(&mut h.tried), now, out);
            return;
        }
        if !h.finished() {
            // XX, message 2 read: message 3 finishes it.
            let Some(d) = h.write(&[]) else { return };
            out.send.push((from, d));
        }
        let Some(t) = h.into_transport(now) else { return };
        match owner {
            Owner::Friend(k) if k == key => self.open_friend(key, t, now, out),
            Owner::Asking(id) => {
                // The one asked: a friend already, or asked now.
                if self.friends.contains_key(&key) {
                    self.asked.retain(|a| a.id != id);
                    self.open_friend(key, t, now, out);
                    self.save();
                    return;
                }
                let Some(a) = self.asked.iter_mut().find(|a| a.id == id && a.transport.is_none()) else { return };
                if a.player.as_ref().is_some_and(|(k, _)| *k != key) {
                    return;
                }
                log::info!("friends: reached the one asked");
                self.channels.insert(t.index, Owner::Asking(id));
                a.asked = None;
                a.transport = Some(t);
                self.generation += 1;
            }
            Owner::Asker(_) => {
                if self.blocked.contains(&key) {
                    // Declined before: told again (that answer is sent once,
                    // and lost it would have them ask for ever).
                    let mut t = t;
                    out.push(t.addr, seal(&mut t, &Message::Decline));
                    return;
                }
                if self.friends.contains_key(&key) {
                    self.open_friend(key, t, now, out);
                    return;
                }
                if !self.requests.contains_key(&key) && self.requests.len() >= MAX_REQUESTS {
                    return;
                }
                self.channels.insert(t.index, Owner::Asker(Some(key)));
                let request = Request { name: String::new(), id: 0, transport: t, asked: false };
                if let Some(old) = self.requests.insert(key, request) {
                    self.channels.remove(&old.transport.index);
                }
            }
            Owner::Friend(_) => {}
        }
    }

    /// `from` answered for code `id` without being its owner: it is not
    /// asked again, and the other addresses its message 1 went to are asked
    /// at once (their answers came to a handshake no longer there).
    fn not_the_owner(&mut self, id: &str, from: SocketAddrV4, tried: Vec<SocketAddrV4>, now: Instant, out: &mut Out) {
        let Some(i) = self.asked.iter().position(|a| a.id == id) else { return };
        let a = &mut self.asked[i];
        a.refuse(from);
        for addr in &tried {
            a.contacted.remove(addr);
        }
        for addr in tried {
            self.ask_at(i, addr, now, out);
        }
    }

    /// A friend's new channel.
    fn open_friend(&mut self, key: Key, t: Transport, now: Instant, out: &mut Out) {
        let lobbies = crate::p2p::get().map(|p| p.lobbies()).unwrap_or_default();
        let presence = self.presence_for_friends(lobbies);
        let Some(f) = self.friends.get_mut(&key) else { return };
        if f.transport.is_none() {
            log::info!("friends: {} is online", f.name);
            // Not the friends there at the start, all at once (the summary),
            // nor one just made (its request's notices tell it).
            if self.summarized && unix().saturating_sub(f.since) > JUST_MADE {
                self.notices.push_back(Notice::Online(f.name.clone()));
            }
        }
        self.channels.insert(t.index, Owner::Friend(key));
        let previous = f.previous.take();
        f.previous = f.transport.replace(t);
        let t = f.transport.as_mut().unwrap();
        f.presence_sent = Some((now, presence.clone()));
        out.push(t.addr, seal(t, &Message::Presence(presence)));
        // Messages still waiting go on the new channel.
        for (_, d, sent, _) in &mut f.outbox {
            *sent = now;
            out.push(t.addr, t.seal(d, now));
        }
        self.drop_channel(previous);
        self.generation += 1;
    }

    fn on_data(&mut self, from: SocketAddrV4, index: u32, d: &[u8], now: Instant, out: &mut Out) {
        let Some(owner) = self.channels.get(&index).cloned() else { return };
        let mut confirmed = None;
        let payload = match &owner {
            Owner::Friend(key) => {
                let Some(f) = self.friends.get_mut(key) else { return };
                if f.pending.as_ref().is_some_and(|t| t.index == index) {
                    let opened = f.pending.as_mut().and_then(|t| t.open(d, from, now));
                    if opened.is_some() {
                        confirmed = f.pending.take();
                    }
                    opened
                } else {
                    let t = [f.transport.as_mut(), f.previous.as_mut()].into_iter().flatten().find(|t| t.index == index);
                    t.and_then(|t| t.open(d, from, now))
                }
            }
            Owner::Asker(Some(key)) => self.requests.get_mut(key).and_then(|r| r.transport.open(d, from, now)),
            Owner::Asking(id) => {
                self.asked.iter_mut().find(|a| a.id == *id).and_then(|a| a.transport.as_mut()).and_then(|t| t.open(d, from, now))
            }
            Owner::Asker(None) => None,
        };
        if let (Some(t), Owner::Friend(key)) = (confirmed, &owner) {
            self.open_friend(*key, t, now, out);
        }
        let Some(message) = payload.and_then(|p| serde_json::from_slice::<Message>(&p).ok()) else { return };
        match owner {
            Owner::Friend(key) => self.on_friend(key, message, now, out),
            Owner::Asker(Some(key)) => self.on_asker(key, message),
            Owner::Asking(id) => self.on_asked(&id, message, now, out),
            Owner::Asker(None) => {}
        }
    }

    fn on_friend(&mut self, key: Key, message: Message, now: Instant, out: &mut Out) {
        let online = self.local.playing == Playing::Online;
        let Some(f) = self.friends.get_mut(&key) else { return };
        match message {
            Message::Presence(p) => {
                let p = p.bounded();
                let taken = self.id_taken(p.id, &key);
                let Some(f) = self.friends.get_mut(&key) else { return };
                if f.presence.as_ref() != Some(&p) {
                    // Out of GTA Online: its invite is to a session it left.
                    if p.playing != Playing::Online && f.invited.take().is_some() {
                        self.generation += 1;
                    }
                    if !p.name.is_empty() && p.name != f.name {
                        f.name = p.name.clone();
                    }
                    let id_changed = p.id != f.id && !taken;
                    if id_changed {
                        f.id = p.id;
                    } else if taken {
                        log::warn!("friends: {} says it has another's gamer id", f.name);
                    }
                    f.presence = Some(p);
                    self.generation += 1;
                    if id_changed {
                        self.save();
                    }
                }
            }
            Message::Game { seq, text } => {
                // For GTA Online's session: none kept while the player is
                // elsewhere (they would all come at once, stale, later).
                if !f.first_time(seq, out) || text.len() > game::MAX_EVENT || !online {
                    return;
                }
                log::info!("friends: a game event from {}", f.name);
                self.events.push_back(GameEvent { from: f.id, name: f.name.clone(), text });
                // The newest kept.
                if self.events.len() > 32 {
                    self.events.pop_front();
                }
            }
            Message::Invite { seq, lobby, session } => {
                if !f.first_time(seq, out) || f.invited.as_ref().is_some_and(|i| now - i.at < INVITE_AGAIN) {
                    return;
                }
                log::info!("friends: {} invites this player", f.name);
                let bounded = |t: Option<String>, max| t.filter(|t: &String| t.len() <= max);
                f.invited =
                    Some(Invited { at: now, lobby: bounded(lobby, MAX_NAME), session: bounded(session, MAX_TEXT) });
                self.notices.push_back(Notice::Invite(f.name.clone()));
                self.generation += 1;
            }
            Message::Ack { seq } => f.outbox.retain(|(s, ..)| *s != seq),
            Message::Removed => {
                log::info!("friends: {} is no friend any more", f.name);
                self.remove_friend(&key, out);
            }
            // A friend asking again (with the code): yes.
            Message::Request { .. } => {
                let accept = Message::Accept { name: self.local.name.clone(), id: self.local.id };
                if let Some(t) = self.friends.get_mut(&key).and_then(|f| f.transport.as_mut()) {
                    out.push(t.addr, seal(t, &accept));
                }
            }
            Message::Accept { .. } | Message::Decline | Message::Ping => {}
        }
    }

    fn on_asker(&mut self, key: Key, message: Message) {
        let Some(r) = self.requests.get_mut(&key) else { return };
        if let Message::Request { name, id } = message {
            let name = clean_name(&name);
            if !r.asked {
                self.notices.push_back(Notice::Request(name.clone()));
            }
            if !r.asked || r.name != name {
                log::info!("friends: {name} asks to be friends");
                r.name = name;
                r.id = id;
                r.asked = true;
                self.generation += 1;
            }
        }
    }

    fn on_asked(&mut self, id: &str, message: Message, now: Instant, out: &mut Out) {
        let Some(i) = self.asked.iter().position(|a| a.id == id) else { return };
        match message {
            Message::Accept { name, id } => {
                let mut a = self.asked.remove(i);
                let Some(t) = a.transport.take() else { return };
                let key = t.remote_key;
                self.channels.remove(&t.index);
                if self.add_friend(key, &name, id, unix()) {
                    log::info!("friends: {} said yes", clean_name(&name));
                    self.notices.push_back(Notice::Accepted(clean_name(&name)));
                    self.open_friend(key, t, now, out);
                }
                self.save();
            }
            Message::Decline => {
                let a = &mut self.asked[i];
                a.declined = true;
                let t = a.transport.take();
                self.drop_channel(t);
                self.generation += 1;
                self.save();
            }
            _ => {}
        }
    }

    // ---- the player's answers ----

    fn answer(&mut self, key: &Key, yes: bool, now: Instant, out: &mut Out) {
        let Some(mut r) = self.requests.remove(key) else { return };
        if yes {
            self.channels.remove(&r.transport.index);
            if self.add_friend(*key, &r.name, r.id, unix()) {
                let accept = Message::Accept { name: self.local.name.clone(), id: self.local.id };
                out.push(r.transport.addr, seal(&mut r.transport, &accept));
                self.open_friend(*key, r.transport, now, out);
            }
        } else {
            out.push(r.transport.addr, seal(&mut r.transport, &Message::Decline));
            self.channels.remove(&r.transport.index);
            self.blocked.insert(*key);
        }
        self.generation += 1;
        self.save();
    }

    /// Sends the game's event to the friend with gamer id `id`; false if no
    /// such friend is connected, or if the event is too long.
    fn send_game(&mut self, id: u64, text: &str, now: Instant, out: &mut Out) -> bool {
        if text.len() > game::MAX_EVENT {
            return false;
        }
        let Some(f) = self.friend_of(id) else { return false };
        f.send_reliably(|seq| Message::Game { seq, text: text.to_owned() }, now, out)
    }

    /// Invites the friend with gamer id `id` to this player's session.
    fn send_invite(&mut self, id: u64, lobby: Option<String>, now: Instant, out: &mut Out) -> bool {
        let Some(key) = self.friend_of(id).map(|f| f.key) else { return false };
        self.invite(&key, lobby, now, out)
    }

    /// Invites friend `key` to this player's session: its lobby and session
    /// go with it when the presence does not tell them.
    fn invite(&mut self, key: &Key, lobby: Option<String>, now: Instant, out: &mut Out) -> bool {
        let session = if self.joinable { None } else { self.local.session.clone() };
        let lobby = if self.joinable { None } else { lobby };
        let Some(f) = self.friends.get_mut(key) else { return false };
        log::info!("friends: inviting {}", f.name);
        f.send_reliably(|seq| Message::Invite { seq, lobby, session }, now, out)
    }
}

fn seal(t: &mut Transport, m: &Message) -> Option<Vec<u8>> {
    let d = serde_json::to_vec(m).ok()?;
    t.seal(&d, Instant::now())
}

/// Moves a file that cannot be read aside (`<name>.bad-<time>`): whether
/// it went (its place may be written).
fn put_aside(path: &Path, why: &str) -> bool {
    let aside = path.with_file_name(format!("{FILE}.bad-{}", unix()));
    match std::fs::rename(path, &aside) {
        Ok(()) => {
            log::warn!("friends: {} ({why}): kept as {}", path.display(), aside.display());
            true
        }
        Err(e) => {
            log::warn!("friends: {} ({why}), not kept aside ({e}): not written this time", path.display());
            false
        }
    }
}

/// Writes a file only the player can read (where the system has owners).
fn write_private(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
    let mut file = options.open(path)?;
    file.write_all(bytes)?;
    file.sync_all()
}

// ---- running on the player's socket ------------------------------------------

static SERVICE: Mutex<Option<Service>> = Mutex::new(None);
/// The friends' lookups on the DHT, as last set.
static LOOKUPS: Mutex<Vec<dht::Id>> = Mutex::new(Vec::new());

fn send(p2p: &crate::p2p::P2p, out: Out) {
    for (to, d) in out.send {
        p2p.send_to(to, &d);
    }
}

/// Runs `f` on the service, then sends what it wants sent.
fn with<R>(f: impl FnOnce(&mut Service, &mut Out) -> R) -> Option<R> {
    let mut out = Out::default();
    let r = {
        let mut service = SERVICE.locked();
        f(service.as_mut()?, &mut out)
    };
    if !out.send.is_empty()
        && let Some(p2p) = crate::p2p::get()
    {
        send(p2p, out);
    }
    Some(r)
}

/// `with`, for what the player changes: the file is written at once (on
/// the caller's thread, never the game's).
fn with_saved<R>(f: impl FnOnce(&mut Service, &mut Out) -> R) -> Option<R> {
    let (r, saving) = with(|s, out| {
        let r = f(s, out);
        (r, s.saving(Instant::now(), true))
    })?;
    if let Some(saving) = saving {
        saving.write();
    }
    Some(r)
}

/// Every quarter of a second, on the network thread.
pub(crate) fn tick(p2p: &crate::p2p::P2p, now: Instant) {
    let lobbies = p2p.lobbies();
    let players = p2p.players();
    let mut out = Out::default();
    let (wanted, saving) = {
        let mut service = SERVICE.locked();
        let Some(s) = service.as_mut() else { return };
        s.tick(now, lobbies, &players, p2p.port(), &mut out);
        (s.lookups(unix()), s.saving(now, false))
    };
    send(p2p, out);
    if let Some(saving) = saving {
        saving.write();
    }
    let mut current = LOOKUPS.locked();
    let keys: Vec<dht::Id> = wanted.iter().map(|(k, _)| *k).collect();
    p2p.dht(|dht| {
        for old in current.iter().filter(|k| !keys.contains(k)) {
            dht.remove(old);
        }
        // Their options change as friends connect and leave.
        for (key, options) in &wanted {
            dht.add(*key, *options);
        }
    });
    *current = keys;
}

/// A datagram of the friends' kinds (after the magic and the kind).
pub(crate) fn receive(p2p: &crate::p2p::P2p, from: SocketAddrV4, kind: u8, d: &[u8]) {
    let mut out = Out::default();
    if let Some(s) = SERVICE.locked().as_mut() {
        s.receive(from, kind, d, Instant::now(), &mut out);
    }
    send(p2p, out);
}

/// An address the DHT gave for a key not a lobby's.
pub(crate) fn found(p2p: &crate::p2p::P2p, key: dht::Id, addr: SocketAddrV4) {
    let mut out = Out::default();
    if let Some(s) = SERVICE.locked().as_mut() {
        s.found(key, addr, Instant::now(), &mut out);
    }
    send(p2p, out);
}

// ---- for the application -------------------------------------------------------

/// Starts the friends of the player whose files are in `user_dir` (the
/// network once there is something to do on it). `name`: the player's name.
pub fn start(user_dir: &Path, name: &str) {
    let id = crate::online::gamer_id_in(user_dir);
    let mut service = SERVICE.locked();
    if service.is_some() {
        return;
    }
    let mut s = Service::load(user_dir, name, id);
    let active = s.active();
    let saving = s.saving(Instant::now(), true);
    *service = Some(s);
    drop(service);
    if let Some(saving) = saving {
        saving.write();
    }
    if active {
        start_network();
    }
}

fn start_network() {
    if let Err(e) = crate::p2p::start() {
        log::warn!("friends: no network: {e}");
    }
}

/// Writes what the network changed and is not saved yet (it waits up to a
/// few seconds otherwise): the application ends.
pub fn flush() {
    if let Some(Some(saving)) = with(|s, _| s.saving(Instant::now(), true)) {
        saving.write();
    }
}

/// The player's name as its friends see it.
pub fn set_name(name: &str) {
    with(|s, _| s.local.name = clean_name(name));
}

/// What the player is doing.
pub(crate) fn set_playing(playing: Playing) {
    with_saved(|s, _| {
        // Online, the player has its key, as it had a Social Club account:
        // the players it meets can ask it to be friends.
        if playing == Playing::Online {
            s.identity();
        }
        s.local.playing = playing;
        if playing != Playing::Online {
            s.local.session = None;
            s.events.clear();
        }
    });
}

/// A friend as the application shows it.
#[derive(Clone, Debug, PartialEq)]
pub struct FriendInfo {
    /// Its key (hex): what the application names it by.
    pub key: String,
    pub name: String,
    /// What it is doing, when connected.
    pub playing: Option<Playing>,
    /// Who may join its session (playing GTA Online).
    pub kind: Option<SessionKind>,
    /// The lobby to join it in (its session open to friends, or invited).
    pub lobby: Option<String>,
    /// It invited this player to its session (a few minutes ago at most).
    pub invited: bool,
}

/// A player met in a lobby, who is no friend.
#[derive(Clone, Debug, PartialEq)]
pub struct RecentInfo {
    pub key: String,
    pub name: String,
    /// In one of this player's lobbies now.
    pub here: bool,
    /// Asked to be friends already.
    pub asked: bool,
}

/// A request this player made.
#[derive(Clone, Debug, PartialEq)]
pub struct AskedInfo {
    /// What `cancel` takes.
    pub id: String,
    /// The code (with dashes), or the player's name.
    pub label: String,
    /// The owner was reached and asked.
    pub reached: bool,
    pub declined: bool,
}

/// Everything the application shows.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Snapshot {
    /// The player's friend code (shown with dashes), once made.
    pub code: Option<String>,
    pub friends: Vec<FriendInfo>,
    /// Requests to this player: key, name.
    pub requests: Vec<(String, String)>,
    pub asked: Vec<AskedInfo>,
    /// Players met in lobbies (not friends), the latest first.
    pub recent: Vec<RecentInfo>,
}

/// A number that changes with what `snapshot` shows.
pub fn generation() -> u64 {
    with(|s, _| s.generation).unwrap_or(0)
}

/// How many requests and invites wait for the player (the page's counts),
/// without `snapshot`'s work.
pub fn pending() -> usize {
    with(|s, _| s.pending()).unwrap_or(0)
}

/// Whether the player is in GTA Online (its friends may be invited to its
/// session), not in the story or the menus.
pub fn in_gta_online() -> bool {
    with(|s, _| s.local.playing == Playing::Online).unwrap_or(false)
}

pub fn snapshot() -> Snapshot {
    with(|s, _| {
        let mut friends: Vec<FriendInfo> = s
            .friends
            .values()
            .map(|f| {
                let presence = f.transport.as_ref().and(f.presence.as_ref());
                let online = presence.filter(|p| p.playing == Playing::Online);
                let invite = f.invited.as_ref().filter(|i| presence.is_some() && i.at.elapsed() < INVITED);
                FriendInfo {
                    key: crypto::hex(&f.key),
                    name: if f.name.is_empty() { "?".into() } else { f.name.clone() },
                    playing: presence.map(|p| p.playing),
                    kind: online.map(|p| p.kind),
                    lobby: online
                        .and_then(|p| p.lobbies.first().cloned())
                        .or_else(|| invite.and_then(|i| i.lobby.clone())),
                    invited: invite.is_some(),
                }
            })
            .collect();
        // Playing first, then by name.
        friends.sort_by(|a, b| (b.playing.is_some(), a.name.to_lowercase()).cmp(&(a.playing.is_some(), b.name.to_lowercase())));
        let mut requests: Vec<(String, String)> =
            s.requests.iter().filter(|(_, r)| r.asked).map(|(k, r)| (crypto::hex(k), r.name.clone())).collect();
        requests.sort_by(|a, b| a.1.cmp(&b.1));
        Snapshot {
            code: s.code.as_deref().map(crypto::format_code),
            friends,
            requests,
            asked: s
                .asked
                .iter()
                .map(|a| AskedInfo {
                    id: a.id.clone(),
                    label: a.player.as_ref().map_or_else(|| crypto::format_code(&a.id), |(_, name)| name.clone()),
                    reached: a.transport.is_some(),
                    declined: a.declined,
                })
                .collect(),
            recent: {
                s.recent
                    .iter()
                    .filter(|r| !s.friends.contains_key(&r.key))
                    .map(|r| RecentInfo {
                        key: crypto::hex(&r.key),
                        name: r.name.clone(),
                        here: s.met.iter().any(|(k, ..)| *k == r.key),
                        asked: s.asked.iter().any(|a| a.player.as_ref().is_some_and(|(k, _)| *k == r.key)),
                    })
                    .collect()
            },
        }
    })
    .unwrap_or_default()
}

/// The player's friend code, made the first time (the network starts).
pub fn my_code() -> Option<String> {
    let code = with_saved(|s, _| {
        s.identity();
        s.code.as_deref().map(crypto::format_code)
    })?;
    start_network();
    code
}

/// A new friend code: the old one no longer reaches the player.
pub fn new_code() {
    with_saved(|s, _| {
        s.identity();
        s.new_code();
    });
}

/// Why a code was not taken.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AddError {
    /// Not a code (mistyped).
    Invalid,
    /// The player's own.
    Own,
    /// Asked already.
    Asked,
}

/// Asks the owner of a friend code to be friends (it says yes or no).
pub fn ask(code: &str) -> Result<(), AddError> {
    let code = crypto::parse_code(code).ok_or(AddError::Invalid)?;
    with_saved(|s, _| {
        s.identity();
        if s.code.as_deref() == Some(code.as_str()) {
            return Err(AddError::Own);
        }
        if let Some(a) = s.asked.iter_mut().find(|a| a.id == code) {
            if !a.declined {
                return Err(AddError::Asked);
            }
            a.declined = false;
        } else {
            s.asked.push(Ask::new(code, None));
        }
        s.generation += 1;
        s.save();
        Ok(())
    })
    .unwrap_or(Err(AddError::Invalid))?;
    start_network();
    Ok(())
}

/// Asks a player met in a lobby (by its key) to be friends; it says yes
/// or no, while it plays in one of this player's lobbies.
pub fn ask_player(key: &str) {
    let Some(key) = crypto::unhex::<32>(key) else { return };
    with(|s, _| {
        let Some(name) = s.met_name(&key) else { return };
        if s.friends.contains_key(&key) || s.asked.iter().any(|a| a.player.as_ref().is_some_and(|(k, _)| *k == key)) {
            return;
        }
        s.blocked.remove(&key);
        s.asked.push(Ask::new(crypto::hex(&key), Some((key, name))));
        s.generation += 1;
    });
}

/// Stops asking (a code, or a player's key as `AskedInfo::id` gives it),
/// or forgets a no.
pub fn cancel(id: &str) {
    let id = crypto::parse_code(id).unwrap_or_else(|| id.to_owned());
    with_saved(|s, _| {
        if let Some(i) = s.asked.iter().position(|a| a.id == id) {
            let a = s.asked.remove(i);
            s.drop_channel(a.transport);
            s.generation += 1;
            s.save();
        }
    });
}

/// Answers a request (by the asker's key).
pub fn answer(key: &str, yes: bool) {
    let Some(key) = crypto::unhex::<32>(key) else { return };
    with_saved(|s, out| s.answer(&key, yes, Instant::now(), out));
}

/// Joins a friend's session (by its key) from GTA Online: the game asks
/// whether to leave this one (game.rs).
pub fn join(key: &str) {
    let Some(key) = crypto::unhex::<32>(key) else { return };
    with(|s, _| {
        let Some(id) = s.friends.get(&key).map(|f| f.id).filter(|id| *id != 0) else { return };
        if !s.joins.iter().any(|(i, _)| *i == id) {
            if s.joins.len() >= 4 {
                s.joins.pop_front();
            }
            s.joins.push_back((id, Instant::now()));
        }
    });
}

/// Forgets a friend's invite.
pub fn ignore_invite(key: &str) {
    let Some(key) = crypto::unhex::<32>(key) else { return };
    with(|s, _| {
        if let Some(f) = s.friends.get_mut(&key) {
            f.invited = None;
            s.generation += 1;
        }
    });
}

/// What happened since the last call that the player should know.
pub fn take_notices() -> Vec<Notice> {
    with(|s, _| s.notices.drain(..).collect()).unwrap_or_default()
}

/// Joins a friend (by its key) playing GTA Online: in the game's session
/// it plays in, once this player's game is in GTA Online (game.rs).
pub fn follow(key: &str) {
    let Some(key) = crypto::unhex::<32>(key) else { return };
    with(|s, _| s.follow = Some(Follow { key, asked: Instant::now(), since: None }));
}

/// Invites a friend (by its key) to this player's session in GTA Online:
/// its game asks it whether to join.
pub fn invite(key: &str) {
    let Some(key) = crypto::unhex::<32>(key) else { return };
    let lobby = game_lobby();
    with(|s, out| s.invite(&key, lobby, Instant::now(), out));
}

/// The lobby this player's session is in: a friend's it joined, or its
/// own.
fn game_lobby() -> Option<String> {
    with(|s, _| s.joined.clone()).flatten().or_else(|| crate::p2p::get().and_then(|p| p.lobbies().into_iter().next()))
}

/// No longer friends (the friend is told when connected).
pub fn remove(key: &str) {
    let Some(key) = crypto::unhex::<32>(key) else { return };
    with_saved(|s, out| s.remove_friend(&key, out));
}

// ---- for the game (game.rs) -------------------------------------------------------

/// How long a friend is waited for to be joined, from when the game looks
/// for a session; how long the game may take to get there.
const FOLLOW: Duration = Duration::from_secs(180);
const FOLLOW_START: Duration = Duration::from_secs(15 * 60);

/// A friend as the game lists it.
#[derive(Clone, Debug, Hash, PartialEq, Eq)]
pub(crate) struct GameFriend {
    pub name: String,
    pub id: u64,
    /// Connected (playing Glue 5, whatever it plays).
    pub online: bool,
    /// In a session of GTA Online, which can be joined.
    pub in_session: bool,
}

/// The friends the game lists: connected first.
pub(crate) fn game_friends() -> Vec<GameFriend> {
    with(|s, _| {
        let mut list: Vec<GameFriend> = s
            .friends
            .values()
            .filter(|f| f.id != 0)
            .map(|f| {
                GameFriend {
                    name: f.name.clone(),
                    id: f.id,
                    online: f.transport.is_some() && f.presence.is_some(),
                    in_session: f.session().is_some(),
                }
            })
            .collect();
        list.sort_by(|a, b| (b.online, &a.name).cmp(&(a.online, &b.name)));
        list
    })
    .unwrap_or_default()
}

pub(crate) fn is_friend(id: u64) -> bool {
    with(|s, _| s.friend_of(id).is_some()).unwrap_or(false)
}

/// Sends the game's event to the friends among `ids`: how many it went to.
pub(crate) fn send_game_event(ids: &[u64], text: &str) -> usize {
    with(|s, out| ids.iter().filter(|id| s.send_game(**id, text, Instant::now(), out)).count()).unwrap_or(0)
}

/// The game's events from friends, received since the last call.
pub(crate) fn take_game_events() -> Vec<GameEvent> {
    with(|s, _| s.events.drain(..).collect()).unwrap_or_default()
}

/// The session this player is in (as the engine writes its info).
pub(crate) fn set_session(session: Option<String>) {
    with(|s, _| s.local.session = session);
}

/// The session a friend (gamer id) plays in, as it says or as its invite
/// does.
pub(crate) fn session_of(id: u64) -> Option<String> {
    with(|s, _| s.friend_of(id)?.session()).flatten()
}

/// Gets ready to join a friend (gamer id): its lobbies, its players
/// introduced by it. Joining it there (`joining`), the session it gets
/// into is no longer one this player's friends may join without an invite
/// (the friend's lobby is the friend's to share).
pub(crate) fn reach(id: u64, joining: bool) {
    let Some(p2p) = crate::p2p::get() else { return };
    let ours = p2p.lobbies().into_iter().next();
    let Some((addr, lobbies)) = with(|s, _| s.reach(id, joining, ours.as_deref(), Instant::now())).flatten() else {
        return;
    };
    for lobby in lobbies {
        p2p.visit_lobby(&lobby, addr);
    }
}

/// Asks a player met (by its gamer id, as the game's player list names it)
/// to be friends: false when it is not one met (another kind of player).
pub(crate) fn ask_gamer(id: u64) -> bool {
    let key = with(|s, _| s.met_gamer(id));
    match key.flatten() {
        Some(key) => {
            ask_player(&crypto::hex(&key));
            true
        }
        None => false,
    }
}

/// The friend (gamer id) to join, while it is waited for.
pub(crate) fn following() -> Option<u64> {
    with(|s, _| s.following(Instant::now())).flatten()
}

/// Follows friend `id` (gamer id) instead of joining its session now: the
/// player may not join one yet (game.rs).
pub(crate) fn follow_gamer(id: u64) {
    with(|s, _| {
        let key = s.friends.iter().find(|(_, f)| f.id == id && id != 0).map(|(key, _)| *key);
        if let Some(key) = key {
            s.follow = Some(Follow { key, asked: Instant::now(), since: None });
        }
    });
}

/// The player may not join the friend followed yet: it is waited for from
/// when it may.
pub(crate) fn follow_later() {
    with(|s, _| {
        if let Some(f) = &mut s.follow {
            f.asked = Instant::now();
            f.since = None;
        }
    });
}

/// Invites the friends among `ids` (gamer ids) to this player's session:
/// how many it went to.
pub(crate) fn send_invites(ids: &[u64]) -> usize {
    let lobby = game_lobby();
    with(|s, out| ids.iter().filter(|id| s.send_invite(**id, lobby.clone(), Instant::now(), out)).count())
        .unwrap_or(0)
}

/// The friends (gamer ids) whose sessions the game may be joining.
pub(crate) fn joining() -> Vec<u64> {
    with(|s, _| s.joining.iter().map(|(id, ..)| *id).collect()).unwrap_or_default()
}

/// The game is in a new session: friend `friend`'s (gamer id), when it is
/// one of `joining`'s (game.rs compares the sessions).
pub(crate) fn session_changed(friend: Option<u64>) {
    with(|s, _| s.session_changed(friend, Instant::now()));
}

/// Whether friend `id` (gamer id) invited this player to its session a few
/// minutes ago at most.
pub(crate) fn invited_by(id: u64) -> bool {
    with(|s, _| s.friend_of(id).is_some_and(|f| f.invited.as_ref().is_some_and(|i| i.at.elapsed() < INVITED)))
        .unwrap_or(false)
}

/// The game looks for a friend's session to join.
pub(crate) fn find_friends() {
    with(|s, _| s.finding = true);
}

pub(crate) fn finding_friends() -> bool {
    with(|s, _| s.finding).unwrap_or(false)
}

/// The friends (gamer ids) whose sessions the player chose to join since
/// the last call (a minute ago at most: the game may be between sessions).
pub(crate) fn take_joins() -> Vec<u64> {
    let fresh = |at: &Instant| at.elapsed() < Duration::from_secs(60);
    with(|s, _| s.joins.drain(..).filter(|(_, at)| fresh(at)).map(|(id, _)| id).collect()).unwrap_or_default()
}

/// How this player's session is joined (the game's choice of session).
pub(crate) fn set_session_kind(kind: SessionKind) {
    with(|s, _| {
        s.looking_for_session(Instant::now());
        s.joined = None;
        s.joining.clear();
        s.finding = false;
        if s.local.kind != kind {
            log::info!("friends: in a {kind:?} session");
        }
        s.local.kind = kind;
        s.joinable = kind.joinable();
    });
}

pub(crate) fn stop_following() {
    with(|s, _| s.follow = None);
}

#[cfg(test)]
mod tests;
