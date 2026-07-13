use std::net::TcpStream;

/// GProxy++ reconnect data (mirrors C++ ghost.h GProxyReconnector)
#[allow(dead_code)]
struct GProxyReconnector {
    socket: TcpStream,
    pid: u8,
    reconnect_key: u32,
    last_packet: u32,
    posted_time: u32,
}