// Hooks for the runtime's `friends/game.rs` (see ../hooks.rs).
&[
    // The friends list: the only reader of the platform's, filling the
    // friends manager's cache and the live manager's pages.
    ("friends_read", "rage::rlFriendsReader::Read(int, unsigned int, rage::rlFriend*, unsigned int, unsigned int*, unsigned int*, int, rage::netStatus*)"),
    ("friends_with", "rage::rlFriendsManager::IsFriendsWith(int, rage::rlGamerHandle const&)"),
    // The pause menu (its tabs, put together as it opens, and what it tells
    // its movie) and its lists of players and friends, which want Social
    // Club's connection.
    ("pause_menu_version", "CPauseMenu::SetCurrentMenuVersion(unsigned int, bool)"),
    ("pause_menu_update", "CPauseMenu::Update()"),
    ("player_list_update", "CPlayerListMenu::Update()"),
    ("player_list_input", "CPlayerListMenu::UpdateInput(int)"),
    ("player_list_populate", "CPlayerListMenu::Populate(MenuScreenId)"),
    // An empty list says "the internet connection has been lost": for the
    // friends lists, the warning gets the texts of no friends.
    ("player_list_empty", "CPlayerListMenu::HandleNoPlayers()"),
    ("menu_warning", "CMenuBase::SHOW_WARNING_MESSAGE_EX(PM_COLUMNS, char const*, char const*, int)"),
    ("social_club_connected", "SocialClubMgr::IsConnectedToSocialClub() const"),
    ("gamer_online", "rage::rlGamerInfo::IsOnline() const"),
    // The data behind the pause menu's lists of friends (their cards).
    ("card_data_init", "BasePlayerCardDataManager::Init(CPlayerCardXMLDataManager const&)"),
    // Crews (Rockstar's): the service, the friends' crews the list waits for.
    ("clan_service_ready", "rage::rlClan::IsServiceReady(int)"),
    ("clan_primary", "rage::rlClan::GetPrimaryClans(int, rage::rlGamerHandle const*, unsigned int, rage::rlClanMember*, unsigned int*, unsigned int*, rage::netStatus*)"),
    // Rockstar's stats of gamers, which a friend's card reads.
    ("profile_stats_read", "rage::rlProfileStats::ReadStatsByGamer(int, rage::rlGamerHandle const*, unsigned int, rage::rlProfileStatsReadResults*, rage::netStatus*)"),
    // Called to (re)load the list.
    ("friends_activate", "rage::rlFriendsManager::Activate(int)"),
    ("friends_deactivate", "rage::rlFriendsManager::Deactivate()"),
    // Presence events (invites, their answers, follow invites): sent to
    // gamers, received from them.
    ("presence_send", "CGamePresenceEventDispatcher::SendEventToGamers(rage::netGamePresenceEvent const&, rage::rlGamerHandle const*, int)"),
    ("presence_post", "bool rage::rlPresence::PostMessage<rage::rlScPresenceMessagePublish>(int, rage::rlGamerHandle const*, unsigned int, rage::rlScPresenceMessagePublish const&, unsigned int)"),
    ("presence_process", "CGamePresenceEventDispatcher::ProcessGameEvent(rage::rlGamerInfo const&, rage::rlScPresenceMessageSender const&, rage::RsonReader const&, char const*)"),
    ("rson_reader", "rage::RsonReader::RsonReader(char const*, unsigned int)"),
    // The platform's invites to a session (the pause menu's, a script's),
    // which travel between friends instead.
    ("session_invites", "CNetworkSession::SendInvites(rage::rlGamerHandle const*, unsigned int, char const*, char const*, rage::netStatus*)"),
    // "Send Friend Request" (the players list, NETWORK_ADD_FRIEND).
    ("add_friend", "CLiveManager::AddFriend(rage::rlGamerHandle const&, char const*)"),
    // Friends' sessions, which "Join Friends" and the public matchmaking
    // look for.
    ("presence_query", "rage::rlPresence::Query(int, char const*, char const*, int, int, char*, unsigned int, char**, unsigned int*, unsigned int*, rage::netStatus*)"),
    // Joining a friend: its session, asked for by its card's "Join"; an
    // invite accepted; joining from the host.
    ("session_by_gamer", "rage::rlPresenceQuery::GetSessionByGamerHandle(int, rage::rlGamerHandle*, unsigned int, rage::rlSessionQueryData*, unsigned int, unsigned int*, rage::netStatus*)"),
    ("invite_accept", "InviteMgr::AcceptInvite(rage::rlSessionInfo const&, rage::rlGamerInfo const&, rage::rlGamerHandle const&, unsigned int, unsigned int, int, char const*, int, int)"),
    ("join_request", "InviteMgr::HandleJoinRequest(rage::rlSessionInfo const&, rage::rlGamerInfo const&, rage::rlGamerHandle const&, unsigned int)"),
    ("invite_auto_confirm", "InviteMgr::AutoConfirmInvite()"),
    // Invites blocked by the scripts (on the way into freemode, in the
    // tutorial): a friend's session waits.
    ("invites_blocked", "InviteMgr::BlockInvitesFromScript(bool)"),
    // A session's info, as text for friends and back.
    ("session_info", "rage::snSession::GetSessionInfo() const"),
    ("session_info_text", "rage::rlSessionInfo::ToString(char*, unsigned int, unsigned int*) const"),
    ("session_info_parse", "rage::rlSessionInfo::FromString(char const*, unsigned int*, bool)"),
]
