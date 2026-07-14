// Legacy module kept as a C++ porting reference; superseded by bot::bnet (BnetActor).
#![allow(dead_code)]

use std::{io, usize};
use std::cell::RefCell;
use std::collections::VecDeque;
use std::io::{Error, ErrorKind};
use std::net::SocketAddrV4;
use std::rc::Weak;

use config::Config;
use tracing::info;

use crate::core::{GameBase, GameMap};
use crate::core::bncsutilinterface::BNCSUtilInterface;
use crate::core::bnetprotocol::*;
use crate::core::gamehost::GameHost;
use crate::core::gamemap::{MAPGAMETYPE_PRIVATEGAME, MAPGAMETYPE_UNKNOWN0};
use crate::core::gameprotocol::GAME_PRIVATE;
use crate::core::gamesocket::GameSocket;
use crate::util::*;
// util_truncate_str is imported via util::* (used by queue_chat_command)

#[derive(Debug)]
pub struct BNet {
    gamehost: RefCell<Weak<GameHost>>,
    socket: GameSocket,               // the connection to battle.net
    protocol: BNetProtocol, // battle.net protocol
    bncs_util: BNCSUtilInterface,  // the interface to the bncsutil library (used for logging into battle.net)
    out_packets: VecDeque<Vec<u8>>,           // queue of outgoing packets to be sent (to prevent getting kicked for flooding)
    friends: Vec<String>,                     // vector of friends
    clan: Vec<String>,                        // vector of clan members
    exe_version: Vec<u8>,                     // custom exe version for PvPGN users
    exe_version_hash: Vec<u8>,                // custom exe version hash for PvPGN users
    pub server: String,                     // battle.net server to connect to
    server_addr: SocketAddrV4,                 // battle.net server to connect to
    server_alias: String,                     // battle.net server alias (short name, e.g. "USEast")
    cdkey_roc: String,                        // ROC CD key
    cdkey_tft: String,                        // TFT CD key
    country_abbrev: String,                   // country abbreviation
    country: String,                          // country
    user_name: String,                        // battle.net username
    user_password: String,                    // battle.net password
    first_channel: String,                    // the first chat channel to join upon entering chat (note: store the last channel when entering a game)
    current_channel: String,                  // the current chat channel
    irc_channel: String,                      // IRC channel we're sending the message to
    password_hash_type: String,               // password hash type for PvPGN users
    last_disconnected_time: u64,              // GetTime when we were last disconnected from battle.net
    last_connection_attempt_time: u64,        // GetTime when we last attempted to connect to battle.net
    last_null_time: u64,                      // GetTime when the last null packet was sent for detecting disconnects
    last_out_packet_ticks: u64,               // GetTicks when the last packet was sent for the out_packets queue
    last_admin_refresh_time: u64,             // GetTime when the admin list was last refreshed from the database
    last_ban_refresh_time: u64,               // GetTime when the ban list was last refreshed from the database
    reconnect_delay: u64,                     // interval between two consecutive connect attempts
    last_out_packet_size: usize,              // byte size of the last packet we sent from the out_packets queue
    locale_id: u32,                           // see: http://msdn.microsoft.com/en-us/library/0h88fahh%28VS.85%29.aspx
    host_counter_id: u32,                     // the host counter ID to identify players from this realm
    war3_version: u8,                         // custom warcraft 3 version for PvPGN users
    command_trigger: String,                  // the character prefix to identify commands
    exiting: bool,                            // set to true and this class will be deleted next update
    first_connect: bool,                      // if we haven't tried to connect to battle.net yet
    waiting_to_connect: bool,                 // if we're waiting to reconnect to battle.net after being disconnected
    logged_in: bool,                          // if we've logged into battle.net or not
    in_chat: bool,                            // if we've entered chat or not (but we're not necessarily in a chat channel yet)
    pub pvpgn: bool,                              // if this BNET connection is actually a PvPGN
}

impl BNet {
    pub fn new(config: &Config) -> io::Result<BNet> {
        let prefix = "bnet_";
        let server: String = config.get_string(&format!("{}server", prefix)).unwrap_or(String::new());
        let addr = get_ipv4_address(&server)?;
        let mut server_alias: String = config.get_string(&format!("{}serveralias", prefix)).unwrap_or(String::new());
        let mut cdkey_roc: String = config.get_string(&format!("{}cdkeyroc", prefix)).unwrap_or(String::new());
        let mut cdkey_tft: String = config.get_string(&format!("{}cdkeytft", prefix)).unwrap_or(String::new());
        let country_abbrev: String = config.get_string(&format!("{}countryabbrev", prefix)).unwrap_or(String::from("USA"));
        let country: String = config.get_string(&format!("{}country", prefix)).unwrap_or(String::from("United States"));
        let locale_id: u32 = 1033;

        let user_name: String = config.get_string("username").unwrap_or(String::new());
        let user_password: String = config.get_string("password").unwrap_or(String::new());
        let first_channel: String = config.get_string("firstchannel").unwrap_or(String::from("The Void"));
        let mut bnetcommand_trigger: String = config.get_string("commandtrigger").unwrap_or(String::from("!"));

        if bnetcommand_trigger.is_empty() {
            bnetcommand_trigger = String::from("!");
        }

        let war3_version: u8 = config.get_int("custom_war3version").unwrap_or(30) as u8;
        let exe_version: Vec<u8> = util_extract_numbers(&config.get_string("custom_exeversion").unwrap_or(String::new()), 4);
        let exe_version_hash: Vec<u8> = util_extract_numbers(&config.get_string("custom_exeversionhash").unwrap_or(String::new()), 4);
        let password_hash_type: String = config.get_string("custom_passwordhashtype").unwrap_or(String::new());

        if server.is_empty() {
            return Err(Error::new(ErrorKind::from(ErrorKind::InvalidData), "[BNET] server not found"));
        }

        if server_alias.is_empty() {
            server_alias = server.clone();
        }

        if cdkey_roc.is_empty()
        {
            return Err(Error::new(ErrorKind::from(ErrorKind::InvalidData), format!(
                "[BNET] missing {} cdkeyroc, skipping this battle.net connection", prefix)));
        }

        cdkey_roc = cdkey_roc.replace("-", "").to_uppercase();
        if cdkey_roc.len() != 26 {
            info!("[BNET: {} ] warning - your ROC CD key is not 26 characters long and is probably invalid", server);
        }

        if cdkey_tft.is_empty()
        {
            return Err(Error::new(ErrorKind::from(ErrorKind::InvalidData), format!(
                "[BNET] missing {} cdkeytft, skipping this battle.net connection", prefix)));
        }

        cdkey_tft = cdkey_tft.replace("-", "").to_uppercase();
        if cdkey_tft.len() != 26 {
            info!("[BNET: {} ] warning - your TFT CD key is not 26 characters long and is probably invalid", server);
        }

        if user_name.is_empty()
        {
            return Err(Error::new(ErrorKind::from(ErrorKind::InvalidData), format!(
                "[GHOST] missing {} username, skipping this battle.net connection", prefix)));
        }

        if user_password.is_empty()
        {
            return Err(Error::new(ErrorKind::from(ErrorKind::InvalidData), format!(
                "[GHOST] missing {} password, skipping this battle.net connection", prefix)));
        }

        info!("[GHOST] found battle.net connection #1 for server {}", server_alias);

        let is_pvpgn = password_hash_type == "pvpgn" || exe_version.len() == 4 || exe_version_hash.len() == 4;
        let gamesocket = GameSocket::new()?;
        let bncsutil_interface = BNCSUtilInterface::new("", "");
        let time = get_time();

        return Ok(BNet {
            gamehost: RefCell::new(Weak::new()),
            socket: gamesocket,
            protocol: BNetProtocol::new(),
            bncs_util: bncsutil_interface,
            out_packets: VecDeque::new(),
            friends: vec![],
            clan: vec![],
            exe_version,
            exe_version_hash,
            server,
            server_addr: addr,
            server_alias,
            cdkey_roc,
            cdkey_tft,
            country_abbrev,
            country,
            user_name,
            user_password,
            first_channel,
            current_channel: "".to_string(),
            irc_channel: "".to_string(),
            password_hash_type,
            last_disconnected_time: 0,
            last_connection_attempt_time: 0,
            last_null_time: 0,
            last_out_packet_ticks: 0,
            last_admin_refresh_time: time,
            last_ban_refresh_time: time,
            reconnect_delay: if is_pvpgn { 90 } else { 240 },
            last_out_packet_size: 0,
            locale_id,
            host_counter_id: 0,
            war3_version,
            command_trigger: bnetcommand_trigger,
            exiting: false,
            first_connect: false,
            waiting_to_connect: true,
            logged_in: false,
            in_chat: false,
            pvpgn: is_pvpgn,
        });
    }

    pub fn update(&mut self) -> bool {
        let (time, ticks) = get_time_and_ticks();

        // we return at the end of each if statement so we don't have to deal with errors related to the order of the if statements
        // that means it might take a few ms longer to complete a task involving multiple steps (in this case, reconnecting) due to blocking or sleeping
        // but it's not a big deal at all, maybe 100ms in the worst possible case (based on a 50ms blocking time)

        if self.socket.has_error
        {
            // the socket has an error
            info!("[BNET: {} ] disconnected from battle.net due to socket error", self.server_alias);
            info!("[BNET: {} ] waiting {} seconds to reconnect", self.server_alias, self.reconnect_delay);
            self.bncs_util.reset(&self.user_name, &self.user_password);
            self.socket.reset();
            self.last_disconnected_time = time;
            self.logged_in = false;
            self.in_chat = false;
            self.waiting_to_connect = true;
            return self.exiting;
        }

        if self.socket.is_connected {
            // the socket is connected and everything appears to be working properly
            self.socket.received().expect("something is wrong");

            // extract as many packets as possible from the socket's receive buffer and put them in the m_Packets queue
            let mut bytes = self.socket.get_bytes().to_vec();
            let mut length_processed: usize = 0;

            while bytes.len() >= 4 {
                // byte 0 is always 255
                if bytes[0] != BNET_HEADER_CONSTANT {
                    // Fix: the original continue would cause an infinite loop; a bad header means the data stream is corrupted, so disconnect immediately
                    info!("[BNET: {}] error - received invalid packet from battle.net (bad header constant), disconnecting", self.server_alias);
                    self.socket.disconnect();
                    break;
                }

                // bytes 2 and 3 contain the length of the packet
                let length = (usize::from(bytes[3]) << 8) | usize::from(bytes[2]);

                if length < 4 {
                    info!("[BNET: {}] error - received invalid packet from battle.net (bad length), disconnecting", self.server_alias);
                    self.socket.disconnect();
                    break;
                }

                // Fix: must first confirm the data is long enough before slicing, otherwise &bytes[0..length] would panic
                if bytes.len() < length {
                    break;
                }

                let data = &bytes[0..length];

                match bytes[1] {
                    SID_NULL => {
                        self.protocol.receive_sid_null(data);
                        break;
                    }
                    SID_GETADVLISTEX => {
                        if let Some(game_host) = self.protocol.receive_sid_getadvlistex(data) {
                            info!("[BNET: {}] joining game [{}]", self.server_alias, game_host.game_name)
                        }
                    }
                    SID_ENTERCHAT => {
                        if self.protocol.receive_sid_enterchat(data)
                        {
                            info!("[BNET: {}] joining channel [{}]", self.server_alias, self.first_channel);
                            self.in_chat = true;
                            self.socket.put_bytes(self.protocol.send_sid_joinchannel(&self.first_channel));
                        }
                    }
                    SID_CHATEVENT => {
                        if let Some(chat_event) = self.protocol.receive_sid_chatevent(data) {
                            self.process_chat_event(chat_event);
                        }
                    }
                    SID_CHECKAD => {
                        self.protocol.receive_sid_checkad(data);
                    }
                    SID_STARTADVEX3 => {
                        if self.protocol.receive_sid_startadvex3(data)
                        {
                            self.in_chat = false;
                        } else {
                            info!("[BNET: {}] startadvex3 failed", self.server_alias);
                            // m_Aura->EventBNETGameRefreshFailed(this);
                        }
                    }
                    SID_PING => {
                        self.socket.put_bytes(self.protocol.send_sid_ping(&self.protocol.receive_sid_ping(data)));
                    }
                    SID_AUTH_INFO => {
                        if self.protocol.receive_sid_auth_info(data)
                        {
                            if self.bncs_util.help_sid_auth_check("", "", "", "", "", &[0u8; 1], &[0u8; 1], 0u8)
                            {
                                // override the exe information generated by bncsutil if specified in the config file
                                // apparently this is useful for pvpgn users
                                if self.exe_version.len() == 4
                                {
                                    info!("[BNET: {}] using custom exe version bnet_custom_exeversion = {}",
                                             self.server_alias, format!("{}{}{}{}", self.exe_version[0], self.exe_version[1], self.exe_version[2], self.exe_version[3]));

                                    self.bncs_util.set_exe_version(&self.exe_version);
                                }

                                if self.exe_version_hash.len() == 4
                                {
                                    info!("[BNET: {}] using custom exe version hash bnet_custom_exeversionhash = {}",
                                             self.server_alias, format!("{}{}{}{}", self.exe_version_hash[0], self.exe_version_hash[1], self.exe_version_hash[2], self.exe_version_hash[3]));
                                    self.bncs_util.set_exe_version_hash(&self.exe_version_hash);
                                }

                                info!("[BNET: {}] attempting to auth as Warcraft III: The Frozen Throne", self.server_alias);

                                self.socket.put_bytes(
                                    self.protocol.send_sid_auth_check(
                                        true, self.protocol.get_client_token(), self.bncs_util.get_exe_version(),
                                        self.bncs_util.get_exe_version_hash(), self.bncs_util.get_key_info_roc(),
                                        self.bncs_util.get_key_info_tft(), self.bncs_util.get_exe_info(), "FateBot"));
                            } else {
                                info!("[BNET: {}] logon failed - bncsutil key hash failed (check your Warcraft 3 path and cd keys), disconnecting", self.server_alias);
                                self.socket.disconnect();
                            }
                        }
                    }
                    SID_AUTH_CHECK => {
                        if self.protocol.receive_sid_auth_check(data)
                        {
                            // cd keys accepted
                            info!("[BNET: {}] cd keys accepted", self.server_alias);
                            self.bncs_util.help_sid_auth_accountlogon();
                            self.socket.put_bytes(self.protocol.send_sid_auth_accountlogon(self.bncs_util.get_client_key(), &self.user_name));
                        } else {
                            // cd keys not accepted
                            match util_byte_array_to_u32(self.protocol.get_key_state(), false, 0)
                            {
                                KR_ROC_KEY_IN_USE => {
                                    info!("[BNET: {}] logon failed - ROC CD key in use by user {}, disconnecting", self.server_alias, self.protocol.get_key_state_description());
                                }

                                KR_TFT_KEY_IN_USE => {
                                    info!("[BNET: {}] logon failed - TFT CD key in use by user {}, disconnecting", self.server_alias, self.protocol.get_key_state_description());
                                }

                                KR_OLD_GAME_VERSION => {
                                    info!("[BNET: {}] logon failed - game version is too old, disconnecting", self.server_alias);
                                }

                                KR_INVALID_VERSION => {
                                    info!("[BNET: {}] logon failed - game version is invalid, disconnecting", self.server_alias);
                                }

                                _ => {
                                    info!("[BNET: {}] logon failed - cd keys not accepted, disconnecting", self.server_alias);
                                }
                            }

                            self.socket.disconnect();
                        }
                    }
                    SID_AUTH_ACCOUNTLOGON => {
                        if self.protocol.receive_sid_auth_accountlogon(data)
                        {
                            info!("[BNET: {}] username [{}] accepted", self.server_alias, self.user_name);

                            if self.password_hash_type == "pvpgn"
                            {
                                // pvpgn logon
                                info!("[BNET: {}] using pvpgn logon type (for pvpgn servers only)", self.server_alias);
                                self.bncs_util.help_pvpg_password_hash(&self.user_password);
                                self.socket.put_bytes(self.protocol.send_sid_auth_accountlogonproof(self.bncs_util.get_pvpg_password_hash()));
                            } else {
                                // battle.net logon
                                info!("[BNET: {}] using battle.net logon type (for official battle.net servers only)", self.server_alias);
                                self.bncs_util.help_sid_auth_accountlogonproof(self.protocol.get_slat(), self.protocol.get_server_public_key());
                                self.socket.put_bytes(self.protocol.send_sid_auth_accountlogonproof(self.bncs_util.get_m1()));
                            }
                        } else {
                            info!("[BNET: {}] logon failed - invalid username, disconnecting", self.server_alias);
                            self.socket.disconnect();
                        }
                    }
                    SID_AUTH_ACCOUNTLOGONPROOF => {
                        if self.protocol.receive_sid_auth_accountlogonproof(data)
                        {
                            // logon successful
                            info!("[BNET: {}] logon successful", self.server_alias);
                            self.logged_in = true;
                            self.socket.put_bytes(self.protocol.send_sid_netgameport(self.server_addr.port()));
                            self.socket.put_bytes(self.protocol.send_sid_enterchat());
                            self.socket.put_bytes(self.protocol.send_sid_friendlist());
                            self.socket.put_bytes(self.protocol.send_sid_clanmemberlist());
                        } else {
                            info!("[BNET: {}] logon failed - invalid password, disconnecting", self.server_alias);

                            // try to figure out if the user might be using the wrong logon type since too many people are confused by this
                            let _server = self.server.to_lowercase();
                            if self.pvpgn && (_server == "useast.battle.net" || _server == "uswest.battle.net" || _server == "asia.battle.net" || _server == "europe.battle.net")
                            {
                                info!("[BNET: {}] it looks like you're trying to connect to a battle.net server using a pvpgn logon type, check your config file's battle.net custom data section", self.server_alias);
                            } else if !self.pvpgn && (_server != "useast.battle.net" && _server != "uswest.battle.net" && _server != "asia.battle.net" && _server != "europe.battle.net") {
                                info!("[BNET: {}] it looks like you're trying to connect to a pvpgn server using a battle.net logon type, check your config file's battle.net custom data section", self.server_alias);
                            }

                            self.socket.disconnect();
                        }
                    }
                    SID_FRIENDLIST => {
                        self.friends = self.protocol.receive_sid_friendlist(data);
                    }
                    SID_CLANMEMBERLIST => {
                        self.clan = self.protocol.receive_sid_clanmemberlist(data);
                    }
                    _ => {
                        break;
                    }
                }

                length_processed += length;
                bytes = bytes.drain(length..).collect();
            }

            self.socket.cosume_bytes(length_processed);

            // check if at least one packet is waiting to be sent and if we've waited long enough to prevent flooding
            // this formula has changed many times but currently we wait 1 second if the last packet was "small", 3.5 seconds if it was "medium", and 4 seconds if it was "big"
            let mut wait_ticks: u64 = 4300;
            if self.last_out_packet_size < 10
            {
                wait_ticks = 1300;
            } else if self.last_out_packet_size < 100 {
                wait_ticks = 3300;
            }

            if !self.out_packets.is_empty() && ticks - self.last_out_packet_ticks >= wait_ticks {
                if self.out_packets.len() > 7 {
                    info!("[BNET: {}] packet queue warning - there are {} packets waiting to be sent", self.server_alias, self.out_packets.len());
                }

                if let Some(packet) = self.out_packets.pop_front() {
                    self.last_out_packet_size = packet.len();
                    self.last_out_packet_ticks = ticks;
                    self.socket.put_bytes(packet);
                }
            }

            // send a null packet every 60 seconds to detect disconnects
            if time - self.last_null_time >= 60 && ticks - self.last_out_packet_ticks >= 60000
            {
                self.socket.put_bytes(self.protocol.send_sid_null());
                self.last_null_time = time;
            }

            self.socket.send();
            return self.exiting;
        }


        if self.socket.is_connected && !self.waiting_to_connect
        {
            // the socket was disconnected
            info!("[BNET: {}] disconnected from battle.net", self.server_alias);
            self.last_disconnected_time = time;
            self.bncs_util.reset(&self.user_name, &self.user_password);
            self.socket.reset();
            self.logged_in = false;
            self.in_chat = false;
            self.waiting_to_connect = true;
            return self.exiting;
        }

        if !self.socket.is_connected && self.first_connect || (time - self.last_disconnected_time >= self.reconnect_delay)
        {
            // attempt to connect to battle.net
            self.first_connect = false;
            info!("[BNET: {}] connecting to server to address [ {}]", self.server_alias, self.server.to_string());


            if let Ok(_) = self.socket.connect(self.server_addr) {
                info!("[BNET: {}] resolved and cached server IP address {}", self.server_alias, self.server.to_string());
            }

            self.waiting_to_connect = false;
            self.last_connection_attempt_time = time;
        }

        if self.socket.is_connected {
            // we are currently attempting to connect to battle.net
            return if time - self.last_connection_attempt_time >= 15 {
                // the connection attempt timed out (15 seconds)
                info!("[BNET: {}] connect timed out", self.server_alias);
                info!("[BNET: {}] waiting 90 seconds to reconnect", self.server_alias);
                self.socket.reset();
                self.last_disconnected_time = time;
                self.waiting_to_connect = true;
                self.exiting
            } else {
                // the connection attempt completed
                info!("[BNET: {}] connected", self.server_alias);
                self.socket.put_bytes(self.protocol.send_protocol_initialize_selector());
                self.socket.put_bytes(self.protocol.send_sid_auth_info(self.war3_version, true, self.locale_id, &self.country_abbrev, &self.country));
                self.socket.send();

                self.last_null_time = time;
                self.last_out_packet_ticks = ticks;

                while !self.out_packets.is_empty() {
                    self.out_packets.pop_back();
                }

                self.exiting
            };
        }
        return self.exiting;
    }

    pub fn process_chat_event(&self, chat_event: IncomingChatEvent) {
        let event = chat_event.chat_event;
        let _whisper = event == EID_WHISPER; // used during command dispatch
        let user = chat_event.user;
        let message = chat_event.message;

        if event == EID_IRC
        {
            todo!("need to send message to irc");
            // extract the irc channel
            // string::size_type MessageStart = Message.find(' ');
            // m_IRC   = Message.substr(0, MessageStart);
            // Message = Message.substr(MessageStart + 1);
        } else {
            //m_IRC.clear();
        }

        if event == EID_WHISPER || event == EID_TALK || event == EID_IRC
        {
            if event == EID_WHISPER {
                info!("[WHISPER: {}] [{}] {}", self.server_alias, user, message);
            } else {
                info!("[LOCAL: : {}] [{}] {}", self.server_alias, user, message);
            }

            // handle bot commands
            if message.is_empty() || !message.starts_with(&self.command_trigger)
            {
                info!("[ERROR: {}]{}", self.server_alias, message);
                return;
            }

            // extract the command trigger, the command, and the payload
            // e.g. "!say hello world" -> command: "say", payload: "hello world"
            // hook up command dispatch (!pub/!priv/!map/!unhost...)
            let (_command, _payload) = get_command_and_payload(&message);
        }
    }

    // functions to send packets to battle.net
    pub fn send_get_friends_list(&mut self) {
        if self.logged_in {
            self.socket.put_bytes(self.protocol.send_sid_friendlist());
        }
    }
    pub fn send_get_clan_list(&mut self) {
        if self.logged_in {
            self.socket.put_bytes(self.protocol.send_sid_clanmemberlist());
        }
    }

    pub fn queue_enter_chat(&mut self) {
        if self.logged_in {
            self.socket.put_bytes(self.protocol.send_sid_enterchat());
        }
    }

    pub fn queue_chat_command(&mut self, chat_command: &str) {
        if chat_command.is_empty() {
            return;
        }

        if self.logged_in {
            if self.out_packets.len() <= 10 {
                info!("[QUEUED: {}] {}", self.server_alias, chat_command);
            }

            // Fix: the original &chat_command[0..200] would panic when the message is shorter or the cut falls in the middle of a UTF-8 character
            let max_length = if self.pvpgn { 200 } else { 255 };
            self.out_packets.push_back(self.protocol.send_sid_chatcommand(util_truncate_str(chat_command, max_length)));
        }
    }
    pub fn queue_chat_command_with_irc(&mut self, chat_command: &str, user: &str, whisper: bool, irc: &str) {
        if chat_command.is_empty() {
            return;
        }

        // if the destination is IRC send it there
        if !irc.is_empty()
        {
            //m_Aura->m_IRC->SendMessageIRC(chatCommand, irc);
            return;
        }

        // if whisper is true send the chat command as a whisper to user, otherwise just queue the chat command
        if whisper
        {
            self.queue_chat_command(&format!("/w {} {}", user, chat_command));
        } else {
            self.queue_chat_command(chat_command);
        }
    }
    pub fn queue_game_create(&mut self, state: u8, game_name: &str, map: &GameMap, host_counter: u32) {
        if self.logged_in && map.is_valid {
            if self.current_channel.is_empty() {
                self.first_channel = self.current_channel.to_string();
            }

            self.in_chat = false;
            self.queue_game_refresh(state, game_name, map, host_counter);
        }
    }
    pub fn queue_game_refresh(&mut self, state: u8, game_name: &str, map: &GameMap, host_counter: u32) {
        if self.logged_in && map.is_valid
        {
            // construct a fixed host counter which will be used to identify players from this realm
            // the fixed host counter's 4 most significant bits will contain a 4 bit ID (0-15)
            // the rest of the fixed host counter will contain the 28 least significant bits of the actual host counter
            // since we're destroying 4 bits of information here the actual host counter should not be greater than 2^28 which is a reasonable assumption
            // when a player joins a game we can obtain the ID from the received host counter
            // note: LAN broadcasts use an ID of 0, battle.net refreshes use an ID of 1-10, the rest are unused
            let mut map_game_type = map.get_map_game_type();
            map_game_type |= MAPGAMETYPE_UNKNOWN0;

            if state == GAME_PRIVATE {
                map_game_type |= MAPGAMETYPE_PRIVATEGAME;
            }

            // use an invalid map width/height to indicate reconnectable games
            let mut map_width: Vec<u8> = vec![];
            map_width.push(192);
            map_width.push(7);
            let mut map_height: Vec<u8> = vec![];
            map_height.push(192);
            map_height.push(7);

            self.out_packets.push_back(
                self.protocol.send_sid_startadvex3(
                    state, &map_game_type.to_le_bytes(), &map.get_map_game_flags(), &map_width, &map_height,
                    game_name, &self.user_name, 0, map.get_map_path(), map.get_map_crc(),
                    map.get_map_sha1(), (host_counter & 0x0FFFFFFF) | (self.host_counter_id << 28)))
        }
    }

    pub fn queue_game_uncreate(&mut self) {
        if self.logged_in {
            self.out_packets.push_back(self.protocol.send_sid_stopadv());
        }
    }

    pub fn unqueue_game_refreshes(&mut self) {
        let mut packets: VecDeque<Vec<u8>> = VecDeque::new();

        while let Some(packet) = self.out_packets.pop_back() {
            // TODO: it's very inefficient to have to copy all these packets while searching the queue
            if packet.len() >= 2 && packet[1] != SID_STARTADVEX3
            {
                packets.push_back(packet);
            }
        }

        self.out_packets = packets;
        info!("[BNET: {}] unqueued game refresh packets", self.server_alias);
    }

    // other functions
    pub fn is_admin(&self, _name: &str) -> bool {
        todo!("legacy stub: superseded by db admins table");
    }

    pub fn is_root_admin(&self, _name: &str) -> bool {
        todo!("legacy stub: superseded by config rootadmin");
    }

    pub fn is_banned_name(&self, _name: &str) {
        todo!("legacy stub: superseded by db bans table");
    }

    pub fn hold_friends(&self, game: &GameBase) {
        for friend in &self.friends {
            game.add_to_reserved(friend.clone());
        }
    }
    pub fn hold_clan(&self, game: &GameBase) {
        for clan in &self.clan {
            game.add_to_reserved(clan.clone());
        }
    }

    fn handle_whisper_and_talk(&self, event: u8, user: String, message: String) {
        // handle spoof checking for current game
        // this case covers whispers - we assume that anyone who sends a whisper to the bot with message "spoofcheck" should be considered spoof checked
        // note that this means you can whisper "spoofcheck" even in a public game to manually spoofcheck if the /whois fails
        // Fix: 1) RefCell<Weak<GameHost>> must be borrow + upgrade before use (the original code did not compile)
        //       2) the original && / || precedence was missing parentheses, so any "sc" message would trigger it
        if let Some(gamehost) = self.gamehost.borrow().upgrade() {
            if let Some(current_game) = gamehost.current_game.borrow().as_ref() {
                if event == EID_WHISPER && (message == "s" || message == "sc" || message == "spoofcheck") {
                    current_game.add_to_spoofed(self.server_addr, &user, true);
                }
            }
        }
    }

    fn handle_channel(&mut self, message: String) {
        // keep track of current channel so we can rejoin it after hosting a game
        info!("[BNET: {}] joined channel [{}]", self.server_alias, message);
        self.current_channel = message;
    }

    fn handle_info(&self) {}

    fn handle_error(&self, message: &str) {
        info!("[ERROR: {}] {}", self.server_alias, message);
    }

    fn map_files_match(_pattern: &str) -> Vec<String> {
        todo!("legacy stub: map existence check");
        // transform(begin(pattern), end(pattern), begin(pattern), ::tolower);
        //
        // auto ROCMaps = FilesMatch(m_Aura->m_MapPath, ".w3m");
        // auto TFTMaps = FilesMatch(m_Aura->m_MapPath, ".w3x");
        //
        // vector<string> MapList;
        // MapList.insert(end(MapList), begin(ROCMaps), end(ROCMaps));
        // MapList.insert(end(MapList), begin(TFTMaps), end(TFTMaps));
        //
        // vector<string> Matches;
        //
        // for (auto& mapName : MapList)
        // {
        //     string lowerMapName(mapName);
        //     transform(begin(lowerMapName), end(lowerMapName), begin(lowerMapName), ::tolower);
        //
        //     if (lowerMapName.find(pattern) != string::npos)
        //     Matches.push_back(mapName);
        // }
        //
        // return Matches;
    }
    fn config_files_match(_pattern: &str) -> Vec<String> {
        todo!("legacy stub: map config existence check");
        // transform(begin(pattern), end(pattern), begin(pattern), ::tolower);
        //
        // vector<string> ConfigList = FilesMatch(m_Aura->m_MapCFGPath, ".cfg");
        //
        // vector<string> Matches;
        //
        // for (auto& cfgName : ConfigList)
        // {
        //     string lowerCfgName(cfgName);
        //     transform(begin(lowerCfgName), end(lowerCfgName), begin(lowerCfgName), ::tolower);
        //
        //     if (lowerCfgName.find(pattern) != string::npos)
        //     Matches.push_back(cfgName);
        // }
        //
        // return Matches;
    }
}