use std::io;
use std::io::{Cursor, Read, Seek, SeekFrom};
use std::path::Path;

use byteorder::{LittleEndian, ReadBytesExt};
use config::Config;
use sha1::Digest;
use sha1::digest::Update;
use stormlib::{Archive, OpenArchiveFlags};
use tracing::{info, warn};

use crate::core::gameslot::*;
use crate::util::*;

pub const MAPSPEED_SLOW: u8 = 1;
pub const MAPSPEED_NORMAL: u8 = 2;
pub const MAPSPEED_FAST: u8 = 3;

pub const MAPVIS_HIDETERRAIN: u8 = 1;
pub const MAPVIS_EXPLORED: u8 = 2;
pub const MAPVIS_ALWAYSVISIBLE: u8 = 3;
pub const MAPVIS_DEFAULT: u8 = 4;

pub const MAPOBS_NONE: u8 = 1;
pub const MAPOBS_ONDEFEAT: u8 = 2;
pub const MAPOBS_ALLOWED: u8 = 3;
pub const MAPOBS_REFEREES: u8 = 4;

pub const MAPFLAG_TEAMSTOGETHER: u8 = 1;
pub const MAPFLAG_FIXEDTEAMS: u8 = 2;
pub const MAPFLAG_UNITSHARE: u8 = 4;
pub const MAPFLAG_RANDOMHERO: u8 = 8;
pub const MAPFLAG_RANDOMRACES: u8 = 16;

pub const MAPOPT_HIDEMINIMAP: u32 = 1 << 0;
pub const MAPOPT_MODIFYALLYPRIORITIES: u32 = 1 << 1;
// the bot cares about this one...
pub const MAPOPT_MELEE: u32 = 1 << 2;
pub const MAPOPT_REVEALTERRAIN: u32 = 1 << 4;
// and this one...
pub const MAPOPT_FIXEDPLAYERSETTINGS: u32 = 1 << 5;
// and this one, the rest don't affect the bot's logic
pub const MAPOPT_CUSTOMFORCES: u32 = 1 << 6;
pub const MAPOPT_CUSTOMTECHTREE: u32 = 1 << 7;
pub const MAPOPT_CUSTOMABILITIES: u32 = 1 << 8;
pub const MAPOPT_CUSTOMUPGRADES: u32 = 1 << 9;
pub const MAPOPT_WATERWAVESONCLIFFSHORES: u32 = 1 << 11;
pub const MAPOPT_WATERWAVESONSLOPESHORES: u32 = 1 << 12;

pub const MAPFILTER_MAKER_USER: u8 = 1;
pub const MAPFILTER_MAKER_BLIZZARD: u8 = 2;

pub const MAPFILTER_TYPE_MELEE: u8 = 1;
pub const MAPFILTER_TYPE_SCENARIO: u8 = 2;

pub const MAPFILTER_SIZE_SMALL: u8 = 1;
pub const MAPFILTER_SIZE_MEDIUM: u8 = 2;
pub const MAPFILTER_SIZE_LARGE: u8 = 4;

pub const MAPFILTER_OBS_FULL: u8 = 1;
pub const MAPFILTER_OBS_ONDEATH: u8 = 2;
pub const MAPFILTER_OBS_NONE: u8 = 4;

// always set except for saved games?
pub const MAPGAMETYPE_UNKNOWN0: u32 = 1;
pub const MAPGAMETYPE_SAVEDGAME: u32 = 1 << 9;
pub const MAPGAMETYPE_PRIVATEGAME: u32 = 1 << 11;
pub const MAPGAMETYPE_MAKERUSER: u32 = 1 << 13;
pub const MAPGAMETYPE_MAKERBLIZZARD: u32 = 1 << 14;
pub const MAPGAMETYPE_TYPEMELEE: u32 = 1 << 15;
pub const MAPGAMETYPE_TYPESCENARIO: u32 = 1 << 16;
pub const MAPGAMETYPE_SIZESMALL: u32 = 1 << 17;
pub const MAPGAMETYPE_SIZEMEDIUM: u32 = 1 << 18;
pub const MAPGAMETYPE_SIZELARGE: u32 = 1 << 19;
pub const MAPGAMETYPE_OBSFULL: u32 = 1 << 20;
pub const MAPGAMETYPE_OBSONDEATH: u32 = 1 << 21;
pub const MAPGAMETYPE_OBSNONE: u32 = 1 << 22;

#[derive(Debug)]
pub struct GameMap {
    pub is_valid: bool,
    map_path: String,
    map_size: Vec<u8>,
    map_info: Vec<u8>,
    map_crc: Vec<u8>,
    map_sha1: Vec<u8>,
    map_speed: u8,
    map_visibility: u8,
    map_observers: u8,
    map_flags: u8,
    map_filter_maker: u8,
    map_filter_type: u8,
    map_filter_size: u8,
    map_filter_obs: u8,
    map_options: u32,
    map_width: Vec<u8>,
    map_height: Vec<u8>,
    map_type: String,
    map_matchmaking_category: String,
    map_stats_w3mmd_category: String,
    map_default_hcl: String,
    map_default_player_score: u32,
    map_local_path: String,
    map_load_in_game: bool,
    /// Raw map file contents (binary, used for map_size/map_info calculation and map download)
    map_data: Vec<u8>,
    map_num_players: u32,
    map_num_teams: u32,
    pub slots: Vec<GameSlot>,
}

impl GameMap {
    pub fn new() -> Self {
        info!("[MAP] using hardcoded Emerald Gardens map data for Warcraft 3 version 1.24 & 1.24b");

        let mut slots = Vec::new();
        for i in 0..12 {
            slots.push(GameSlot::new(0, 255, SLOTSTATUS_OPEN, 0, i, i, SLOTRACE_RANDOM | SLOTRACE_SELECTABLE, SLOTCOMP_EASY, 100));
        }

        GameMap {
            is_valid: false,
            map_path: String::from("Maps\\FrozenThrone\\(12)EmeraldGardens.w3x"),
            map_size: util_extract_numbers("174 221 4 0", 4),
            map_info: util_extract_numbers("251 57 68 98", 4),
            map_crc: util_extract_numbers("108 250 204 59", 4),
            map_sha1: util_extract_numbers("35 81 104 182 223 63 204 215 1 17 87 234 220 66 3 185 82 99 6 13", 20),
            map_speed: MAPSPEED_FAST,
            map_visibility: MAPVIS_DEFAULT,
            map_observers: MAPOBS_NONE,
            map_flags: MAPFLAG_TEAMSTOGETHER | MAPFLAG_FIXEDTEAMS,
            map_filter_maker: MAPFILTER_MAKER_BLIZZARD,
            map_filter_type: MAPFILTER_TYPE_MELEE,
            map_filter_size: MAPFILTER_SIZE_LARGE,
            map_filter_obs: MAPFILTER_OBS_NONE,
            map_options: MAPOPT_MELEE,
            map_width: util_extract_numbers("172 0", 2),
            map_height: util_extract_numbers("172 0", 2),
            map_type: "".to_string(),
            map_matchmaking_category: "".to_string(),
            map_stats_w3mmd_category: "".to_string(),
            map_default_hcl: "".to_string(),
            map_default_player_score: 0,
            map_local_path: "".to_string(),
            map_load_in_game: false,
            map_data: vec![],
            map_num_players: 12,
            map_num_teams: 12,
            slots,
        }
    }

    pub fn get_valid(&self) -> bool {
        self.is_valid
    }

    pub fn get_map_path(&self) -> &str {
        &self.map_path
    }

    pub fn get_map_size(&self) -> &Vec<u8> {
        &self.map_size
    }

    pub fn get_map_info(&self) -> &Vec<u8> {
        &self.map_info
    }

    pub fn get_map_crc(&self) -> &Vec<u8> {
        &self.map_crc
    }

    pub fn get_map_sha1(&self) -> &Vec<u8> {
        &self.map_sha1
    }

    pub fn get_map_speed(&self) -> u8 {
        self.map_speed
    }

    pub fn get_map_visibility(&self) -> u8 {
        self.map_visibility
    }

    pub fn get_map_observers(&self) -> u8 {
        self.map_observers
    }

    pub fn get_map_flags(&self) -> u8 {
        self.map_flags
    }

    pub fn get_map_game_flags(&self) -> Vec<u8> {
        let mut game_flags: u32 = 0;

        // Speed
        game_flags |= match self.map_speed {
            MAPSPEED_SLOW => 0x00000000,
            MAPSPEED_NORMAL => 0x00000001,
            MAPSPEED_FAST => 0x00000002,
            _ => 0x00000000, // Default to slow if speed is unknown
        };

        // Visibility
        game_flags |= match self.map_visibility {
            MAPVIS_HIDETERRAIN => 0x00000100,
            MAPVIS_EXPLORED => 0x00000200,
            MAPVIS_ALWAYSVISIBLE => 0x00000400,
            _ => 0x00000800, // Default to default visibility if visibility is unknown
        };

        // Observers
        game_flags |= match self.map_observers {
            MAPOBS_ONDEFEAT => 0x00002000,
            MAPOBS_ALLOWED => 0x00003000,
            MAPOBS_REFEREES => 0x40000000,
            _ => 0x00000000, // Default to no observers if observer setting is unknown
        };

        // Teams/Units/Hero/Race
        if self.map_flags & MAPFLAG_TEAMSTOGETHER != 0 {
            game_flags |= 0x00004000;
        }
        if self.map_flags & MAPFLAG_FIXEDTEAMS != 0 {
            game_flags |= 0x00060000;
        }
        if self.map_flags & MAPFLAG_UNITSHARE != 0 {
            game_flags |= 0x01000000;
        }
        if self.map_flags & MAPFLAG_RANDOMHERO != 0 {
            game_flags |= 0x02000000;
        }
        if self.map_flags & MAPFLAG_RANDOMRACES != 0 {
            game_flags |= 0x04000000;
        }

        util_create_byte_array(game_flags, false)
    }

    pub fn get_map_game_type(&self) -> u32 {
        let mut game_type: u32 = 0;

        // Maker
        if self.map_filter_maker & MAPFILTER_MAKER_USER != 0 {
            game_type |= MAPGAMETYPE_MAKERUSER;
        }
        if self.map_filter_maker & MAPFILTER_MAKER_BLIZZARD != 0 {
            game_type |= MAPGAMETYPE_MAKERBLIZZARD;
        }

        // Type
        if self.map_filter_type & MAPFILTER_TYPE_MELEE != 0 {
            game_type |= MAPGAMETYPE_TYPEMELEE;
        }
        if self.map_filter_type & MAPFILTER_TYPE_SCENARIO != 0 {
            game_type |= MAPGAMETYPE_TYPESCENARIO;
        }

        // Size
        if self.map_filter_size & MAPFILTER_SIZE_SMALL != 0 {
            game_type |= MAPGAMETYPE_SIZESMALL;
        }
        if self.map_filter_size & MAPFILTER_SIZE_MEDIUM != 0 {
            game_type |= MAPGAMETYPE_SIZEMEDIUM;
        }
        if self.map_filter_size & MAPFILTER_SIZE_LARGE != 0 {
            game_type |= MAPGAMETYPE_SIZELARGE;
        }

        // Obs
        if self.map_filter_obs & MAPFILTER_OBS_FULL != 0 {
            game_type |= MAPGAMETYPE_OBSFULL;
        }
        if self.map_filter_obs & MAPFILTER_OBS_ONDEATH != 0 {
            game_type |= MAPGAMETYPE_OBSONDEATH;
        }
        if self.map_filter_obs & MAPFILTER_OBS_NONE != 0 {
            game_type |= MAPGAMETYPE_OBSNONE;
        }

        game_type
    }

    pub fn get_map_options(&self) -> u32 {
        self.map_options
    }

    pub fn get_map_layout_style(&self) -> u8 {
        if (self.map_options & MAPOPT_CUSTOMFORCES) == 0 {
            return 0;
        }

        if (self.map_options & MAPOPT_FIXEDPLAYERSETTINGS) == 0 {
            return 1;
        }

        return 3;
    }

    pub fn get_map_width(&self) -> &Vec<u8> {
        &self.map_width
    }

    pub fn get_map_height(&self) -> &Vec<u8> {
        &self.map_height
    }

    pub fn get_map_type(&self) -> &str {
        &self.map_type
    }

    pub fn get_map_matchmaking_category(&self) -> &str {
        &self.map_matchmaking_category
    }

    pub fn get_map_stats_w3mmd_category(&self) -> &str {
        &self.map_stats_w3mmd_category
    }

    pub fn get_map_default_hcl(&self) -> &str {
        &self.map_default_hcl
    }

    pub fn get_map_default_player_score(&self) -> u32 {
        self.map_default_player_score
    }

    pub fn get_map_local_path(&self) -> &str {
        &self.map_local_path
    }

    pub fn get_map_load_in_game(&self) -> bool {
        self.map_load_in_game
    }

    pub fn get_map_data(&self) -> &[u8] {
        &self.map_data
    }

    pub fn get_map_nuplayers(&self) -> u32 {
        self.map_num_players
    }

    pub fn get_map_nuteams(&self) -> u32 {
        self.map_num_teams
    }

    pub fn get_slots(&self) -> &Vec<GameSlot> {
        &self.slots
    }

    pub fn load(&mut self, cfg: &Config) {
        let mut sha1 = sha1::Sha1::new();
        let bot_mappath = cfg.get_string("bot_mappath").unwrap_or("".to_string());
        self.map_local_path = cfg.get_string("map_localpath").unwrap_or("".to_string());

        let mpq_path = Path::new(&bot_mappath).join(&self.map_local_path);
        let mpq_path = mpq_path.to_str().unwrap_or("");
        if !mpq_path.is_empty() {
            // Fix: .w3x is a binary file; the original read_to_string would always fail UTF-8 and return an empty string,
            // causing map_size / map_info to be entirely wrong and map download to be unusable
            self.map_data = std::fs::read(&mpq_path).unwrap_or_else(|_| {
                warn!("[MAP] warning - unable to read map file [{}]", mpq_path);
                vec![]
            });
        }

        let mpq_result = Archive::open(
            &mpq_path,
            OpenArchiveFlags::MPQ_OPEN_NO_LISTFILE | OpenArchiveFlags::MPQ_OPEN_NO_ATTRIBUTES,
        );

        if mpq_result.is_err() {
            warn!("[MAP] warning - unable to load MPQ file [ {} ]", &mpq_path);
            return;
        } else {
            info!("[MAP] loading MPQ file [ {} ]", &mpq_path);
        }

        // try to calculate map_size, map_info, map_crc, map_sha1
        let mut map_size: Vec<u8> = vec![];
        let mut map_info: Vec<u8> = vec![];
        let mut map_crc: Vec<u8> = vec![];
        let mut map_sha1: Vec<u8> = vec![];

        // calculate map_size
        map_size = util_create_byte_array(self.map_data.len() as u32, false);
        info!("[MAP] calculated map_size = {}", util_byte_array_to_dec_string(&map_size));

        // calculate map_info (this is actually the CRC)
        let crc32 = util_calc_crc32(self.map_data.as_ref());
        map_info = util_create_byte_array(crc32, false);
        info!("[MAP] calculated map_info = {}", util_byte_array_to_dec_string(&map_info));

        // calculate map_crc (this is not the CRC) and map_sha1
        // a big thank you to Strilanc for figuring the map_crc algorithm out
        let map_cfg_path = cfg.get_string("bot_mapcfgpath").unwrap_or("".to_string());
        // Fix: originally the common_j / blizzard_j filenames were read swapped; SHA1 is sensitive to feed order, producing a wrong map_sha1
        let common_j: String = util_file_read_full(Path::new(&map_cfg_path).join("common.j").to_str().unwrap_or(""));
        let blizzard_j: String = util_file_read_full(Path::new(&map_cfg_path).join("blizzard.j").to_str().unwrap_or(""));

        let mut mpq = mpq_result.unwrap();

        if common_j.is_empty() || blizzard_j.is_empty() {
            warn!("[MAP] unable to calculate map_crc/sha1 - unable to read file [ {0}common.j or {0}blizzard.j]", map_cfg_path);
        } else {
            let mut val: u32 = 0;
            // update: it's possible for maps to include their own copies of common.j and/or blizzard.j
            // this code now overrides the default copies if required
            // The order must match C++ map.cpp:
            //   (in-map Scripts\common.j or default common.j) → (in-map Scripts\blizzard.j or default blizzard.j)
            // Fix: the original missed the "feed the default file when there is no override" branch, so map_crc/map_sha1 was always wrong
            let mut overrode_common_j = false;
            let mut overrode_blizzard_j = false;

            if let Ok(mut override_common_j) = mpq.open_file("Scripts\\common.j") {
                if let Ok(data) = override_common_j.read_all() {
                    info!("[MAP] overriding default common.j with map copy while calculating map_crc/sha1");
                    overrode_common_j = true;
                    val = val ^ Self::xor_rotate_left(&data, data.len());
                    Update::update(&mut sha1, &data);
                }
            }

            if !overrode_common_j {
                val = val ^ Self::xor_rotate_left(common_j.as_bytes(), common_j.len());
                Update::update(&mut sha1, common_j.as_bytes());
            }

            if let Ok(mut override_blizzard_j) = mpq.open_file("Scripts\\blizzard.j") {
                if let Ok(data) = override_blizzard_j.read_all() {
                    info!("[MAP] overriding default blizzard.j with map copy while calculating map_crc/sha1");
                    overrode_blizzard_j = true;
                    val = val ^ Self::xor_rotate_left(&data, data.len());
                    Update::update(&mut sha1, &data);
                }
            }

            if !overrode_blizzard_j {
                val = val ^ Self::xor_rotate_left(blizzard_j.as_bytes(), blizzard_j.len());
                Update::update(&mut sha1, blizzard_j.as_bytes());
            }

            val = rotl(val, 3);
            val = rotl(val ^ 0x03F1379E, 3);
            Update::update(&mut sha1, &[0x9E, 0x37, 0xF1, 0x03]);

            let mut file_list: Vec<&str> = vec![];
            file_list.push("war3map.j");
            file_list.push("scripts\\war3map.j");
            file_list.push("war3map.w3e");
            file_list.push("war3map.wpm");
            file_list.push("war3map.doo");
            file_list.push("war3map.w3u");
            file_list.push("war3map.w3b");
            file_list.push("war3map.w3d");
            file_list.push("war3map.w3a");
            file_list.push("war3map.w3q");

            let mut found_script = false;

            for file_name in file_list {
                // don't use scripts\war3map.j if we've already used war3map.j (yes, some maps have both but only war3map.j is used)
                // Fix: the original continued unconditionally, so maps whose script lives under scripts\ (common)
                // were never read → SHA1/CRC always wrong. The C++ is if( FoundScript && ... ) continue;
                if found_script && file_name == "scripts\\war3map.j" {
                    continue;
                }

                if let Ok(mut file) = mpq.open_file(file_name) {
                    if let Ok(data) = file.read_all() {
                        if file_name == "war3map.j" || file_name == "scripts\\war3map.j" {
                            found_script = true;
                        }

                        val = rotl(val ^ Self::xor_rotate_left(&data, data.len()), 3);
                        Update::update(&mut sha1, &data);
                    }
                } else {
                    warn!("[MAP] couldn't find {}", file_name);
                }
            }

            if !found_script {
                warn!("[MAP] couldn't find war3map.j or scripts\\war3map.j in MPQ file, calculated map_crc/sha1 is probably wrong");
            }

            map_crc = util_create_byte_array(val, false);
            info!("[MAP] calculated map_crc = {}", util_byte_array_to_dec_string(&map_crc));

            map_sha1 = sha1.finalize().to_vec();
            info!("[MAP] calculated map_sha1 = {}", util_byte_array_to_dec_string(&map_sha1));
        }

        let mut w3i: Option<War3mapInfo> = None;
        if let Ok(mut file) = mpq.open_file("war3map.w3i") {
            if let Ok(data) = file.read_all() {
                if let Ok(w3i_result) = read_war3map_i(&data) {
                    w3i = Some(w3i_result);
                }
            }
        } else {
            warn!("[MAP] couldn't find war3map.w3i");
            return;
        }
        let mut w3i = w3i.unwrap();

        // update map_path from config.
        self.map_path = cfg.get_string("map_path").unwrap_or("".to_string());

        // update map_size from config.
        let cfg_map_size = cfg.get_string("map_size").unwrap_or("".to_string());
        if map_size.is_empty() || !cfg_map_size.is_empty() {
            info!("[MAP] overriding calculated map_size with config value map_size = {}", cfg_map_size);
            map_size = util_extract_numbers(&cfg_map_size, 4);
        }
        self.map_size = map_size;

        // update map_info from config.
        let cfg_map_info = cfg.get_string("map_info").unwrap_or("".to_string());
        if map_info.len() == 0 || !cfg_map_info.is_empty() {
            info!("[MAP] overriding calculated map_info with config value map_info = {}", cfg_map_info);
            map_info = util_extract_numbers(&cfg_map_info, 4);
        }
        self.map_info = map_info;

        // update map_crc from config.
        let cfg_map_crc = cfg.get_string("map_crc").unwrap_or("".to_string());
        if map_crc.len() == 0 || !cfg_map_crc.is_empty() {
            info!("[MAP] overriding calculated map_crc with config value map_crc = {}", cfg_map_crc);
            map_crc = util_extract_numbers(&cfg_map_crc, 4);
        }
        self.map_crc = map_crc;

        // update map_sha1 from config.
        let cfg_map_sh1 = cfg.get_string("map_sha1").unwrap_or("".to_string());
        if map_sha1.len() == 0 || !cfg_map_sh1.is_empty() {
            info!("[MAP] overriding calculated map_sha1 with config value map_sha1 = {cfg_map_sh1}");
            map_sha1 = util_extract_numbers(&cfg_map_sh1, 20);
        }
        self.map_sha1 = map_sha1;

        // update other settings.
        self.map_speed = get_u8_from_config(&cfg, "map_speed", MAPSPEED_FAST);
        self.map_visibility = get_u8_from_config(&cfg, "map_visibility", MAPVIS_DEFAULT);
        self.map_observers = get_u8_from_config(&cfg, "map_observers", MAPOBS_NONE);

        self.map_flags = get_u8_from_config(&cfg, "map_flags", MAPFLAG_TEAMSTOGETHER | MAPFLAG_FIXEDTEAMS);
        self.map_filter_maker = get_u8_from_config(&cfg, "map_filter_maker", MAPFILTER_MAKER_USER);

        let cfg_map_filter_type = cfg.get_int("map_filter_type");
        if cfg_map_filter_type.is_ok()
        {
            let cfg_map_filter_type = get_u8_from_config(cfg, "map_filter_type", MAPFILTER_TYPE_SCENARIO);
            info!("[MAP] overriding calculated map_filter_type with config value map_filter_type = {}", cfg_map_filter_type);
            w3i.map_filter_type = cfg_map_filter_type;
        }
        self.map_filter_type = w3i.map_filter_type;

        self.map_filter_size = get_u8_from_config(cfg, "map_filter_size", MAPFILTER_SIZE_LARGE);
        self.map_filter_obs = get_u8_from_config(cfg, "map_filter_obs", MAPFILTER_OBS_NONE);

        // todotodo: it might be possible for MapOptions to legitimately be zero so this is not a valid way of checking if it wasn't parsed out earlier
        let cfg_map_options = cfg.get_int("map_options");
        if w3i.map_options == 0 || cfg_map_options.is_ok() {
            let cfg_map_options = cfg_map_options.unwrap_or(0) as u32;
            info!("[MAP] overriding calculated map_options with config value map_options = {}", cfg_map_options);
            w3i.map_options = cfg_map_options;
        }
        self.map_options = w3i.map_options;

        let cfg_map_width = cfg.get_string("map_width").unwrap_or("".to_string());
        if w3i.map_width.is_empty() || !cfg_map_width.is_empty() {
            info!("[MAP] overriding calculated map_width with config value map_width = {}", cfg_map_width);
            w3i.map_width = util_extract_numbers(&cfg_map_width, 2);
        }
        self.map_width = w3i.map_width;

        let cfg_map_height = cfg.get_string("map_height").unwrap_or("".to_string());
        if w3i.map_height.is_empty() || !cfg_map_height.is_empty() {
            info!("[MAP] overriding calculated map_height with config value map_height = {}", cfg_map_height);
            w3i.map_height = util_extract_numbers(&cfg_map_height, 2);
        }
        self.map_height = w3i.map_height;

        self.map_type = cfg.get_string("map_type").unwrap_or("".to_string());
        self.map_matchmaking_category = cfg.get_string("map_matchmakingcategory").unwrap_or("".to_string());
        self.map_stats_w3mmd_category = cfg.get_string("map_statsw3mmdcategory").unwrap_or("".to_string());
        self.map_default_hcl = cfg.get_string("map_defaulthcl").unwrap_or("".to_string());
        self.map_default_player_score = get_u32_from_config(&cfg, "map_defaultplayerscore", 1000);
        self.map_load_in_game = cfg.get_int("map_loadingame").unwrap_or(0) != 0;

        let cfg_map_numplayers = cfg.get_int("map_numplayers");
        if w3i.map_num_players == 0 || cfg_map_numplayers.is_ok() {
            let cfg_map_numplayers = get_u32_from_config(&cfg, "map_numplayers", 0);
            info!("[MAP] overriding calculated map_numplayers with config value map_numplayers = {}", cfg_map_numplayers);
            w3i.map_num_players = cfg_map_numplayers;
        }
        self.map_num_players = w3i.map_num_players;

        let cfg_map_num_teams = cfg.get_int("map_numteams");
        if w3i.map_num_teams == 0 || cfg_map_num_teams.is_ok() {
            let cfg_map_num_teams = get_u32_from_config(&cfg, "map_numteams", 0);
            info!("[MAP] overriding calculated map_numplayers with config value map_numteams = {}", cfg_map_num_teams);
            w3i.map_num_teams = cfg_map_num_teams;
        }
        self.map_num_teams = w3i.map_num_teams;

        let cfg_slot = cfg.get_string("map_slot1");
        if w3i.slots.len() == 0 || cfg_slot.is_ok() {
            w3i.slots.clear();
            for i in 0..MAX_SLOTS
            {
                info!("[MAP] overriding slots");
                let slot_string = cfg.get_string(format!("map_slot{}", i + 1).as_str()).unwrap_or("".to_string());
                if slot_string.is_empty() {
                    break;
                }

                let slot = GameSlot::new_from_array(&util_extract_numbers(&slot_string, 9));
                w3i.slots.push(slot);
            }
        }
        self.slots = w3i.slots;

        // if random races is set force every slot's race to random
        if (self.map_flags & MAPFLAG_RANDOMRACES) != 0
        {
            info!("[MAP] forcing races to random");
            for slot in self.slots.iter_mut() {
                slot.race = SLOTRACE_RANDOM;
            }
        }

        // add observer slots
        if self.map_observers == MAPOBS_ALLOWED || self.map_observers == MAPOBS_REFEREES
        {
            let mut default_max_slots = MAX_SLOTS;
            if w3i.editor_version < 6060 {
                default_max_slots = 12;
            }
            let cfg_max_slots = get_u32_from_config(&cfg, "map_maxslots", default_max_slots);
            let slot_len = self.slots.len() as u32;
            info!("[MAP] adding {} observer slots", cfg_max_slots - slot_len);

            while slot_len < cfg_max_slots {
                self.slots.push(GameSlot::new(0, 255, SLOTSTATUS_OPEN, 0, MAX_SLOTS.try_into().unwrap(), MAX_SLOTS.try_into().unwrap(), SLOTRACE_RANDOM, SLOTCOMP_EASY, 100));
            }
        }

        self.check_valid();
    }

    pub fn check_valid(&mut self) {
        // Fix: originally is_valid started false and this function only ever set it false, so it could never be valid;
        // corresponds to m_Valid = true at the start of C++ CMap::Load()
        self.is_valid = true;

        if self.map_path.is_empty() || self.map_path.len() > 53 {
            self.is_valid = false;
            warn!("[MAP] invalid map_path detected");
        } else if self.map_path.starts_with('\\') {
            warn!("[MAP] warning - map_path starts with '\\', any replays saved by GHost++ will not be playable in Warcraft III");
        }

        if self.map_path.contains('/') {
            warn!("[MAP] warning - map_path contains forward slashes '/' but it must use Windows style back slashes '\\'");
        }

        if self.map_size.len() != 4 {
            self.is_valid = false;
            warn!("[MAP] invalid map_size detected");
        } else if !self.map_data.is_empty() && self.map_data.len() != util_byte_array_to_u32(&self.map_size, false, 0) as usize {
            self.is_valid = false;
            warn!("[MAP] invalid map_size detected - size mismatch with actual map data");
        }

        if self.map_info.len() != 4 {
            self.is_valid = false;
            warn!("[MAP] invalid map_info detected");
        }

        if self.map_crc.len() != 4 {
            self.is_valid = false;
            warn!("[MAP] invalid map_crc detected");
        }

        if self.map_sha1.len() != 20 {
            self.is_valid = false;
            warn!("[MAP] invalid map_sha1 detected");
        }

        match self.map_speed {
            MAPSPEED_SLOW | MAPSPEED_NORMAL | MAPSPEED_FAST => (),
            _ => {
                self.is_valid = false;
                warn!("[MAP] invalid map_speed detected");
            }
        }

        match self.map_visibility {
            MAPVIS_HIDETERRAIN | MAPVIS_EXPLORED | MAPVIS_ALWAYSVISIBLE | MAPVIS_DEFAULT => (),
            _ => {
                self.is_valid = false;
                warn!("[MAP] invalid map_visibility detected");
            }
        }

        match self.map_observers {
            MAPOBS_NONE | MAPOBS_ONDEFEAT | MAPOBS_ALLOWED | MAPOBS_REFEREES => (),
            _ => {
                self.is_valid = false;
                warn!("[MAP] invalid map_observers detected");
            }
        }

        if !(self.map_flags <= (MAPFLAG_TEAMSTOGETHER | MAPFLAG_FIXEDTEAMS | MAPFLAG_UNITSHARE | MAPFLAG_RANDOMHERO | MAPFLAG_RANDOMRACES)) {
            self.is_valid = false;
            warn!("[MAP] invalid map_flags detected = {}", self.map_flags);
        }

        match self.map_filter_maker {
            MAPFILTER_MAKER_USER | MAPFILTER_MAKER_BLIZZARD => (),
            _ => {
                self.is_valid = false;
                warn!("[MAP] invalid map_filter_maker detected");
            }
        }

        match self.map_filter_type {
            MAPFILTER_TYPE_MELEE | MAPFILTER_TYPE_SCENARIO => (),
            _ => {
                self.is_valid = false;
                warn!("[MAP] invalid map_filter_type detected");
            }
        }

        match self.map_filter_size {
            MAPFILTER_SIZE_SMALL | MAPFILTER_SIZE_MEDIUM | MAPFILTER_SIZE_LARGE => (),
            _ => {
                self.is_valid = false;
                warn!("[MAP] invalid map_filter_type detected");
            }
        }

        match self.map_filter_obs {
            MAPFILTER_OBS_FULL | MAPFILTER_OBS_ONDEATH | MAPFILTER_OBS_NONE => (),
            _ => {
                self.is_valid = false;
                warn!("[MAP] invalid map_filter_obs detected");
            }
        }

        if self.map_width.len() != 2 {
            self.is_valid = false;
            warn!("[MAP] invalid map_width detected");
        }

        if self.map_height.len() != 2 {
            self.is_valid = false;
            warn!("[MAP] invalid map_height detected");
        }

        if self.map_num_players == 0 || self.map_num_players > MAX_SLOTS {
            self.is_valid = false;
            warn!("[MAP] invalid map_numplayers detected");
        }

        if self.map_num_teams == 0 || self.map_num_teams > MAX_SLOTS {
            self.is_valid = false;
            warn!("[MAP] invalid map_numteams detected");
        }

        if self.slots.is_empty() || self.slots.len() > MAX_SLOTS as usize {
            self.is_valid = false;
            warn!("[MAP] invalid map_slot<x> detected");
        }
    }

    fn xor_rotate_left(data: &[u8], length: usize) -> u32 {
        let mut i: usize = 0;
        let mut val: u32 = 0;

        if length > 3 {
            while i < length - 3 {
                val = rotl(
                    val ^ ((data[i] as u32)
                        + ((data[i + 1] as u32) << 8)
                        + ((data[i + 2] as u32) << 16)
                        + ((data[i + 3] as u32) << 24)),
                    3,
                );
                i += 4;
            }
        }

        while i < length {
            val = rotl(val ^ data[i] as u32, 3);
            i += 1;
        }

        val
    }
}

fn rotl(x: u32, n: u32) -> u32 {
    (x << n) | (x >> (32 - n))
}

#[allow(dead_code)] // Counterpart of a C++ util function, kept for future use
fn rotr(x: u32, n: u32) -> u32 {
    (x >> n) | (x << (32 - n))
}

fn read_null_terminated_string<R: Read>(reader: &mut R) -> io::Result<String> {
    let mut buf = Vec::new();
    for byte in reader.bytes() {
        let byte = byte?;
        if byte == 0 {
            break;
        }
        buf.push(byte);
    }
    Ok(String::from_utf8_lossy(&buf).into_owned())
}

struct War3mapInfo {
    editor_version: u32,
    map_options: u32,
    map_width: Vec<u8>,
    map_height: Vec<u8>,
    map_num_players: u32,
    map_num_teams: u32,
    map_filter_type: u8,
    slots: Vec<GameSlot>,
}

#[allow(unused_assignments)] // The initial value is only for type inference; the actual value is read in by the cursor
fn read_war3map_i(data: &Vec<u8>) -> io::Result<War3mapInfo> {
    let mut cursor = Cursor::new(data);

    let editor_version: u32; // used to determine maximum slots when adding observers
    let mut map_options: u32 = 0;
    let mut map_width: Vec<u8> = vec![];
    let mut map_height: Vec<u8> = vec![];
    let mut map_num_players: u32 = 0;
    let mut map_num_teams: u32 = 0;
    let mut map_filter_type = MAPFILTER_TYPE_SCENARIO;
    let mut slots: Vec<GameSlot> = vec![];

    let raw_map_width: u32;
    let raw_map_height: u32;
    let raw_map_flags: u32;
    let raw_map_num_players: u32;
    let raw_map_num_teams: u32;

    let file_format = cursor.read_u32::<LittleEndian>()?;

    if file_format != 18 && file_format != 25 {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, "[MAP] unable to calculate map_options, map_width, map_height, map_slot<x>, map_numplayers, map_numteams - unable to extract war3map.w3i from MPQ file"));
    }

    info!("file_format: {}", file_format);

    cursor.seek(SeekFrom::Current(4))?;

    editor_version = cursor.read_u32::<LittleEndian>()?;
    info!("editor_version: {}", editor_version);

    // map name
    read_null_terminated_string(&mut cursor)?;
    // map author
    read_null_terminated_string(&mut cursor)?;
    // map description
    read_null_terminated_string(&mut cursor)?;
    // players recommended
    read_null_terminated_string(&mut cursor)?;
    // camera bounds
    cursor.seek(SeekFrom::Current(32))?;
    // camera bounds complements
    cursor.seek(SeekFrom::Current(16))?;
    // map width
    raw_map_width = cursor.read_u32::<LittleEndian>()?;
    // map height
    raw_map_height = cursor.read_u32::<LittleEndian>()?;
    // flags
    raw_map_flags = cursor.read_u32::<LittleEndian>()?;
    // map main ground type
    cursor.seek(SeekFrom::Current(1))?;

    if file_format == 18 {
        // campaign background number
        cursor.seek(SeekFrom::Current(4))?;
    } else if file_format == 25 {
        // loading screen background number
        cursor.seek(SeekFrom::Current(4))?;
        // path of custom loading screen model
        read_null_terminated_string(&mut cursor)?;
    }

    // map loading screen text
    read_null_terminated_string(&mut cursor)?;
    // map loading screen title
    read_null_terminated_string(&mut cursor)?;
    // map loading screen subtitle
    read_null_terminated_string(&mut cursor)?;

    if file_format == 18 {
        // map loading screen number
        cursor.seek(SeekFrom::Current(4))?;
    } else if file_format == 25 {
        // used game data set
        cursor.seek(SeekFrom::Current(4))?;
        // prologue screen path
        read_null_terminated_string(&mut cursor)?;
    }

    // prologue screen text
    read_null_terminated_string(&mut cursor)?;
    // prologue screen title
    read_null_terminated_string(&mut cursor)?;
    // prologue screen subtitle
    read_null_terminated_string(&mut cursor)?;

    if file_format == 25
    {
        // uses terrain fog
        cursor.seek(SeekFrom::Current(4))?;
        // fog start z height
        cursor.seek(SeekFrom::Current(4))?;
        // fog end z height
        cursor.seek(SeekFrom::Current(4))?;
        // fog density
        cursor.seek(SeekFrom::Current(4))?;
        // fog red value
        cursor.seek(SeekFrom::Current(1))?;
        // fog green value
        cursor.seek(SeekFrom::Current(1))?;
        // fog blue value
        cursor.seek(SeekFrom::Current(1))?;
        // fog alpha value
        cursor.seek(SeekFrom::Current(1))?;
        // global weather id
        cursor.seek(SeekFrom::Current(4))?;
        // custom sound environment
        read_null_terminated_string(&mut cursor)?;
        // tileset id of the used custom light environment
        cursor.seek(SeekFrom::Current(1))?;
        // custom water tinting red value
        cursor.seek(SeekFrom::Current(1))?;
        // custom water tinting green value
        cursor.seek(SeekFrom::Current(1))?;
        // custom water tinting blue value
        cursor.seek(SeekFrom::Current(1))?;
        // custom water tinting alpha value
        cursor.seek(SeekFrom::Current(1))?;
    }

    // number of players
    raw_map_num_players = cursor.read_u32::<LittleEndian>()?;

    let mut closed_slots: u32 = 0;
    for _ in 0..raw_map_num_players {
        let mut slot = GameSlot::new(0, 255, SLOTSTATUS_OPEN, 0, 0, 1, SLOTRACE_RANDOM, SLOTCOMP_EASY, 100);

        // colour
        let colour = cursor.read_u32::<LittleEndian>()?;
        slot.colour = colour as u8;

        // status
        match cursor.read_u32::<LittleEndian>()? {
            1 => slot.slot_status = SLOTSTATUS_OPEN,
            2 => {
                slot.slot_status = SLOTSTATUS_OCCUPIED;
                slot.computer = 1;
                slot.computer_type = SLOTCOMP_NORMAL;
            }
            _ => {
                slot.slot_status = SLOTSTATUS_CLOSED;
                closed_slots += 1;
            }
        }

        // race
        match cursor.read_u32::<LittleEndian>()? {
            1 => slot.race = SLOTRACE_HUMAN,
            2 => slot.race = SLOTRACE_ORC,
            3 => slot.race = SLOTRACE_UNDEAD,
            4 => slot.race = SLOTRACE_NIGHTELF,
            _ => slot.race = SLOTRACE_RANDOM
        }

        // fixed start position
        cursor.seek(SeekFrom::Current(4))?;
        // player name
        read_null_terminated_string(&mut cursor)?;
        // start position x
        cursor.seek(SeekFrom::Current(4))?;
        // start position y
        cursor.seek(SeekFrom::Current(4))?;
        // ally low priorities
        cursor.seek(SeekFrom::Current(4))?;
        // ally high priorities
        cursor.seek(SeekFrom::Current(4))?;

        if slot.slot_status != SLOTSTATUS_CLOSED {
            slots.push(slot);
        }
    }

    // number of teams
    raw_map_num_teams = cursor.read_u32::<LittleEndian>()?;
    for i in (0..raw_map_num_teams).map(|i| i as u8) {
        // flags
        cursor.read_u32::<LittleEndian>()?;

        let mut player_mask = cursor.read_u32::<LittleEndian>()?;

        for j in (0..MAX_SLOTS).map(|j| j as u8) {
            if (player_mask & 1) != 0
            {
                for mut _slot in slots.iter_mut() {
                    if _slot.colour == j {
                        _slot.team = i;
                    }
                }
            }

            player_mask >>= 1;
        }

        read_null_terminated_string(&mut cursor)?;
    }

    // the bot only cares about the following options: melee, fixed player settings, custom forces
    // let's not confuse the user by displaying erroneous map options so zero them out now
    map_options = raw_map_flags & (MAPOPT_MELEE | MAPOPT_FIXEDPLAYERSETTINGS | MAPOPT_CUSTOMFORCES);
    info!("[MAP] calculated map_options = {}", map_options);
    map_width = util_create_byte_array(raw_map_width, false);
    info!("[MAP] calculated map_width = {}", raw_map_width);
    map_height = util_create_byte_array(raw_map_height, false);
    info!("[MAP] calculated map_height = {}", raw_map_height);
    map_num_players = raw_map_num_players - closed_slots;
    info!("[MAP] calculated map_numplayers = {}", map_num_players);
    map_num_teams = raw_map_num_teams;
    info!("[MAP] calculated map_numteams = {}", map_num_teams);

    let mut slot_num: u32 = 1;
    for _slot in slots.iter() {
        info!("[MAP] calculated map_slot {} ={}", slot_num, util_byte_array_to_dec_string(&_slot.get_byte_array()));
        slot_num += 1;
    }

    if (map_options & MAPOPT_MELEE) != 0
    {
        info!("[MAP] found melee map, initializing slots");
        // give each slot a different team and set the race to random
        let mut team: u8 = 0;

        for mut _slot in slots.iter_mut()
        {
            team += 1;
            _slot.team = team;
            _slot.race = SLOTRACE_RANDOM;
        }

        map_filter_type = MAPFILTER_TYPE_MELEE;
    }

    if (map_options & MAPOPT_FIXEDPLAYERSETTINGS) == 0
    {
        // make races selectable
        for mut _slot in slots.iter_mut() {
            _slot.race = _slot.race | SLOTRACE_SELECTABLE;
        }
    }

    Ok(War3mapInfo {
        editor_version,
        map_options,
        map_width,
        map_height,
        map_num_players,
        map_num_teams,
        map_filter_type,
        slots,
    })
}

#[cfg(test)]
mod tests {
    use stormlib::{Archive, OpenArchiveFlags};
    use crate::core::gamemap::*;

    #[test]
    fn try_read_war3map_i() {
        let file_path = "maps/FateV1.7N_Fix_CHT.w3x";
        let mpq_result = Archive::open(
            file_path,
            OpenArchiveFlags::MPQ_OPEN_NO_LISTFILE | OpenArchiveFlags::MPQ_OPEN_NO_ATTRIBUTES,
        );

        if mpq_result.is_err() {
            panic!("[MAP] warning - unable to load MPQ file [ {} ]", file_path);
        }

        info!("[MAP] loading MPQ file [ {} ]", file_path);
        let mut mpq = mpq_result.unwrap();

        let w3i_result = mpq.open_file("war3map.w3i");
        if w3i_result.is_err() {
            panic!("[MAP] warning - unable to load MPQ file [ {} ]", file_path);
        }

        let mut w3i = w3i_result.unwrap();
        let data = w3i.read_all();
        if data.is_err() {
            panic!("[MAP] warning - unable to load MPQ file [ war3map.i ]")
        }

        let data = data.unwrap();
        let result = read_war3map_i(&data);

        assert_eq!(result.is_ok(), true);
    }

    #[test]
    fn try_load_map() {
        let settings = Config::builder()
            .add_source(config::File::with_name("config/ghost"))
            .add_source(config::File::with_name("config/map"))
            .build()
            .unwrap();

        let mut gamemap = GameMap::new();
        gamemap.load(&settings);
        gamemap.check_valid();
        assert_eq!(gamemap.is_valid, true);
    }
}