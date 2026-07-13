use std::collections::VecDeque;
use tracing::warn;
use crate::core::gameplayer::GamePlayer;
use crate::core::gameslot::{GameSlot, MAX_SLOTS};
use crate::util::*;

// Base Protocol
pub const W3GS_HEADER_CONSTANT: u8 = 247;
pub const GAME_NONE: u8 = 0;
pub const GAME_FULL: u8 = 2;
pub const GAME_PUBLIC: u8 = 16;
pub const GAME_PRIVATE: u8 = 17;
pub const GAMETYPE_CUSTOM: u8 = 1;
pub const GAMETYPE_BLIZZARD: u8 = 9;
pub const PLAYERLEAVE_DISCONNECT: u8 = 1;
pub const PLAYERLEAVE_LOST: u8 = 7;
pub const PLAYERLEAVE_LOSTBUILDINGS: u8 = 8;
pub const PLAYERLEAVE_WON: u8 = 9;
pub const PLAYERLEAVE_DRAW: u8 = 10;
pub const PLAYERLEAVE_OBSERVER: u8 = 11;
pub const PLAYERLEAVE_LOBBY: u8 = 13;
pub const PLAYERLEAVE_GPROXY: u8 = 100;
pub const REJECTJOIN_FULL: u8 = 9;
pub const REJECTJOIN_STARTED: u8 = 10;
pub const REJECTJOIN_WRONGPASSWORD: u8 = 27;

// W3GS Protocol
pub const W3GS_PING_FROM_HOST: u8 = 1;            // 0x01
pub const W3GS_SLOTINFOJOIN: u8 = 4;              // 0x04
pub const W3GS_REJECTJOIN: u8 = 5;                // 0x05
pub const W3GS_PLAYERINFO: u8 = 6;                // 0x06
pub const W3GS_PLAYERLEAVE_OTHERS: u8 = 7;        // 0x07
pub const W3GS_GAMELOADED_OTHERS: u8 = 8;         // 0x08
pub const W3GS_SLOTINFO: u8 = 9;                  // 0x09
pub const W3GS_COUNTDOWN_START: u8 = 10;          // 0x0A
pub const W3GS_COUNTDOWN_END: u8 = 11;            // 0x0B
pub const W3GS_INCOMING_ACTION: u8 = 12;          // 0x0C
pub const W3GS_CHAT_FROM_HOST: u8 = 15;           // 0x0F
pub const W3GS_START_LAG: u8 = 16;                // 0x10
pub const W3GS_STOP_LAG: u8 = 17;                 // 0x11
pub const W3GS_HOST_KICK_PLAYER: u8 = 28;         // 0x1C
pub const W3GS_REQJOIN: u8 = 30;                  // 0x1E
pub const W3GS_LEAVEGAME: u8 = 33;                // 0x21
pub const W3GS_GAMELOADED_SELF: u8 = 35;          // 0x23
pub const W3GS_OUTGOING_ACTION: u8 = 38;          // 0x26
pub const W3GS_OUTGOING_KEEPALIVE: u8 = 39;       // 0x27
pub const W3GS_CHAT_TO_HOST: u8 = 40;             // 0x28
pub const W3GS_DROPREQ: u8 = 41;                  // 0x29
pub const W3GS_SEARCHGAME: u8 = 47;               // 0x2F (UDP/LAN)
pub const W3GS_GAMEINFO: u8 = 48;                 // 0x30 (UDP/LAN)
pub const W3GS_CREATEGAME: u8 = 49;               // 0x31 (UDP/LAN)
pub const W3GS_REFRESHGAME: u8 = 50;              // 0x32 (UDP/LAN)
pub const W3GS_DECREATEGAME: u8 = 51;             // 0x33 (UDP/LAN)
pub const W3GS_CHAT_OTHERS: u8 = 52;              // 0x34
pub const W3GS_PING_FROM_OTHERS: u8 = 53;         // 0x35
pub const W3GS_PONG_TO_OTHERS: u8 = 54;           // 0x36
pub const W3GS_MAPCHECK: u8 = 61;                 // 0x3D
pub const W3GS_STARTDOWNLOAD: u8 = 63;            // 0x3F
pub const W3GS_MAPSIZE: u8 = 66;                  // 0x42
pub const W3GS_MAPPART: u8 = 67;                  // 0x43
pub const W3GS_MAPPARTOK: u8 = 68;                // 0x44
pub const W3GS_MAPPARTNOTOK: u8 = 69;             // 0x45 - just a guess, received this packet after forgetting to send a crc in W3GS_MAPPART (f7 45 0a 00 01 02 01 00 00 00)
pub const W3GS_PONG_TO_HOST: u8 = 70;             // 0x46
pub const W3GS_INCOMING_ACTION2: u8 = 72;         // 0x48 - received this packet when there are too many actions to fit in W3GS_INCOMING_ACTION

// ChatToHostType Protocol
pub const CTH_MESSAGE: u8 = 0;          // a chat message
pub const CTH_MESSAGEEXTRA: u8 = 1;     // a chat message with extra flags
pub const CTH_TEAMCHANGE: u8 = 2;       // a team change request
pub const CTH_COLOURCHANGE: u8 = 3;     // a colour change request
pub const CTH_RACECHANGE: u8 = 4;       // a race change request
pub const CTH_HANDICAPCHANGE: u8 = 5;   // a handicap change request

#[derive(Debug)]
pub struct GameProtocol;

impl GameProtocol {
    pub fn receive_w3gs_reqjoin(data: &[u8]) -> Option<IncomingJoinPlayer> {
        // 2 bytes					-> Header
        // 2 bytes					-> Length
        // 4 bytes					-> Host Counter (Game ID)
        // 4 bytes					-> Entry Key (used in LAN)
        // 1 byte					-> ???
        // 2 bytes					-> Listen Port
        // 4 bytes					-> Peer Key
        // null terminated string	-> Name
        // 4 bytes					-> ???
        // 2 bytes					-> InternalPort (???)
        // 4 bytes					-> InternalIP

        if validate_length(&data) && data.len() >= 20 {
            let host_counter = util_byte_array_to_u32(data, false, 4);
            let entry_key = util_byte_array_to_u32(data, false, 8);
            let name = util_extract_cstring(data, 19);

            if !name.is_empty() && data.len() >= name.len() + 30
            {
                let internal_ip: Vec<u8> = data[name.len() + 26..name.len() + 30].to_vec();
                return Some(IncomingJoinPlayer {
                    host_counter,
                    entry_key,
                    name,
                    internal_ip,
                });
            }
        }

        None
    }

    pub fn receive_w3gs_leavegame(data: &Vec<u8>) -> u32 {
        // 2 bytes					-> Header
        // 2 bytes					-> Length
        // 4 bytes					-> Reason
        if validate_length(data) && data.len() >= 8 {
            return util_byte_array_to_u32(data, false, 4);
        }

        return 0;
    }

    pub fn receive_w3gs_gameloaded_self(data: &Vec<u8>) -> bool {
        // 2 bytes					-> Header
        // 2 bytes					-> Length
        return validate_length(data);
    }

    pub fn receive_w3gs_outgoing_action(data: &Vec<u8>, pid: u8) -> Option<IncomingAction> {
        // 2 bytes					-> Header
        // 2 bytes					-> Length
        // 4 bytes					-> CRC
        // remainder of packet		-> Action
        if pid != 255 && validate_length(data) && data.len() >= 8 {
            let crc = data[4..8].to_vec();
            let action = data[8..].to_vec();

            return Some(IncomingAction {
                pid,
                crc,
                action,
            });
        }

        None
    }

    pub fn receive_w3gs_outgoing_keepalive(data: &Vec<u8>) -> u32 {
        // 2 bytes					-> Header
        // 2 bytes					-> Length
        // 1 byte					-> ???
        // 4 bytes					-> CheckSum??? (used in replays)

        if validate_length(data) && data.len() == 9 {
            return util_byte_array_to_u32(data, false, 5);
        }

        return 0;
    }

    pub fn receive_w3gs_chat_to_host(data: &Vec<u8>) -> Option<IncomingChatPlayer> {
        // 2 bytes					-> Header
        // 2 bytes					-> Length
        // 1 byte					-> Total
        // for( 1 .. Total )
        //		1 byte				-> ToPID
        // 1 byte					-> FromPID
        // 1 byte					-> Flag
        // if( Flag == 16 )
        //		null term string	-> Message
        // elseif( Flag == 17 )
        //		1 byte				-> Team
        // elseif( Flag == 18 )
        //		1 byte				-> Colour
        // elseif( Flag == 19 )
        //		1 byte				-> Race
        // elseif( Flag == 20 )
        //		1 byte				-> Handicap
        // elseif( Flag == 32 )
        //		4 bytes				-> ExtraFlags
        //		null term string	-> Message

        if validate_length(data)
        {
            let mut i: usize = 5;
            let total: usize = data[4] as usize;

            if total > 0 && total <= MAX_SLOTS as usize && data.len() >= (i + total).into()
            {
                let to_pids = data[i..i + total].to_vec();
                i += total;
                let from_pid: u8 = data[i];
                let flag: u8 = data[i + 1];
                i += 2;

                if flag == 16 && data.len() >= i + 1
                {
                    // chat message
                    let message = util_extract_cstring(data, i);
                    return Some(IncomingChatPlayer::new_message(from_pid, to_pids, flag, message));
                } else if flag >= 17 && flag <= 20 && data.len() >= i + 1
                {
                    // team/colour/race/handicap change request: flag is immediately followed by a 1-byte value
                    return Some(IncomingChatPlayer::new_with_flag(from_pid, to_pids, flag, data[i]));
                } else if flag == 32 && data.len() >= i + 5
                {
                    // chat message with extra flags
                    let extra_flags = data[i..i + 4].to_vec();
                    let message = util_extract_cstring(data, i + 4);
                    return Some(IncomingChatPlayer::new_messageextra(from_pid, to_pids, flag, message, extra_flags));
                }
            }
        }

        None
    }

    pub fn receive_w3gs_searchgame(data: &Vec<u8>, version: u32) -> bool {
        // 2 bytes					-> Header
        // 2 bytes					-> Length
        // 4 bytes					-> ProductID
        // 4 bytes					-> Version
        // 4 bytes					-> ??? (Zero)

        let product_id: u32 = 1462982736;    // "W3XP"

        if validate_length(data) && data.len() >= 16
        {
            if util_byte_array_to_u32(data, false, 4) == product_id
            {
                if util_byte_array_to_u32(data, false, 8) == version
                {
                    if util_byte_array_to_u32(data, false, 12) == 0 {
                        return true;
                    }
                }
            }
        }

        return false;
    }

    pub fn receive_w3gs_mapsize(data: &Vec<u8>) -> Option<IncomingMapSize> {
        // 2 bytes					-> Header
        // 2 bytes					-> Length
        // 4 bytes					-> ???
        // 1 byte					-> SizeFlag (1 = have map, 3 = continue download)
        // 4 bytes					-> MapSize

        if validate_length(data) && data.len() >= 13 {
            return Some(IncomingMapSize {
                size_flag: data[8],
                map_size: util_byte_array_to_u32(data, false, 9),
            });
        }

        return None;
    }

    pub fn receive_w3gs_mappartok(data: &Vec<u8>) -> u32 {
        // 2 bytes					-> Header
        // 2 bytes					-> Length
        // 1 byte					-> SenderPID
        // 1 byte					-> ReceiverPID
        // 4 bytes					-> ???
        // 4 bytes					-> MapSize
        if validate_length(data) && data.len() >= 14 {
            return util_byte_array_to_u32(data, false, 10);
        }

        return 0;
    }

    pub fn receive_w3gs_pong_to_host(data: &Vec<u8>) -> u32 {
        // 2 bytes					-> Header
        // 2 bytes					-> Length
        // 4 bytes					-> Pong

        // the pong value is just a copy of whatever was sent in SEND_W3GS_PING_FROM_HOST which was GetTicks( ) at the time of sending
        // so as long as we trust that the client isn't trying to fake us out and mess with the pong value we can find the round trip time by simple subtraction
        // (the subtraction is done elsewhere because the very first pong value seems to be 1 and we want to discard that one)

        if validate_length(data) && data.len() >= 8 {
            return util_byte_array_to_u32(data, false, 4);
        }

        return 1;
    }

    pub fn send_w3gs_ping_from_host() -> Vec<u8> {
        let mut packet = vec![];
        // W3GS header constant
        packet.push(W3GS_HEADER_CONSTANT);
        // W3GS_PING_FROM_HOST
        packet.push(W3GS_PING_FROM_HOST);
        // packet length will be assigned later
        packet.push(0);
        // packet length will be assigned later
        packet.push(0);
        // ping value: must be u32 (4 bytes); get_ticks() is u64, and calling to_le_bytes directly would send 8 bytes
        // causing the client to receive a malformed PING and drop the connection (mirrors C++ GetTicks() returning uint32_t)
        packet.extend((get_ticks() as u32).to_le_bytes());
        assign_length(&mut packet);
        return packet;
    }

    pub fn send_w3gs_slotinfojoin(pid: u8, port: Vec<u8>, external_ip: Vec<u8>, slots: &Vec<GameSlot>, random_seed: u32, layout_style: u8, player_slots: u8) -> Vec<u8> {
        let zeros: [u8; 4] = [0, 0, 0, 0];

        let slot_info: Vec<u8> = Self::encode_slot_info(slots, random_seed, layout_style, player_slots);
        let mut packet = vec![];

        if port.len() == 2 && external_ip.len() == 4
        {
            // W3GS header constant
            packet.push(W3GS_HEADER_CONSTANT);
            // W3GS_SLOTINFOJOIN
            packet.push(W3GS_SLOTINFOJOIN);
            // packet length will be assigned later
            packet.push(0);
            // packet length will be assigned later
            packet.push(0);
            // SlotInfo length
            packet.extend((slot_info.len() as u16).to_le_bytes());
            // SlotInfo
            packet.extend(slot_info);
            // PID
            packet.push(pid);
            // AF_INET
            packet.push(2);
            // AF_INET continued...
            packet.push(0);
            // port
            packet.extend(port);
            // external IP
            packet.extend(external_ip);
            // ???
            packet.extend(&zeros);
            // ???
            packet.extend(&zeros);
            assign_length(&mut packet);
        } else {
            warn!("[GAMEPROTO] invalid parameters passed to SEND_W3GS_SLOTINFOJOIN");
        }

        return packet;
    }

    pub fn send_w3gs_rejectjoin(reason: u32) -> Vec<u8> {
        let mut packet = vec![];
        // W3GS header constant
        packet.push(W3GS_HEADER_CONSTANT);
        // W3GS_REJECTJOIN
        packet.push(W3GS_REJECTJOIN);
        // packet length will be assigned later
        packet.push(0);
        // packet length will be assigned later
        packet.push(0);
        // reason
        packet.extend(reason.to_le_bytes());
        assign_length(&mut packet);
        return packet;
    }

    pub fn send_w3gs_playerinfo(pid: u8, name: String, external_ip: Vec<u8>, internal_ip: Vec<u8>) -> Vec<u8> {
        let player_join_counter: [u8; 4] = [2, 0, 0, 0];
        let zeros: [u8; 4] = [0, 0, 0, 0];

        let mut packet = vec![];

        if !name.is_empty() && name.len() <= 15 && external_ip.len() == 4 && internal_ip.len() == 4
        {
            // W3GS header constant
            packet.push(W3GS_HEADER_CONSTANT);
            // W3GS_PLAYERINFO
            packet.push(W3GS_PLAYERINFO);
            // packet length will be assigned later
            packet.push(0);
            // packet length will be assigned later
            packet.push(0);
            // player join counter
            packet.extend(player_join_counter);
            // PID
            packet.push(pid);
            // player name (client ANSI encoding: Chinese names display correctly)
            packet.extend(util_encode_ansi(&name));
            packet.push(0); // name null terminator (matches the default terminator of the C++ AppendByteArrayFast string overload)
            // ???
            packet.push(1);
            // ???
            packet.push(0);
            // AF_INET
            packet.push(2);
            // AF_INET continued...
            packet.push(0);
            // port
            packet.push(0);
            // port continued...
            packet.push(0);
            // external IP
            packet.extend(external_ip);
            // ???
            packet.extend(zeros);
            // ???
            packet.extend(zeros);
            // AF_INET
            packet.push(2);
            // AF_INET continued...
            packet.push(0);
            // port
            packet.push(0);
            // port continued...
            packet.push(0);
            // internal IP
            packet.extend(internal_ip);
            // ???
            packet.extend(zeros);
            // ???
            packet.extend(zeros);
            assign_length(&mut packet);
        } else {
            warn!("[GAMEPROTO] invalid parameters passed to SEND_W3GS_PLAYERINFO");
        }

        return packet;
    }

    pub fn send_w3gs_playerleave_others(pid: u8, left_code: u32) -> Vec<u8> {
        let mut packet = vec![];

        if pid != 255
        {
            // W3GS header constant
            packet.push(W3GS_HEADER_CONSTANT);
            // W3GS_PLAYERLEAVE_OTHERS
            packet.push(W3GS_PLAYERLEAVE_OTHERS);
            // packet length will be assigned later
            packet.push(0);
            // packet length will be assigned later
            packet.push(0);
            // PID
            packet.push(pid);
            // left code (see PLAYERLEAVE_ constants in gameprotocol.h)
            packet.extend(left_code.to_le_bytes());
            assign_length(&mut packet);
        } else {
            warn!("[GAMEPROTO] invalid parameters passed to SEND_W3GS_PLAYERLEAVE_OTHERS");
        }

        return packet;
    }

    pub fn send_w3gs_gameloaded_others(pid: u8) -> Vec<u8> {
        let mut packet = vec![];

        if pid != 255
        {
            // W3GS header constant
            packet.push(W3GS_HEADER_CONSTANT);
            // W3GS_GAMELOADED_OTHERS
            packet.push(W3GS_GAMELOADED_OTHERS);
            // packet length will be assigned later
            packet.push(0);
            // packet length will be assigned later
            packet.push(0);
            // PID
            packet.push(pid);
            assign_length(&mut packet);
        } else {
            warn!("[GAMEPROTO] invalid parameters passed to SEND_W3GS_GAMELOADED_OTHERS");
        }

        return packet;
    }

    pub fn send_w3gs_slotinfo(slots: &Vec<GameSlot>, random_seed: u32, layout_style: u8, player_slots: u8) -> Vec<u8> {
        let slot_info = Self::encode_slot_info(slots, random_seed, layout_style, player_slots);
        let mut packet = vec![];
        // W3GS header constant
        packet.push(W3GS_HEADER_CONSTANT);
        // W3GS_SLOTINFO
        packet.push(W3GS_SLOTINFO);
        // packet length will be assigned later
        packet.push(0);
        // packet length will be assigned later
        packet.push(0);
        // SlotInfo length
        packet.extend((slot_info.len() as u16).to_le_bytes());
        // SlotInfo
        packet.extend(slot_info);
        assign_length(&mut packet);
        return packet;
    }

    pub fn send_w3gs_countdown_start() -> Vec<u8> {
        let mut packet = vec![];
        packet.push(W3GS_HEADER_CONSTANT);        // W3GS header constant
        packet.push(W3GS_COUNTDOWN_START);        // W3GS_COUNTDOWN_START
        packet.push(0);                            // packet length will be assigned later
        packet.push(0);                            // packet length will be assigned later
        assign_length(&mut packet);
        return packet;
    }

    pub fn send_w3gs_countdown_end() -> Vec<u8> {
        let mut packet = vec![];
        packet.push(W3GS_HEADER_CONSTANT);        // W3GS header constant
        packet.push(W3GS_COUNTDOWN_END);            // W3GS_COUNTDOWN_END
        packet.push(0);                            // packet length will be assigned later
        packet.push(0);                            // packet length will be assigned later
        assign_length(&mut packet);
        return packet;
    }

    pub fn send_w3gs_incoming_action(actions: &mut VecDeque<IncomingAction>, send_interval: u16) -> Vec<u8> {
        let mut packet = vec![];
        // W3GS header constant
        packet.push(W3GS_HEADER_CONSTANT);
        // W3GS_INCOMING_ACTION
        packet.push(W3GS_INCOMING_ACTION);
        // packet length will be assigned later
        packet.push(0);
        // packet length will be assigned later
        packet.push(0);
        // send interval
        packet.extend(send_interval.to_le_bytes());

        // create subpacket
        let subpacket: Vec<u8> = Self::incoming_action_subpacket(actions);
        Self::extend_incoming_action_subpacket(&mut packet, &subpacket);
        assign_length(&mut packet);
        return packet;
    }

    pub fn send_w3gs_chat_from_host(from_pid: u8, to_pids: Vec<u8>, flag: u8, flag_extra: Vec<u8>, message: String) -> Vec<u8> {
        let mut packet = vec![];

        if !to_pids.is_empty() && !message.is_empty() && message.len() < 255
        {
            // W3GS header constant
            packet.push(W3GS_HEADER_CONSTANT);
            // W3GS_CHAT_FROM_HOST
            packet.push(W3GS_CHAT_FROM_HOST);
            // packet length will be assigned later
            packet.push(0);
            // packet length will be assigned later
            packet.push(0);
            // number of receivers
            packet.push(to_pids.len() as u8);
            // receivers
            packet.extend(to_pids);
            // sender
            packet.push(from_pid);
            // flag
            packet.push(flag);
            packet.extend(flag_extra);    // extra flag
            packet.extend(util_encode_ansi(&message));    // message (client ANSI: Chinese correct)
            packet.push(0); // message null terminator (matches the default terminator of the C++ AppendByteArrayFast string overload)
            assign_length(&mut packet);
        } else {
            warn!("[GAMEPROTO] invalid parameters passed to SEND_W3GS_CHAT_FROM_HOST");
        }

        return packet;
    }

    pub fn send_w3gs_start_lag(players: &Vec<GamePlayer>, load_in_game: bool) -> Vec<u8> {
        let mut packet = vec![];

        let mut num_laggers: u8 = 0;

        for player in players {
            if load_in_game
            {
                // Fix: the C++ is !GetFinishedLoading() (only those not finished loading count as laggers); the original condition was inverted
                if !player.finished_loading {
                    num_laggers += 1;
                }
            } else {
                if player.lagging {
                    num_laggers += 1;
                }
            }
        }

        if num_laggers > 0
        {
            packet.push(W3GS_HEADER_CONSTANT);    // W3GS header constant
            packet.push(W3GS_START_LAG);            // W3GS_START_LAG
            packet.push(0);                        // packet length will be assigned later
            packet.push(0);                        // packet length will be assigned later
            packet.push(num_laggers);

            for player in players {
                {
                    if load_in_game
                    {
                        // Fix: the C++ is !GetFinishedLoading() (only those not finished loading count as laggers); the original condition was inverted
                        if !player.finished_loading {
                            packet.push(player.pid);
                            packet.extend(0u32.to_le_bytes());
                        }
                    } else {
                        if player.lagging
                        {
                            packet.push(player.pid);
                            // Fix: subtracting u64 and calling to_le_bytes directly would send 8 bytes; C++ GetTicks() is uint32_t and the protocol field is 4 bytes
                            packet.extend(((get_ticks() - player.started_lagging_ticks) as u32).to_le_bytes());
                        }
                    }
                }
            }

            assign_length(&mut packet);
        } else {
            warn!("[GAMEPROTO] no laggers passed to SEND_W3GS_START_LAG");
        }

        return packet;
    }

    pub fn send_w3gs_stop_lag(player: &GamePlayer, load_in_game: bool) -> Vec<u8> {
        let mut packet = vec![];
        packet.push(W3GS_HEADER_CONSTANT);    // W3GS header constant
        packet.push(W3GS_STOP_LAG);            // W3GS_STOP_LAG
        packet.push(0);                        // packet length will be assigned later
        packet.push(0);                        // packet length will be assigned later
        packet.push(player.pid);

        if load_in_game {
            packet.extend(0u32.to_le_bytes());
        } else {
            // Fix: subtracting u64 and calling to_le_bytes directly would send 8 bytes; C++ GetTicks() is uint32_t and the protocol field is 4 bytes
            packet.extend(((get_ticks() - player.started_lagging_ticks) as u32).to_le_bytes());
        }

        assign_length(&mut packet);
        return packet;
    }

    pub fn send_w3gs_searchgame(tft: bool, war3_version: u8) -> Vec<u8> {
        // "WAR3"
        let product_id_roc: [u8; 4] = [51, 82, 65, 87];
        // "W3XP"
        let product_id_tft: [u8; 4] = [80, 88, 51, 87];
        let version: [u8; 4] = [war3_version, 0, 0, 0];
        let unknown: [u8; 4] = [0, 0, 0, 0];

        let mut packet = vec![];
        // W3GS header constant
        packet.push(W3GS_HEADER_CONSTANT);
        // W3GS_SEARCHGAME
        packet.push(W3GS_SEARCHGAME);
        // packet length will be assigned later
        packet.push(0);
        // packet length will be assigned later
        packet.push(0);

        if tft {
            // Product ID (TFT)
            packet.extend(product_id_tft);
        } else {
            // Product ID (ROC)
            packet.extend(product_id_roc);
        }
        // Version
        packet.extend(version);
        // ???
        packet.extend(unknown);
        assign_length(&mut packet);
        return packet;
    }

    pub fn send_w3gs_gameinfo(
        tft: bool, war3_version: u8, map_game_type: Vec<u8>, map_flags: Vec<u8>,
        map_width: Vec<u8>, map_height: Vec<u8>, game_name: String, host_name: String,
        up_time: u32, map_path: String, map_crc: Vec<u8>, slots_total: u32,
        slots_open: u32, port: u16, host_counter: u32, entry_key: u32,
    ) -> Vec<u8> {
        // "WAR3"
        let product_id_roc: [u8; 4] = [51, 82, 65, 87];
        // "W3XP"
        let product_id_tft: [u8; 4] = [80, 88, 51, 87];
        let version: [u8; 4] = [war3_version, 0, 0, 0];
        // Fix: the Unknown2 of C++ SEND_W3GS_GAMEINFO is {1,0,0,0}; it was originally mistakenly written as all 0
        let unknown2: [u8; 4] = [1, 0, 0, 0];

        let mut packet = vec![];

        if map_game_type.len() == 4 &&
            map_flags.len() == 4 &&
            map_width.len() == 2 &&
            map_height.len() == 2 &&
            !game_name.is_empty() &&
            !host_name.is_empty() &&
            !map_path.is_empty() &&
            map_crc.len() == 4
        {
            // make the stat string
            let mut stat_string = vec![];
            stat_string.extend(map_flags);
            stat_string.push(0);
            stat_string.extend(map_width);
            stat_string.extend(map_height);
            stat_string.extend(map_crc);
            stat_string.extend(map_path.into_bytes());
            stat_string.push(0); // map_path null terminator (C++ AppendByteArrayFast string overload)
            stat_string.extend(host_name.into_bytes());
            stat_string.push(0); // host_name null terminator
            stat_string.push(0); // matches the extra StatString.push_back(0) in C++
            stat_string = util_encode_stat_string(&stat_string);

            // make the rest of the packet
            // W3GS header constant
            packet.push(W3GS_HEADER_CONSTANT);
            // W3GS_GAMEINFO
            packet.push(W3GS_GAMEINFO);
            // packet length will be assigned later
            packet.push(0);
            // packet length will be assigned later
            packet.push(0);

            if tft {
                // Product ID (TFT)
                packet.extend(product_id_tft);
            } else {
                // Product ID (ROC)
                packet.extend(product_id_roc);
            }

            // Version
            packet.extend(version);
            // Host Counter
            packet.extend(host_counter.to_le_bytes());
            // Entry Key
            packet.extend(entry_key.to_le_bytes());
            // Game Name
            packet.extend(game_name.into_bytes());
            packet.push(0); // game_name null terminator (C++ AppendByteArrayFast string overload)
            // ??? (maybe game password)
            packet.push(0);
            // Stat String
            packet.extend(stat_string);
            // Stat String null terminator (the stat string is encoded to remove all even numbers i.e. zeros)
            packet.push(0);
            // Slots Total
            packet.extend(slots_total.to_le_bytes());
            // Game Type
            packet.extend(map_game_type);
            // ???
            packet.extend(unknown2);
            // Slots Open
            packet.extend(slots_open.to_le_bytes());
            // time since creation
            packet.extend(up_time.to_le_bytes());
            // port
            packet.extend(port.to_le_bytes());
            assign_length(&mut packet);
        } else {
            warn!("[GAMEPROTO] invalid parameters passed to SEND_W3GS_GAMEINFO");
        }

        return packet;
    }

    pub fn send_w3gs_creategame(tft: bool, war3_version: u8) -> Vec<u8> {
        let product_id_roc: [u8; 4] = [51, 82, 65, 87];
        // "W3XP"
        let product_id_tft: [u8; 4] = [80, 88, 51, 87];
        let version: [u8; 4] = [war3_version, 0, 0, 0];
        let host_counter: [u8; 4] = [1, 0, 0, 0];

        let mut packet = vec![];
        // W3GS header constant
        packet.push(W3GS_HEADER_CONSTANT);
        // W3GS_CREATEGAME
        packet.push(W3GS_CREATEGAME);
        // packet length will be assigned later
        packet.push(0);
        // packet length will be assigned later
        packet.push(0);

        if tft {
            // Product ID (TFT)
            packet.extend(product_id_tft);
        } else {
            // Product ID (ROC)
            packet.extend(product_id_roc);
        }

        // Version
        packet.extend(version);
        // Host Counter
        packet.extend(host_counter);
        assign_length(&mut packet);
        return packet;
    }

    pub fn send_w3gs_refreshgame(players: u32, player_slots: u32) -> Vec<u8> {
        let host_counter = [1, 0, 0, 0];

        let mut packet = vec![];
        // W3GS header constant
        packet.push(W3GS_HEADER_CONSTANT);
        // W3GS_REFRESHGAME
        packet.push(W3GS_REFRESHGAME);
        // packet length will be assigned later
        packet.push(0);
        // packet length will be assigned later
        packet.push(0);
        // Host Counter
        packet.extend(host_counter);
        // Players
        packet.extend(players.to_le_bytes());
        // Player Slots
        packet.extend(player_slots.to_le_bytes());
        assign_length(&mut packet);
        return packet;
    }

    pub fn send_w3gs_decreategame() -> Vec<u8> {
        let host_counter = [1, 0, 0, 0];

        let mut packet = vec![];
        // W3GS header constant
        packet.push(W3GS_HEADER_CONSTANT);
        // W3GS_DECREATEGAME
        packet.push(W3GS_DECREATEGAME);
        // packet length will be assigned later
        packet.push(0);
        // packet length will be assigned later
        packet.push(0);
        // Host Counter
        packet.extend(host_counter);
        assign_length(&mut packet);
        return packet;
    }

    pub fn send_w3gs_mapcheck(
        map_path: String, map_size: Vec<u8>, map_info: Vec<u8>,
        map_crc: Vec<u8>, map_sha1: Vec<u8>) -> Vec<u8> {
        let unknown = [1, 0, 0, 0];

        let mut packet = vec![];

        if !map_path.is_empty() &&
            map_size.len() == 4 &&
            map_info.len() == 4 &&
            map_crc.len() == 4 &&
            map_sha1.len() == 20
        {
            // W3GS header constant
            packet.push(W3GS_HEADER_CONSTANT);
            // W3GS_MAPCHECK
            packet.push(W3GS_MAPCHECK);
            // packet length will be assigned later
            packet.push(0);
            // packet length will be assigned later
            packet.push(0);
            // ???
            packet.extend(unknown);
            // map path (client ANSI: Chinese filenames correct)
            packet.extend(util_encode_ansi(&map_path));
            packet.push(0); // map path null terminator (matches the default terminator of the C++ AppendByteArrayFast string overload)
            // map size
            packet.extend(map_size);
            // map info
            packet.extend(map_info);
            // map crc
            packet.extend(map_crc);
            // map sha1
            packet.extend(map_sha1);
            assign_length(&mut packet);
        } else {
            warn!("[GAMEPROTO] invalid parameters passed to SEND_W3GS_MAPCHECK");
        }

        return packet;
    }

    pub fn send_w3gs_startdownload(from_pid: u8) -> Vec<u8> {
        let unknown = [1, 0, 0, 0];

        let mut packet = vec![];
        // W3GS header constant
        packet.push(W3GS_HEADER_CONSTANT);
        // W3GS_STARTDOWNLOAD
        packet.push(W3GS_STARTDOWNLOAD);
        // packet length will be assigned later
        packet.push(0);
        // packet length will be assigned later
        packet.push(0);
        // ???
        packet.extend(unknown);
        // from PID
        packet.push(from_pid);
        assign_length(&mut packet);
        return packet;
    }

    pub fn send_w3gs_mappart(from_pid: u8, to_pid: u8, start: usize, map_data: &[u8]) -> Vec<u8> {
        let unknown = [1, 0, 0, 0];

        let mut packet = vec![];

        if start < map_data.len()
        {
            // W3GS header constant
            packet.push(W3GS_HEADER_CONSTANT);
            // W3GS_MAPPART
            packet.push(W3GS_MAPPART);
            // packet length will be assigned later
            packet.push(0);
            // packet length will be assigned later
            packet.push(0);
            // to PID
            packet.push(to_pid);
            // from PID
            packet.push(from_pid);
            // ???
            packet.extend(unknown);
            // start position
            // Fix: the original usize.to_le_bytes() would send 8 bytes; the protocol uses u32 (4 bytes)
            packet.extend((start as u32).to_le_bytes());

            // calculate end position (don't send more than 1442 map bytes in one packet)
            let mut end: usize = start + 1442;

            if end > map_data.len() {
                end = map_data.len();
            }

            // calculate crc
            // Fix: 1) the slice range was originally mistakenly written as [start..end - start] (panics when start>0)
            //       2) the CRC originally sent only 2 bytes; the protocol uses the full 4 bytes (mirrors C++ SEND_W3GS_MAPPART)
            let chunk = &map_data[start..end];
            let crc32 = util_calc_crc32(chunk);
            packet.extend(crc32.to_le_bytes());

            // map data
            packet.extend(chunk);
            assign_length(&mut packet);
        } else {
            warn!("[GAMEPROTO] invalid parameters passed to SEND_W3GS_MAPPART");
        }

        return packet;
    }
    /// START_LAG (pid version, used by GameActor; mirrors C++ SEND_W3GS_START_LAG)
    /// laggers: (pid, milliseconds already lagged -- 0 at the start)
    pub fn send_w3gs_start_lag_pids(laggers: &[(u8, u32)]) -> Vec<u8> {
        let mut packet = vec![];
        if laggers.is_empty() {
            warn!("[GAMEPROTO] no laggers passed to send_w3gs_start_lag_pids");
            return packet;
        }
        packet.push(W3GS_HEADER_CONSTANT);
        packet.push(W3GS_START_LAG);
        packet.push(0);
        packet.push(0);
        packet.push(laggers.len() as u8);
        for (pid, lag_ms) in laggers {
            packet.push(*pid);
            packet.extend(lag_ms.to_le_bytes());
        }
        assign_length(&mut packet);
        packet
    }

    /// STOP_LAG (pid version; mirrors C++ SEND_W3GS_STOP_LAG)
    pub fn send_w3gs_stop_lag_pid(pid: u8, lag_ms: u32) -> Vec<u8> {
        let mut packet = vec![];
        packet.push(W3GS_HEADER_CONSTANT);
        packet.push(W3GS_STOP_LAG);
        packet.push(0);
        packet.push(0);
        packet.push(pid);
        packet.extend(lag_ms.to_le_bytes());
        assign_length(&mut packet);
        packet
    }

    pub fn send_w3gs_incoming_action2(actions: &mut VecDeque<IncomingAction>) -> Vec<u8> {
        let mut packet = vec![];
        // W3GS header constant
        packet.push(W3GS_HEADER_CONSTANT);
        // W3GS_INCOMING_ACTION2
        packet.push(W3GS_INCOMING_ACTION2);
        // packet length will be assigned later
        packet.push(0);
        // packet length will be assigned later
        packet.push(0);
        // ??? (send interval?)
        packet.push(0);
        // ??? (send interval?)
        packet.push(0);

        // create subpacket
        let subpacket: Vec<u8> = Self::incoming_action_subpacket(actions);
        Self::extend_incoming_action_subpacket(&mut packet, &subpacket);

        assign_length(&mut packet);
        return packet;
    }

    fn encode_slot_info(slots: &Vec<GameSlot>, random_seed: u32, layout_style: u8, player_slots: u8) -> Vec<u8> {
        let mut slot_info: Vec<u8> = vec![];
        // number of slots
        slot_info.push(slots.len() as u8);

        for i in 0..slots.len() {
            slot_info.extend(slots[i].get_byte_array());
        }

        // random seed
        slot_info.extend(random_seed.to_le_bytes());
        // LayoutStyle (0 = melee, 1 = custom forces, 3 = custom forces + fixed player settings)
        slot_info.push(layout_style);
        // number of player slots (non observer)
        slot_info.push(player_slots);
        return slot_info;
    }

    fn incoming_action_subpacket(actions: &mut VecDeque<IncomingAction>) -> Vec<u8> {
        let mut subpacket: Vec<u8> = vec![];
        while let Some(action) = actions.pop_front() {
            subpacket.push(action.pid);
            let action_len = action.action.len() as u16;
            subpacket.extend(&action_len.to_le_bytes());
            subpacket.extend(&action.action);
        }

        return subpacket;
    }

    fn extend_incoming_action_subpacket(packet: &mut Vec<u8>, subpacket: &Vec<u8>) {
        if subpacket.len() > 0 {
            // calculate crc (we only care about the first 2 bytes though)
            let crc32 = util_calc_crc32(&subpacket);
            let crc32 = crc32.to_le_bytes();
            let crc32 = &crc32[0..2];

            // finish subpacket
            // crc
            packet.extend(crc32);
            // subpacket
            packet.extend(subpacket);
        }
    }
}

#[derive(Debug)]
pub struct IncomingJoinPlayer {
    pub host_counter: u32,
    pub entry_key: u32,
    pub name: String,
    pub internal_ip: Vec<u8>,
}

#[derive(Debug)]
pub struct IncomingAction {
    pub pid: u8,
    pub crc: Vec<u8>,
    pub action: Vec<u8>,
}

impl IncomingAction {
    pub fn get_length(&self) -> usize {
        self.action.len() + 3
    }
}

pub struct IncomingChatPlayer {
    pub host_type: u8,
    pub from_pid: u8,
    pub to_pids: Vec<u8>,
    pub flag: u8,
    pub message: String,
    pub extra_flags: Vec<u8>,
    /// The value byte carried by team/colour/race/handicap change requests (flag 17-20)
    pub byte: u8,
}

impl IncomingChatPlayer {
    pub fn new_message(from_pid: u8, to_pids: Vec<u8>, flag: u8, message: String) -> Self {
        Self {
            host_type: CTH_MESSAGE,
            from_pid,
            to_pids,
            flag,
            message,
            extra_flags: vec![],
            byte: 0,
        }
    }

    pub fn new_messageextra(from_pid: u8, to_pids: Vec<u8>, flag: u8, message: String, extra_flags: Vec<u8>) -> Self {
        Self {
            host_type: CTH_MESSAGEEXTRA,
            from_pid,
            to_pids,
            flag,
            message,
            extra_flags,
            byte: 0,
        }
    }

    pub fn new_with_flag(from_pid: u8, to_pids: Vec<u8>, flag: u8, byte: u8) -> Self {
        Self {
            host_type: match flag {
                17 => CTH_TEAMCHANGE,
                18 => CTH_COLOURCHANGE,
                19 => CTH_RACECHANGE,
                20 => CTH_HANDICAPCHANGE,
                _ => CTH_MESSAGE
            },
            from_pid,
            to_pids,
            flag,
            message: String::new(),
            extra_flags: vec![],
            byte,
        }
    }
}

pub struct IncomingMapSize {
    pub size_flag: u8,
    pub map_size: u32,
}
