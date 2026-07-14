//! Packet framing codec.
//!
//! Both the BNCS and W3GS wire formats are `[magic:u8][id:u8][len:u16 LE (including the 4-byte header)]`:
//! - BNCS (battle.net): magic = 0xFF
//! - W3GS (in-game):    magic = 0xF7
//! - GPS  (GProxy++):   magic = 0xF8 (may appear interleaved on player connections, see gameplayer extract_packets)
//!
//! The decoder outputs a [`Frame`] whose `data` is the **complete packet (including the header)**,
//! so it can be fed directly to the existing `bnetprotocol::receive_*` / `gameprotocol::receive_*`
//! (which, like the C++, parse using offsets that include the header).
//!
//! The encoder is a passthrough: the `send_*` family of functions already produce complete packet bytes, so they can be written out directly.
//! Replaces the old manual buffers of `commandpacket.rs` and `gamesocket.rs`.

use std::io;

use bytes::{Bytes, BytesMut};
use tokio_util::codec::{Decoder, Encoder};

pub const BNCS_HEADER_CONSTANT: u8 = 0xFF; // 255, same as bnetprotocol::BNET_HEADER_CONSTANT
pub const W3GS_HEADER_CONSTANT: u8 = 0xF7; // 247, same as gameprotocol::W3GS_HEADER_CONSTANT
pub const GPS_HEADER_CONSTANT: u8 = 0xF8; // 248, same as gpsprotocol::GPS_HEADER_CONSTANT

/// A complete protocol packet
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Frame {
    /// Header constant (0xFF / 0xF7 / 0xF8)
    pub magic: u8,
    /// Packet ID (e.g. SID_AUTH_INFO, W3GS_REQJOIN)
    pub id: u8,
    /// The complete packet content, including the 4-byte header
    pub data: Bytes,
}

impl Frame {
    /// The payload after the header
    pub fn payload(&self) -> &[u8] {
        &self.data[4..]
    }
}

#[derive(Debug, thiserror::Error)]
pub enum FrameError {
    #[error("bad magic byte: got 0x{got:02X}, accepted {accepted:02X?}")]
    BadMagic { got: u8, accepted: &'static [u8] },

    #[error("bad frame length: {0} (must be >= 4)")]
    BadLength(u16),

    #[error("io error: {0}")]
    Io(#[from] io::Error),
}

/// `[magic][id][len:u16 LE]` framer.
/// `accepted` is the set of permitted magic bytes:
/// - battle.net connections use [`FrameCodec::bncs()`]
/// - player connections use [`FrameCodec::w3gs()`] (accepts both W3GS and GPS, since the GProxy handshake is interleaved on the same TCP)
#[derive(Debug, Clone)]
pub struct FrameCodec {
    accepted: &'static [u8],
}

impl FrameCodec {
    pub fn bncs() -> Self {
        Self { accepted: &[BNCS_HEADER_CONSTANT] }
    }

    pub fn w3gs() -> Self {
        Self { accepted: &[W3GS_HEADER_CONSTANT, GPS_HEADER_CONSTANT] }
    }
}

impl Decoder for FrameCodec {
    type Item = Frame;
    type Error = FrameError;

    fn decode(&mut self, src: &mut BytesMut) -> Result<Option<Frame>, FrameError> {
        // header not yet fully arrived
        if src.len() < 4 {
            return Ok(None);
        }

        let magic = src[0];

        if !self.accepted.contains(&magic) {
            return Err(FrameError::BadMagic { got: magic, accepted: self.accepted });
        }

        let length = u16::from_le_bytes([src[2], src[3]]);

        if length < 4 {
            return Err(FrameError::BadLength(length));
        }

        let length = length as usize;

        // Packet body not yet fully arrived: reserve space and wait for the next read
        if src.len() < length {
            src.reserve(length - src.len());
            return Ok(None);
        }

        let data = src.split_to(length).freeze();

        Ok(Some(Frame { magic, id: data[1], data }))
    }
}

/// Send-side passthrough: the `send_*` functions already produce complete packets
impl Encoder<Vec<u8>> for FrameCodec {
    type Error = FrameError;

    fn encode(&mut self, item: Vec<u8>, dst: &mut BytesMut) -> Result<(), FrameError> {
        dst.extend_from_slice(&item);
        Ok(())
    }
}

impl Encoder<Bytes> for FrameCodec {
    type Error = FrameError;

    fn encode(&mut self, item: Bytes, dst: &mut BytesMut) -> Result<(), FrameError> {
        dst.extend_from_slice(&item);
        Ok(())
    }
}

impl Encoder<Frame> for FrameCodec {
    type Error = FrameError;

    fn encode(&mut self, item: Frame, dst: &mut BytesMut) -> Result<(), FrameError> {
        dst.extend_from_slice(&item.data);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Manually assemble a valid packet: [magic][id][len u16 LE][payload...]
    fn make_packet(magic: u8, id: u8, payload: &[u8]) -> Vec<u8> {
        let len = (payload.len() + 4) as u16;
        let mut packet = vec![magic, id];
        packet.extend_from_slice(&len.to_le_bytes());
        packet.extend_from_slice(payload);
        packet
    }

    #[test]
    fn decode_single_frame() {
        let mut codec = FrameCodec::bncs();
        let packet = make_packet(BNCS_HEADER_CONSTANT, 0x50, &[1, 2, 3, 4]);
        let mut buf = BytesMut::from(&packet[..]);

        let frame = codec.decode(&mut buf).unwrap().unwrap();
        assert_eq!(frame.magic, BNCS_HEADER_CONSTANT);
        assert_eq!(frame.id, 0x50);
        assert_eq!(&frame.data[..], &packet[..]);
        assert_eq!(frame.payload(), &[1, 2, 3, 4]);
        assert!(buf.is_empty());

        // Returns None when there is no more data
        assert!(codec.decode(&mut buf).unwrap().is_none());
    }

    #[test]
    fn decode_partial_then_complete() {
        let mut codec = FrameCodec::w3gs();
        let packet = make_packet(W3GS_HEADER_CONSTANT, 0x1E, &[9, 9, 9, 9, 9, 9]);

        // First feed half (less than the header)
        let mut buf = BytesMut::from(&packet[..3]);
        assert!(codec.decode(&mut buf).unwrap().is_none());

        // Complete the header but leave the payload incomplete
        buf.extend_from_slice(&packet[3..6]);
        assert!(codec.decode(&mut buf).unwrap().is_none());

        // Fill in the rest
        buf.extend_from_slice(&packet[6..]);
        let frame = codec.decode(&mut buf).unwrap().unwrap();
        assert_eq!(frame.id, 0x1E);
        assert_eq!(&frame.data[..], &packet[..]);
    }

    #[test]
    fn decode_multiple_frames_in_one_read() {
        let mut codec = FrameCodec::bncs();
        let p1 = make_packet(BNCS_HEADER_CONSTANT, 0x25, &[0xAA; 4]);
        let p2 = make_packet(BNCS_HEADER_CONSTANT, 0x0F, b"hello\0");

        let mut buf = BytesMut::new();
        buf.extend_from_slice(&p1);
        buf.extend_from_slice(&p2);

        let f1 = codec.decode(&mut buf).unwrap().unwrap();
        let f2 = codec.decode(&mut buf).unwrap().unwrap();
        assert_eq!(f1.id, 0x25);
        assert_eq!(f2.id, 0x0F);
        assert_eq!(f2.payload(), b"hello\0");
        assert!(codec.decode(&mut buf).unwrap().is_none());
    }

    #[test]
    fn decode_bad_magic() {
        let mut codec = FrameCodec::bncs();
        let packet = make_packet(0x00, 0x50, &[]);
        let mut buf = BytesMut::from(&packet[..]);

        assert!(matches!(
            codec.decode(&mut buf),
            Err(FrameError::BadMagic { got: 0x00, .. })
        ));
    }

    #[test]
    fn decode_bad_length() {
        let mut codec = FrameCodec::bncs();
        // len = 2 (< 4)
        let mut buf = BytesMut::from(&[BNCS_HEADER_CONSTANT, 0x50, 2, 0][..]);

        assert!(matches!(codec.decode(&mut buf), Err(FrameError::BadLength(2))));
    }

    #[test]
    fn w3gs_codec_accepts_gps_frames() {
        // The GProxy++ handshake (GPS_HEADER) is interleaved with W3GS on the same player connection
        let mut codec = FrameCodec::w3gs();
        let packet = crate::core::gpsprotocol::send_gpss_ack(42);
        let mut buf = BytesMut::from(&packet[..]);

        let frame = codec.decode(&mut buf).unwrap().unwrap();
        assert_eq!(frame.magic, GPS_HEADER_CONSTANT);
        assert_eq!(frame.id, crate::core::gpsprotocol::GPS_ACK);
        // Compatible with the length field produced by the existing util::assign_length
        assert!(crate::util::validate_length(&frame.data));
    }

    #[test]
    fn encode_roundtrip() {
        let mut codec = FrameCodec::bncs();
        let packet = make_packet(BNCS_HEADER_CONSTANT, 0x0A, &[7, 7]);

        let mut wire = BytesMut::new();
        Encoder::<Vec<u8>>::encode(&mut codec, packet.clone(), &mut wire).unwrap();

        let frame = codec.decode(&mut wire).unwrap().unwrap();
        assert_eq!(&frame.data[..], &packet[..]);

        // Encoding the Frame back also matches
        let mut wire2 = BytesMut::new();
        Encoder::<Frame>::encode(&mut codec, frame, &mut wire2).unwrap();
        assert_eq!(&wire2[..], &packet[..]);
    }
}
