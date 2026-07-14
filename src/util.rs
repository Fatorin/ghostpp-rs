use config::Config;
use std::fs::File;
use std::io;
use std::io::{Error, ErrorKind, Read};
use std::net::{SocketAddr, SocketAddrV4, ToSocketAddrs};
use std::str::FromStr;
use std::sync::OnceLock;
use std::time::Instant;
use tracing::warn;

// ---- Chat/name encoding (the client is UTF-8 throughout; only lenient decoding, to avoid a bad byte turning a whole line empty) ----

/// packet bytes → String, lossy UTF-8 decode: bad bytes become the replacement char (U+FFFD), never returns empty.
/// (Replaces the old strict from_utf8 — on failure it returned an empty string, so a single bad byte ate the entire chat line.)
pub fn util_decode_ansi(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

/// String → packet bytes (UTF-8 passthrough)
pub fn util_encode_ansi(s: &str) -> Vec<u8> {
    s.as_bytes().to_vec()
}

/// Current time as "YYYY-MM-DD HH:MM:SS" (UTC; used by database records)
pub fn now_datetime_string() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let (y, mo, d, h, mi, s) = crate::bncsutil::checkrevision::civil_from_unix(secs);
    format!("{y:04}-{mo:02}-{d:02} {h:02}:{mi:02}:{s:02}")
}

/// Program startup reference point.
/// Fix: the old `get_ticks()` called `Instant::now().elapsed()` on every call, always returning 0,
/// which broke all logic based on time deltas (keepalive, reconnect, flood protection).
static START_INSTANT: OnceLock<Instant> = OnceLock::new();

pub fn get_time_and_ticks() -> (u64, u64) {
    let ticks = get_ticks();
    let time = ticks / 1000;
    (time, ticks)
}

/// Mirrors C++ GetTime(): seconds since startup
pub fn get_time() -> u64 {
    get_ticks() / 1000
}

/// Mirrors C++ GetTicks(): milliseconds since startup
pub fn get_ticks() -> u64 {
    let start = START_INSTANT.get_or_init(Instant::now);
    start.elapsed().as_millis() as u64
}

pub fn util_create_byte_array(i: u32, reverse: bool) -> Vec<u8> {
    let mut result = vec![
        (i & 0xFF) as u8,
        ((i >> 8) & 0xFF) as u8,
        ((i >> 16) & 0xFF) as u8,
        ((i >> 24) & 0xFF) as u8,
    ];

    if reverse {
        result.reverse();
    }

    result
}

pub fn util_extract_numbers(s: &str, count: usize) -> Vec<u8> {
    let mut result = Vec::new();
    let mut ss = s.split_whitespace();

    for _ in 0..count {
        if let Some(num_str) = ss.next() {
            if let Ok(c) = u8::from_str(num_str) {
                result.push(c);
            } else {
                // todotodo: if c > 255 handle the error instead of truncating
                break;
            }
        } else {
            break;
        }
    }

    result
}

pub fn util_byte_array_to_u16(b: &[u8], reverse: bool, start: usize) -> u16 {
    if b.len() >= start + 2 {
        let mut array = [0u8; 2];
        array.copy_from_slice(&b[start..start + 2]);
        if reverse {
            array.reverse();
        }
        u16::from_le_bytes(array)
    } else {
        0
    }
}

pub fn util_byte_array_to_u32(b: &[u8], reverse: bool, start: usize) -> u32 {
    if b.len() >= start + 4 {
        let mut array = [0u8; 4];
        array.copy_from_slice(&b[start..start + 4]);
        if reverse {
            array.reverse();
        }
        u32::from_le_bytes(array)
    } else {
        0
    }
}

/// Mirrors C++ UTIL_ExtractCString: take content up to the first NUL; if no NUL is found, take to the end.
/// Fix: in the old version, even after finding the NUL the following line unconditionally overwrote it with "the whole span to the end".
pub fn util_extract_cstring(b: &[u8], start: usize) -> String {
    if start >= b.len() {
        return String::new();
    }

    let end = b[start..]
        .iter()
        .position(|&c| c == 0)
        .map(|pos| start + pos)
        .unwrap_or(b.len());

    // Human text such as names/chat is decoded with the client ANSI encoding (Traditional Chinese Big5, etc.).
    // Pure protocol strings are ASCII; Big5/GBK are ASCII-compatible, so behavior is unchanged.
    util_decode_ansi(&b[start..end])
}

pub fn util_file_read_full(file_path: &str) -> String {
    let mut file = match File::open(file_path) {
        Ok(file) => file,
        Err(_) => {
            warn!("[UTIL] warning - unable to read file [{}]", file_path);
            return String::new();
        }
    };

    let mut contents = String::new();
    match file.read_to_string(&mut contents) {
        Ok(_) => contents,
        Err(_) => {
            warn!("[UTIL] warning - unable to read file");
            return String::new();
        }
    }
}

/// Mirrors C++ UTIL_ToHexString: lowercase hexadecimal, no prefix, no zero padding
pub fn util_to_hex_string(n: u32) -> String {
    format!("{:x}", n)
}

pub fn util_byte_array_to_dec_string(vec: &[u8]) -> String {
    let mut dec_string = String::new();
    for (index, byte) in vec.iter().enumerate() {
        if index > 0 {
            dec_string.push(' ');
        }
        dec_string.push_str(&byte.to_string());
    }
    dec_string
}

pub fn util_byte_array_to_hex_string(vec: &[u8]) -> String {
    let mut hex_string = String::new();
    for byte in vec {
        hex_string.push_str(&format!("{:02x}", byte));
    }
    hex_string
}

pub fn util_calc_crc32(data: &[u8]) -> u32 {
    let mut hasher = crc32fast::Hasher::new();
    hasher.update(data);
    let checksum = hasher.finalize();
    return checksum;
}

pub fn get_u8_from_config(conig: &Config, key: &str, default_value: u8) -> u8 {
    let result = conig.get_int(key);
    if result.is_err() {
        return default_value;
    }

    return match u8::try_from(result.unwrap()) {
        Ok(value) => value,
        Err(_) => return default_value,
    };
}

pub fn get_u16_from_config(conig: &Config, key: &str, default_value: u16) -> u16 {
    let result = conig.get_int(key);
    if result.is_err() {
        return default_value;
    }

    return match u16::try_from(result.unwrap()) {
        Ok(value) => value,
        Err(_) => return default_value,
    };
}

pub fn get_u32_from_config(conig: &Config, key: &str, default_value: u32) -> u32 {
    let result = conig.get_int(key);
    if result.is_err() {
        return default_value;
    }

    return match u32::try_from(result.unwrap()) {
        Ok(value) => value,
        Err(_) => return default_value,
    };
}

pub fn util_encode_stat_string(data: &[u8]) -> Vec<u8> {
    let mut result: Vec<u8> = Vec::new();
    let mut mask: u8 = 1;

    for (i, &byte) in data.iter().enumerate() {
        if byte % 2 == 0 {
            result.push(byte + 1);
        } else {
            result.push(byte);
            mask |= 1 << ((i % 7) + 1);
        }

        if i % 7 == 6 || i == data.len() - 1 {
            let insert_pos = result.len() - 1 - (i % 7);
            result.insert(insert_pos, mask);
            mask = 1;
        }
    }

    result
}

// for protocol
pub fn assign_length(content: &mut Vec<u8>) -> bool {
    if content.len() >= 4 && content.len() <= 65535
    {
        let length_bytes = content.len().to_le_bytes();
        content[2] = length_bytes[0];
        content[3] = length_bytes[1];
        return true;
    }

    return false;
}

// for protocol
pub fn validate_length(content: &[u8]) -> bool {
    // verify that bytes 3 and 4 (indices 2 and 3) of the content array describe the length
    if content.len() < 4 {
        return false;
    }

    return ((content[3] as usize) << 8 | content[2] as usize) == content.len();
}


pub fn get_ipv4_address(hostname: &str) -> io::Result<SocketAddrV4> {
    match hostname.to_socket_addrs() {
        Ok(addresses) => {
            for addr in addresses {
                if let SocketAddr::V4(ipv4_addr) = addr {
                    return Ok(ipv4_addr);
                }
            }
            Err(Error::new(ErrorKind::from(ErrorKind::InvalidData), "can't reslvoe ipv4 address."))
        }
        Err(err) => {
            Err(Error::new(ErrorKind::from(ErrorKind::InvalidData), format!("DNS resolution error: {}", err)))
        }
    }
}

/// Truncate a string to a byte length, but guarantee it never cuts in the middle of a UTF-8 character
pub fn util_truncate_str(s: &str, max_bytes: usize) -> &str {
    if s.len() <= max_bytes {
        return s;
    }

    let mut end = max_bytes;
    while !s.is_char_boundary(end) {
        end -= 1;
    }

    &s[..end]
}

pub fn get_command_and_payload(message: &str) -> (String, String) {
    let command;
    let payload;

    if let Some(payload_start) = message.find(' ') {
        command = &message[1..payload_start];
        payload = &message[payload_start + 1..];
    } else {
        command = &message[1..];
        payload = "";
    }

    (command.to_lowercase(), payload.to_string())
}

#[cfg(test)]
mod tests {
    use crate::util::*;

    #[test]
    fn test_util_byte_array_to_hex_string() {
        let vec = vec![0x2a, 0xae, 0x6c, 0x35, 0xc9, 0x4f, 0xcf, 0xb4, 0x15, 0xdb, 0xe9, 0x5f, 0x40, 0x8b, 0x9c, 0xe9, 0x1e, 0xe8, 0x46, 0xed];
        let hex_string = util_byte_array_to_hex_string(&vec);
        let expected_hex = "2aae6c35c94fcfb415dbe95f408b9ce91ee846ed";
        assert_eq!(hex_string, expected_hex);
    }

    #[test]
    fn test_util_file_read_full() {
        let file_path = "config/blizzard.j";
        let data = util_file_read_full(&file_path);
        assert!(data.len() > 0);
    }

    #[test]
    fn test_get_command_and_payload() {
        let (command, payload) = get_command_and_payload("!host 1234");
        assert_eq!(command, String::from("host"));
        assert_eq!(payload, String::from("1234"));
    }
}