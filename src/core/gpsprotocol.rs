use crate::util::assign_length;

pub const GPS_HEADER_CONSTANT: u8 = 248;
pub const REJECTGPS_INVALID: u8 = 1;
pub const REJECTGPS_NOTFOUND: u8 = 2;
pub const GPS_INIT: u8 = 1;
pub const GPS_RECONNECT: u8 = 2;
pub const GPS_ACK: u8 = 3;
pub const GPS_REJECT: u8 = 4;

pub fn send_gpsc_init(version: u32) -> Vec<u8> {
    let mut packet = vec![];
    packet.push(GPS_HEADER_CONSTANT);
    // Fix: originally pushed the header twice by mistake; the second byte should be the packet ID
    packet.push(GPS_INIT);
    packet.push(0);
    packet.push(0);
    packet.extend(version.to_le_bytes());
    assign_length(&mut packet);
    packet
}

pub fn send_gpsc_reconnect(pid: u8, reconnect_key: u32, last_packet: u32) -> Vec<u8> {
    let mut packet = vec![];
    packet.push(GPS_HEADER_CONSTANT);
    packet.push(GPS_RECONNECT);
    packet.push(0);
    packet.push(0);
    packet.push(pid);
    packet.extend(reconnect_key.to_le_bytes());
    packet.extend(last_packet.to_le_bytes());
    assign_length(&mut packet);
    packet
}

pub fn send_gpsc_ack(last_packet: u32) -> Vec<u8> {
    let mut packet = vec![];
    packet.push(GPS_HEADER_CONSTANT);
    packet.push(GPS_ACK);
    packet.push(0);
    packet.push(0);
    packet.extend(last_packet.to_le_bytes());
    assign_length(&mut packet);
    packet
}

pub fn send_gpss_init(reconnect_port: u16, pid: u8, reconnect_key: u32, num_empty_actions: u8) -> Vec<u8> {
    let mut packet = vec![];
    packet.push(GPS_HEADER_CONSTANT);
    packet.push(GPS_INIT);
    packet.push(0);
    packet.push(0);
    packet.extend(reconnect_port.to_le_bytes());
    packet.push(pid);
    packet.extend(reconnect_key.to_le_bytes());
    packet.push(num_empty_actions);
    assign_length(&mut packet);
    packet
}

pub fn send_gpss_reconnect(last_packet: u32) -> Vec<u8> {
    let mut packet = vec![];
    packet.push(GPS_HEADER_CONSTANT);
    packet.push(GPS_RECONNECT);
    packet.push(0);
    packet.push(0);
    packet.extend(last_packet.to_le_bytes());
    assign_length(&mut packet);
    packet
}

pub fn send_gpss_ack(last_packet: u32) -> Vec<u8> {
    let mut packet = vec![];
    packet.push(GPS_HEADER_CONSTANT);
    packet.push(GPS_ACK);
    packet.push(0);
    packet.push(0);
    packet.extend(last_packet.to_le_bytes());
    assign_length(&mut packet);
    packet
}

// ---- RECEIVE (server side parsing GPS packets sent by the client) ----

/// GPS_INIT: returns the GProxy version (packet contains 4-byte header + 4-byte version)
pub fn receive_gps_init(data: &[u8]) -> Option<u32> {
    if data.len() >= 8 && data[0] == GPS_HEADER_CONSTANT && data[1] == GPS_INIT {
        Some(u32::from_le_bytes([data[4], data[5], data[6], data[7]]))
    } else {
        None
    }
}

#[derive(Debug, Clone, Copy)]
pub struct GpsReconnect {
    pub pid: u8,
    pub key: u32,
    /// Total number of packets the client has received (the server uses this to decide the resend start point)
    pub last_packet: u32,
}

/// GPS_RECONNECT: fixed 13 bytes (mirrors the Length == 13 check in C++ ghost.cpp)
pub fn receive_gps_reconnect(data: &[u8]) -> Option<GpsReconnect> {
    if data.len() == 13 && data[0] == GPS_HEADER_CONSTANT && data[1] == GPS_RECONNECT {
        Some(GpsReconnect {
            pid: data[4],
            key: u32::from_le_bytes([data[5], data[6], data[7], data[8]]),
            last_packet: u32::from_le_bytes([data[9], data[10], data[11], data[12]]),
        })
    } else {
        None
    }
}

/// GPS_ACK: client reports the total number of packets received (mirrors the C++ Data.size() == 8 check)
pub fn receive_gps_ack(data: &[u8]) -> Option<u32> {
    if data.len() == 8 && data[0] == GPS_HEADER_CONSTANT && data[1] == GPS_ACK {
        Some(u32::from_le_bytes([data[4], data[5], data[6], data[7]]))
    } else {
        None
    }
}

pub fn send_gpss_reject(reason: u32) -> Vec<u8> {
    let mut packet = vec![];
    packet.push(GPS_HEADER_CONSTANT);
    packet.push(GPS_REJECT);
    packet.push(0);
    packet.push(0);
    packet.extend(reason.to_le_bytes());
    assign_length(&mut packet);
    packet
}