//! Bot global configuration (config values are passed as constructor arguments, no back-pointers stored).
//! Mirrors the bot_* settings read by the C++ ghost.cpp CGHost constructor;
//! replaces the config fields that were mixed into state within `core::gamehost::GameHost`.

use config::Config;

use crate::util::{get_u16_from_config, get_u32_from_config, get_u8_from_config};

#[derive(Debug, Clone)]
pub struct BotConfig {
    /// bot_war3path
    pub war3_path: String,
    /// bot_tft
    pub tft: bool,
    /// bot_bindaddress
    pub bind_address: String,
    /// bot_hostport
    pub host_port: u16,
    /// bot_savereplays (auto-save a .w3g for each game)
    pub save_replays: bool,
    /// bot_replaypath (directory where replays are stored)
    pub replay_path: String,
    /// replay_war3version (version number in the replay header, defaults to 28)
    pub replay_war3_version: u32,
    /// replay_buildnumber (build in the replay header, 1.28 uses 6060)
    pub replay_build_number: u16,
    /// bot_reconnect (master switch for GProxy++ disconnect reconnection)
    pub reconnect: bool,
    /// bot_reconnectport (GProxy++)
    pub reconnect_port: u16,
    /// bot_reconnectwaittime (minutes)
    pub reconnect_wait_time: u32,
    /// bot_maxgames
    pub max_games: u32,
    /// bot_commandtrigger
    pub command_trigger: String,
    /// bot_mapcfgpath
    pub map_cfg_path: String,
    /// bot_mappath
    pub map_path: String,
    /// bot_virtualhostname (<= 15 bytes)
    pub virtual_host_name: String,
    /// bot_hideipaddresses
    pub hide_ip_addresses: bool,
    /// bot_checkmultipleipusage
    pub check_multiple_ip_usage: bool,
    /// bot_spoofchecks (0=no check, 1=required, 2=optional)
    pub spoof_checks: u8,
    /// bot_requirespoofchecks
    pub require_spoof_checks: bool,
    /// bot_reserveadmins
    pub reserve_admins: bool,
    /// bot_refreshmessages
    pub refresh_messages: bool,
    /// bot_autolock
    pub auto_lock: bool,
    /// bot_allowdownloads (0=disabled, 1=allowed, 2=conditional)
    pub allow_downloads: u8,
    /// bot_pingduringdownloads
    pub ping_during_downloads: bool,
    /// bot_maxdownloaders
    pub max_downloaders: u8,
    /// bot_maxdownloadspeed (KB/s)
    pub max_download_speed: u32,
    /// bot_lcpings
    pub lc_pings: bool,
    /// bot_autokickping
    pub auto_kick_ping: u16,
    /// bot_banmethod (1=name, 2=ip, 3=both)
    pub ban_method: u8,
    /// bot_ipblacklistfile
    pub ip_blacklist_file: String,
    /// bot_lobbytimelimit (minutes)
    pub lobby_time_limit: u32,
    /// bot_latency (ms, action send interval; the lag tolerance time window is fixed, auto-derived from latency, no sync_limit needed)
    pub latency: u32,
    /// bot_defaultmap
    pub default_map: String,
    /// bot_motdfile
    pub motd_file: String,
    /// bot_gameloadedfile
    pub game_loaded_file: String,
    /// bot_gameoverfile
    pub game_over_file: String,
    /// bot_localadminmessages
    pub local_admin_messages: bool,
    /// lan_war3version
    pub lan_war3_version: u8,
    /// tcp_nodelay
    pub tcp_no_delay: bool,
    /// bot_mapgametype(refresh hack)
    pub map_game_type: u32,
    /// bot_gamenotstartuntilXplayers
    pub start_game_at_x_players: u8,

    // --- autohost ---
    /// autohost_gamename
    pub auto_host_game_name: String,
    /// autohost_owner
    pub auto_host_owner: String,
    /// autohost_maxgames
    pub auto_host_maximum_games: u32,
    /// autohost_startplayers
    pub auto_host_auto_start_players: u8,

    /// udp_broadcasttarget
    pub udp_broadcast_target: String,
}

impl BotConfig {
    pub fn load(config: &Config) -> Self {
        let default_host_name = String::from("|cFF4080C0GHost");
        let virtual_host_name = match config.get_string("bot_virtualhostname") {
            Ok(name) if name.len() <= 15 && !name.is_empty() => name,
            _ => default_host_name,
        };

        let mut command_trigger = config.get_string("bot_commandtrigger").unwrap_or_default();
        if command_trigger.is_empty() {
            command_trigger = String::from("!");
        }

        Self {
            war3_path: config
                .get_string("bot_war3path")
                .unwrap_or_else(|_| String::from("C:\\Program Files\\Warcraft III\\")),
            tft: config.get_bool("bot_tft").unwrap_or(true),
            bind_address: config.get_string("bot_bindaddress").unwrap_or_default(),
            host_port: get_u16_from_config(config, "bot_hostport", 6112),
            save_replays: config.get_bool("bot_savereplays").unwrap_or(true),
            replay_path: config
                .get_string("bot_replaypath")
                .unwrap_or_else(|_| "replays".to_string()),
            // Version mapping: 1.24~1.28 → build 6059; 1.29 → build 6060 (GHost default.cfg)
            replay_war3_version: get_u32_from_config(config, "replay_war3version", 28),
            replay_build_number: get_u16_from_config(config, "replay_buildnumber", 6059),
            reconnect: config.get_bool("bot_reconnect").unwrap_or(true),
            reconnect_port: get_u16_from_config(config, "bot_reconnectport", 6114),
            reconnect_wait_time: get_u32_from_config(config, "bot_reconnectwaittime", 3),
            max_games: get_u32_from_config(config, "bot_maxgames", 5),
            command_trigger,
            map_cfg_path: config.get_string("bot_mapcfgpath").unwrap_or_default(),
            map_path: config.get_string("bot_mappath").unwrap_or_default(),
            virtual_host_name,
            hide_ip_addresses: config.get_bool("bot_hideipaddresses").unwrap_or(false),
            check_multiple_ip_usage: config.get_bool("bot_checkmultipleipusage").unwrap_or(true),
            spoof_checks: get_u8_from_config(config, "bot_spoofchecks", 2),
            require_spoof_checks: config.get_bool("bot_requirespoofchecks").unwrap_or(false),
            reserve_admins: config.get_bool("bot_reserveadmins").unwrap_or(true),
            refresh_messages: config.get_bool("bot_refreshmessages").unwrap_or(false),
            auto_lock: config.get_bool("bot_autolock").unwrap_or(false),
            allow_downloads: get_u8_from_config(config, "bot_allowdownloads", 0),
            ping_during_downloads: config.get_bool("bot_pingduringdownloads").unwrap_or(false),
            max_downloaders: get_u8_from_config(config, "bot_maxdownloaders", 3),
            max_download_speed: get_u32_from_config(config, "bot_maxdownloadspeed", 100),
            lc_pings: config.get_bool("bot_lcpings").unwrap_or(false),
            auto_kick_ping: get_u16_from_config(config, "bot_autokickping", 400),
            ban_method: get_u8_from_config(config, "bot_banmethod", 1),
            ip_blacklist_file: config
                .get_string("bot_ipblacklistfile")
                .unwrap_or_else(|_| String::from("ipblacklist.txt")),
            lobby_time_limit: get_u32_from_config(config, "bot_lobbytimelimit", 10),
            latency: get_u32_from_config(config, "bot_latency", 100),
            default_map: config
                .get_string("bot_defaultmap")
                .unwrap_or_else(|_| String::from("map")),
            motd_file: config
                .get_string("bot_motdfile")
                .unwrap_or_else(|_| String::from("motd.txt")),
            game_loaded_file: config
                .get_string("bot_gameloadedfile")
                .unwrap_or_else(|_| String::from("gameloaded.txt")),
            game_over_file: config
                .get_string("bot_gameoverfile")
                .unwrap_or_else(|_| String::from("gameover.txt")),
            local_admin_messages: config.get_bool("bot_localadminmessages").unwrap_or(true),
            lan_war3_version: get_u8_from_config(config, "lan_war3version", 30),
            tcp_no_delay: config.get_bool("tcp_nodelay").unwrap_or(false),
            map_game_type: get_u32_from_config(config, "bot_mapgametype", 0),
            start_game_at_x_players: get_u8_from_config(config, "bot_gamenotstartuntilXplayers", 4),
            auto_host_game_name: config.get_string("autohost_gamename").unwrap_or_default(),
            auto_host_owner: config.get_string("autohost_owner").unwrap_or_default(),
            auto_host_maximum_games: get_u32_from_config(config, "autohost_maxgames", 5),
            auto_host_auto_start_players: get_u8_from_config(config, "autohost_startplayers", 5),
            udp_broadcast_target: config.get_string("udp_broadcasttarget").unwrap_or_default(),
        }
    }
}
