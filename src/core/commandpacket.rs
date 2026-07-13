#[derive(Debug)]
pub struct CommandPacket {
    pub packet_type: u8,
    pub id: u32,
    pub data: Vec<u8>,
}