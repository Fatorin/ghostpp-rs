// Legacy module kept as a C++ porting reference; superseded by bot::BotCore + bot::BotConfig.
#![allow(dead_code)]

use std::cell::RefCell;
use std::collections::HashMap;
use std::io;
use std::net::{Ipv4Addr, SocketAddrV4, TcpListener, TcpStream, UdpSocket};
use std::rc::Rc;
use std::str::FromStr;
use std::sync::Arc;

use config::Config;

use crate::core::{GameBase, GameMap};
use crate::core::bnet::BNet;
use crate::util;
use crate::util::{get_u16_from_config, get_u32_from_config, get_u8_from_config};

#[derive(Debug)]
pub struct GameHost {
    // a UDP socket for sending broadcasts and other junk (used with !sendlan)
    udp_socket: UdpSocket,

    // listening socket for GProxy++ reliable reconnects
    reconnect_socket: Option<TcpListener>,

    // vector of sockets attempting to reconnect (connected but not identified yet)
    reconnect_sockets: Vec<TcpStream>,

    // all our battle.net connections (there can be more than one)
    bnets: RefCell<HashMap<usize, Arc<BNet>>>,

    // this core is still in the lobby state
    pub current_game: RefCell<Option<Arc<GameBase>>>,

    // these games are in progress
    games: RefCell<HashMap<usize, Arc<GameBase>>>,

    // the threads for games in progress and stuff
    // boost::thread_group m_GameThreads;
    // boost::mutex m_GamesMutex;

    // database
    // CGHostDB *m_DB;
    // local database (for temporary data)
    // CGHostDB *m_DBLocal;

    // vector of orphaned callables waiting to die
    // vector<CBaseCallable *> m_Callables;
    // boost::mutex m_CallablesMutex;

    // vector of local IP addresses
    local_addresses: Vec<u8>,

    // the currently loaded map
    pub currently_map: Rc<GameMap>,

    // the map to use when autohosting
    pub auto_host_map: Rc<GameMap>,

    // set to true to force ghost to shutdown next update (used by SignalCatcher)
    exiting: bool,
    // set to true to force ghost to disconnect from all battle.net connections and wait for all games to finish before shutting down
    exiting_nice: bool,

    // set to false to prevent new games from being created
    enabled: bool,
    // GHost++ version string
    version: String,
    // the current host counter (a unique number to identify a core, incremented each time a core is created)
    host_counter: u32,
    // the base core name to auto host with
    pub auto_host_game_name: String,
    auto_host_owner: String,
    auto_host_server: String,
    // maximum number of games to auto host
    auto_host_maximum_games: u32,
    // when using auto hosting auto start the core when this many players have joined
    auto_host_auto_start_players: u8,
    // GetTime when the last auto host was attempted
    last_auto_host_time: u64,

    // if all games finished (used when exiting nicely)
    all_games_finished: bool,
    // GetTime when all games finished (used when exiting nicely)
    all_games_finished_time: u32,

    // config value: Warcraft 3 path
    warcraft3path: String,
    // config value: TFT enabled or not
    is_tft: bool,
    // config value: the address to host games on
    pub bind_address: String,
    // config value: the port to host games on
    host_port: u16,
    // config value: the port to listen for GProxy++ reliable reconnects on
    reconnect_port: u16,
    // config value: the maximum number of minutes to wait for a GProxy++ reliable reconnect
    pub reconnect_wait_time: u32,
    // config value: maximum number of games in progress
    max_games: u32,
    // config value: the command trigger inside games
    command_trigger: String,
    // config value: map cfg path
    map_cfgpath: String,
    // config value: map path
    map_path: String,
    // config value: virtual host name
    virtual_host_name: String,
    // config value: hide IP addresses from players
    hide_ipaddresses: bool,
    // config value: check for multiple IP address usage
    check_multiple_ipusage: bool,
    // config value: do automatic spoof checks or not
    spoof_checks: u8,
    // config value: require spoof checks or not
    require_spoof_checks: bool,
    // config value: consider admins to be reserved players or not
    reserve_admins: bool,
    // config value: display refresh messages or not (by default)
    refresh_messages: bool,
    // config value: auto lock games when the owner is present
    auto_lock: bool,
    // config value: allow map downloads or not
    allow_downloads: u8,
    // config value: ping during map downloads or not
    ping_during_downloads: bool,
    // config value: maximum number of map downloaders at the same time
    max_downloaders: u8,
    // config value: maximum total map download speed in KB/sec
    max_download_speed: u32,
    // config value: use LC style pings (divide actual pings by two)
    lcpings: bool,
    // config value: auto kick players with ping higher than this
    auto_kick_ping: u16,
    // config value: ban method (ban by name/ip/both)
    ban_method: u8,
    // config value: IP blacklist file (ipblacklist.txt)
    ipblack_list_file: String,
    // config value: auto close the core lobby after this many minutes without any reserved players
    lobby_time_limit: u32,
    // config value: the latency (by default)
    latency: u32,
    // config value: the maximum number of packets a player can fall out of sync before starting the lag screen (by default)
    sync_limit: u32,
    // config value: default map (map.cfg)
    default_map: String,
    // config value: motd.txt
    motdfile: String,
    // config value: gameloaded.txt
    game_loaded_file: String,
    // config value: gameover.txt
    game_over_file: String,
    // config value: send local admin messages or not
    local_admin_messages: bool,
    // config value: LAN warcraft 3 version
    lanwar3version: u8,
    // config value: use Nagle's algorithm or not
    tcpno_delay: bool,
    // config value: the MapGameType overwrite (aka: refresh hack)
    map_game_type: u32,
    start_game_at_xplayers: u8,
    //vector<GProxyReconnector * > m_PendingReconnects;

    input_message: String,
}

impl GameHost {
    pub fn new(config: &Config) -> Result<Self, io::Error> {
        let bind_address = config.get_string("bot_bindaddress").unwrap_or(String::new());
        let host_port = get_u16_from_config(&config, "bot_hostport", 6112);
        let udp_broadcasttarget = config.get_string("udp_broadcasttarget").unwrap_or(String::new());
        let udp_addr = Ipv4Addr::from_str(&udp_broadcasttarget).unwrap_or(Ipv4Addr::UNSPECIFIED);
        let udp_socket_addr = SocketAddrV4::new(udp_addr, 0);
        let udp_socket = UdpSocket::bind(udp_socket_addr)?;

        let map = Rc::new(GameMap::new());

        Ok(Self {
            udp_socket,
            reconnect_socket: None,
            reconnect_sockets: vec![],
            bnets: RefCell::new(HashMap::new()),
            current_game: RefCell::new(None),
            games: RefCell::new(HashMap::new()),
            local_addresses: vec![],
            currently_map: Rc::clone(&map),
            auto_host_map: Rc::clone(&map),
            exiting: false,
            exiting_nice: false,
            enabled: true,
            version: String::from("1.0.0"),
            host_counter: 1,
            auto_host_game_name: config.get_string("autohost_gamename").unwrap_or(String::new()),
            auto_host_owner: config.get_string("autohost_owner").unwrap_or(String::new()),
            auto_host_server: String::new(),
            auto_host_maximum_games: get_u32_from_config(&config, "autohost_maxgames", 5),
            auto_host_auto_start_players: get_u8_from_config(&config, "autohost_startplayers", 5),
            last_auto_host_time: util::get_time(),
            all_games_finished: false,
            all_games_finished_time: 0,
            warcraft3path: config.get_string("bot_war3path").unwrap_or(String::from("C:\\Program Files\\Warcraft III\\")),
            is_tft: config.get_bool("bot_tft").unwrap_or(true),
            bind_address,
            host_port,
            reconnect_port: get_u16_from_config(&config, "bot_reconnectport", 6113),
            reconnect_wait_time: get_u32_from_config(&config, "bot_reconnectwaittime", 3),
            max_games: get_u32_from_config(&config, "bot_maxgames", 5),
            command_trigger: config.get_string("bot_commandtrigger").unwrap_or(String::from("!")),
            map_cfgpath: config.get_string("bot_mapcfgpath").unwrap_or(String::new()),
            map_path: config.get_string("bot_mappath").unwrap_or(String::new()),
            virtual_host_name: {
                let default_host_name = String::from("|cFF4080C0GHost");
                match config.get_string("bot_virtualhostname") {
                    Ok(name) => {
                        if name.len() > 15 {
                            default_host_name
                        } else {
                            name
                        }
                    }
                    Err(_) => default_host_name,
                }
            },
            hide_ipaddresses: config.get_bool("bot_hideipaddresses").unwrap_or(false),
            check_multiple_ipusage: config.get_bool("bot_checkmultipleipusage").unwrap_or(true),
            spoof_checks: get_u8_from_config(&config, "bot_spoofchecks", 2),
            require_spoof_checks: config.get_bool("bot_requirespoofchecks").unwrap_or(false),
            reserve_admins: config.get_bool("bot_reserveadmins").unwrap_or(true),
            refresh_messages: config.get_bool("bot_refreshmessages").unwrap_or(false),
            auto_lock: config.get_bool("bot_autolock").unwrap_or(false),
            allow_downloads: get_u8_from_config(&config, "bot_allowdownloads", 0),
            ping_during_downloads: config.get_bool("bot_pingduringdownloads").unwrap_or(false),
            max_downloaders: get_u8_from_config(&config, "bot_maxdownloaders", 3),
            max_download_speed: get_u32_from_config(&config, "bot_maxdownloadspeed", 100),
            lcpings: config.get_bool("bot_lcpings").unwrap_or(false),
            auto_kick_ping: get_u16_from_config(&config, "bot_autokickping", 400),
            ban_method: get_u8_from_config(&config, "bot_banmethod", 1),
            ipblack_list_file: config.get_string("bot_ipblacklistfile").unwrap_or(String::from("ipblacklist.txt")),
            lobby_time_limit: get_u32_from_config(&config, "bot_lobbytimelimit", 10),
            latency: get_u32_from_config(&config, "bot_latency", 100),
            sync_limit: get_u32_from_config(&config, "bot_synclimit", 50),
            default_map: config.get_string("bot_defaultmap").unwrap_or(String::from("map")),
            motdfile: config.get_string("bot_motdfile").unwrap_or(String::from("motd.txt")),
            game_loaded_file: config.get_string("bot_gameloadedfile").unwrap_or(String::from("gameloaded.txt")),
            game_over_file: config.get_string("bot_gameoverfile").unwrap_or(String::from("gameover.txt")),
            local_admin_messages: config.get_bool("bot_localadminmessages").unwrap_or(true),
            lanwar3version: get_u8_from_config(&config, "lan_war3version", 30),
            tcpno_delay: config.get_bool("tcp_nodelay").unwrap_or(false),
            map_game_type: get_u32_from_config(&config, "bot_mapgametype", 0),
            start_game_at_xplayers: get_u8_from_config(&config, "bot_gamenotstartuntilXplayers", 4),
            input_message: String::new(),
        })
    }

    pub fn load_map(&self, _config: &Config) {
    }

    /// [obsolete] the original 50ms select polling main loop.
    /// After the tokio migration, replaced by `bot::BotCore::run()`:
    /// each BnetActor / GameActor carries its own timer, so centralized update is no longer needed.
    /// This struct is kept as a reference for config fields only.
    pub fn update(&mut self) -> bool {
        false
    }

    pub fn event_bnet_game_refreshed(_bnet: &mut BNet) {
    }

    pub fn event_bnet_game_refresh_failed(_bnet: &mut BNet) {
    }

    pub fn event_game_deleted(_game: &mut GameBase) {
    }

    pub fn input_loop() {
    }

    /// [superseded] the game-hosting flow lives in BotCore:
    /// BotCore receives !pub/!priv → sends BnetCommand::CreateGame to all BnetActors (broadcast/whisper)
    /// → spawns GameActor → the Listener routes new connections to the lobby GameActor.
    /// The old implementation could not compile due to RefCell/Arc borrow conflicts; for the original logic see C++ ghost.cpp CGHost::CreateGame.
    pub fn create_game(
        &mut self,
        map: GameMap,
        _game_state: u8,
        game_name: String,
        _owner_name: String,
        _creator_name: String,
        _creator_server: &BNet,
        _whisper: bool,
    ) {
        if !self.enabled {
            return;
        }

        if game_name.len() > 31 {
            return;
        }

        if !map.is_valid {
            return;
        }

        todo!("legacy stub: superseded by BotCore + GameActor");
    }
}