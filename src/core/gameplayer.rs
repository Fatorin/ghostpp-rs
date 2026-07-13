use std::collections::VecDeque;

use tracing::debug;

use crate::core::commandpacket::CommandPacket;
use crate::core::GameBase;
use crate::core::gameprotocol::*;
use crate::core::gamesocket::GameSocket;
use crate::core::gpsprotocol::*;
use crate::util::{get_time, util_byte_array_to_u16};

#[derive(Debug)]
pub struct GamePlayer {
    gamebase: GameBase,
    #[allow(dead_code)] // Handled during the GameActor rewrite
    protocol: GameProtocol,
    pub socket: GameSocket,
    packets: VecDeque<CommandPacket>,
    delete: bool,
    pub error: bool,
    pub error_message: String,
    incoming_join_player: Option<IncomingJoinPlayer>,

    pub pid: u8,
    pub name: String,                              // the player's name
    pub internal_ip: Vec<u8>,                      // the player's internal IP address as reported by the player when connecting
    pub pings: VecDeque<u32>,                      // store the last few (20) pings received so we can take an average
    pub checksums: VecDeque<u32>,                  // the last few checksums the player has sent (for detecting desyncs)
    pub left_reason: String,                       // the reason the player left the game
    pub spoofed_realm: String,                     // the realm the player last spoof checked on
    pub joined_realm: String,                      // the realm the player joined on (probable, can be spoofed)
    pub total_packets_sent: usize,
    pub total_packets_received: usize,
    pub left_code: u32,                            // the code to be sent in W3GS_PLAYERLEAVE_OTHERS for why this player left the game
    pub login_attempts: u32,                       // the number of attempts to login (used with CAdminGame only)
    pub sync_counter: u32,                         // the number of keepalive packets received from this player
    pub join_time: u64,                            // GetTime when the player joined the game (used to delay sending the /whois a few seconds to allow for some lag)
    pub last_map_part_sent: u32,                   // the last mappart sent to the player (for sending more than one part at a time)
    pub last_map_part_acked: u32,                  // the last mappart acknowledged by the player
    pub started_downloading_ticks: u32,            // GetTicks when the player started downloading the map
    pub finished_downloading_time: u32,            // GetTime when the player finished downloading the map
    pub finished_loading_ticks: u64,               // GetTicks when the player finished loading the game
    pub started_lagging_ticks: u64,                // GetTicks when the player started lagging
    pub stats_sent_time: u32,                      // GetTime when we sent this player's stats to the chat (to prevent players from spamming !stats)
    pub stats_dot_a_sent_time: u32,                // GetTime when we sent this player's dota stats to the chat (to prevent players from spamming !statsdota)
    pub last_gproxy_wait_notice_sent_time: u32,
    pub load_in_game_data: VecDeque<Vec<u8>>,      // queued data to be sent when the player finishes loading when using "load in game"
    pub score: f64,                                // the player's generic "score" for the matchmaking algorithm
    pub logged_in: bool,                           // if the player has logged in or not (used with CAdminGame only)
    pub spoofed: bool,                             // if the player has spoof checked or not
    pub reserved: bool,                            // if the player is reserved (VIP) or not
    pub whois_should_be_sent: bool,                // if a battle.net /whois should be sent for this player or not
    pub whois_sent: bool,                          // if we've sent a battle.net /whois for this player yet (for spoof checking)
    pub download_allowed: bool,                    // if we're allowed to download the map or not (used with permission based map downloads)
    pub download_started: bool,                    // if we've started downloading the map or not
    pub download_finished: bool,                   // if we've finished downloading the map or not
    pub finished_loading: bool,                    // if the player has finished loading or not
    pub lagging: bool,                             // if the player is lagging or not (on the lag screen)
    pub drop_vote: bool,                           // if the player voted to drop the laggers or not (on the lag screen)
    pub kick_vote: bool,                           // if the player voted to kick a player or not
    pub start_vote: bool,
    pub muted: bool,                               // if the player is muted or not
    pub left_message_sent: bool,                   // if the playerleave message has been sent or not
    pub gproxy: bool,                              // if the player is using GProxy++
    pub gproxy_disconnect_notice_sent: bool,       // if a disconnection notice has been sent or not when using GProxy++
    pub gproxy_buffer: VecDeque<Vec<u8>>,
    pub gproxy_reconnect_key: u32,
    pub last_recv_time: u64,
    pub last_gproxy_ack_time: u64,
}

impl GamePlayer {
    pub fn new(
        gamebase: GameBase,
        protocol: GameProtocol,
        socket: GameSocket,
        pid: u8,
        joined_realm: String,
        name: String,
        internal_ip: Vec<u8>,
    ) -> Self {
        Self {
            gamebase,
            protocol,
            socket,
            packets: Default::default(),
            delete: false,
            error: false,
            error_message: String::new(),
            incoming_join_player: None,
            pid,
            name,
            internal_ip,
            pings: Default::default(),
            checksums: Default::default(),
            left_reason: String::new(),
            spoofed_realm: String::new(),
            joined_realm,
            total_packets_sent: 0,
            total_packets_received: 0,
            left_code: PLAYERLEAVE_LOBBY as u32,
            login_attempts: 0,
            sync_counter: 0,
            join_time: 0,
            last_map_part_sent: 0,
            last_map_part_acked: 0,
            started_downloading_ticks: 0,
            finished_downloading_time: 0,
            finished_loading_ticks: 0,
            started_lagging_ticks: 0,
            stats_sent_time: 0,
            stats_dot_a_sent_time: 0,
            last_gproxy_wait_notice_sent_time: 0,
            load_in_game_data: Default::default(),
            score: -100000.0,
            logged_in: false,
            spoofed: false,
            reserved: false,
            whois_should_be_sent: false,
            whois_sent: false,
            download_allowed: false,
            download_started: false,
            download_finished: false,
            finished_loading: false,
            lagging: false,
            drop_vote: false,
            kick_vote: false,
            start_vote: false,
            muted: false,
            left_message_sent: false,
            gproxy: false,
            gproxy_disconnect_notice_sent: false,
            gproxy_buffer: Default::default(),
            gproxy_reconnect_key: 0,
            last_recv_time: get_time(),
            last_gproxy_ack_time: 0,
        }
    }

    pub fn update(&mut self) -> bool {
        if self.delete {
            return true;
        }

        if !self.socket.is_connected {
            return false;
        }

        // wait 4 seconds after joining before sending the /whois or /w
        // if we send the /whois too early battle.net may not have caught up with where the player is and return erroneous results
        if self.whois_should_be_sent && !self.spoofed && !self.whois_sent && !self.joined_realm.is_empty() && get_time() - self.join_time >= 4
        {
            // todotodo: we could get kicked from battle.net for sending a command with invalid characters, do some basic checking
            // for( vector<CBNET *> :: iterator i = m_Game->m_GHost->m_BNETs.begin( ); i != m_Game->m_GHost->m_BNETs.end( ); ++i )
            // {
            //     if( (*i)->GetServer( ) == m_JoinedRealm )
            //     {
            //         if( m_Game->GetGameState( ) == GAME_PUBLIC )
            //         {
            //             if( (*i)->GetPasswordHashType( ) == "pvpgn" )
            //             (*i)->QueueChatCommand( "/whereis " + m_Name );
            //             else
            //             (*i)->QueueChatCommand( "/whois " + m_Name );
            //         }
            //         else if( m_Game->GetGameState( ) == GAME_PRIVATE )
            //         (*i)->QueueChatCommand( m_Game->m_GHost->m_Language->SpoofCheckByReplying( ), m_Name, true );
            //     }
            // }
            //
            self.whois_sent = true;
        }

        // check for socket timeouts
        // if we don't receive anything from a player for 30 seconds we can assume they've dropped
        // this works because in the lobby we send pings every 5 seconds and expect a response to each one
        // and in the game the Warcraft 3 client sends keepalives frequently (at least once per second it looks like)
        if self.socket.is_connected {
            if get_time() - self.last_recv_time >= 30u64 {
                // m_Game->EventPlayerDisconnectTimedOut( this );

                unimplemented!();
            }
        }

        // GProxy++ acks
        if self.gproxy && get_time() - self.last_gproxy_ack_time >= 10u64
        {
            if self.socket.is_connected {
                self.socket.put_bytes(send_gpss_ack(self.total_packets_received as u32));
                self.last_gproxy_ack_time = get_time();
            }
        }

        // base class update, received packet
        let _received_result = self.socket.received(); // Triggers a disconnect event on failure

        let deleting = if self.gproxy && self.gamebase.game_loaded {
            self.delete || self.error
        } else {
            self.delete || self.error || self.socket.has_error || self.socket.is_connected
        };

        // try to find out why we're requesting deletion
        // in cases other than the ones covered here m_LeftReason should have been set when m_DeleteMe was set
        if self.error
        {
            // m_Game -> EventPlayerDisconnectPlayerError(this);
            self.socket.reset();
            return deleting;
        }

        if self.socket.is_connected
        {
            if self.socket.has_error
            {
                // m_Game -> EventPlayerDisconnectSocketError(this);
                self.socket.reset();
            } else if !self.socket.is_connected
            {
                // m_Game -> EventPlayerDisconnectConnectionClosed(this);
                self.socket.reset();
            }
        }

        return deleting;
    }

    pub fn internal_update(&mut self) -> bool {
        if self.delete {
            return true;
        }

        if !self.socket.is_connected {
            return false;
        }

        self.extract_packets();
        self.process_packets();
        self.delete || self.error || self.socket.has_error || !self.socket.is_connected
    }

    pub fn get_external_ip(&self) -> Vec<u8> {
        self.socket.get_ip()
    }

    pub fn get_external_ip_string(&self) -> String {
        self.socket.get_ip_string()
    }

    pub fn get_name_terminated(&self) -> String {
        let lower_name = self.name.to_lowercase();
        let start = lower_name.find("|c");
        let end = lower_name.find("|r");

        if let Some(start_idx) = start {
            if end.is_none() || end.unwrap() < start_idx {
                return format!("{}|r", self.name);
            }
        }

        self.name.to_string()
    }

    pub fn get_ping(&self, lcp_ping: bool) -> u32 {
        if self.pings.is_empty() {
            return 0;
        }

        let mut avg_ping: u32 = 0;
        for ping in self.pings.iter() {
            avg_ping += ping;
        }

        avg_ping = avg_ping / self.pings.len() as u32;

        return if lcp_ping {
            avg_ping / 2
        } else {
            avg_ping
        };
    }

    pub fn add_load_in_game_data(&mut self, load_in_game_data: Vec<u8>) {
        self.load_in_game_data.push_back(load_in_game_data);
    }

    pub fn extract_packets(&mut self) {
        let mut bytes = self.socket.get_bytes().clone();

        if bytes.is_empty() {
            self.error = true;
            self.error_message = String::from("received packet failed.");
        }

        // a packet is at least 4 bytes so loop as long as the buffer contains 4 bytes
        while bytes.len() >= 4
        {
            if bytes[0] != W3GS_HEADER_CONSTANT && bytes[0] != GPS_HEADER_CONSTANT
            {
                self.error = true;
                self.error_message = String::from("received invalid packet from player (bad header constant)");
            }

            // bytes 2 and 3 contain the length of the packet
            let length = util_byte_array_to_u16(&bytes, false, 2) as usize;

            if length < 4
            {
                self.error = true;
                self.error_message = String::from("received invalid packet from player (bad length)");
            }

            if bytes.len() < length
            {
                return;
            }

            self.packets.push_back(CommandPacket {
                packet_type: bytes[0],
                id: bytes[1] as u32,
                data: bytes.drain(0..length).collect(),
            });

            self.total_packets_received += 1;
        }
    }

    pub fn process_packets(&mut self) {
        if !self.socket.is_connected {
            return;
        }

        // process all the received packets in the Packets queue
        while let Some(packet) = self.packets.pop_front() {
            if packet.packet_type != W3GS_HEADER_CONSTANT {
                continue;
            }

            match packet.id as u8 {
                W3GS_REQJOIN => {
                    // the only packet we care about as a potential player is W3GS_REQJOIN, ignore everything else
                    self.incoming_join_player = GameProtocol::receive_w3gs_reqjoin(&packet.data);
                    // don't continue looping because there may be more packets waiting and this parent class doesn't handle them
                    // EventPlayerJoined creates the new player, NULLs the socket, and sets the delete flag on this object so it'll be deleted shortly
                    // any unprocessed packets will be copied to the new CGamePlayer in the constructor or discarded if we get deleted because the game is full
                    if let Some(incoming_join_player) = &self.incoming_join_player {
                        self.gamebase.event_player_joined(incoming_join_player);
                        unimplemented!();
                    }
                }
                _ => { debug!("unsupported packet id"); }
            }
        }
    }
    pub fn send(&mut self, data: Vec<u8>) {
        self.total_packets_sent += 1;

        if self.gproxy && self.gamebase.game_loaded {
            self.gproxy_buffer.push_back(data.clone());
        }

        if self.socket.is_connected {
            self.socket.put_bytes(data);
        }
    }

    pub fn event_gproxy_reconnect(&mut self, game_socket: GameSocket, last_packet: usize) {
        self.socket = game_socket;
        self.socket.put_bytes(send_gpss_reconnect(self.total_packets_received as u32));

        let packets_already_unqueued = self.total_packets_sent - self.gproxy_buffer.len();

        if last_packet > packets_already_unqueued
        {
            let mut packets_to_unqueue = last_packet - packets_already_unqueued;

            if packets_to_unqueue > self.gproxy_buffer.len() {
                packets_to_unqueue = self.gproxy_buffer.len();
            }

            while packets_to_unqueue > 0
            {
                if let Some(_) = self.gproxy_buffer.pop_back() {
                    packets_to_unqueue -= 1;
                }
            }
        }

        // send remaining packets from buffer, preserve buffer
        let mut temp_buffer: VecDeque<Vec<u8>> = Default::default();
        while !self.gproxy_buffer.is_empty()
        {
            if let Some(buffer) = self.gproxy_buffer.pop_back() {
                self.socket.put_bytes(buffer.clone());
                temp_buffer.push_back(buffer);
            }
        }

        self.gproxy_buffer = temp_buffer;
        self.gproxy_disconnect_notice_sent = false;
        unimplemented!();
        // m_Game -> SendAllChat(m_Game->m_GHost->m_Language->PlayerReconnectedWithGProxy(m_Name));
    }
}