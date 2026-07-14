// Legacy module: a field/method skeleton of C++ CBaseGame, kept as a porting reference;
// superseded by game::actor (GameActor).
#![allow(unused_variables)]
#![allow(dead_code)]

use std::cell::RefCell;
use std::collections::{HashSet, VecDeque};
use std::net::{SocketAddrV4, TcpListener};
use std::rc::Weak;
use std::sync::Arc;

use crate::core::gamehost::GameHost;
use crate::core::GameMap;
use crate::core::gameplayer::GamePlayer;
use crate::core::gameprotocol::{IncomingAction, IncomingChatPlayer, IncomingJoinPlayer, IncomingMapSize};
use crate::core::gameslot::GameSlot;

#[derive(Debug)]
pub struct GameBase {
    game_host: RefCell<Weak<GameHost>>,
    /// listening socket
    listener: Arc<TcpListener>,
    /// vector of slots
    slots: Vec<GameSlot>,
    /// vector of players
    players: Vec<GamePlayer>,
    ///score_checks: Vec<Box<CallableScoreCheck>>,  /// vector of score checks
    /// queue of actions to be sent
    actions: VecDeque<IncomingAction>,
    /// vector of player names with reserved slots (from the !hold command)
    reserved: Vec<String>,
    /// set of player names to NOT print ban messages for when joining because they've already been printed
    ignored_names: HashSet<String>,
    /// set of IP addresses to blacklist from joining (todo: convert to u32 for efficiency)
    ip_blacklist: HashSet<String>,
    /// map data
    map: GameMap,
    /// set to true and this class will be deleted next update
    exiting: bool,
    /// if we're currently saving game data to the database
    saving: bool,
    /// the port to host games on
    host_port: u16,
    /// game state, public or private
    game_state: u8,
    /// virtual host's PID
    virtual_host_pid: u8,
    /// the fake player's PID (if present)
    fake_player_pid: u8,
    gproxy_empty_actions: u8,
    /// game name
    game_name: String,
    /// last game name (the previous game name before it was rehosted)
    last_game_name: String,
    /// virtual host's name
    virtual_host_name: String,
    /// name of the player who owns this game (should be considered an admin)
    owner_name: String,
    /// name of the player who created this game
    creator_name: String,
    /// battle.net server the player who created this game was on
    creator_server: String,
    /// a message to be sent every announce_interval seconds
    announce_message: String,
    /// the stat string when the game started (used when saving replays)
    stat_string: String,
    /// the player to be kicked with the currently running kick vote
    kick_vote_player: String,
    /// the "HostBot Command Library" command string, used to pass a limited amount of data to specially designed maps
    hcl_command_string: String,
    /// the random seed sent to the Warcraft III clients
    random_seed: u32,
    /// a unique game number
    pub host_counter: u32,
    /// random entry key for LAN, used to prove that a player is actually joining from LAN
    entry_key: u32,
    /// the number of ms to wait between sending action packets (we queue any received during this time)
    latency: u32,
    /// the maximum number of packets a player can fall out of sync before starting the lag screen
    sync_limit: u32,
    /// the number of actions sent so far (for determining if anyone is lagging)
    sync_counter: u32,
    /// ingame ticks
    game_ticks: u32,
    /// GetTime when the game was created
    creation_time: u32,
    /// GetTime when the last ping was sent
    last_ping_time: u32,
    /// GetTime when the last game refresh was sent
    last_refresh_time: u32,
    /// GetTicks when the last map download cycle was performed
    last_download_ticks: u32,
    /// # of map bytes downloaded in the last second
    download_counter: u32,
    /// GetTicks when the download counter was last reset
    last_download_counter_reset_ticks: u32,
    /// GetTime when the last announce message was sent
    last_announce_time: u32,
    /// how many seconds to wait between sending the announce_message
    announce_interval: u32,
    /// the last time we tried to auto start the game
    last_auto_start_time: u32,
    /// auto start the game when there are this many players or more
    auto_start_players: u32,
    /// GetTicks when the last countdown message was sent
    last_countdown_ticks: u32,
    countdown_counter: u32,
    /// the countdown is finished when this reaches zero
    started_loading_ticks: u32,
    /// GetTicks when the game started loading
    start_players: u32,
    /// number of players when the game started
    last_lag_screen_reset_time: u32,
    /// GetTime when the "lag" screen was last reset
    last_action_sent_ticks: u32,
    /// GetTicks when the last action packet was sent
    last_action_late_by: u32,
    /// the number of ticks we were late sending the last action packet by
    started_lagging_time: u32,
    /// GetTime when the last lag screen started
    last_lag_screen_time: u32,
    /// GetTime when the last lag screen was active (continuously updated)
    last_reserved_seen: u32,
    /// GetTime when the last reserved player was seen in the lobby
    started_kick_vote_time: u32,
    /// GetTime when the kick vote was started
    started_vote_start_time: u32,
    /// GetTime when the votestart was started
    game_over_time: u32,
    /// GetTime when the game was over
    last_player_leave_ticks: u32,
    /// GetTicks when the most recent player left the game
    minimum_score: f64,
    /// the minimum allowed score for matchmaking mode
    maximum_score: f64,
    /// the maximum allowed score for matchmaking mode
    slot_info_changed: bool,
    /// if the slot info has changed and hasn't been sent to the players yet (optimization)
    locked: bool,
    /// if the game owner is the only one allowed to run game commands or not
    refresh_messages: bool,
    /// if we should display "game refreshed..." messages or not
    refresh_error: bool,
    /// if there was an error refreshing the game
    refresh_rehosted: bool,
    /// if we just rehosted and are waiting for confirmation that it was successful
    mute_all: bool,
    /// if we should stop forwarding ingame chat messages targeted for all players or not
    mute_lobby: bool,
    /// if we should stop forwarding lobby chat messages
    countdown_started: bool,
    /// if the game start countdown has started or not
    start_vote_started: bool,
    pub game_loading: bool,
    /// if the game is currently loading or not
    pub game_loaded: bool,
    /// if the game has loaded or not
    load_in_game: bool,
    /// if the load-in-game feature is enabled or not
    lagging: bool,
    /// if the lag screen is active or not
    auto_save: bool,
    /// if we should auto save the game before someone disconnects
    matchmaking: bool,
    /// if matchmaking mode is enabled
    local_admin_messages: bool,
    /// if local admin messages should be relayed or not
    do_delete: i32,
    /// notifies thread to exit
    last_reconnect_handle_time: u32,
    /// last time we tried to handle GProxy reconnects

    /// vector of strings we should announce to the current game
    do_say_games: Vec<String>,

    // mutex for the above vector
    // boost::mutex m_SayGamesMutex;

    /// vector of spoof add function call structures
    do_spoof_add: Vec<QueuedSpoofAdd>,
    // boost::mutex m_SpoofAddMutex;
}

impl GameBase {
    /// [superseded] this constructor originally read GameHost config in reverse via `RefCell<Weak<GameHost>>`,
    /// which does not compile in Rust (Weak needs upgrade, and it forms a C++-style back-pointer graph).
    /// GameActor took over: the required config (bind_address, reconnect_wait_time, ...)
    /// is passed in as `GameConfig` construction parameters, and the listener is held centrally by BotCore which routes connections.
    /// For the original logic see C++ game_base.cpp CBaseGame::CBaseGame.
    pub fn new(
        game_host: RefCell<Weak<GameHost>>,
        map: GameMap,
        host_port: u16,
        game_state: u8,
        game_name: String,
        owner_name: String,
        creator_name: String,
        creator_server: String,
    ) -> Self {
        // Kept as a reference for the original field initialization:
        // - listener: originally each game bound host_port on its own
        // - gproxy_empty_actions: reconnect_wait_time in minutes - 1, capped at 9 (C++ comment:
        //   wait time of 1 minute = 0 empty actions, 2 minutes = 1, ...)
        let _ = (game_host, map, host_port, game_state, game_name,
                 owner_name, creator_name, creator_server);
        todo!("legacy stub: superseded by GameActor");
    }

    /// Legacy field-initialization content (for porting reference, do not call)
    #[allow(dead_code, unreachable_code, unused_variables)]
    fn legacy_new_reference(
        game_host: RefCell<Weak<GameHost>>,
        map: GameMap,
        host_port: u16,
        game_state: u8,
        game_name: String,
        owner_name: String,
        creator_name: String,
        creator_server: String,
    ) -> Self {
        let listener: Arc<TcpListener> = todo!();
        let gproxy_empty_actions: u8 = 0;

        Self {
            game_host,
            listener,
            slots: map.slots.clone(),
            players: vec![],
            actions: Default::default(),
            reserved: vec![],
            ignored_names: Default::default(),
            ip_blacklist: Default::default(),
            map,
            exiting: false,
            saving: false,
            host_port,
            game_state,
            virtual_host_pid: 0,
            fake_player_pid: 0,
            gproxy_empty_actions,
            game_name,
            last_game_name: "".to_string(),
            virtual_host_name: "".to_string(),
            owner_name,
            creator_name,
            creator_server,
            announce_message: "".to_string(),
            stat_string: "".to_string(),
            kick_vote_player: "".to_string(),
            hcl_command_string: "".to_string(),
            random_seed: 0,
            host_counter: 0,
            entry_key: 0,
            latency: 0,
            sync_limit: 0,
            sync_counter: 0,
            game_ticks: 0,
            creation_time: 0,
            last_ping_time: 0,
            last_refresh_time: 0,
            last_download_ticks: 0,
            download_counter: 0,
            last_download_counter_reset_ticks: 0,
            last_announce_time: 0,
            announce_interval: 0,
            last_auto_start_time: 0,
            auto_start_players: 0,
            last_countdown_ticks: 0,
            countdown_counter: 0,
            started_loading_ticks: 0,
            start_players: 0,
            last_lag_screen_reset_time: 0,
            last_action_sent_ticks: 0,
            last_action_late_by: 0,
            started_lagging_time: 0,
            last_lag_screen_time: 0,
            last_reserved_seen: 0,
            started_kick_vote_time: 0,
            started_vote_start_time: 0,
            game_over_time: 0,
            last_player_leave_ticks: 0,
            minimum_score: 0.0,
            maximum_score: 0.0,
            slot_info_changed: false,
            locked: false,
            refresh_messages: false,
            refresh_error: false,
            refresh_rehosted: false,
            mute_all: false,
            mute_lobby: false,
            countdown_started: false,
            start_vote_started: false,
            game_loading: false,
            game_loaded: false,
            load_in_game: false,
            lagging: false,
            auto_save: false,
            matchmaking: false,
            local_admin_messages: false,
            do_delete: 0,
            last_reconnect_handle_time: 0,
            do_say_games: vec![],
            do_spoof_add: vec![],
        }
    }

    pub fn start_loop(&self) {}
    pub fn do_delete(&self) {}
    pub fn ready_delete(&self) {}

    /// Function signatures in Rust style
    pub fn get_next_timed_action_ticks(&self) -> u32 { 0 }
    pub fn get_slots_occupied(&self) -> u32 { 0 }
    pub fn get_slots_open(&self) -> u32 { 0 }
    pub fn get_num_players(&self) -> u32 { 0 }
    pub fn get_num_human_players(&self) -> u32 { 0 }
    pub fn get_description(&self) -> String { String::new() }

    pub fn set_announce(&self, interval: u32, message: String) {}

    /// processing functions
    pub fn set_fd(&self, fd: &mut dyn std::io::Read, send_fd: &mut dyn std::io::Write, nfds: &mut i32) -> u32 { 0 }
    pub fn update(&self, fd: &mut dyn std::io::Read, send_fd: &mut dyn std::io::Write) -> bool { false }
    pub fn update_post(&self, send_fd: &mut dyn std::io::Write) {}

    /// generic functions to send packets to players
    pub fn send(&self, player: &GamePlayer, data: &[u8]) {}
    pub fn send_to_pid(&self, pid: u8, data: &[u8]) {}
    pub fn send_to_pids(&self, pids: &[u8], data: &[u8]) {}
    pub fn send_all(&self, data: &[u8]) {}

    /// functions to send packets to players
    pub fn send_chat(&self, from_pid: u8, player: &GamePlayer, message: String) {}
    pub fn send_chat_to_pid(&self, from_pid: u8, to_pid: u8, message: String) {}
    pub fn send_chat_to_player(&self, player: &GamePlayer, message: String) {}
    pub fn send_chat_to_pid_only(&self, to_pid: u8, message: String) {}
    pub fn send_all_chat(&self, from_pid: u8, message: String) {}
    pub fn send_all_chat_message(&self, message: String) {}
    pub fn send_local_admin_chat(&self, message: String) {}
    pub fn send_all_slot_info(&self) {}
    pub fn send_virtual_host_player_info(&self, player: &GamePlayer) {}
    pub fn send_fake_player_info(&self, player: &GamePlayer) {}
    pub fn send_all_actions(&self) {}
    pub fn send_welcome_message(&self, player: &GamePlayer) {}
    pub fn send_end_message(&self) {}

    /// events
    /// note: these are only called while iterating through the potentials or players vectors
    /// therefore you can't modify those vectors and must use the player's delete_me member to flag for deletion
    pub fn event_player_deleted(&self, player: &GamePlayer) {}
    pub fn event_player_disconnect_timed_out(&self, player: &GamePlayer) {}
    pub fn event_player_disconnect_player_error(&self, player: &GamePlayer) {}
    pub fn event_player_disconnect_socket_error(&self, player: &GamePlayer) {}
    pub fn event_player_disconnect_connection_closed(&self, player: &GamePlayer) {}
    pub fn event_player_joined(&self, join_player: &IncomingJoinPlayer) {}
    pub fn event_player_joined_with_score(&self, potential: &GamePlayer, join_player: &IncomingJoinPlayer, score: f64) {}
    pub fn event_player_left(&self, player: &GamePlayer, reason: u32) {}
    pub fn event_player_loaded(&self, player: &GamePlayer) {}
    pub fn event_player_action(&self, player: &GamePlayer, action: &IncomingAction) -> bool { false }
    pub fn event_player_keep_alive(&self, player: &GamePlayer, check_sum: u32) {}
    pub fn event_player_chat_to_host(&self, player: &GamePlayer, chat_player: &IncomingChatPlayer) {}
    pub fn event_player_bot_command(&self, player: &GamePlayer, command: String, payload: String) -> bool { false }
    pub fn event_player_change_team(&self, player: &GamePlayer, team: u8) {}
    pub fn event_player_change_colour(&self, player: &GamePlayer, colour: u8) {}
    pub fn event_player_change_race(&self, player: &GamePlayer, race: u8) {}
    pub fn event_player_change_handicap(&self, player: &GamePlayer, handicap: u8) {}
    pub fn event_player_drop_request(&self, player: &GamePlayer) {}
    pub fn event_player_map_size(&self, player: &GamePlayer, map_size: &IncomingMapSize) {}
    pub fn event_player_pong_to_host(&self, player: &GamePlayer, pong: u32) {}

    /// these events are called outside of any iterations
    pub fn event_game_refreshed(&self, server: String) {}
    pub fn event_game_started(&self) {}
    pub fn event_game_loaded(&self) {}

    /// other functions
    pub fn get_sid_from_pid(&self, pid: u8) -> u8 { 0 }
    pub fn get_player_from_pid(&self, pid: u8) -> Option<&GamePlayer> { None }
    pub fn get_player_from_sid(&self, sid: u8) -> Option<&GamePlayer> { None }
    pub fn get_player_from_name(&self, name: String, sensitive: bool) -> Option<&GamePlayer> { None }
    pub fn get_player_from_name_partial(&self, name: String, player: &mut Option<&GamePlayer>) -> u32 { 0 }
    pub fn get_player_from_colour(&self, colour: u8) -> Option<&GamePlayer> { None }
    pub fn get_new_pid(&self) -> u8 { 0 }
    pub fn get_new_colour(&self) -> u8 { 0 }
    pub fn get_pids(&self) -> Vec<u8> { vec![] }
    pub fn get_pids_exclude(&self, exclude_pid: u8) -> Vec<u8> { vec![] }
    pub fn get_host_pid(&self) -> u8 { 0 }
    pub fn get_empty_slot(&self, reserved: bool) -> u8 { 0 }
    pub fn get_empty_slot_by_team(&self, team: u8, pid: u8) -> u8 { 0 }
    pub fn swap_slots(&self, sid1: u8, sid2: u8) {}
    pub fn open_slot(&self, sid: u8, kick: bool) {}
    pub fn close_slot(&self, sid: u8, kick: bool) {}
    pub fn computer_slot(&self, sid: u8, skill: u8, kick: bool) {}
    pub fn colour_slot(&self, sid: u8, colour: u8) {}
    pub fn open_all_slots(&self) {}
    pub fn close_all_slots(&self) {}
    pub fn shuffle_slots(&self) {}
    pub fn balance_slots_recursive(&self, player_ids: Vec<u8>, team_sizes: &mut [u8], player_scores: &mut [f64], start_team: u8) -> Vec<u8> { vec![] }
    pub fn balance_slots(&self) {}
    pub fn add_to_spoofed(&self, server: SocketAddrV4, name: &str, send_message: bool) {}
    pub fn add_to_reserved(&self, name: String) {}
    pub fn is_owner(&self, name: String) -> bool { false }
    pub fn is_reserved(&self, name: String) -> bool { false }
    pub fn is_downloading(&self) -> bool { false }
    pub fn is_game_data_saved(&self) -> bool { false }
    pub fn save_game_data(&self) {}
    pub fn start_countdown(&self, force: bool, interval: i32) {}
    pub fn start_countdown_auto(&self, require_spoof_checks: bool) {}
    pub fn stop_players(&self, reason: String) {}
    pub fn stop_laggers(&self, reason: String) {}
    pub fn create_virtual_host(&self) {}
    pub fn delete_virtual_host(&self) {}
    pub fn create_fake_player(&self) {}
    pub fn delete_fake_player(&self) {}
}

#[derive(Debug)]
struct QueuedSpoofAdd {
    server: String,
    name: String,
    send_message: bool,
    ///empty if no failure
    fail_message: String,
}