//! Battle.net (BNCS) protocol encode/decode (mirrors C++ bnetprotocol.cpp).
//! Full implementation of all SEND_/RECEIVE_.
//!
//! Packet format: `[0xFF][id][len u16 LE (including the 4-byte header)][payload...]`.
//! The `data` received by RECEIVE_ is the complete packet including the header (same as C++, parsed with header-inclusive offsets).

use std::net::Ipv4Addr;

use tracing::warn;

use crate::util::*;

pub const BNET_HEADER_CONSTANT: u8 = 255;
pub const SID_NULL: u8 = 0;                     // 0x0
pub const SID_STOPADV: u8 = 2;                  // 0x2
pub const SID_GETADVLISTEX: u8 = 9;             // 0x9
pub const SID_ENTERCHAT: u8 = 10;               // 0xA
pub const SID_JOINCHANNEL: u8 = 12;             // 0xC
pub const SID_CHATCOMMAND: u8 = 14;             // 0xE
pub const SID_CHATEVENT: u8 = 15;               // 0xF
pub const SID_CHECKAD: u8 = 21;                 // 0x15
pub const SID_STARTADVEX3: u8 = 28;             // 0x1C
pub const SID_DISPLAYAD: u8 = 33;               // 0x21
pub const SID_NOTIFYJOIN: u8 = 34;              // 0x22
pub const SID_PING: u8 = 37;                    // 0x25
pub const SID_LOGONRESPONSE: u8 = 41;           // 0x29
pub const SID_NETGAMEPORT: u8 = 69;             // 0x45
pub const SID_AUTH_INFO: u8 = 80;               // 0x50
pub const SID_AUTH_CHECK: u8 = 81;              // 0x51
pub const SID_AUTH_ACCOUNTLOGON: u8 = 83;       // 0x53
pub const SID_AUTH_ACCOUNTLOGONPROOF: u8 = 84;  // 0x54
pub const SID_WARDEN: u8 = 94;                  // 0x5E
pub const SID_FRIENDLIST: u8 = 101;             // 0x65
pub const SID_FRIENDSUPDATE: u8 = 102;          // 0x66
pub const SID_CLANMEMBERLIST: u8 = 125;         // 0x7D
pub const SID_CLANMEMBERSTATUSCHANGE: u8 = 127; // 0x7F

pub const KR_GOOD: u32 = 0;
pub const KR_OLD_GAME_VERSION: u32 = 256;
pub const KR_INVALID_VERSION: u32 = 257;
pub const KR_ROC_KEY_IN_USE: u32 = 513;
pub const KR_TFT_KEY_IN_USE: u32 = 529;

pub const EID_SHOWUSER: u8 = 1;  // received when you join a channel
pub const EID_JOIN: u8 = 2;
pub const EID_LEAVE: u8 = 3;
pub const EID_WHISPER: u8 = 4;
pub const EID_TALK: u8 = 5;
pub const EID_BROADCAST: u8 = 6;
pub const EID_CHANNEL: u8 = 7;
pub const EID_USERFLAGS: u8 = 9;
pub const EID_WHISPERSENT: u8 = 10;
pub const EID_CHANNELFULL: u8 = 13;
pub const EID_CHANNELDOESNOTEXIST: u8 = 14;
pub const EID_CHANNELRESTRICTED: u8 = 15;
pub const EID_INFO: u8 = 18;
pub const EID_ERROR: u8 = 19;
pub const EID_EMOTE: u8 = 23;
pub const EID_IRC: u8 = 0; // internal flag only

#[derive(Debug)]
pub struct BNetProtocol {
    client_token: Vec<u8>,          // set in constructor
    login_type: Vec<u8>,            // set in RECEIVE_SID_AUTH_INFO
    server_token: Vec<u8>,          // set in RECEIVE_SID_AUTH_INFO
    mpqfile_time: Vec<u8>,          // set in RECEIVE_SID_AUTH_INFO
    ix86ver_file_name: String,      // set in RECEIVE_SID_AUTH_INFO
    value_string_formula: String,   // set in RECEIVE_SID_AUTH_INFO
    key_state: Vec<u8>,             // set in RECEIVE_SID_AUTH_CHECK
    key_state_description: String,  // set in RECEIVE_SID_AUTH_CHECK
    salt: Vec<u8>,                  // set in RECEIVE_SID_AUTH_ACCOUNTLOGON
    server_public_key: Vec<u8>,     // set in RECEIVE_SID_AUTH_ACCOUNTLOGON
    unique_name: String,            // set in RECEIVE_SID_ENTERCHAT
}

impl BNetProtocol {
    pub fn new() -> Self {
        Self {
            // Fixed client token used by GHost
            client_token: vec![220, 1, 203, 7],
            login_type: vec![],
            server_token: vec![],
            mpqfile_time: vec![],
            ix86ver_file_name: String::new(),
            value_string_formula: String::new(),
            key_state: vec![],
            key_state_description: String::new(),
            salt: vec![],
            server_public_key: vec![],
            unique_name: String::new(),
        }
    }

    // ---- getters ----

    pub fn get_client_token(&self) -> &[u8] {
        &self.client_token
    }

    pub fn get_server_token(&self) -> &[u8] {
        &self.server_token
    }

    pub fn get_mpq_file_time(&self) -> &[u8] {
        &self.mpqfile_time
    }

    pub fn get_ix86_ver_file_name(&self) -> &str {
        &self.ix86ver_file_name
    }

    pub fn get_value_string_formula(&self) -> &str {
        &self.value_string_formula
    }

    pub fn get_key_state(&self) -> &[u8] {
        &self.key_state
    }

    pub fn get_key_state_description(&self) -> &str {
        &self.key_state_description
    }

    /// Keeps the old spelling (used by bnet.rs): salt
    pub fn get_slat(&self) -> &[u8] {
        &self.salt
    }

    pub fn get_salt(&self) -> &[u8] {
        &self.salt
    }

    pub fn get_server_public_key(&self) -> &[u8] {
        &self.server_public_key
    }

    pub fn get_unique_name(&self) -> &str {
        &self.unique_name
    }

    // ---- RECEIVE ----

    pub fn receive_sid_null(&self, data: &[u8]) -> bool {
        validate_length(data)
    }

    /// 2+2 header/len, 4 GamesFound, then 10 ??? / 2 port / 4 ip / cstring name / 2 ??? / 8 hostcounter
    pub fn receive_sid_getadvlistex(&self, data: &[u8]) -> Option<IncomingGameHost> {
        if !validate_length(data) || data.len() < 8 {
            return None;
        }
        let games_found = util_byte_array_to_u32(data, false, 4);
        if games_found == 0 || data.len() < 25 {
            return None;
        }
        let port = util_byte_array_to_u16(data, false, 18);
        let ip = data[20..24].to_vec();
        let game_name = util_extract_cstring(data, 24);
        if data.len() < game_name.len() + 35 {
            return None;
        }
        // host counter: 4 hex bytes (here we simply take the 8 ascii-hex digits, leaving parsing to the caller)
        let host_counter = data[game_name.len() + 27..game_name.len() + 35].to_vec();
        Some(IncomingGameHost {
            game_name,
            ip,
            host_counter,
            port,
        })
    }

    pub fn receive_sid_enterchat(&mut self, data: &[u8]) -> bool {
        if validate_length(data) && data.len() >= 5 {
            self.unique_name = util_extract_cstring(data, 4);
            return true;
        }
        false
    }

    /// 4 EventID / 4 ??? / 4 Ping / 12 ??? / cstring User / cstring Message
    pub fn receive_sid_chatevent(&self, data: &[u8]) -> Option<IncomingChatEvent> {
        if !validate_length(data) || data.len() < 29 {
            return None;
        }
        let event_id = util_byte_array_to_u32(data, false, 4);
        let ping = util_byte_array_to_u32(data, false, 12);
        let user = util_extract_cstring(data, 28);
        let message = util_extract_cstring(data, user.len() + 29);

        let event = event_id as u8;
        match event {
            EID_SHOWUSER | EID_JOIN | EID_LEAVE | EID_WHISPER | EID_TALK | EID_BROADCAST
            | EID_CHANNEL | EID_USERFLAGS | EID_WHISPERSENT | EID_CHANNELFULL
            | EID_CHANNELDOESNOTEXIST | EID_CHANNELRESTRICTED | EID_INFO | EID_ERROR
            | EID_EMOTE => Some(IncomingChatEvent {
                chat_event: event,
                ping,
                user,
                message,
            }),
            _ => None,
        }
    }

    pub fn receive_sid_checkad(&self, data: &[u8]) -> bool {
        validate_length(data)
    }

    pub fn receive_sid_startadvex3(&self, data: &[u8]) -> bool {
        // 4 bytes Status; 0 = success
        validate_length(data) && data.len() >= 8 && util_byte_array_to_u32(data, false, 4) == 0
    }

    pub fn receive_sid_ping(&self, data: &[u8]) -> Vec<u8> {
        if validate_length(data) && data.len() >= 8 {
            data[4..8].to_vec()
        } else {
            Vec::new()
        }
    }

    /// 4 LogonType / 4 ServerToken / 4 ??? / 8 MPQFileTime / cstring IX86VerFileName / cstring ValueStringFormula
    pub fn receive_sid_auth_info(&mut self, data: &[u8]) -> bool {
        if validate_length(data) && data.len() >= 25 {
            self.login_type = data[4..8].to_vec();
            self.server_token = data[8..12].to_vec();
            self.mpqfile_time = data[16..24].to_vec();
            self.ix86ver_file_name = util_extract_cstring(data, 24);
            self.value_string_formula =
                util_extract_cstring(data, self.ix86ver_file_name.len() + 25);
            return true;
        }
        false
    }

    /// 4 KeyState / cstring KeyStateDescription; only KeyState == KR_GOOD is success
    pub fn receive_sid_auth_check(&mut self, data: &[u8]) -> bool {
        if validate_length(data) && data.len() >= 9 {
            self.key_state = data[4..8].to_vec();
            self.key_state_description = util_extract_cstring(data, 8);
            return util_byte_array_to_u32(&self.key_state, false, 0) == KR_GOOD;
        }
        false
    }

    /// 4 Status; if 0, followed by 32 Salt + 32 ServerPublicKey
    pub fn receive_sid_auth_accountlogon(&mut self, data: &[u8]) -> bool {
        if validate_length(data) && data.len() >= 8 {
            let status = util_byte_array_to_u32(data, false, 4);
            if status == 0 && data.len() >= 72 {
                self.salt = data[8..40].to_vec();
                self.server_public_key = data[40..72].to_vec();
                return true;
            }
        }
        false
    }

    /// 4 Status; 0 or 0xE is treated as success
    pub fn receive_sid_auth_accountlogonproof(&self, data: &[u8]) -> bool {
        if validate_length(data) && data.len() >= 8 {
            let status = util_byte_array_to_u32(data, false, 4);
            return status == 0 || status == 0xE;
        }
        false
    }

    /// 1 Total, then each entry: cstring Account / 1 Status / 1 Area / 4 ??? / cstring Location
    pub fn receive_sid_friendlist(&self, data: &[u8]) -> Vec<String> {
        let mut friends = Vec::new();
        if validate_length(data) && data.len() >= 5 {
            let mut i = 5usize;
            let mut total = data[4];
            while total > 0 {
                total -= 1;
                if data.len() < i + 1 {
                    break;
                }
                let account = util_extract_cstring(data, i);
                i += account.len() + 1;
                if data.len() < i + 7 {
                    break;
                }
                i += 6; // 1 status + 1 area + 4 ???
                let location = util_extract_cstring(data, i);
                i += location.len() + 1;
                friends.push(account);
            }
        }
        friends
    }

    /// 4 ??? / 1 Total, then each entry: cstring Name / 1 Rank / 1 Status / cstring Location
    pub fn receive_sid_clanmemberlist(&self, data: &[u8]) -> Vec<String> {
        let mut clan = Vec::new();
        if validate_length(data) && data.len() >= 9 {
            let mut i = 9usize;
            let mut total = data[8];
            while total > 0 {
                total -= 1;
                if data.len() < i + 1 {
                    break;
                }
                let name = util_extract_cstring(data, i);
                i += name.len() + 1;
                if data.len() < i + 3 {
                    break;
                }
                i += 2; // rank + status
                let location = util_extract_cstring(data, i);
                i += location.len() + 1;
                clan.push(name);
            }
        }
        clan
    }

    // ---- SEND ----

    pub fn send_protocol_initialize_selector(&self) -> Vec<u8> {
        vec![1]
    }

    fn header(id: u8) -> Vec<u8> {
        vec![BNET_HEADER_CONSTANT, id, 0, 0]
    }

    pub fn send_sid_null(&self) -> Vec<u8> {
        let mut p = Self::header(SID_NULL);
        assign_length(&mut p);
        p
    }

    pub fn send_sid_stopadv(&self) -> Vec<u8> {
        let mut p = Self::header(SID_STOPADV);
        assign_length(&mut p);
        p
    }

    pub fn send_sid_getadvlistex(&self, game_name: &str) -> Vec<u8> {
        let mut p = Self::header(SID_GETADVLISTEX);
        p.extend_from_slice(&[255, 3, 0, 0]); // Map Filter
        p.extend_from_slice(&[255, 3, 0, 0]);
        p.extend_from_slice(&[0, 0, 0, 0]);
        p.extend_from_slice(&[1, 0, 0, 0]); // max games to list
        p.extend(util_encode_ansi(game_name)); // client ANSI: Chinese game names correct
        p.push(0); // game_name null terminator (matches the default terminator of the C++ AppendByteArrayFast string overload); originally missing, which would shift the following two NULLs forward
        p.push(0); // game password NULL
        p.push(0); // game stats NULL
        assign_length(&mut p);
        p
    }

    pub fn send_sid_enterchat(&self) -> Vec<u8> {
        let mut p = Self::header(SID_ENTERCHAT);
        p.push(0); // Account Name NULL
        p.push(0); // Stat String NULL
        assign_length(&mut p);
        p
    }

    pub fn send_sid_joinchannel(&self, channel: &str) -> Vec<u8> {
        let mut p = Self::header(SID_JOINCHANNEL);
        if !channel.is_empty() {
            p.extend_from_slice(&[2, 0, 0, 0]); // no-create join
        } else {
            p.extend_from_slice(&[1, 0, 0, 0]); // first join
        }
        p.extend_from_slice(channel.as_bytes());
        p.push(0);
        assign_length(&mut p);
        p
    }

    pub fn send_sid_chatcommand(&self, command: &str) -> Vec<u8> {
        let mut p = Self::header(SID_CHATCOMMAND);
        // Chat/whisper content uses client ANSI (Chinese correct); pure commands are ASCII and unaffected
        p.extend(util_encode_ansi(command));
        p.push(0);
        assign_length(&mut p);
        p
    }

    pub fn send_sid_checkad(&self) -> Vec<u8> {
        let mut p = Self::header(SID_CHECKAD);
        p.extend_from_slice(&[0u8; 16]); // 4 × ???
        assign_length(&mut p);
        p
    }

    pub fn send_sid_startadvex3(
        &self,
        state: u8,
        map_game_type: &[u8],
        map_flags: &[u8],
        map_width: &[u8],
        map_height: &[u8],
        game_name: &str,
        host_name: &str,
        up_time: u32,
        map_path: &str,
        map_crc: &[u8],
        map_sha1: &[u8],
        host_counter: u32,
    ) -> Vec<u8> {
        // host counter → 8 hex digits, zero-padded then character order reversed
        let mut host_counter_string = util_to_hex_string(host_counter);
        while host_counter_string.len() < 8 {
            host_counter_string.insert(0, '0');
        }
        host_counter_string = host_counter_string.chars().rev().collect();

        // Assemble the stat string (mirrors game_base field order) then encode
        let mut stat_string: Vec<u8> = Vec::new();
        stat_string.extend_from_slice(map_flags);
        stat_string.push(0);
        stat_string.extend_from_slice(map_width);
        stat_string.extend_from_slice(map_height);
        stat_string.extend_from_slice(map_crc);
        stat_string.extend(util_encode_ansi(map_path));
        stat_string.push(0);
        stat_string.extend(util_encode_ansi(host_name));
        stat_string.push(0); // host_name terminator
        stat_string.push(0); // matches the extra push_back(0) at C++ bnetprotocol.cpp:690; without it everything from mapSHA1 onward is misaligned
        stat_string.extend_from_slice(map_sha1);
        let stat_string = util_encode_stat_string(&stat_string);

        if map_game_type.len() != 4
            || map_flags.len() != 4
            || map_width.len() != 2
            || map_height.len() != 2
            || game_name.is_empty()
            || host_name.is_empty()
            || map_path.is_empty()
            || map_crc.len() != 4
            || map_sha1.len() != 20
            || stat_string.len() >= 128
            || host_counter_string.len() != 8
        {
            warn!("[BNETPROTO] invalid parameters passed to send_sid_startadvex3");
            return Vec::new();
        }

        let mut p = Self::header(SID_STARTADVEX3);
        p.push(state); // 16 public / 17 private / 18 close
        p.extend_from_slice(&[0, 0, 0]); // state continued
        p.extend_from_slice(&up_time.to_le_bytes());
        p.extend_from_slice(map_game_type);
        p.extend_from_slice(&[255, 3, 0, 0]); // ???
        p.extend_from_slice(&[0, 0, 0, 0]); // custom game
        p.extend(util_encode_ansi(game_name)); // client ANSI: Chinese game names correct
        p.push(0); // game name null terminator (matches C++ AppendByteArrayFast terminator=true)
        p.push(0); // empty game password null (matches C++ push_back(0)); without it PVPGN swallows the rest as the password and drops the packet
        // slots free: matches C++ `if( MAX_SLOTS > 12 ) push_back(110) else push_back(98)`
        // this project's gameslot::MAX_SLOTS = 24; the original hard-coded 98 (11 PIDs) would under-allocate PIDs
        if crate::core::gameslot::MAX_SLOTS > 12 {
            p.push(110);
        } else {
            p.push(98);
        }
        p.extend_from_slice(host_counter_string.as_bytes());
        p.extend_from_slice(&stat_string);
        p.push(0); // stat string null terminator
        assign_length(&mut p);
        p
    }

    pub fn send_sid_notifyjoin(&self, game_name: &str) -> Vec<u8> {
        let mut p = Self::header(SID_NOTIFYJOIN);
        p.extend_from_slice(&[0, 0, 0, 0]); // Product ID
        p.extend_from_slice(&[14, 0, 0, 0]); // Product Version (W3 = 14)
        p.extend(util_encode_ansi(game_name)); // client ANSI: Chinese game names correct
        p.push(0); // game_name null terminator (matches the default terminator of the C++ AppendByteArrayFast string overload); originally missing
        p.push(0); // game password NULL
        assign_length(&mut p);
        p
    }

    pub fn send_sid_ping(&self, ping_value: &[u8]) -> Vec<u8> {
        if ping_value.len() != 4 {
            warn!("[BNETPROTO] invalid parameters passed to send_sid_ping");
            return Vec::new();
        }
        let mut p = Self::header(SID_PING);
        p.extend_from_slice(ping_value);
        assign_length(&mut p);
        p
    }

    pub fn send_sid_logonresponse(
        &self,
        client_token: &[u8],
        server_token: &[u8],
        password_hash: &[u8],
        account_name: &str,
    ) -> Vec<u8> {
        let mut p = Self::header(SID_LOGONRESPONSE);
        p.extend_from_slice(client_token);
        p.extend_from_slice(server_token);
        p.extend_from_slice(password_hash);
        p.extend_from_slice(account_name.as_bytes());
        p.push(0);
        assign_length(&mut p);
        p
    }

    pub fn send_sid_netgameport(&self, server_port: u16) -> Vec<u8> {
        let mut p = Self::header(SID_NETGAMEPORT);
        p.extend_from_slice(&server_port.to_le_bytes());
        assign_length(&mut p);
        p
    }

    pub fn send_sid_auth_info(
        &self,
        ver: u8,
        tft: bool,
        locale_id: u32,
        country_abbrev: &str,
        country: &str,
    ) -> Vec<u8> {
        let mut p = Self::header(SID_AUTH_INFO);
        p.extend_from_slice(&[0, 0, 0, 0]); // Protocol ID
        p.extend_from_slice(&[54, 56, 88, 73]); // Platform "IX86"
        if tft {
            p.extend_from_slice(&[80, 88, 51, 87]); // "W3XP"
        } else {
            p.extend_from_slice(&[51, 82, 65, 87]); // "WAR3"
        }
        p.extend_from_slice(&[ver, 0, 0, 0]); // Version
        p.extend_from_slice(&[83, 85, 110, 101]); // Language "enUS"
        p.extend_from_slice(&[127, 0, 0, 1]); // Local IP
        p.extend_from_slice(&[44, 1, 0, 0]); // Time Zone Bias (300)
        p.extend_from_slice(&locale_id.to_le_bytes()); // Locale ID
        p.extend_from_slice(&locale_id.to_le_bytes()); // Language ID
        p.extend_from_slice(country_abbrev.as_bytes());
        p.push(0);
        p.extend_from_slice(country.as_bytes());
        p.push(0);
        assign_length(&mut p);
        p
    }

    pub fn send_sid_auth_check(
        &self,
        tft: bool,
        client_token: &[u8],
        exe_version: &[u8],
        exe_version_hash: &[u8],
        key_info_roc: &[u8],
        key_info_tft: &[u8],
        exe_info: &str,
        key_owner_name: &str,
    ) -> Vec<u8> {
        let num_keys: u32 = if tft { 2 } else { 1 };

        if client_token.len() != 4
            || exe_version.len() != 4
            || exe_version_hash.len() != 4
            || key_info_roc.len() != 36
            || (tft && key_info_tft.len() != 36)
        {
            warn!("[BNETPROTO] invalid parameters passed to send_sid_auth_check");
            return Vec::new();
        }

        let mut p = Self::header(SID_AUTH_CHECK);
        p.extend_from_slice(client_token);
        p.extend_from_slice(exe_version);
        p.extend_from_slice(exe_version_hash);
        p.extend_from_slice(&num_keys.to_le_bytes()); // number of keys
        p.extend_from_slice(&0u32.to_le_bytes()); // using spawn = 0
        p.extend_from_slice(key_info_roc);
        if tft {
            p.extend_from_slice(key_info_tft);
        }
        p.extend_from_slice(exe_info.as_bytes());
        p.push(0);
        p.extend_from_slice(key_owner_name.as_bytes());
        p.push(0);
        assign_length(&mut p);
        p
    }

    pub fn send_sid_auth_accountlogon(&self, client_public_key: &[u8], account_name: &str) -> Vec<u8> {
        if client_public_key.len() != 32 {
            warn!("[BNETPROTO] invalid parameters passed to send_sid_auth_accountlogon");
            return Vec::new();
        }
        let mut p = Self::header(SID_AUTH_ACCOUNTLOGON);
        p.extend_from_slice(client_public_key);
        p.extend_from_slice(account_name.as_bytes());
        p.push(0);
        assign_length(&mut p);
        p
    }

    pub fn send_sid_auth_accountlogonproof(&self, client_password_proof: &[u8]) -> Vec<u8> {
        if client_password_proof.len() != 20 {
            warn!("[BNETPROTO] invalid parameters passed to send_sid_auth_accountlogonproof");
            return Vec::new();
        }
        let mut p = Self::header(SID_AUTH_ACCOUNTLOGONPROOF);
        p.extend_from_slice(client_password_proof);
        assign_length(&mut p);
        p
    }

    pub fn send_sid_friendlist(&self) -> Vec<u8> {
        let mut p = Self::header(SID_FRIENDLIST);
        assign_length(&mut p);
        p
    }

    pub fn send_sid_clanmemberlist(&self) -> Vec<u8> {
        let mut p = Self::header(SID_CLANMEMBERLIST);
        p.extend_from_slice(&[0, 0, 0, 0]); // cookie
        assign_length(&mut p);
        p
    }
}

#[derive(Debug, Clone)]
pub struct IncomingGameHost {
    pub game_name: String,
    pub ip: Vec<u8>,
    pub host_counter: Vec<u8>,
    pub port: u16,
}

impl IncomingGameHost {
    pub fn get_ip_string(&self) -> String {
        if self.ip.len() < 4 {
            return String::new();
        }
        Ipv4Addr::new(self.ip[0], self.ip[1], self.ip[2], self.ip[3]).to_string()
    }
}

#[derive(Debug, Clone)]
pub struct IncomingChatEvent {
    pub user: String,
    pub message: String,
    /// Originally an ENUM, changed to u8
    pub chat_event: u8,
    pub ping: u32,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn set_len(mut p: Vec<u8>) -> Vec<u8> {
        assign_length(&mut p);
        p
    }

    #[test]
    fn send_headers_have_valid_length() {
        let proto = BNetProtocol::new();
        for p in [
            proto.send_sid_null(),
            proto.send_sid_stopadv(),
            proto.send_sid_enterchat(),
            proto.send_sid_friendlist(),
            proto.send_sid_clanmemberlist(),
            proto.send_sid_joinchannel("The Void"),
            proto.send_sid_chatcommand("/w x hi"),
            proto.send_sid_netgameport(6112),
        ] {
            assert_eq!(p[0], BNET_HEADER_CONSTANT);
            assert!(validate_length(&p), "length field must match packet size");
        }
    }

    #[test]
    fn ping_roundtrip() {
        let proto = BNetProtocol::new();
        // Simulate a SID_PING sent by the server: header + id + len + 4-byte ping value
        let incoming = set_len(vec![BNET_HEADER_CONSTANT, SID_PING, 0, 0, 0xDE, 0xAD, 0xBE, 0xEF]);
        let ping_val = proto.receive_sid_ping(&incoming);
        assert_eq!(ping_val, vec![0xDE, 0xAD, 0xBE, 0xEF]);
        let reply = proto.send_sid_ping(&ping_val);
        assert!(validate_length(&reply));
        assert_eq!(&reply[4..8], &[0xDE, 0xAD, 0xBE, 0xEF]);
    }

    #[test]
    fn receive_auth_check_good_and_bad() {
        let mut proto = BNetProtocol::new();
        // KeyState = 0 (KR_GOOD) + empty description
        let good = set_len(vec![BNET_HEADER_CONSTANT, SID_AUTH_CHECK, 0, 0, 0, 0, 0, 0, 0]);
        assert!(proto.receive_sid_auth_check(&good));
        // KeyState = 512 (0x200) or other non-0 → failure
        let bad = set_len(vec![BNET_HEADER_CONSTANT, SID_AUTH_CHECK, 0, 0, 0x01, 0x02, 0, 0, 0]);
        assert!(!proto.receive_sid_auth_check(&bad));
    }

    #[test]
    fn receive_enterchat_sets_unique_name() {
        let mut proto = BNetProtocol::new();
        let mut p = vec![BNET_HEADER_CONSTANT, SID_ENTERCHAT, 0, 0];
        p.extend_from_slice(b"GhostBot\0");
        let p = set_len(p);
        assert!(proto.receive_sid_enterchat(&p));
        assert_eq!(proto.get_unique_name(), "GhostBot");
    }

    #[test]
    fn auth_check_send_requires_36_byte_keyinfo() {
        let proto = BNetProtocol::new();
        let empty = proto.send_sid_auth_check(true, &[1, 2, 3, 4], &[1, 0, 0, 0], &[0; 4], &[], &[], "", "x");
        assert!(empty.is_empty(), "missing keyinfo should return empty Vec");

        let ok = proto.send_sid_auth_check(
            true, &[1, 2, 3, 4], &[1, 0, 0, 0], &[2, 0, 0, 0],
            &[0u8; 36], &[0u8; 36], "info", "owner",
        );
        assert!(validate_length(&ok));
        assert_eq!(ok[1], SID_AUTH_CHECK);
    }
}
