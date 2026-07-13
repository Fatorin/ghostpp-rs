//! Battle.net "Broken SHA-1" (XSHA1).
//!
//! Mirrors `calcHashBuf` in bncsutil `bsha1.cpp` (USE_NEW_BSHA1 = 0 path).
//! Purpose: PVPGN password proof (`HELP_PvPGNPasswordHash` → `hashPassword` → `calcHashBuf`)
//! and legacy SID_LOGONRESPONSE. Different from standard SHA-1; must not be mixed.
//!
//! The algorithm operates entirely on 32-bit little-endian values (LSB4 is the identity on
//! little-endian platforms, so u32 is used directly); all add/sub must be wrapping, otherwise a
//! debug build will overflow-panic.

// Initial values
const BSHA_IC1: u32 = 0x67452301;
const BSHA_IC2: u32 = 0xEFCDAB89;
const BSHA_IC3: u32 = 0x98BADCFE;
const BSHA_IC4: u32 = 0x10325476;
const BSHA_IC5: u32 = 0xC3D2E1F0;

// Per-round constants
const BSHA_OC1: u32 = 0x5A827999;
const BSHA_OC2: u32 = 0x6ED9EBA1;
const BSHA_OC3: u32 = 0x70E44324;
const BSHA_OC4: u32 = 0x359D3E2A;

/// Compute the 20-byte XSHA1 over `data` (mostly string content, up to 1024 bytes).
/// Corresponds to C++ `calcHashBuf(input, length, result)`.
pub fn calc_hash_buf(input: &[u8]) -> [u8; 20] {
    // 1024-byte buffer, zero-padded; read in as little-endian u32
    let mut bytes = [0u8; 1024];
    let n = input.len().min(1024);
    bytes[..n].copy_from_slice(&input[..n]);

    let mut ldata = [0u32; 256];
    for i in 0..256 {
        let b = i * 4;
        ldata[i] = u32::from_le_bytes([bytes[b], bytes[b + 1], bytes[b + 2], bytes[b + 3]]);
    }

    // Data expansion: ldata[i+16] = ROL(1, (ldata[i]^ldata[i+8]^ldata[i+2]^ldata[i+13]) % 32)
    for i in 0..64 {
        let x = ldata[i] ^ ldata[i + 8] ^ ldata[i + 2] ^ ldata[i + 13];
        ldata[i + 16] = 1u32.rotate_left(x % 32);
    }

    let mut a = BSHA_IC1;
    let mut b = BSHA_IC2;
    let mut c = BSHA_IC3;
    let mut d = BSHA_IC4;
    let mut e = BSHA_IC5;
    let mut g: u32 = 0;

    // BSHA_COP: state rotation
    macro_rules! cop {
        () => {
            e = d;
            d = c;
            c = b.rotate_left(30);
            b = a;
            a = g;
        };
    }

    let mut idx = 0usize;
    macro_rules! next_f {
        () => {{
            let f = ldata[idx];
            idx += 1;
            f
        }};
    }

    // 20 × OP1
    for _ in 0..20 {
        let f = next_f!();
        g = f
            .wrapping_add(a.rotate_left(5))
            .wrapping_add(e)
            .wrapping_add((b & c) | (!b & d))
            .wrapping_add(BSHA_OC1);
        cop!();
    }
    // 20 × OP2
    for _ in 0..20 {
        let f = next_f!();
        g = (d ^ c ^ b)
            .wrapping_add(e)
            .wrapping_add(g.rotate_left(5))
            .wrapping_add(f)
            .wrapping_add(BSHA_OC2);
        cop!();
    }
    // 20 × OP3
    for _ in 0..20 {
        let f = next_f!();
        g = f
            .wrapping_add(g.rotate_left(5))
            .wrapping_add(e)
            .wrapping_add((c & b) | (d & c) | (d & b))
            .wrapping_sub(BSHA_OC3);
        cop!();
    }
    // 20 × OP4
    for _ in 0..20 {
        let f = next_f!();
        g = (d ^ c ^ b)
            .wrapping_add(e)
            .wrapping_add(g.rotate_left(5))
            .wrapping_add(f)
            .wrapping_sub(BSHA_OC4);
        cop!();
    }

    let mut result = [0u8; 20];
    result[0..4].copy_from_slice(&BSHA_IC1.wrapping_add(a).to_le_bytes());
    result[4..8].copy_from_slice(&BSHA_IC2.wrapping_add(b).to_le_bytes());
    result[8..12].copy_from_slice(&BSHA_IC3.wrapping_add(c).to_le_bytes());
    result[12..16].copy_from_slice(&BSHA_IC4.wrapping_add(d).to_le_bytes());
    result[16..20].copy_from_slice(&BSHA_IC5.wrapping_add(e).to_le_bytes());
    result
}

/// PVPGN password proof: single XSHA1 (plaintext password).
/// Corresponds to bncsutil `hashPassword`.
pub fn hash_password(password: &str) -> [u8; 20] {
    calc_hash_buf(password.as_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hash_len_and_nonzero() {
        let h = hash_password("password");
        assert_eq!(h.len(), 20);
        assert!(h.iter().any(|&b| b != 0));
    }

    #[test]
    fn deterministic() {
        assert_eq!(hash_password("hunter2"), hash_password("hunter2"));
        assert_ne!(hash_password("hunter2"), hash_password("hunter3"));
    }

    /// Regression vector: XSHA1 of the fixed input "ghostpp".
    /// This value is indirectly validated by real-machine PVPGN login (2026-07 FateAnother accepted the password proof).
    /// If bsha1 is broken in the future, this test goes red immediately.
    #[test]
    fn regression_vector_password() {
        const EXPECTED: [u8; 20] = [
            0x6A, 0xB8, 0x4F, 0x3D, 0x49, 0x63, 0xE5, 0x77, 0x24, 0x07, 0x6F, 0xBF, 0x4C, 0x6B,
            0x32, 0x82, 0x58, 0xE4, 0x75, 0xD7,
        ];
        assert_eq!(
            hash_password("ghostpp"),
            EXPECTED,
            "XSHA1(\"ghostpp\") regression value mismatch — bsha1 may have been broken"
        );
    }
}
