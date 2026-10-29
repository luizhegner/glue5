//! Friends in GTA Online, through the game's own menus: the pause menu's
//! FRIENDS tab, invites, and joining a friend's session.
//!
//! The engine asks Social Club for all of it; here:
//! - the friends list is ours (`rlFriendsReader::Read`, the only reader of
//!   the platform's list), loaded again as it changes; the browser build's
//!   friends and players menus, made to fail, are mended (`MENU_PATCHES`,
//!   `card_data_init`), and what they wait for from Rockstar (crews, stats)
//!   comes at once, empty; with no friend yet, a friends list says so, not
//!   that the connection was lost (`player_list_empty`);
//! - "Send Friend Request" on a player asks it (a player met, p2p.rs), and
//!   "Join Friends" finds the friends' sessions in their presence, as
//!   Social Club's presence queries did;
//! - each player tells its friends its session (the engine publishes it to
//!   Social Club only, and not on a LAN); joining a friend's session (the
//!   app's "Join", a friend's invite) goes the way the pause menu's "Join"
//!   does (`InviteMgr::HandleJoinRequest`), the player first getting into
//!   the friend's lobby (p2p.rs), its players introduced by the friend;
//! - an invite to a session (the pause menu's, a script's, the app's),
//!   which Social Club's overlay showed, travels between friends, and the
//!   friend's game asks whether to join;
//! - the game's presence events (invites to jobs, their answers) go over
//!   the friends' channel, and the ones a friend sends come in where Social
//!   Club's would (`ProcessGameEvent`), once checked: only invites and their
//!   answers, really from that friend.
//!
//! Which lobby the player's session is in, for its kind, is online.rs's.

use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use glue5_game::{Env, exports, orig};

use crate::memory::Mem;

/// `rlFriendsManager` (this build): initialised (bit 0), its activation
/// (0 idle, 1 activating, 2 active, 3 failed).
const FRIENDS_MANAGER: u64 = 15215328;
const FRIENDS_STATE: u64 = 15215332;
/// `rlFriend`: name, id, relationship, flags (online, same title), in a
/// session.
const FRIEND_SIZE: u64 = 384;
const FRIEND_ID: u64 = 56;
const FRIEND_RELATIONSHIP: u64 = 64;
const RELATIONSHIP_FRIEND: u32 = 32;
const FRIEND_FLAGS: u64 = 68;
const FRIEND_IN_SESSION: u64 = 376;
const FRIEND_NAME: usize = 32;
/// `InviteFlags::IF_AllowPrivate`: an invite's, into a private session.
const IF_ALLOW_PRIVATE: u32 = 32;
/// `netStatus`: state (3: succeeded), result.
const SUCCEEDED: u32 = 3;
/// `CNetworkSession*`, and its game `snSession*`.
const NETWORK_SESSION: u64 = 29044456;
const GAME_SESSION: u64 = 32;
/// `rlSessionInfo`: the session's id at +8 (0: none); its text (base 64).
const SESSION_INFO: u64 = 480;
const SESSION_ID: u64 = 8;
const SESSION_TEXT: u64 = 128;
/// `CGamePresenceEventDispatcher*`, and the local gamer's `rlGamerInfo`.
const DISPATCHER: u64 = 29510448;
const LOCAL_GAMER: u64 = 15219624;
/// An `rlGamerHandle`: id, service (3: Social Club), local index.
const HANDLE_SIZE: u64 = 16;
/// The largest presence message, sent to friends or from them.
pub(super) const MAX_EVENT: usize = 1024;
/// The events a friend may send: an invite, its cancellation, the answer to
/// one, a follow invite.
const EVENTS: &[&str] = &["ginv", "ginvc", "grep", "finv"];
/// How often the session is told; how long a session is played before a
/// friend's is joined from it, and before it is tried again.
const PUBLISH: Duration = Duration::from_secs(2);
const SETTLED: Duration = Duration::from_secs(10);
const JOIN_AGAIN: Duration = Duration::from_secs(30);
/// How often the friends list is made again with nothing changed (an
/// invite runs out by itself).
const LIST_AGAIN: Duration = Duration::from_secs(10);

/// The browser build has no friends: its friends menus (the pause menu's
/// FRIENDS tab, online and in the story, the corona's invite list) ask a
/// virtual method whether there is a system error, and theirs is a stub
/// that answers yes (an identical-code-folded `return 1`), so they say "the
/// internet connection has been lost". Their vtables get the base player
/// list's (`return 0`): (slot's address, the stub's table index, the
/// replacement's).
const MENU_PATCHES: &[(u64, u64, u64)] = &[
    (5042512, 34242, 34181), // CFriendsMenuMP
    (5043128, 34263, 34181), // CFriendsMenuSP
    (5047456, 34242, 34181), // CCoronaFriendsInviteMenu
];

/// The patched slot's offset in a friends menu's vtable (`MENU_PATCHES`).
const SYSTEM_ERROR_SLOT: u64 = 264;

/// Whether this menu is one of the friends menus (by its vtable).
fn is_friends_menu(m: Mem, menu: u64) -> bool {
    let vtable = m.u64(menu);
    MENU_PATCHES.iter().any(|&(slot, _, _)| slot - SYSTEM_ERROR_SLOT == vtable)
}

fn patch_menus(m: Mem) {
    for &(at, stub, base) in MENU_PATCHES {
        match m.u64(at) {
            v if v == stub => {
                m.set_u64(at, base);
                log::info!("friends: friends menu at {at} patched");
            }
            v if v == base => {}
            v => log::warn!("friends: friends menu at {at} not patched: found {v}"),
        }
    }
}

/// The game thread's state.
struct State {
    menus_patched: bool,
    /// The list as last loaded, and when activation was last asked.
    list: Option<u64>,
    activated: Option<Instant>,
    /// The list as last made (its hash), when, and the friends' generation
    /// then (`super::generation`).
    current: u64,
    listed: Option<Instant>,
    generation: u64,
    published: Option<Instant>,
    /// When the friend followed was last looked at.
    followed: Option<Instant>,
    /// The game session's id, since when.
    session: Option<(u64, Instant)>,
    /// When the friend followed was last joined.
    joined: Option<Instant>,
}

static STATE: Mutex<State> = Mutex::new(State {
    menus_patched: false,
    list: None,
    activated: None,
    current: 0,
    listed: None,
    generation: 0,
    published: None,
    followed: None,
    session: None,
    joined: None,
});

/// Runs `f` with `size` zeroed bytes of the game's stack.
unsafe fn with_stack<R>(env: *mut Env, size: u64, f: impl FnOnce(u64) -> R) -> Option<R> {
    let top = unsafe { exports::emscripten_stack_get_current(env) }.ok()?;
    let at = unsafe { exports::_emscripten_stack_alloc(env, size.next_multiple_of(16)) }.ok()?;
    unsafe { Mem::of(env) }.bytes_mut(at, size as usize).fill(0);
    let r = f(at);
    let _ = unsafe { exports::_emscripten_stack_restore(env, top) };
    Some(r)
}

/// A NUL-terminated string of at most `max` bytes.
fn bounded_str(m: Mem, at: u64, max: usize) -> String {
    let bytes = m.bytes(at, max);
    let end = bytes.iter().position(|&b| b == 0).unwrap_or(max);
    String::from_utf8_lossy(&bytes[..end]).into_owned()
}

fn hash<T: std::hash::Hash>(t: &T) -> u64 {
    use std::hash::{BuildHasher, BuildHasherDefault, DefaultHasher};
    BuildHasherDefault::<DefaultHasher>::default().hash_one(t)
}

/// Called by the live manager's update, on the game thread, before it.
pub(crate) unsafe fn update(env: *mut Env) {
    let m = unsafe { Mem::of(env) };
    let mut state = STATE.lock().unwrap();
    let now = Instant::now();
    if !state.menus_patched {
        state.menus_patched = true;
        patch_menus(m);
    }
    // The friends list, loaded again as it changes (made again when the
    // friends change, and now and then).
    if m.u8(FRIENDS_MANAGER) & 1 != 0 {
        let generation = super::generation();
        if state.generation != generation || state.listed.is_none_or(|t| now - t > LIST_AGAIN) {
            state.generation = generation;
            state.listed = Some(now);
            state.current = hash(&super::game_friends());
        }
        let list = state.current;
        let since = |t: Option<Instant>, d: u64| t.is_none_or(|t| now - t > Duration::from_secs(d));
        match m.u32(FRIENDS_STATE) {
            0 | 3 if since(state.activated, 5) => {
                state.activated = Some(now);
                state.list = Some(list);
                unsafe { orig::friends_activate(env, 0) };
                log::debug!("friends: the game's list loaded ({})", m.u32(FRIENDS_STATE));
            }
            2 if state.list != Some(list) && since(state.activated, 2) => {
                state.activated = Some(now);
                state.list = Some(list);
                unsafe { orig::friends_deactivate(env) };
                unsafe { orig::friends_activate(env, 0) };
                log::debug!("friends: the game's list loaded again ({})", m.u32(FRIENDS_STATE));
            }
            _ => {}
        }
    }
    // The session this player is in, for its friends.
    if state.published.is_none_or(|t| now - t > PUBLISH) {
        state.published = Some(now);
        let session = unsafe { session_text(env) };
        let id = session.as_ref().map(|(id, _)| *id);
        if state.session.map(|(s, _)| s) != id {
            state.session = id.map(|id| (id, now));
            if let Some(id) = id {
                // Whose it is, among the friends' the game may be joining.
                let friend = super::joining()
                    .into_iter()
                    .find(|f| super::session_of(*f).is_some_and(|text| unsafe { session_id(env, &text) } == Some(id)));
                super::session_changed(friend);
            }
        }
        super::set_session(session.map(|(_, t)| t));
    }
    // A friend being followed: into its session (as often as the session
    // is told).
    if state.followed.is_none_or(|t| now - t > PUBLISH) {
        state.followed = Some(now);
        if let Some(id) = super::following() {
            unsafe { follow(env, &mut state, id, now) };
        }
    }
    // Friends' sessions the player chose to join: the game asks whether to
    // leave this one, or the story (kept while it is between sessions). In
    // GTA Online before the player may join one (in the creator, the
    // tutorial), the friend is followed instead.
    let online = super::in_gta_online();
    if let Some(ours) = state.session.map(|(id, _)| id).or_else(|| (!online).then_some(0)) {
        let joins = super::take_joins();
        if online && !joins.is_empty() && !unsafe { may_join(env) } {
            joins.into_iter().for_each(super::follow_gamer);
        } else {
            for id in joins {
                unsafe { join(env, ours, id, true, false) };
            }
        }
    }
    drop(state);
    // Friends' events.
    for event in super::take_game_events() {
        match checked(&event.text, event.from, &event.name) {
            Some(text) => unsafe { inject(env, &text, event.from) },
            None => log::warn!("friends: an event from {} refused", event.name),
        }
    }
}

/// The info of the player's game session: its id and text, when it is in
/// one.
unsafe fn session_text(env: *mut Env) -> Option<(u64, String)> {
    let m = unsafe { Mem::of(env) };
    let network = m.u64(NETWORK_SESSION);
    let session = (network != 0).then(|| m.u64(network + GAME_SESSION)).filter(|s| *s != 0)?;
    let info = unsafe { orig::session_info(env, session) };
    let id = (info != 0).then(|| m.u64(info + SESSION_ID)).filter(|id| *id != 0)?;
    let text = unsafe {
        with_stack(env, SESSION_TEXT + 8, |buf| {
            orig::session_info_text(env, info, buf, buf + SESSION_TEXT);
            bounded_str(m, buf, SESSION_TEXT as usize)
        })
    }?;
    (!text.is_empty()).then_some((id, text))
}

/// A session's id, from its info's text.
unsafe fn session_id(env: *mut Env, text: &str) -> Option<u64> {
    let m = unsafe { Mem::of(env) };
    unsafe {
        with_stack(env, SESSION_INFO + text.len() as u64 + 1, |info| {
            let text_at = info + SESSION_INFO;
            m.bytes_mut(text_at, text.len()).copy_from_slice(text.as_bytes());
            (orig::session_info_parse(env, info, text_at, 0) != 0).then(|| m.u64(info + SESSION_ID))
        })
    }
    .flatten()
}

/// Joins the session of the friend followed (gamer id `id`), once this
/// player's own has settled and it may join one; done when they share one.
unsafe fn follow(env: *mut Env, state: &mut State, id: u64, now: Instant) {
    let Some((ours, since)) = state.session else { return };
    if !unsafe { may_join(env) } {
        super::follow_later();
        return;
    }
    let may_ask = now - since >= SETTLED && state.joined.is_none_or(|t| now - t >= JOIN_AGAIN);
    match unsafe { join(env, ours, id, may_ask, true) } {
        Some(Joining::There) => {
            log::info!("friends: in the friend's session");
            super::stop_following();
        }
        Some(Joining::Asked) => state.joined = Some(now),
        None => {}
    }
}

/// Whether GTA Online's scripts block invites (`NETWORK_BLOCK_INVITES`: on
/// the way into freemode, in the tutorial).
static INVITES_BLOCKED: AtomicBool = AtomicBool::new(false);

/// Hook for `InviteMgr::BlockInvitesFromScript(bool)`.
unsafe extern "C" fn invites_blocked(env: *mut Env, block: u32) {
    INVITES_BLOCKED.store(block != 0, Ordering::Relaxed);
    unsafe { orig::invites_blocked(env, block) }
}

/// `MPPLY_LAST_MP_CHAR` (the player's character slot) and
/// `MP<slot>_CHAR_ISACTIVE` (whether the slot has a character): the stats
/// the scripts ask.
const LAST_CHARACTER: u32 = 0x2f2f_120f;
const CHARACTER_ACTIVE: [u32; 2] = [0x79dc_c136, 0x8819_ed33];

/// Whether GTA Online's scripts take the player into a friend's session
/// now: it has a character (the creator is behind it) and they do not
/// block invites. They drop the invite otherwise ("you need to create a
/// character", "still on the tutorial"). Asked in a game session only: the
/// engine's stats are not there from the start.
unsafe fn may_join(env: *mut Env) -> bool {
    if INVITES_BLOCKED.load(Ordering::Relaxed) {
        return false;
    }
    let m = unsafe { Mem::of(env) };
    unsafe {
        with_stack(env, 8, |out| {
            let slot = if crate::stats::get_int(env, LAST_CHARACTER, out, u32::MAX) != 0 { m.u32(out) } else { 0 };
            m.set_u32(out, 0);
            let active = CHARACTER_ACTIVE[(slot as usize).min(1)];
            crate::stats::get_bool(env, active, out, u32::MAX) != 0 && m.u32(out) != 0
        })
    }
    .unwrap_or(false)
}

enum Joining {
    /// In the friend's session already.
    There,
    /// The game was asked to join it.
    Asked,
}

/// Joins the session of friend `id` (gamer id) from this player's session
/// `ours`, if `ask`, as the pause menu's "Join" does, its lobby reached
/// first; `confirmed`: without the game asking whether to (the player
/// asked). Nothing when its session is not known.
unsafe fn join(env: *mut Env, ours: u64, id: u64, ask: bool, confirmed: bool) -> Option<Joining> {
    let text = super::session_of(id)?;
    let m = unsafe { Mem::of(env) };
    unsafe {
        with_stack(env, SESSION_INFO + HANDLE_SIZE + text.len() as u64 + 1, |at| {
            let (info, handle, text_at) = (at, at + SESSION_INFO, at + SESSION_INFO + HANDLE_SIZE);
            m.bytes_mut(text_at, text.len()).copy_from_slice(text.as_bytes());
            if orig::session_info_parse(env, info, text_at, 0) == 0 {
                return None;
            }
            if m.u64(info + SESSION_ID) == ours {
                return Some(Joining::There);
            }
            if !ask {
                return None;
            }
            log::info!("friends: joining a friend's session{}", if confirmed { "" } else { " (if the player says so)" });
            super::reach(id, true);
            m.set_u64(handle, id);
            m.set_u8(handle + 8, 3);
            // Invited, into a private session too, as an accepted invite is.
            let flags = if super::invited_by(id) { IF_ALLOW_PRIVATE } else { 0 };
            orig::join_request(env, info, LOCAL_GAMER, handle, flags);
            if confirmed {
                // Asked for by the player: no "join?" alert.
                orig::invite_auto_confirm(env);
            }
            Some(Joining::Asked)
        })
    }
    .flatten()
}

/// An event from friend `from` (gamer id) as the game gets it: only the
/// events friends send, an invite only from its sender, under the name the
/// player knows it by. Nothing when it is not one.
fn checked(text: &str, from: u64, name: &str) -> Option<String> {
    if text.len() > MAX_EVENT {
        return None;
    }
    let mut v: serde_json::Value = serde_json::from_str(text).ok()?;
    let event = v.get_mut("gm.evt")?;
    let kind = event.get("e")?.as_str()?.to_owned();
    if !EVENTS.contains(&kind.as_str()) {
        return None;
    }
    let data = event.get_mut("d")?.as_object_mut()?;
    match data.get("h") {
        Some(h) if handle_id(h.as_str()?)? != from => return None,
        None if kind == "ginv" || kind == "finv" => return None,
        _ => {}
    }
    if data.contains_key("n") {
        data.insert("n".into(), name.into());
    }
    let text = serde_json::to_string(&v).ok()?;
    (text.len() < MAX_EVENT).then_some(text)
}

/// The gamer id in a handle as events write it: base 64 of its 16 bytes.
fn handle_id(b64: &str) -> Option<u64> {
    let bytes = base64(b64)?;
    Some(u64::from_le_bytes(bytes.get(..8)?.try_into().unwrap()))
}

fn base64(text: &str) -> Option<Vec<u8>> {
    let value = |c: u8| match c {
        b'A'..=b'Z' => Some(c - b'A'),
        b'a'..=b'z' => Some(c - b'a' + 26),
        b'0'..=b'9' => Some(c - b'0' + 52),
        b'+' | b'-' => Some(62),
        b'/' | b'_' => Some(63),
        _ => None,
    };
    let mut out = Vec::with_capacity(text.len() * 3 / 4);
    let (mut acc, mut bits) = (0u32, 0);
    for c in text.bytes().filter(|c| *c != b'=') {
        acc = acc << 6 | u32::from(value(c)?);
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
        }
    }
    Some(out)
}

/// Hands an event from friend `from` to the game, as Social Club would.
unsafe fn inject(env: *mut Env, text: &str, from: u64) {
    let m = unsafe { Mem::of(env) };
    let dispatcher = m.u64(DISPATCHER);
    if dispatcher == 0 {
        return;
    }
    let len = text.len() as u64;
    let text_size = (len + 1).next_multiple_of(16);
    unsafe {
        with_stack(env, text_size + 32 + 32 + 16, |at| {
            let (reader, sender, channel) = (at + text_size, at + text_size + 32, at + text_size + 64);
            m.bytes_mut(at, text.len()).copy_from_slice(text.as_bytes());
            m.bytes_mut(channel, 5).copy_from_slice(b"self\0");
            // The sender: the friend's handle, of type 0 (a local message: the
            // engine checks nothing against it, `checked` did).
            m.set_u64(sender, from);
            m.set_u8(sender + 8, 3);
            orig::rson_reader(env, reader, at, len as u32);
            orig::presence_process(env, dispatcher, LOCAL_GAMER, sender, reader, channel);
        })
    };
}

// ---- hooks ----

/// Hook for `rage::rlFriendsReader::Read(int, unsigned int, rlFriend*,
/// unsigned int, unsigned int*, unsigned int*, int, netStatus*)`: the
/// friends, at once.
#[allow(clippy::too_many_arguments)]
pub(crate) unsafe extern "C" fn friends_read(
    env: *mut Env,
    _reader: u64,
    gamer: u32,
    start: u32,
    friends: u64,
    max: u32,
    num: u64,
    total: u64,
    _flags: u32,
    status: u64,
) -> u32 {
    if gamer != 0 || total == 0 || friends == 0 {
        return 0;
    }
    let m = unsafe { Mem::of(env) };
    let list = super::game_friends();
    let page: Vec<_> = list.iter().skip(start as usize).take(max as usize).collect();
    for (i, f) in page.iter().enumerate() {
        let e = friends + i as u64 * FRIEND_SIZE;
        m.bytes_mut(e, FRIEND_SIZE as usize).fill(0);
        m.write_str(e, &f.name, FRIEND_NAME);
        m.set_u64(e + FRIEND_ID, f.id);
        m.set_u32(e + FRIEND_RELATIONSHIP, RELATIONSHIP_FRIEND);
        // Online and in this game, whatever it plays.
        m.set_u8(e + FRIEND_FLAGS, if f.online { 3 } else { 0 });
        m.set_u32(e + FRIEND_IN_SESSION, f.in_session as u32);
    }
    if num != 0 {
        m.set_u32(num, page.len() as u32);
    }
    m.set_u32(total, list.len() as u32);
    log::debug!("friends: the game reads {} friends from {start}", page.len());
    if status != 0 {
        m.set_u32(status, SUCCEEDED);
        m.set_u32(status + 4, 0);
    }
    1
}

thread_local! {
    /// Putting the pause menu's tabs together, or in its lists of players
    /// and friends.
    static IN_PLAYER_MENUS: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// Whether the game is putting the pause menu's tabs together, or in its
/// lists of players and friends.
pub(crate) fn in_player_menus() -> bool {
    IN_PLAYER_MENUS.get()
}

fn with_player_menus<R>(f: impl FnOnce() -> R) -> R {
    // The pause menu's update runs its lists' within it.
    let outer = IN_PLAYER_MENUS.replace(true);
    let r = f();
    IN_PLAYER_MENUS.set(outer);
    r
}

/// Hook for `CPauseMenu::SetCurrentMenuVersion(unsigned int, bool)`: the
/// pause menu's tabs, put together as it opens (see `social_club_connected`).
pub(crate) unsafe extern "C" fn pause_menu_version(env: *mut Env, version: u32, a: u32) -> u32 {
    with_player_menus(|| unsafe { orig::pause_menu_version(env, version, a) })
}

/// Hook for `CPauseMenu::Update()`: it tells the menu's movie whether the
/// player is signed in to Social Club, which shows the friends only then.
pub(crate) unsafe extern "C" fn pause_menu_update(env: *mut Env) {
    with_player_menus(|| unsafe { orig::pause_menu_update(env) });
}

/// Hooks for `CPlayerListMenu::Update()` and `UpdateInput(int)`: the
/// pause menu's lists of players and friends.
pub(crate) unsafe extern "C" fn player_list_update(env: *mut Env, menu: u64) {
    with_player_menus(|| unsafe { orig::player_list_update(env, menu) });
}

pub(crate) unsafe extern "C" fn player_list_input(env: *mut Env, menu: u64, input: u32) -> u32 {
    with_player_menus(|| unsafe { orig::player_list_input(env, menu, input) })
}

/// Hook for `CPlayerListMenu::Populate(MenuScreenId)`, which the menu's
/// movie calls back as a list opens (outside `CPauseMenu::Update`): it
/// asks the players' crews, without which the list shows none of them.
pub(crate) unsafe extern "C" fn player_list_populate(env: *mut Env, menu: u64, screen: u64) -> u32 {
    with_player_menus(|| unsafe { orig::player_list_populate(env, menu, screen) })
}

thread_local! {
    /// A friends list is telling the player it is empty.
    static NO_FRIENDS: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// Hook for `CPlayerListMenu::HandleNoPlayers()`: a list with no one in it
/// (its refresh found none) warns "the internet connection has been lost",
/// whatever the list; Social Club's lists were empty only when offline. A
/// friends list is empty until the player has a friend: it says so (see
/// `menu_warning`).
pub(crate) unsafe extern "C" fn player_list_empty(env: *mut Env, menu: u64) {
    let friends = is_friends_menu(unsafe { Mem::of(env) }, menu);
    let outer = NO_FRIENDS.replace(friends);
    unsafe { orig::player_list_empty(env, menu) };
    NO_FRIENDS.set(outer);
}

/// Hook for `CMenuBase::SHOW_WARNING_MESSAGE_EX(PM_COLUMNS, char const*,
/// char const*, int)` (the text's label, then the title's): an empty
/// friends list's warning gets the labels of no friends (texts.rs).
pub(crate) unsafe extern "C" fn menu_warning(env: *mut Env, menu: u64, column: u32, text: u64, title: u64, flags: u32) {
    let ours = if NO_FRIENDS.get() { unsafe { no_friends_labels(env) } } else { None };
    let (text, title) = ours.unwrap_or((text, title));
    unsafe { orig::menu_warning(env, menu, column, text, title, flags) }
}

/// The labels of an empty friends list's warning, in the game's heap
/// (made once).
unsafe fn no_friends_labels(env: *mut Env) -> Option<(u64, u64)> {
    static LABELS: Mutex<Option<(u64, u64)>> = Mutex::new(None);
    let mut labels = LABELS.lock().unwrap();
    if labels.is_none() {
        let m = unsafe { Mem::of(env) };
        let place = |label: &str| {
            let at = unsafe { orig::builtin_malloc(env, label.len() as u64 + 1) };
            (at != 0).then(|| {
                m.write_str(at, label, label.len() + 1);
                at
            })
        };
        *labels = place(crate::texts::NO_FRIENDS).zip(place(crate::texts::NO_FRIENDS_TITLE));
    }
    *labels
}

/// Hook for `SocialClubMgr::IsConnectedToSocialClub() const`: connected
/// for the pause menu and its lists of players and friends, which
/// otherwise say "the internet connection has been lost" instead of the
/// friends; elsewhere (Social Club's own menus, the gallery, crews) still
/// not, as their content is Rockstar's.
pub(crate) unsafe extern "C" fn social_club_connected(env: *mut Env) -> u32 {
    if IN_PLAYER_MENUS.get() {
        return 1;
    }
    unsafe { orig::social_club_connected(env) }
}

/// Hook for `rage::rlGamerInfo::IsOnline() const`: the player is signed in
/// to Social Club offline (online.rs), and the lists of players and
/// friends take no input from an offline player; for them it is online.
pub(crate) unsafe extern "C" fn gamer_online(env: *mut Env, gamer: u64) -> u32 {
    if IN_PLAYER_MENUS.get() && gamer == LOCAL_GAMER {
        return 1;
    }
    unsafe { orig::gamer_online(env, gamer) }
}

/// Hook for `rage::rlClan::IsServiceReady(int)`: crews are Rockstar's,
/// whose service is never ready here; in the pause menu's lists of players
/// and friends it is, for them to ask their players' crews (none, at once:
/// `clan_primary`) instead of waiting for them ("Downloading...") forever.
pub(crate) unsafe extern "C" fn clan_service_ready(env: *mut Env, gamer: u32) -> u32 {
    if IN_PLAYER_MENUS.get() && gamer == 0 {
        return 1;
    }
    unsafe { orig::clan_service_ready(env, gamer) }
}

/// Hook for `rage::rlClan::GetPrimaryClans(int, rlGamerHandle const*,
/// unsigned int, rlClanMember*, unsigned int*, unsigned int*, netStatus*)`
/// (one count is gone from the compiled function): gamers' crews, from
/// Rockstar's services: none, at once.
pub(crate) unsafe extern "C" fn clan_primary(
    env: *mut Env,
    _gamer: u32,
    _gamers: u64,
    _count: u32,
    _members: u64,
    found: u64,
    status: u64,
) -> u32 {
    let m = unsafe { Mem::of(env) };
    if found != 0 {
        m.set_u32(found, 0);
    }
    if status != 0 {
        m.set_u32(status, SUCCEEDED);
        m.set_u32(status + 4, 0);
    }
    1
}

/// Hook for `CLiveManager::AddFriend(rlGamerHandle const&, char const*)`
/// (the message is gone from the compiled function): "Send Friend
/// Request" on a player of the session, which Social Club's overlay took:
/// a friend request to it, when it is a player met.
pub(crate) unsafe extern "C" fn add_friend(env: *mut Env, handle: u64) -> u32 {
    let m = unsafe { Mem::of(env) };
    let sent = handle != 0 && super::ask_gamer(m.u64(handle));
    log::info!("friends: a friend request from the game's menu{}", if sent { "" } else { ": not a player met" });
    sent as u32
}

/// Hook for `rage::rlPresence::Query(int, char const*, char const*, int,
/// int, char*, unsigned int, char**, unsigned int*, unsigned int*,
/// netStatus*)` (the offset is gone from the compiled function): Social
/// Club's queries of presence; "SessionByGamerHandle" is how "Join
/// Friends" (and a public session's matchmaking, first) finds friends'
/// sessions: answered from the friends' presence, as Social Club would,
/// `{"_id": <handle>, "gsinfo": <session info>}` per friend whose session
/// may be joined, its lobby reached.
#[allow(clippy::too_many_arguments)]
pub(crate) unsafe extern "C" fn presence_query(
    env: *mut Env,
    gamer: u32,
    name: u64,
    params: u64,
    count: u32,
    buf: u64,
    size: u32,
    records: u64,
    found: u64,
    total: u64,
    status: u64,
) -> u32 {
    let m = unsafe { Mem::of(env) };
    if name == 0 || bounded_str(m, name, 64) != "SessionByGamerHandle" {
        return unsafe { orig::presence_query(env, gamer, name, params, count, buf, size, records, found, total, status) };
    }
    // `@ghandle,"SC <id>","SC <id>"...`: the friends asked about.
    let params = if params != 0 { bounded_str(m, params, 4096) } else { String::new() };
    log::debug!("friends: the game's query for sessions: {params}");
    let (mut at, end, mut n) = (buf, buf + u64::from(size), 0u64);
    // For "Join Friends", the session joined is a friend's.
    let joining = super::finding_friends();
    for handle in params.split(',').filter(|h| !h.starts_with('@')).map(|h| h.trim().trim_matches('"')) {
        if n >= u64::from(count) || records == 0 {
            break;
        }
        let Some(id) = handle.rsplit(' ').next().and_then(|id| id.parse::<u64>().ok()) else { continue };
        let Some(session) = super::session_of(id) else { continue };
        let record = serde_json::json!({ "_id": handle, "gsinfo": session }).to_string();
        if at + record.len() as u64 + 1 > end {
            break;
        }
        super::reach(id, joining);
        m.bytes_mut(at, record.len()).copy_from_slice(record.as_bytes());
        m.set_u8(at + record.len() as u64, 0);
        m.set_u64(records + n * 8, at);
        at += record.len() as u64 + 1;
        n += 1;
    }
    log::info!("friends: the game looks for friends' sessions: {n}");
    for out in [found, total] {
        if out != 0 {
            m.set_u32(out, n as u32);
        }
    }
    if status != 0 {
        m.set_u32(status, SUCCEEDED);
        m.set_u32(status + 4, 0);
    }
    1
}

/// Hook for `rage::rlFriendsManager::IsFriendsWith(int, rlGamerHandle
/// const&)`: the invites a player gets are from friends.
pub(crate) unsafe extern "C" fn friends_with(env: *mut Env, gamer: u32, handle: u64) -> u32 {
    let m = unsafe { Mem::of(env) };
    if handle != 0 && super::is_friend(m.u64(handle)) {
        return 1;
    }
    unsafe { orig::friends_with(env, gamer, handle) }
}

/// The vtables of `CMPFriendPlayerCardDataManager` and
/// `CSPFriendPlayerCardDataManager` (the FRIENDS tab, online and in the
/// story) and `CActivePlayerCardDataManager` (the ONLINE tab's Players),
/// and a card data manager's state (0: to read the players, 6: ready, 8
/// and 9: failed) and when it started reading (the engine's frame time).
const CARD_DATA: [u64; 3] = [6188416, 6188096, 6187512];
const CARD_DATA_STATE: u64 = 204;
const CARD_DATA_STARTED: u64 = 212;
const FRAME_TIME: u64 = 12321192;

/// Hook for `BasePlayerCardDataManager::Init(CPlayerCardXMLDataManager
/// const&)`: the card data of the friends and of the session's players is
/// set up as failed in the browser build (state 8, which nothing leaves,
/// and their lists say "the internet connection has been lost"); here it
/// starts reading the players instead, as the other builds do.
pub(crate) unsafe extern "C" fn card_data_init(env: *mut Env, data: u64, xml: u64) {
    unsafe { orig::card_data_init(env, data, xml) };
    let m = unsafe { Mem::of(env) };
    if CARD_DATA.contains(&m.u64(data)) && m.u32(data + CARD_DATA_STATE) == 8 {
        m.set_u32(data + CARD_DATA_STATE, 0);
        m.set_u32(data + CARD_DATA_STARTED, m.u32(FRAME_TIME));
    }
}

/// Hook for `rage::rlProfileStats::ReadStatsByGamer(int, rlGamerHandle
/// const*, unsigned int, rlProfileStatsReadResults*, netStatus*)`:
/// Rockstar's stats of gamers, which a friend's card waits for (and says
/// "the internet connection has been lost" when they fail): none, at once.
pub(crate) unsafe extern "C" fn profile_stats_read(
    env: *mut Env,
    _gamer: u32,
    _handles: u64,
    _count: u32,
    _results: u64,
    status: u64,
) -> u32 {
    if status != 0 {
        let m = unsafe { Mem::of(env) };
        m.set_u32(status, SUCCEEDED);
        m.set_u32(status + 4, 0);
    }
    1
}

/// Hook for `CGamePresenceEventDispatcher::SendEventToGamers(...)`: an
/// event for gamers, which goes on only with Rockstar's credentials.
pub(crate) unsafe extern "C" fn presence_send(env: *mut Env, event: u64, handles: u64, count: u32) {
    crate::cloud::with_credentials(|| unsafe { orig::presence_send(env, event, handles, count) });
}

/// Hook for `rlPresence::PostMessage<rlScPresenceMessagePublish>(...)`:
/// the event (its text at the message's start) goes to the friends among
/// the gamers, not to Social Club.
pub(crate) unsafe extern "C" fn presence_post(env: *mut Env, handles: u64, count: u32, message: u64) {
    let m = unsafe { Mem::of(env) };
    // Whole or not at all (cut, it would not parse).
    let text = bounded_str(m, message, MAX_EVENT + 1);
    if text.len() > MAX_EVENT {
        log::warn!("friends: the game's event is too long ({} bytes or more): not sent", text.len());
        return;
    }
    let ids: Vec<u64> = (0..u64::from(count.min(32))).map(|i| m.u64(handles + i * HANDLE_SIZE)).collect();
    let sent = super::send_game_event(&ids, &text);
    let kind = serde_json::from_str::<serde_json::Value>(&text)
        .ok()
        .and_then(|v| Some(v.get("gm.evt")?.get("e")?.as_str()?.to_owned()))
        .unwrap_or_default();
    log::info!("friends: the game's event {kind:?} to {sent} friends of {count} gamers");
}

/// Hook for `CNetworkSession::SendInvites(rlGamerHandle const*, unsigned
/// int, char const*, char const*, netStatus*)` (the status is gone from
/// the compiled function): the platform's invites to this player's session
/// (the pause menu's, a script's, the engine's when an invite to a job is
/// not answered), which Social Club's overlay showed; friends get ours,
/// and their game asks them whether to join.
pub(crate) unsafe extern "C" fn session_invites(
    env: *mut Env,
    _session: u64,
    handles: u64,
    count: u32,
    _subject: u64,
    _salutation: u64,
) -> u32 {
    let m = unsafe { Mem::of(env) };
    let ids: Vec<u64> = (0..u64::from(count.min(32))).map(|i| m.u64(handles + i * HANDLE_SIZE)).collect();
    super::send_invites(&ids);
    1
}

/// Hook for `rlPresenceQuery::GetSessionByGamerHandle(...)` (the counts
/// are gone from the compiled function): a friend's session, as it said,
/// and its lobby reached; for anyone else, Social Club's answer.
pub(crate) unsafe extern "C" fn session_by_gamer(
    env: *mut Env,
    gamer: u32,
    handle: u64,
    results: u64,
    found: u64,
    status: u64,
) -> u32 {
    let m = unsafe { Mem::of(env) };
    let id = if handle != 0 { m.u64(handle) } else { 0 };
    let Some(text) = super::session_of(id) else {
        return unsafe { orig::session_by_gamer(env, gamer, handle, results, found, status) };
    };
    // rlSessionQueryData: the handle, then the session's info.
    let h = m.bytes(handle, HANDLE_SIZE as usize).to_vec();
    m.bytes_mut(results, h.len()).copy_from_slice(&h);
    let parsed = unsafe {
        with_stack(env, text.len() as u64 + 1, |at| {
            m.bytes_mut(at, text.len()).copy_from_slice(text.as_bytes());
            orig::session_info_parse(env, results + HANDLE_SIZE, at, 0) != 0
        })
    };
    if parsed != Some(true) {
        return 0;
    }
    if found != 0 {
        m.set_u32(found, 1);
    }
    if status != 0 {
        m.set_u32(status, SUCCEEDED);
        m.set_u32(status + 4, 0);
    }
    log::info!("friends: joining a friend from its card");
    super::reach(id, true);
    1
}

/// Hook for `InviteMgr::AcceptInvite(rlSessionInfo const&, rlGamerInfo
/// const&, rlGamerHandle const& inviter, ...)`: an invite accepted; the
/// inviter's lobby is reached before the engine asks to join.
#[allow(clippy::too_many_arguments)]
pub(crate) unsafe extern "C" fn invite_accept(
    env: *mut Env,
    info: u64,
    gamer: u64,
    inviter: u64,
    a: u32,
    b: u32,
    c: u32,
    d: u64,
    e: u32,
    f: u32,
) {
    let m = unsafe { Mem::of(env) };
    if inviter != 0 {
        super::reach(m.u64(inviter), true);
    }
    unsafe { orig::invite_accept(env, info, gamer, inviter, a, b, c, d, e, f) }
}

pub(crate) fn hooks(hooks: &mut glue5_game::Hooks) {
    hooks.friends_read = Some(friends_read);
    hooks.friends_with = Some(friends_with);
    hooks.add_friend = Some(add_friend);
    hooks.presence_query = Some(presence_query);
    hooks.profile_stats_read = Some(profile_stats_read);
    hooks.card_data_init = Some(card_data_init);
    hooks.clan_service_ready = Some(clan_service_ready);
    hooks.clan_primary = Some(clan_primary);
    hooks.pause_menu_version = Some(pause_menu_version);
    hooks.pause_menu_update = Some(pause_menu_update);
    hooks.player_list_update = Some(player_list_update);
    hooks.player_list_input = Some(player_list_input);
    hooks.player_list_populate = Some(player_list_populate);
    hooks.player_list_empty = Some(player_list_empty);
    hooks.menu_warning = Some(menu_warning);
    hooks.social_club_connected = Some(social_club_connected);
    hooks.gamer_online = Some(gamer_online);
    hooks.presence_send = Some(presence_send);
    hooks.presence_post = Some(presence_post);
    hooks.session_invites = Some(session_invites);
    hooks.session_by_gamer = Some(session_by_gamer);
    hooks.invites_blocked = Some(invites_blocked);
    hooks.invite_accept = Some(invite_accept);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn events_are_checked() {
        // A handle: id 12345, service 3, index 0.
        let mut h = [0u8; 16];
        h[..8].copy_from_slice(&12345u64.to_le_bytes());
        h[8] = 3;
        let b64 = "OTAAAAAAAAADAAAAAAAAAA==";
        assert_eq!(base64(b64).unwrap(), h);
        let invite = format!(r#"{{"gm.evt":{{"e":"ginv","d":{{"v":4,"s":"abc","h":"{b64}","n":"Mallory","f":0}}}}}}"#);
        let ok = checked(&invite, 12345, "Ann").unwrap();
        assert!(ok.contains(r#""n":"Ann""#) && ok.contains(r#""s":"abc""#), "{ok}");
        // From someone else, or not an invite.
        assert_eq!(checked(&invite, 999, "Ann"), None);
        assert_eq!(checked(r#"{"gm.evt":{"e":"TextMessage","d":{}}}"#, 12345, "Ann"), None);
        assert_eq!(checked(r#"{"gm.evt":{"e":"ginv","d":{"v":4}}}"#, 12345, "Ann"), None);
        assert_eq!(checked("not json", 12345, "Ann"), None);
    }
}
