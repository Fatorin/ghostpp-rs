pub const SLOTSTATUS_OPEN: u8 = 0;
pub const SLOTSTATUS_CLOSED: u8 = 1;
pub const SLOTSTATUS_OCCUPIED: u8 = 2;
pub const SLOTRACE_HUMAN: u8 = 1;
pub const SLOTRACE_ORC: u8 = 2;
pub const SLOTRACE_NIGHTELF: u8 = 4;
pub const SLOTRACE_UNDEAD: u8 = 8;
pub const SLOTRACE_RANDOM: u8 = 32;
pub const SLOTRACE_SELECTABLE: u8 = 64;
pub const SLOTCOMP_EASY: u8 = 0;
pub const SLOTCOMP_NORMAL: u8 = 1;
pub const SLOTCOMP_HARD: u8 = 2;
/// Maximum number of slots. War3 1.26~1.28 (this project's target versions) cap at 12; 24 only exists on 1.29+.
/// Mirrors C++ reference tree gameslot.h MAX_SLOTS = 12.
/// This value affects the STARTADVEX3 slots-free byte (12→98 'b', >12→110 'n').
pub const MAX_SLOTS: u32 = 12;

#[derive(Clone, Debug)]
pub struct GameSlot {
    // player id
    pub pid: u8,
    // download status (0% to 100%)
    pub download_status: u8,
    // slot status (0 = open, 1 = closed, 2 = occupied)
    pub slot_status: u8,
    // computer (0 = no, 1 = yes)
    pub computer: u8,
    // team
    pub team: u8,
    // colour
    pub colour: u8,
    // race (1 = human, 2 = orc, 4 = night elf, 8 = undead, 32 = random, 64 = selectable)
    pub race: u8,
    // computer type (0 = easy, 1 = human or normal comp, 2 = hard comp)
    pub computer_type: u8,
    // handicap
    pub handicap: u8,
}

impl GameSlot {
    pub fn new_from_array(value: &Vec<u8>) -> Self {
        let mut game_slot = GameSlot {
            pid: 0,
            download_status: 0,
            slot_status: SLOTSTATUS_OPEN,
            computer: 0,
            team: 0,
            colour: 1,
            race: SLOTRACE_RANDOM,
            computer_type: SLOTCOMP_NORMAL,
            handicap: 0,
        };

        if value.len() >= 7 {
            game_slot.pid = value[0];
            game_slot.download_status = value[1];
            game_slot.slot_status = value[2];
            game_slot.computer = value[3];
            game_slot.team = value[4];
            game_slot.colour = value[5];
            game_slot.race = value[6];

            if value.len() >= 8 {
                game_slot.computer_type = value[7];
            }

            if value.len() >= 9 {
                game_slot.handicap = value[8];
            }
        }

        game_slot
    }

    pub fn new(
        pid: u8,
        download_status: u8,
        slot_status: u8,
        computer: u8,
        team: u8,
        colour: u8,
        race: u8,
        computer_type: u8,
        handicap: u8,
    ) -> Self {
        GameSlot {
            pid,
            download_status,
            slot_status,
            computer,
            team,
            colour,
            race,
            computer_type,
            handicap,
        }
    }

    pub fn get_byte_array(&self) -> Vec<u8> {
        vec![self.pid, self.download_status, self.slot_status, self.computer, self.team, self.colour, self.race, self.computer_type, self.handicap]
    }
}