use std::io;
use std::io::{BufRead, BufReader, Error, ErrorKind, Write};
use std::net::{SocketAddrV4, TcpStream};

use tracing::warn;

#[derive(Debug)]
pub struct GameSocket {
    socket: Option<TcpStream>,
    send_buffer: Vec<u8>,
    receive_buffer: Vec<u8>,
    pub has_error: bool,
    pub error_code: u8,
    #[allow(dead_code)] // Legacy: superseded by net::conn
    last_recv: u32,
    #[allow(dead_code)]
    last_send: u32,
    pub(crate) is_connected: bool,
}

impl GameSocket {
    pub fn new() -> io::Result<Self> {
        Ok(Self {
            socket: None,
            send_buffer: vec![],
            receive_buffer: vec![],
            has_error: true,
            error_code: 0,
            last_recv: 0,
            last_send: 0,
            is_connected: false,
        })
    }

    pub fn connect(&mut self, _addr: SocketAddrV4) -> io::Result<()> {
        // Implement connection (this struct will be replaced by net::conn / tokio, so left unimplemented)
        Ok(())
    }

    pub fn send(&mut self) {
        if let Some(ref mut socket) = self.socket {
            match socket.write(&self.send_buffer) {
                Ok(result) => {
                    self.receive_buffer.drain(0..result);
                }
                Err(err) => { warn!("[GameSocket] send data failed, {}", err) }
            }
        }
    }

    pub fn received(&mut self) -> io::Result<()> {
        if let Some(socket) = &self.socket {
            if !self.is_connected {
                return Err(Error::new(ErrorKind::NotConnected, "Socket is not connected"));
            }

            let mut reader = BufReader::new(socket);
            let received = reader.fill_buf()?.to_vec();
            reader.consume(received.len());
            self.receive_buffer.extend(received);
        }

        Ok(())
    }

    /// Fix: originally returned "the bytes of the IP string", but the W3GS protocol (PLAYERINFO/SLOTINFOJOIN)
    /// requires a 4-byte network-order address
    pub fn get_ip(&self) -> Vec<u8> {
        if let Some(socket) = &self.socket {
            if let Ok(std::net::SocketAddr::V4(addr)) = socket.peer_addr() {
                return addr.ip().octets().to_vec();
            }
        }

        vec![0, 0, 0, 0]
    }

    pub fn get_ip_string(&self) -> String {
        if let Some(socket) = &self.socket {
            socket.peer_addr()
                .and_then(|addr| Ok(addr))
                .map_or(String::new(), |addr| addr.ip().to_string())
        } else {
            String::new()
        }
    }

    pub fn get_bytes(&self) -> &Vec<u8> {
        &self.receive_buffer
    }

    pub fn cosume_bytes(&mut self, length: usize) {
        self.receive_buffer.drain(..length);
    }

    pub fn put_bytes(&mut self, data: Vec<u8>) {
        self.send_buffer.extend(data);
    }

    pub fn reset(&mut self) {
        self.receive_buffer.clear();
        self.send_buffer.clear();
    }

    pub fn disconnect(&mut self) {}
}