//! Replay (.w3g) storage (mirrors C++ packed.cpp + replay.cpp).
//!
//! Two-layer structure:
//! - **Record layer** (replay.cpp): header area (host/players/GameStartRecord) + start blocks
//!   (0x1A/0x1B/0x1C) + data blocks (TimeSlot 0x1F/0x1E, Chat 0x20, Leave 0x17).
//! - **Container layer** (packed.cpp): "Warcraft III recorded game\x1A\0" magic +
//!   68-byte header (CRC32) + 8192-byte segmented zlib-compressed blocks (each with a folded CRC).

use std::io::Write;

use flate2::write::ZlibEncoder;
use flate2::Compression;

use crate::core::gameslot::GameSlot;
use crate::util::util_calc_crc32;

const REPLAY_LEAVEGAME: u8 = 0x17;
const REPLAY_FIRSTSTARTBLOCK: u8 = 0x1A;
const REPLAY_SECONDSTARTBLOCK: u8 = 0x1B;
const REPLAY_THIRDSTARTBLOCK: u8 = 0x1C;
const REPLAY_TIMESLOT2: u8 = 0x1E; // corresponds to W3GS_INCOMING_ACTION2
const REPLAY_TIMESLOT: u8 = 0x1F; // corresponds to W3GS_INCOMING_ACTION
const REPLAY_CHATMESSAGE: u8 = 0x20;

/// Replay recorder for a single game (mirrors CReplay)
pub struct ReplayRecorder {
    host_pid: u8,
    host_name: String,
    /// (pid, name), including host
    players: Vec<(u8, String)>,
    slots: Vec<GameSlot>,
    random_seed: u32,
    /// map layout style(C++ SelectMode)
    select_mode: u8,
    /// map num players(C++ StartSpotCount)
    start_spot_count: u8,
    map_game_type: u32,
    game_name: String,
    /// Already-encoded stat string (built at EventGameStarted, mirrors game_base.cpp:156-167)
    stat_string: Vec<u8>,
    /// Players who left during loading (placed between the second and third start blocks)
    loading_blocks: Vec<u8>,
    /// In-game data blocks (TimeSlot / Chat / Leave)
    compiled_blocks: Vec<u8>,
    /// Accumulated game length in ms (sum of TimeSlot intervals)
    replay_length_ms: u32,
}

impl ReplayRecorder {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        host_pid: u8,
        host_name: String,
        players: Vec<(u8, String)>,
        slots: Vec<GameSlot>,
        random_seed: u32,
        select_mode: u8,
        start_spot_count: u8,
        map_game_type: u32,
        game_name: String,
        stat_string: Vec<u8>,
    ) -> Self {
        Self {
            host_pid,
            host_name,
            players,
            slots,
            random_seed,
            select_mode,
            start_spot_count,
            map_game_type,
            game_name,
            stat_string,
            loading_blocks: Vec::new(),
            compiled_blocks: Vec::new(),
            replay_length_ms: 0,
        }
    }

    pub fn replay_length_ms(&self) -> u32 {
        self.replay_length_ms
    }

    /// TimeSlot (mirrors CReplay::AddTimeSlot): interval accumulates the game length
    pub fn add_time_slot(&mut self, time_increment: u16, actions: &[(u8, Vec<u8>)]) {
        self.push_time_slot(REPLAY_TIMESLOT, time_increment, actions);
        self.replay_length_ms += time_increment as u32;
    }

    /// TimeSlot2 (mirrors AddTimeSlot2; corresponds to the INCOMING_ACTION2 sub-packet, does not count toward length)
    pub fn add_time_slot2(&mut self, actions: &[(u8, Vec<u8>)]) {
        self.push_time_slot(REPLAY_TIMESLOT2, 0, actions);
    }

    fn push_time_slot(&mut self, id: u8, time_increment: u16, actions: &[(u8, Vec<u8>)]) {
        let mut block = vec![id, 0, 0];
        block.extend(time_increment.to_le_bytes());
        for (pid, action) in actions {
            block.push(*pid);
            block.extend((action.len() as u16).to_le_bytes());
            block.extend(action);
        }
        let len = ((block.len() - 3) as u16).to_le_bytes();
        block[1] = len[0];
        block[2] = len[1];
        self.compiled_blocks.extend(block);
    }

    /// Chat (mirrors AddChatMessage; flag 32 in-game, mode = extra_flags u32)
    pub fn add_chat(&mut self, pid: u8, flag: u8, chat_mode: u32, message: &str) {
        let mut block = vec![REPLAY_CHATMESSAGE, pid, 0, 0, flag];
        block.extend(chat_mode.to_le_bytes());
        block.extend(message.as_bytes());
        block.push(0); // null appended by the C++ AppendByteArrayFast string overload
        let len = ((block.len() - 4) as u16).to_le_bytes();
        block[2] = len[0];
        block[3] = len[1];
        self.compiled_blocks.extend(block);
    }

    /// Leave (mirrors AddLeaveGame / AddLeaveGameDuringLoading: reason is always 1)
    pub fn add_leave(&mut self, pid: u8, result: u32, during_loading: bool) {
        let mut block = vec![REPLAY_LEAVEGAME];
        block.extend(1u32.to_le_bytes()); // reason
        block.push(pid);
        block.extend(result.to_le_bytes());
        block.extend(1u32.to_le_bytes());
        if during_loading {
            self.loading_blocks.extend(block);
        } else {
            self.compiled_blocks.extend(block);
        }
    }

    /// Assemble the uncompressed replay data (mirrors BuildReplay)
    fn build(&self) -> Vec<u8> {
        let mut r: Vec<u8> = Vec::with_capacity(4096 + self.compiled_blocks.len());
        // Unknown (4.0)
        r.extend([16u8, 1, 0, 0]);
        // Host Record (4.1)
        r.push(0);
        r.push(self.host_pid);
        r.extend(self.host_name.as_bytes());
        r.push(0);
        r.push(1); // AdditionalSize
        r.push(0); // AdditionalData
        // GameName (4.2) + Null (4.0)
        r.extend(self.game_name.as_bytes());
        r.push(0);
        r.push(0);
        // StatString (4.3)
        r.extend(&self.stat_string);
        r.push(0);
        // PlayerCount (4.6) = number of slots (mirrors C++ which uses m_Slots.size())
        r.extend((self.slots.len() as u32).to_le_bytes());
        // GameType (4.7) / LanguageID (4.8)
        r.extend(self.map_game_type.to_le_bytes());
        r.extend(0x0012F8B0u32.to_le_bytes());
        // PlayerList (4.9): players other than the host
        for (pid, name) in &self.players {
            if *pid == self.host_pid {
                continue;
            }
            r.push(22);
            r.push(*pid);
            r.extend(name.as_bytes());
            r.push(0);
            r.push(1);
            r.push(0);
            r.extend(0u32.to_le_bytes());
        }
        // GameStartRecord (4.10)
        r.push(25);
        r.extend(((7 + self.slots.len() * 9) as u16).to_le_bytes());
        r.push(self.slots.len() as u8);
        for s in &self.slots {
            r.extend([
                s.pid,
                s.download_status,
                s.slot_status,
                s.computer,
                s.team,
                s.colour,
                s.race,
                s.computer_type,
                s.handicap,
            ]);
        }
        r.extend(self.random_seed.to_le_bytes());
        r.push(self.select_mode);
        r.push(self.start_spot_count);
        // ReplayData (5.0): start blocks + players who left during loading + data blocks
        r.push(REPLAY_FIRSTSTARTBLOCK);
        r.extend(1u32.to_le_bytes());
        r.push(REPLAY_SECONDSTARTBLOCK);
        r.extend(1u32.to_le_bytes());
        r.extend(&self.loading_blocks);
        r.push(REPLAY_THIRDSTARTBLOCK);
        r.extend(1u32.to_le_bytes());
        r.extend(&self.compiled_blocks);
        r
    }

    /// Build + packed compression (mirrors CPacked::Compress; TFT always true).
    /// Returns the complete .w3g file contents.
    pub fn build_and_compress(&self, war3_version: u32, build_number: u16) -> std::io::Result<Vec<u8>> {
        let decompressed = self.build();
        let decompressed_size = decompressed.len() as u32;

        // 8192-byte segments, each zlib-compressed separately (final segment zero-padded)
        let mut padded = decompressed;
        let pad = 8192 - (padded.len() % 8192);
        padded.resize(padded.len() + pad, 0);

        let mut blocks: Vec<Vec<u8>> = Vec::new();
        let mut compressed_total = 0u32;
        for chunk in padded.chunks(8192) {
            let mut enc = ZlibEncoder::new(Vec::new(), Compression::default());
            enc.write_all(chunk)?;
            let block = enc.finish()?;
            compressed_total += block.len() as u32;
            blocks.push(block);
        }

        // ---- 68-byte header ----
        let mut header: Vec<u8> = Vec::with_capacity(68);
        header.extend(b"Warcraft III recorded game\x1A\0");
        header.extend(68u32.to_le_bytes()); // header size
        header.extend((68 + compressed_total + blocks.len() as u32 * 8).to_le_bytes());
        header.extend(1u32.to_le_bytes()); // header version
        header.extend(decompressed_size.to_le_bytes());
        header.extend((blocks.len() as u32).to_le_bytes());
        header.extend(b"PX3W"); // "W3XP" reversed (TFT)
        header.extend(war3_version.to_le_bytes());
        header.extend(build_number.to_le_bytes());
        header.extend(32768u16.to_le_bytes()); // flags(multiplayer)
        header.extend(self.replay_length_ms.to_le_bytes());
        header.extend(0u32.to_le_bytes()); // CRC placeholder
        let crc = util_calc_crc32(&header);
        let n = header.len();
        header[n - 4..].copy_from_slice(&crc.to_le_bytes());

        // ---- Assemble file: header + each compressed block (8-byte block header + data) ----
        let mut out = header;
        for block in &blocks {
            let mut bh: Vec<u8> = Vec::with_capacity(8);
            bh.extend((block.len() as u16).to_le_bytes());
            bh.extend(8192u16.to_le_bytes());
            bh.extend(0u32.to_le_bytes());
            // Folded CRC: low 16 = block header (CRC field zeroed), high 16 = data
            let mut c1 = util_calc_crc32(&bh);
            c1 ^= c1 >> 16;
            let mut c2 = util_calc_crc32(block);
            c2 ^= c2 >> 16;
            let bcrc = (c1 & 0xFFFF) | (c2 << 16);
            bh[4..8].copy_from_slice(&bcrc.to_le_bytes());
            out.extend(bh);
            out.extend(block);
        }
        Ok(out)
    }
}
