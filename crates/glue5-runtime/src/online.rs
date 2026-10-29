//! GTA Online without Rockstar's services: the game plays on a lobby of
//! players (p2p.rs) as on a LAN (vnet.rs), and keeps its characters on the
//! disk (cloud.rs).
//!
//! The engine signs in through Social Club, whose DLL it talks to: `rlPc`
//! keeps a pointer to each of the DLL's interfaces, and the host puts
//! objects there whose vtables lead to host functions, so the engine sees
//! a signed-in, offline player. `rlPc::IsInitialized()` still says no, so
//! what asks it (saves, control settings) keeps its path without Social
//! Club; only code reading the pointers directly (presence) meets the
//! stand-ins, whose methods do nothing but sign the player in.
//!
//! The rest are the checks on the way into freemode that Rockstar's
//! services would pass, passed here.
//!
//! The network is up in the story too, as on PC, for its pause menu's
//! ONLINE and FRIENDS tabs and a friend's invite or join to lead into GTA
//! Online. The story stays offline all the same: its scripts and the
//! engine see a player online (`online_here`) only in GTA Online, and in
//! what leads there (the menus, the transition, an invite accepted).

use std::ffi::c_void;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, Ordering};

use glue5_game::{Env, exports, orig};

use crate::memory::Mem;

/// `rlPc` in this build: Social Club's interface pointers are at +88 to
/// +208, the profile manager's at +96.
const RL_PC: u64 = 15211472;
const INTERFACES: std::ops::Range<u64> = 88..216;
const PROFILE_MANAGER: u64 = 96;
/// Methods in a stand-in's vtable, more than any interface has.
const METHODS: u64 = 64;

/// The engine's switches for GTA Online without Rockstar's services: LAN
/// sessions (the lobby is the LAN, vnet.rs), no cloud files, catalog,
/// shop transactions or online policies to wait for, sessions for everyone
/// whatever their aim (the game keeps players aiming freely, with a mouse,
/// apart from assisted ones, as touch play on a phone is: they never met;
/// invites and their sessions' hosts too, `event_network_add` and
/// `host_aim_preference`), and the story's prologue taken as played
/// (joining a friend's session asks for it: "GTA Online will be available
/// once you have completed the prologue", and the game answered the
/// friend's session every frame).
pub(crate) const ENGINE_ARGS: &[&str] = &[
    "-lan",
    "-netSessionIgnoreAim",
    "-netSpPrologueComplete",
    "-mpavailable",
    "-netMpSkipCloudFileRequirements",
    "-sc_noOnlinePolicy",
    "-netshoptransactionnull",
    "-netGameTransactionsNoToken",
    "-nocatalog",
];


/// The local gamer's id (as Rockstar's ids): made up once per user
/// directory, kept in it.
pub(crate) fn gamer_id() -> u64 {
    gamer_id_in(&crate::rt().config.user_dir)
}

pub(crate) fn gamer_id_in(user_dir: &std::path::Path) -> u64 {
    static ID: OnceLock<u64> = OnceLock::new();
    *ID.get_or_init(|| {
        let path = user_dir.join("online-id");
        if let Some(id) = std::fs::read_to_string(&path).ok().and_then(|s| s.trim().parse().ok()) {
            return id;
        }
        use std::hash::{BuildHasher, Hasher};
        // Positive 32-bit: the scripts keep ids in ints.
        let id = std::collections::hash_map::RandomState::new().build_hasher().finish() % 0x7fff_0000 + 0x1_0000;
        if let Err(e) = std::fs::write(&path, id.to_string()) {
            log::warn!("online: keeping the gamer id in {}: {e}", path.display());
        }
        id
    })
}

/// In GTA Online: from the start (the start page's GTA Online), or since
/// the switch to it from the story, until the game is back in the story.
static IN_GTA_ONLINE: AtomicBool = AtomicBool::new(false);

/// The scripts leading from the story to GTA Online: the pause menu's ONLINE
/// tab (its sessions, the creators), the transition (which also takes a
/// friend's invite or join). For them the player is online in the story too.
const LEADING_ONLINE: &[&[u8]] = &[b"PauseMenu_Multiplayer", b"MainTransition"];
/// `GtaThread`'s script name.
const THREAD_NAME: u64 = 428;
/// The invite manager's accepted invite (its session's info): valid from
/// the invite accepted (a friend's session joined, an invite) until the
/// scripts take it or turn it down (`InviteMgr::ClearInvite`).
const ACCEPTED_INVITE: u64 = 29260624;

/// Whether the local player is online for what asks now: in GTA Online,
/// and in the story only for what leads there (the scripts above, the
/// pause menu's lists of players and friends, an invite accepted) and for its privileges. The story's own scripts see
/// it offline, as they did without the network: online, they would wait
/// for Rockstar's services (the stock market, the cloud, the newswire...).
///
/// An invite accepted in the story is the story's scripts' to take: the
/// game holds it (its processing suppressed since the start) while
/// InGameHud loads the GTA Online character's stats (only then, in the
/// story), and main_persistent turns it down unless they loaded and the
/// cloud is there. Online for them while it is pending, as for a player
/// online on PC (`tell_online` tells them so).
pub(crate) unsafe fn online_here(env: *mut Env) -> bool {
    if IN_GTA_ONLINE.load(Ordering::Relaxed) || REFRESHING_PRIVILEGES.get() || crate::friends::game::in_player_menus() {
        return true;
    }
    if unsafe { orig::session_info_valid(env, ACCEPTED_INVITE) } != 0 {
        return true;
    }
    let thread = unsafe { orig::script_active_thread(env) };
    if thread == 0 {
        return false;
    }
    let name = unsafe { Mem::of(env) }.bytes(thread + THREAD_NAME, 32);
    let name = &name[..name.iter().position(|&b| b == 0).unwrap_or(name.len())];
    LEADING_ONLINE.contains(&name)
}

/// The game goes to GTA Online (from the story, or at the start): the
/// player's friends see it there, and it is in its lobby (everyone's,
/// unless another was asked for) unless it is on its way to a friend's.
fn enter_gta_online() {
    if IN_GTA_ONLINE.swap(true, Ordering::Relaxed) {
        return;
    }
    log::info!("online: into GTA Online");
    crate::friends::set_playing(crate::friends::Playing::Online);
    if let Some(p2p) = crate::p2p::get()
        && p2p.lobbies().is_empty()
    {
        p2p.join_lobby(crate::rt().config.online.as_ref().map_or("", |o| o.lobby.as_str()));
    }
}

/// The game is back in the story: out of the lobbies.
fn leave_gta_online() {
    if !IN_GTA_ONLINE.swap(false, Ordering::Relaxed) {
        return;
    }
    log::info!("online: back to the story");
    crate::friends::set_playing(crate::friends::Playing::Story);
    if let Some(p2p) = crate::p2p::get() {
        p2p.leave_lobbies();
    }
}

fn gamer_name() -> String {
    crate::rt().config.online.as_ref().map_or_else(String::new, |o| o.name.clone())
}

/// Sets the game up for GTA Online: the hooks of this module and the
/// others', the stand-ins' methods, the lobby (straight into freemode; from
/// the story, once it goes there). Before installing the game module.
pub(crate) fn start(online: &crate::Online, hooks: &mut glue5_game::Hooks) {
    self::hooks(hooks);
    crate::vnet::hooks(hooks);
    crate::cloud::hooks(hooks);
    crate::jobs::hooks(hooks);
    crate::friends::game::hooks(hooks);
    crate::stats::hooks(hooks);
    // The stand-ins' methods, host functions the game calls through their
    // vtables (`glue5_game::host_function`): QueryInterface, a method doing
    // nothing (0: S_OK, false, none), the profile manager's IsSignedIn and
    // GetSignInInfo.
    static FNS: OnceLock<[usize; 4]> = OnceLock::new();
    glue5_game::set_host_functions(FNS.get_or_init(|| {
        [
            query_interface as *const () as usize,
            nothing as *const () as usize,
            signed_in as *const () as usize,
            sign_in_info as *const () as usize,
        ]
    }));
    match crate::p2p::start() {
        Ok(p2p) if online.freemode => p2p.join_lobby(&online.lobby),
        Ok(_) => {}
        Err(e) => log::error!("p2p: {e}"),
    }
    if online.freemode {
        enter_gta_online();
    }
}

const QUERY_INTERFACE: u32 = 0;
const NOTHING: u32 = 1;
const SIGNED_IN: u32 = 2;
const SIGN_IN_INFO: u32 = 3;

// Host functions: called with the thread's instance first, not its `Env`
// (see `glue5_game::set_host_functions`); the wasm memory is the runtime's.

/// QueryInterface(iid, void** out): no other interface is offered.
unsafe extern "C" fn query_interface(_: *mut c_void, _this: u64, _iid: u64, out: u64) -> u64 {
    if out != 0 {
        crate::rt().memory.mem().set_u64(out, 0);
    }
    0x8000_4002 // E_NOINTERFACE
}

unsafe extern "C" fn nothing(_: *mut c_void) -> u64 {
    0
}

unsafe extern "C" fn signed_in(_: *mut c_void) -> u64 {
    1
}

/// GetSignInInfo(IProfile*): { vtable*, s64 id, char nickname[17] }.
unsafe extern "C" fn sign_in_info(_: *mut c_void, _this: u64, profile: u64) -> u64 {
    let m = crate::rt().memory.mem();
    m.set_u64(profile + 8, gamer_id());
    m.write_str(profile + 16, &gamer_name(), 17);
    0
}

/// Puts the stand-ins in `rlPc`: one object for the profile manager, one
/// for all the other interfaces.
unsafe fn install(env: *mut Env) {
    let m = unsafe { Mem::of(env) };
    let object = |methods: &[u32]| {
        let vtable = unsafe { orig::builtin_malloc(env, METHODS * 8) };
        for i in 0..METHODS {
            let f = methods.get(i as usize).copied().unwrap_or(NOTHING);
            m.set_u64(vtable + i * 8, glue5_game::host_function(f));
        }
        let object = unsafe { orig::builtin_malloc(env, 8) };
        m.set_u64(object, vtable);
        object
    };
    // IsSignedIn: yes; IsOnline: no (Social Club's own online paths stay off).
    let profile_manager = object(&[QUERY_INTERFACE, SIGNED_IN, NOTHING, SIGN_IN_INFO]);
    let other = object(&[QUERY_INTERFACE]);
    for offset in INTERFACES.step_by(8) {
        m.set_u64(RL_PC + offset, if offset == PROFILE_MANAGER { profile_manager } else { other });
    }
}

/// Hook for `CLiveManager::Init(unsigned int)`: the stand-ins go in before
/// the live manager first asks who is signed in.
pub(crate) unsafe extern "C" fn live_manager_init(env: *mut Env, mode: u32) {
    static DONE: std::sync::Once = std::sync::Once::new();
    DONE.call_once(|| unsafe { install(env) });
    unsafe { orig::live_manager_init(env, mode) };
    // Init clears the store's state; nothing would populate it again.
    unsafe { populate_store_data(env) };
}

/// Hook for `CLiveManager::Update(unsigned int)`: the device's network
/// stays up (see `vnet::network_up`), the tunables' request is over;
/// friends (friends/game.rs).
pub(crate) unsafe extern "C" fn live_manager_update(env: *mut Env, mode: u32) {
    unsafe { crate::friends::game::update(env) };
    unsafe { tell_online(env) };
    crate::vnet::network_up(unsafe { Mem::of(env) });
    unsafe { tunables_requested(env) };
    unsafe { orig::live_manager_update(env, mode) };
    crate::vnet::network_up(unsafe { Mem::of(env) });
}

/// The story's scripts were told the player is online (`tell_online`).
static TOLD_ONLINE: AtomicBool = AtomicBool::new(false);

/// `CEventNetworkSignInStateChanged` (the scripts'
/// EVENT_NETWORK_SIGN_IN_STATE_CHANGED): its vtable, its size, and its data
/// at +32 (the script's struct: the gamer's index, then whether it is the
/// active user, was signed in, was online, is signed in, is online, signed
/// in twice), 8 bytes a field.
const SIGN_IN_EVENT: u64 = 6212240;
const SIGN_IN_EVENT_SIZE: u64 = 88;
const SIGN_IN_DATA: u64 = 32;

/// The story's scripts work out at their start whether the player may play
/// online, signed in online (main_persistent's privileges; offline there, as
/// the story is), and again when told the sign-in state changed. InGameHud
/// loads the stats for an invite only with them. So when an invite accepted
/// in the story makes the player online for them (`online_here`), they are
/// told, once, as the engine tells them a player signed in online.
unsafe fn tell_online(env: *mut Env) {
    if IN_GTA_ONLINE.load(Ordering::Relaxed)
        || TOLD_ONLINE.load(Ordering::Relaxed)
        || unsafe { orig::session_info_valid(env, ACCEPTED_INVITE) } == 0
    {
        return;
    }
    TOLD_ONLINE.store(true, Ordering::Relaxed);
    let m = unsafe { Mem::of(env) };
    let Ok(top) = (unsafe { exports::emscripten_stack_get_current(env) }) else { return };
    let Ok(event) = (unsafe { exports::_emscripten_stack_alloc(env, SIGN_IN_EVENT_SIZE) }) else { return };
    m.bytes_mut(event, SIGN_IN_EVENT_SIZE as usize).fill(0);
    m.set_u64(event, SIGN_IN_EVENT);
    // Index 0, the active user, signed in before and now, online now.
    for (field, value) in [(1, 1), (2, 1), (4, 1), (5, 1)] {
        m.set_u32(event + SIGN_IN_DATA + field * 8, value);
    }
    unsafe {
        let group = orig::event_network_group(env);
        orig::event_network_add(env, group, event);
        orig::ref_aware_drop(env, event);
        let _ = exports::_emscripten_stack_restore(env, top);
    }
    log::info!("online: the story's scripts told the player is online, for an invite");
}

/// The invite events the scripts get (`CEventNetworkInviteAccepted`,
/// `CEventNetworkInviteConfirmed`: their vtables) and, in their data, the
/// aim of the invite's session (the script's field 16, at +160).
const INVITE_EVENTS: [u64; 2] = [6208512, 6209600];
const INVITE_AIM: u64 = 160;

/// Hook for `CEventGroupScriptNetwork::Add(rage::fwEvent const&)`: the
/// scripts' network events. An invite's session aims as the player does:
/// sessions here are for everyone whatever their aim (`ENGINE_ARGS`'
/// -netSessionIgnoreAim), and the scripts would turn the invite down
/// otherwise ("a different targeting mode"), or make the player aim as
/// the session does, which touch play's aim assist then undoes, the
/// warning coming again.
pub(crate) unsafe extern "C" fn event_network_add(env: *mut Env, group: u64, event: u64) -> u64 {
    let m = unsafe { Mem::of(env) };
    if INVITE_EVENTS.contains(&m.u64(event)) {
        m.set_u32(event + INVITE_AIM, unsafe { orig::gamepad_aim_state(env) });
    }
    unsafe { orig::event_network_add(env, group, event) }
}

/// Hook for `NETWORK_SESSION_GET_HOST_AIM_PREFERENCE`: the session's host
/// aims as the player does, for the same reason; the scripts joining a
/// session by an invite would make the player aim as the host otherwise.
pub(crate) unsafe extern "C" fn host_aim_preference(env: *mut Env, info: u64) {
    unsafe { orig::host_aim_preference(env, info) };
    let m = unsafe { Mem::of(env) };
    m.set_u32(m.u64(info), unsafe { orig::aim_state(env) });
}

/// Rockstar's cloud tunables never come: their request counts as over
/// (`CTunables`' flag, set at the end of `OnCloudEvent` either way), as
/// what waits for it (invites, CanEnterMultiplayer) asks, and the network's
/// settings are read from them as they would be then: their defaults (an
/// invite's lifetime among them, else 0: gone as it comes). The scripts
/// keep their defaults too.
unsafe fn tunables_requested(env: *mut Env) {
    const TUNABLES: u64 = 19275808;
    const CLOUD_REQUEST_DONE: u64 = 464;
    let m = unsafe { Mem::of(env) };
    let tunables = m.u64(TUNABLES);
    if tunables != 0 && m.u8(tunables + CLOUD_REQUEST_DONE) == 0 {
        // Read before the flag is set: once set, the reader would also ask
        // Rockstar for its content's URLs.
        unsafe { orig::network_tunables_read(env, 0) };
        m.set_u8(tunables + CLOUD_REQUEST_DONE, 1);
        log::info!("online: the network's tunables, their defaults");
    }
}

/// `CNetworkSession`'s host flags (its `LogHostFlags`).
const HF_IS_PRIVATE: u32 = 1;
const HF_SINGLEPLAYER_ONLY: u32 = 4;
const HF_CLOSED: u32 = 8;
const HF_CLOSED_FRIENDS: u32 = 16;
const HF_CLOSED_CREW: u32 = 32;

/// Hook for `CNetworkSession::HostSession(NetworkGameConfig&, unsigned
/// int)`, which every session the game hosts goes through (a job's
/// excepted): the lobby it is in follows its kind, as the pause menu's
/// sessions are. A public one in everyone's; the others (invite only,
/// crew, closed friends, solo) in a new private lobby, whose key is the
/// only way in: friends get it with an invite, or from the presence for a
/// friends' session; nobody for a solo one.
pub(crate) unsafe extern "C" fn session_host_config(env: *mut Env, session: u64, config: u64, flags: u32) -> u32 {
    use crate::friends::SessionKind;
    let kind = if flags & HF_SINGLEPLAYER_ONLY != 0 {
        SessionKind::Solo
    } else if flags & HF_CLOSED_FRIENDS != 0 {
        SessionKind::Friends
    } else if flags & (HF_IS_PRIVATE | HF_CLOSED | HF_CLOSED_CREW) != 0 {
        SessionKind::Closed
    } else {
        SessionKind::Public
    };
    in_lobby_for(kind);
    unsafe { orig::session_host_config(env, session, config, flags) }
}

/// Hook for `CNetworkSession::DoFreeroamMatchmaking(int, int, unsigned int,
/// unsigned int)`: looking for a public session, in everyone's lobby.
pub(crate) unsafe extern "C" fn freeroam_matchmaking(env: *mut Env, session: u64, mode: u32, max: u32, flags: u32) -> u32 {
    in_lobby_for(crate::friends::SessionKind::Public);
    unsafe { orig::freeroam_matchmaking(env, session, mode, max, flags) }
}

/// Hook for `CNetworkSession::DoFriendMatchmaking(int, int, unsigned int,
/// unsigned int)`: "Join Friends", into a friend's session, found in the
/// friends' presence (friends/game.rs): the friend's lobby is the
/// friend's to share.
pub(crate) unsafe extern "C" fn friend_matchmaking(env: *mut Env, session: u64, mode: u32, max: u32, flags: u32) -> u32 {
    crate::friends::find_friends();
    unsafe { orig::friend_matchmaking(env, session, mode, max, flags) }
}

/// Moves the player to the lobby for a session of `kind`: everyone's (the
/// friend's, while one is followed there), or a new private one.
fn in_lobby_for(kind: crate::friends::SessionKind) {
    crate::friends::set_session_kind(kind);
    let Some(p2p) = crate::p2p::get() else { return };
    if kind == crate::friends::SessionKind::Public {
        if crate::friends::following().is_none() {
            p2p.switch_lobby("");
        }
    } else {
        // Its key, the way in: 128 bits from the system (as long a name as
        // friends take).
        let mut key = [0u8; 16];
        if let Err(e) = getrandom::fill(&mut key) {
            // (In a hook: no panic. The lobby stays as it is.)
            log::error!("online: no random numbers for a private lobby: {e}");
            return;
        }
        p2p.switch_lobby(&key.iter().map(|b| format!("{b:02x}")).collect::<String>());
    }
}

/// Hook for `CLiveManager::BailOnMultiplayer(eBailReason, int)`: the
/// browser build has no network hardware (`netHardware::IsAvailable()` is
/// false there, folded into the callers), so every session ends at once
/// "because the hardware went offline". That one bail is ignored.
pub(crate) unsafe extern "C" fn bail_on_multiplayer(env: *mut Env, reason: u32, context: u32) {
    const BAIL_PROFILE_CHANGE: u32 = 2;
    const BAIL_CTX_PROFILE_CHANGE_HARDWARE_OFFLINE: u32 = 0x3001;
    if reason == BAIL_PROFILE_CHANGE && context == BAIL_CTX_PROFILE_CHANGE_HARDWARE_OFFLINE {
        return;
    }
    unsafe { orig::bail_on_multiplayer(env, reason, context) }
}

/// Hook for `rage::rlPc::IsInitialized()`: no, with the stand-ins in (see
/// the module's notes).
pub(crate) unsafe extern "C" fn rl_pc_initialized(_: *mut Env) -> u32 {
    0
}

/// Privileges Rockstar's servers would grant (`rlRosPrivilegeId` bits): all
/// the game's defaults but 7 (banned), 12 (Rockstar developer) and 20
/// (cheater).
const PRIVILEGES: u32 = 0b0110_1111_1110_1111_0111_1111;

/// Hook for `rage::rlRosCredentials::HasPrivilege(rlRosPrivilegeId)`: the
/// privileges, without credentials (valid ones would send the engine to
/// Rockstar's web services).
pub(crate) unsafe extern "C" fn ros_privilege(_env: *mut Env, _creds: u64, id: u32) -> u32 {
    (id < 32 && PRIVILEGES >> id & 1 != 0) as u32
}

/// Hook for `rage::rlRosCredentials::HasPrivilegeEndDate(rlRosPrivilegeId,
/// bool*, unsigned long*)`: no privilege ends. Without credentials the
/// engine answers the same after an error line, and CNetwork asks every
/// frame.
pub(crate) unsafe extern "C" fn ros_privilege_end_date(
    _env: *mut Env,
    _creds: u64,
    _id: u32,
    _ends: u64,
    _date: u64,
) -> u32 {
    0
}

/// The profile setting that says the story's prologue is done, which GTA
/// Online asks for.
const PROLOGUE_COMPLETE: u32 = 903;

/// Hook for `CProfileSettings::GetInt(ProfileSettingId)`.
pub(crate) unsafe extern "C" fn profile_setting_int(env: *mut Env, this: u64, id: u32) -> u32 {
    if id == PROLOGUE_COMPLETE { 1 } else { unsafe { orig::profile_setting_int(env, this, id) } }
}

/// Hooks for natives whose answer depends on Rockstar's servers or on
/// content this game data lacks: the result scripts get here.
macro_rules! native_results {
    ($($(#[$doc:meta])* $hook:ident = $value:expr;)*) => {$(
        $(#[$doc])*
        pub(crate) unsafe extern "C" fn $hook(env: *mut Env, info: u64) {
            unsafe { orig::$hook(env, info) };
            let m = unsafe { Mem::of(env) };
            m.set_u32(m.u64(info), $value);
        }
    )*};
}

native_results! {
    /// `GET_EVER_HAD_BAD_PACK_ORDER`: the game data has the 2014 DLC packs
    /// only, the scripts expect later ones.
    bad_pack_order = 0;
    /// `SAVEMIGRATION_IS_MP_ENABLED`: no character to bring over from the
    /// consoles (the servers' service).
    savemigration_mp_enabled = 0;
    /// `SC_PROFANITY_GET_STRING_STATUS`: passed.
    profanity_status = 0;
    /// `SC_PROFANITY_GET_CHECK_IS_PENDING`: never waiting on it.
    profanity_pending = 0;
    /// `SC_PROFANITY_GET_CHECK_IS_VALID`: Social Club's text check (names); every text passes.
    profanity_valid = 1;
    /// `ARE_ONLINE_POLICIES_UP_TO_DATE`: there are no Rockstar policies to
    /// accept (`-sc_noOnlinePolicy`); an invite is dropped otherwise.
    online_policies_current = 1;
}

/// Hook for `NETWORK_HAS_SOCIAL_CLUB_ACCOUNT`: the player has one where it
/// is online (the pause menu's Creator needs it).
pub(crate) unsafe extern "C" fn social_club_account(env: *mut Env, info: u64) {
    unsafe { orig::social_club_account(env, info) };
    if unsafe { online_here(env) } {
        let m = unsafe { Mem::of(env) };
        m.set_u32(m.u64(info), 1);
    }
}

/// Hook for `PLAYSTATS_CREATE_MATCH_HISTORY_ID_2(int&, int&)`: a match's id,
/// the Rockstar account's and the time, or the network card's address
/// (which the engine cannot read here); the minigames' host (golf) waits
/// for one. The player's id and the time.
pub(crate) unsafe extern "C" fn match_history_id(env: *mut Env, info: u64) {
    let m = unsafe { Mem::of(env) };
    let params = m.u64(info + 16);
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs());
    m.set_u32(m.u64(params), gamer_id() as u32);
    m.set_u32(m.u64(params + 8), now as u32);
    m.set_u32(m.u64(info), 1);
}

/// Hook for `CNetwork::CanAccessNetworkFeatures(eNetworkAccessArea,
/// eNetworkAccessCode*)`: the gate of every online feature (multiplayer,
/// creators, store...), whose checks are about Rockstar's services; granted
/// where the player is online.
pub(crate) unsafe extern "C" fn can_access_network_features(env: *mut Env, area: u32, code: u64) -> u32 {
    if !unsafe { online_here(env) } {
        return unsafe { orig::can_access_network_features(env, area, code) };
    }
    if code != 0 {
        unsafe { Mem::of(env) }.set_u32(code, 0); // Access_Granted
    }
    1
}

/// Hook for `rage::rlPresence::IsOnline(int)`: the local gamer counts as
/// online where it is (`online_here`: sessions and the privileges need
/// it); the stand-in profile manager still says offline, so Social Club's
/// own online paths stay off.
pub(crate) unsafe extern "C" fn presence_online(env: *mut Env, index: u32) -> u32 {
    if index == 0 && unsafe { online_here(env) } { 1 } else { unsafe { orig::presence_online(env, index) } }
}

/// Hooks for `NetworkInterface::IsLocalPlayerOnline(bool)` and
/// `HasValidRosCredentials(int)` (the index is gone from the compiled
/// function), which `-lan` answers yes for a signed-in player (on a LAN
/// there are no Rockstar services to be online with): only where the player
/// is online (`online_here`), not in the story's own scripts.
pub(crate) unsafe extern "C" fn local_player_online(env: *mut Env, lan: u32) -> u32 {
    unsafe { online_here(env) && orig::local_player_online(env, lan) != 0 }.into()
}

pub(crate) unsafe extern "C" fn ros_credentials_held(env: *mut Env) -> u32 {
    unsafe { online_here(env) && orig::ros_credentials_held(env) != 0 }.into()
}

/// Hook for `SWITCH_TO_MULTI_FIRSTPART` (the camera's swoop up from the
/// story, as the transition to GTA Online starts, for whatever reason: the
/// pause menu, a friend's invite or join): into GTA Online.
pub(crate) unsafe extern "C" fn switch_to_multiplayer(env: *mut Env, info: u64) {
    enter_gta_online();
    unsafe { orig::switch_to_multiplayer(env, info) }
}

/// Hook for `CNetworkSession::OnReturnToSinglePlayer()`: back in the story.
pub(crate) unsafe extern "C" fn return_to_story(env: *mut Env, session: u64) {
    leave_gta_online();
    unsafe { orig::return_to_story(env, session) }
}

thread_local! {
    /// The engine is working out the player's privileges.
    static REFRESHING_PRIVILEGES: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// Hook for `CLiveManager::RefreshPrivileges()`, as the player signs in:
/// the privileges are the account's, the same in the story as in GTA
/// Online. Worked out offline, they kept Rockstar's content only (the
/// ONLINE tab's creators off; "Your profile only has permissions for
/// Rockstar content" on the way to GTA Online).
pub(crate) unsafe extern "C" fn refresh_privileges(env: *mut Env) {
    let outer = REFRESHING_PRIVILEGES.replace(true);
    unsafe { orig::refresh_privileges(env) };
    REFRESHING_PRIVILEGES.set(outer);
}

/// Hook for `cCommerceManager::PopulateThinData(bool)` (the argument is
/// gone from the compiled function): the store's catalogue comes from
/// Rockstar's servers, and while it loads it holds the network memory
/// sessions need (CanEnterMultiplayer, matchmaking, hosting). Here it is
/// done at once, empty: the state the engine sets when a download ends.
pub(crate) unsafe extern "C" fn populate_store_data(env: *mut Env) {
    const STATE: u64 = 29308996; // 2: idle
    const POPULATED: u64 = 29309000; // u8 populated, u8 in progress
    const USE_CACHE: u64 = 29309002;
    let m = unsafe { Mem::of(env) };
    m.set_u8(USE_CACHE, 0);
    m.set_u32(STATE, 2);
    m.set_u16(POPULATED, 1);
}

/// Hook for `CHighQualityScreenshot::StoreScreenShotInBuffer(...)` (one
/// argument is gone from the compiled function): the character's mugshot
/// ends the process in libjpeg's error exit here. It fails instead, which
/// the game takes (the character has no photo).
#[allow(clippy::too_many_arguments)]
pub(crate) unsafe extern "C" fn photo_to_jpeg(
    env: *mut Env,
    _out: u64,
    size: u64,
    _max: u32,
    _pixels: u64,
    _width: u32,
    _height: u32,
    _pitch: u32,
    _photo: u64,
) -> u32 {
    unsafe { Mem::of(env) }.set_u32(size, 0);
    0
}

/// Hook for `CNetworkSession::CanEnterMultiplayer()`: with `-lan` the
/// engine counts the ROS credentials as valid and then waits for Rockstar's
/// cloud tunables, which never come. The other conditions stand: the network
/// memory free (the store's data, see `populate_store_data`; no save
/// queued, which the engine asks only of story saves) and the session idle. Then the lobby's players are waited for
/// (`P2p::may_enter`).
pub(crate) unsafe extern "C" fn can_enter_multiplayer(env: *mut Env, session: u64) -> u32 {
    const STATE: u64 = 99916;
    const STORE: u64 = 29309000; // populated, in progress
    const SAVE_QUEUE: u64 = 23275800;
    let m = unsafe { Mem::of(env) };
    let can = unsafe { orig::can_enter_multiplayer(env, session) } != 0 || {
        let store_ready = m.u8(STORE) & 1 != 0 && m.u8(STORE + 1) == 0;
        store_ready && m.u32(SAVE_QUEUE) == 0 && m.u32(session + STATE) == 0
    };
    (can && crate::p2p::get().is_none_or(|p| p.may_enter())) as u32
}

/// Changes to the game's scripts (bytecode), made as each program loads:
/// (script, code offset, the bytes there, the bytes put instead). A patch
/// whose bytes differ is left out and logged.
const SCRIPT_PATCHES: &[(&str, u32, &[u8], &[u8])] = &[
    // g_bFM_Rank_Unlocks_ON = FALSE: GTA Online's activities (golf, tennis,
    // darts, arm wrestling, the flight school, survivals...) are open from
    // the start instead of at a rank and after a contact's call, as the
    // game's own scripts allow; with Rockstar's jobs gone, ranks come
    // slowly here.
    ("MP_FM_registration", 194, &[OP_PUSH_1, 99, 72, 7, 28], &[OP_PUSH_0, 99, 72, 7, 28]),
    // CAN_ENTER_MP_AFTER_VALID_CHARACTER_CREATOR_FLOW: with no character yet,
    // the straight-into-freemode start (LOBBY_AUTO_MULTIPLAYER_FREEMODE)
    // must not keep the player from the character creator.
    ("MainTransition", 2510677, &[0x2c, 0x01, 0x03, 0xab], &[OP_PUSH_0, OP_NOP, OP_NOP, OP_NOP]),
    // SHOULD_RUN_ROCKSTAR_WELCOME_SCREEN, -sc_noOnlinePolicy: the welcome
    // screen (Rockstar's policies) is skipped, but it is what takes the
    // loading screen down, and the pause menu (the character creator) does
    // not run under one. Its debug print becomes SHUTDOWN_LOADING_SCREEN.
    (
        "MainTransition",
        2504003,
        &[0x73, 0x64, 0x7c, 0xde, 0x12, 0x66, 0x2c, 0x08, 0x03, 0x79],
        &[0x2c, 0x00, 0x00, 0xf9, OP_NOP, OP_NOP, OP_NOP, OP_NOP, OP_NOP, OP_NOP],
    ),
];
const OP_NOP: u8 = 0;
const OP_PUSH_0: u8 = 113;
const OP_PUSH_1: u8 = 114;
/// Script code is kept in pages of this size.
const SCRIPT_PAGE: u32 = 0x4000;

/// Hook for `rage::scrProgram::scrProgram(rage::datResource&)`: a script
/// program has loaded; the patches for it go in.
pub(crate) unsafe extern "C" fn script_program_loaded(env: *mut Env, this: u64, rsc: u64) -> u64 {
    let program = unsafe { orig::script_program_loaded(env, this, rsc) };
    let m = unsafe { Mem::of(env) };
    let name = m.str(m.u64(this + 96));
    let (pages, size) = (m.u64(this + 16), m.u32(this + 28));
    for &(script, at, old, new) in SCRIPT_PATCHES.iter().filter(|p| p.0 == name) {
        if at + old.len() as u32 > size || at % SCRIPT_PAGE + old.len() as u32 > SCRIPT_PAGE {
            log::warn!("online: script patch {script}+{at} is out of the code");
            continue;
        }
        let code = m.u64(pages + u64::from(at / SCRIPT_PAGE) * 8) + u64::from(at % SCRIPT_PAGE);
        let bytes = m.bytes_mut(code, old.len());
        if bytes != old {
            log::warn!("online: script patch {script}+{at} skipped: found {bytes:02x?}");
            continue;
        }
        bytes.copy_from_slice(new);
        log::info!("online: script patch {script}+{at}");
    }
    program
}

/// gameconfig.xml: the scripts are a later version than the game data, and
/// they register more broadcast data blocks than it provides; the engine
/// quits when a script finds none free of its size or larger ("No local
/// player broadcast data handler", "No host broadcast data handler").
///
/// - Player blocks: freemode alone fills the larger sizes, and with the
///   shops and ambient scripts around the player all 31 were in use (an
///   iPhone log listed them), so the two largest sizes and the three
///   smallest get more instances.
/// - Host blocks: freemode's own arrays (GSBD_*) hold most of the large
///   ones, so a job's controller has none left for its arrays: a 1v1
///   deathmatch's 10760-byte array found every handler of 15008 bytes or
///   more taken, one of them by freemode's 4584-byte GSBD_FM_events for
///   want of an 8000-byte one. The 8000-byte handlers go from 4 to 6 and
///   the 15008-byte ones from 2 to 3 (+31 KB). They come out of the network
///   heap, 13 MiB in all and nearly full once a session runs: doubling the
///   three largest sizes (+214 KB) left it short as two players met.
///
/// A smaller block takes a larger one when its size has none left. Each
/// longer number costs a tab of its line's indentation, so the file keeps
/// its length.
pub(crate) fn more_broadcast_data(text: String) -> Option<String> {
    let text = more_instances(text, "HostBroadcastData", &[(7500, 3), (4000, 6)])?;
    more_instances(text, "PlayerBroadcastData", &[(3600, 3), (2500, 4), (150, 12), (80, 12), (40, 16)])
}

/// `text` with the `<NumInstances>` of block `name`'s items of these sizes
/// set, the text's length kept.
fn more_instances(text: String, name: &str, sizes: &[(u32, u32)]) -> Option<String> {
    const FIELD: &str = "<NumInstances value=\"";
    let start = text.find(&format!("<{name}>"))?;
    let end = start + text[start..].find(&format!("</{name}>"))?;
    let mut block = text[start..end].to_string();
    for &(size, instances) in sizes {
        let at = block.find(&format!("<SizeOfData value=\"{size}\"/>"))?;
        let field = at + block[at..].find(FIELD)?;
        let digits = field + FIELD.len()..field + FIELD.len() + block[field + FIELD.len()..].find('"')?;
        let new = instances.to_string();
        let longer = new.len().checked_sub(digits.len())?;
        if !block[..field].ends_with(&"\t".repeat(longer)) {
            return None;
        }
        block.replace_range(digits, &new);
        block.drain(field - longer..field);
    }
    Some(format!("{}{}{}", &text[..start], block, &text[end..]))
}

/// Hook for `rage::internal::GetGamerHandle(int, rlGamerHandle*)` (the index
/// is gone from the compiled function): a Social Club handle, { u64 id, u8
/// service = 3, u8 local index }.
pub(crate) unsafe extern "C" fn gamer_handle(env: *mut Env, handle: u64) -> u32 {
    let m = unsafe { Mem::of(env) };
    // All 16 bytes, as the engine's: invites send them whole to friends.
    m.bytes_mut(handle, 16).fill(0);
    m.set_u64(handle, gamer_id());
    m.set_u8(handle + 8, 3);
    1
}

/// Hook for `rage::internal::GetOfflineName(int, char*, unsigned int)`.
pub(crate) unsafe extern "C" fn offline_name(env: *mut Env, name: u64) -> u32 {
    unsafe { Mem::of(env) }.write_str(name, &gamer_name(), 17);
    1
}

/// `rlNetworkMode`s: Rockstar's network, the LAN (the lobby, vnet.rs).
const NETWORK_ONLINE: u32 = 1;
const NETWORK_LAN: u32 = 2;

/// Rockstar's network is the lobby's LAN here: the transition sessions
/// that jobs are played in are hosted and joined online whatever the
/// game's mode (`CNetworkSession::HostTransition`).
fn on_lan(mode: u32) -> u32 {
    if mode == NETWORK_ONLINE { NETWORK_LAN } else { mode }
}

/// Hooks for `rage::snSession::Host(int, rlNetworkMode, ...)` and
/// `Join(int, rlSessionInfo const&, rlNetworkMode, ...)`: on the LAN.
#[allow(clippy::too_many_arguments)]
pub(crate) unsafe extern "C" fn session_host(
    env: *mut Env,
    session: u64,
    gamer: u32,
    mode: u32,
    public_slots: u32,
    private_slots: u32,
    attributes: u64,
    flags: u32,
    data: u64,
    size: u32,
    status: u64,
) -> u32 {
    let mode = on_lan(mode);
    unsafe { orig::session_host(env, session, gamer, mode, public_slots, private_slots, attributes, flags, data, size, status) }
}

#[allow(clippy::too_many_arguments)]
pub(crate) unsafe extern "C" fn session_join(
    env: *mut Env,
    session: u64,
    gamer: u32,
    info: u64,
    mode: u32,
    slot: u32,
    flags: u32,
    data: u64,
    size: u32,
    group: u64,
    group_size: u32,
    status: u64,
) -> u32 {
    let mode = on_lan(mode);
    unsafe { orig::session_join(env, session, gamer, info, mode, slot, flags, data, size, group, group_size, status) }
}

/// Hooks for `rage::rlSessionFinder::Find(int, rlNetworkMode, ...)` and
/// the two `rlSessionManager::QueryDetail(rlNetworkMode, ...)`: finding
/// and asking about sessions (a job's, joined or looked for), on the LAN.
#[allow(clippy::too_many_arguments)]
pub(crate) unsafe extern "C" fn session_find(
    env: *mut Env,
    gamer: u32,
    mode: u32,
    a: u32,
    b: u32,
    filter: u64,
    details: u64,
    c: u32,
    found: u64,
    status: u64,
) -> u32 {
    unsafe { orig::session_find(env, gamer, on_lan(mode), a, b, filter, details, c, found, status) }
}

#[allow(clippy::too_many_arguments)]
pub(crate) unsafe extern "C" fn session_query(
    env: *mut Env,
    mode: u32,
    a: u32,
    b: u32,
    c: u32,
    d: u32,
    e: u32,
    infos: u64,
    count: u32,
    gamer: u64,
    details: u64,
    found: u64,
    status: u64,
) -> u32 {
    unsafe { orig::session_query(env, on_lan(mode), a, b, c, d, e, infos, count, gamer, details, found, status) }
}

pub(crate) unsafe extern "C" fn session_query_any(env: *mut Env, mode: u32, a: u32, b: u64, c: u64, d: u64, e: u64) -> u32 {
    unsafe { orig::session_query_any(env, on_lan(mode), a, b, c, d, e) }
}

/// This module's hooks, online.
pub(crate) fn hooks(hooks: &mut glue5_game::Hooks) {
    hooks.session_host = Some(session_host);
    hooks.session_join = Some(session_join);
    hooks.session_find = Some(session_find);
    hooks.session_query = Some(session_query);
    hooks.session_query_any = Some(session_query_any);
    hooks.rl_gamer_handle = Some(gamer_handle);
    hooks.rl_offline_name = Some(offline_name);
    hooks.live_manager_init = Some(live_manager_init);
    hooks.live_manager_update = Some(live_manager_update);
    hooks.event_network_add = Some(event_network_add);
    hooks.host_aim_preference = Some(host_aim_preference);
    hooks.session_host_config = Some(session_host_config);
    hooks.freeroam_matchmaking = Some(freeroam_matchmaking);
    hooks.friend_matchmaking = Some(friend_matchmaking);
    hooks.bail_on_multiplayer = Some(bail_on_multiplayer);
    hooks.rl_pc_initialized = Some(rl_pc_initialized);
    hooks.ros_privilege = Some(ros_privilege);
    hooks.ros_privilege_end_date = Some(ros_privilege_end_date);
    hooks.profile_setting_int = Some(profile_setting_int);
    hooks.bad_pack_order = Some(bad_pack_order);
    hooks.savemigration_mp_enabled = Some(savemigration_mp_enabled);
    hooks.profanity_status = Some(profanity_status);
    hooks.profanity_pending = Some(profanity_pending);
    hooks.profanity_valid = Some(profanity_valid);
    hooks.social_club_account = Some(social_club_account);
    hooks.online_policies_current = Some(online_policies_current);
    hooks.match_history_id = Some(match_history_id);
    hooks.script_program_loaded = Some(script_program_loaded);
    hooks.presence_online = Some(presence_online);
    hooks.local_player_online = Some(local_player_online);
    hooks.ros_credentials_held = Some(ros_credentials_held);
    hooks.switch_to_multiplayer = Some(switch_to_multiplayer);
    hooks.return_to_story = Some(return_to_story);
    hooks.refresh_privileges = Some(refresh_privileges);
    hooks.can_enter_multiplayer = Some(can_enter_multiplayer);
    hooks.populate_store_data = Some(populate_store_data);
    hooks.photo_to_jpeg = Some(photo_to_jpeg);
    hooks.can_access_network_features = Some(can_access_network_features);
}

#[test]
fn broadcast_data_keeps_the_length() {
    let item = |size: u32, n: u32| {
        format!(
            "\t\t<Item>\r\n\t\t\t<SizeOfData value=\"{size}\"/>\r\n\t\t\t<NumInstances value=\"{n}\"/>\r\n\t\t</Item>\r\n"
        )
    };
    let sizes = [(3600, 1), (2500, 2), (150, 4), (80, 4), (40, 8)];
    let host = [(35000, 2), (11000, 2), (7500, 2), (4000, 4)];
    let text = format!(
        "<HostBroadcastData>\r\n{}</HostBroadcastData>\r\n<PlayerBroadcastData>\r\n{}</PlayerBroadcastData>\r\n",
        host.map(|(s, n)| item(s, n)).concat(),
        sizes.map(|(s, n)| item(s, n)).concat()
    );
    let patched = more_broadcast_data(text.clone()).unwrap();
    assert_eq!(patched.len(), text.len());
    assert!(patched.contains(&item(35000, 2)));
    assert!(patched.contains("<SizeOfData value=\"7500\"/>\r\n\t\t\t<NumInstances value=\"3\"/>"));
    assert!(patched.contains("<SizeOfData value=\"4000\"/>\r\n\t\t\t<NumInstances value=\"6\"/>"));
    assert!(patched.contains("<SizeOfData value=\"3600\"/>\r\n\t\t\t<NumInstances value=\"3\"/>"));
    assert!(patched.contains("<SizeOfData value=\"150\"/>\r\n\t\t<NumInstances value=\"12\"/>"));
    assert!(patched.contains("<SizeOfData value=\"40\"/>\r\n\t\t<NumInstances value=\"16\"/>"));
}
