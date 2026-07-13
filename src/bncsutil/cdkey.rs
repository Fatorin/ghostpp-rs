//! Warcraft III CD key decoder (mirrors the WAR3 path in bncsutil `cdkeydecoder.cpp`).
//!
//! Purpose: during PVPGN / battle.net login, `SID_AUTH_CHECK` needs the 26-character W3 key
//! decoded into product / value1 / value2, then assembled into a 36-byte keyinfo (see [`create_key_info`]).
//!
//! The algorithm mirrors the C version line by line: base-5 bignum multiply-accumulate → `decode_key_table`
//! bit reshuffle → extract product/value1/value2 according to endianness. Wrapping arithmetic throughout.

use sha1::{Digest, Sha1};

use super::keytables::{W3_KEY_MAP, W3_TRANSLATE_MAP};

const W3_KEYLEN: usize = 26;
const W3_BUFLEN: usize = W3_KEYLEN * 2; // 52

#[derive(Debug, Clone)]
pub struct CdKeyW3 {
    /// getProduct(): W3 product code
    product: u32,
    /// getVal1()
    value1: u32,
    /// 10-byte value2 (getLongVal2)
    value2: [u8; 10],
}

impl CdKeyW3 {
    /// Decode a 26-character W3 key. Returns None if the format is invalid.
    pub fn decode(key: &str) -> Option<CdKeyW3> {
        if key.len() != W3_KEYLEN || !key.bytes().all(|b| b.is_ascii_alphanumeric()) {
            return None;
        }

        let mut table = [0u8; W3_BUFLEN];
        // a / b are both overwritten in the first loop iteration; the initial values only mirror the C version (a=0, b=0x21)
        let mut a: usize;
        let mut b: usize = 0x21;

        for ch in key.bytes() {
            let up = ch.to_ascii_uppercase();
            a = (b + 0x07B5) % W3_BUFLEN;
            b = (a + 0x07B5) % W3_BUFLEN;
            let decode = W3_KEY_MAP[up as usize];
            if decode == 0xFF {
                return None; // Illegal character (no entry in the lookup table)
            }
            table[a] = decode / 5;
            table[b] = decode % 5;
        }

        // base-5 bignum: values = Σ table[51..0] * 5^k (accumulated from high to low digits)
        let mut values = [0u32; 4];
        for i in (0..W3_BUFLEN).rev() {
            mult(&mut values, 5, table[i] as u32);
        }

        decode_key_table(&mut values);

        // product = values[0] >> 10 (values has not been byteswapped yet at this point)
        let product = values[0] >> 0x0A;

        // On little-endian platforms the C version byteswaps each of the 4 ints; equivalent to laying out
        // each value as big-endian into a contiguous 16-byte mem buffer
        let mut mem = [0u8; 16];
        mem[0..4].copy_from_slice(&values[0].to_be_bytes());
        mem[4..8].copy_from_slice(&values[1].to_be_bytes());
        mem[8..12].copy_from_slice(&values[2].to_be_bytes());
        mem[12..16].copy_from_slice(&values[3].to_be_bytes());

        // value1 = (*(u32*)(mem+2)) & 0xFFFFFF03 (native LE read)
        let value1 = u32::from_le_bytes([mem[2], mem[3], mem[4], mem[5]]) & 0xFFFFFF03;

        // value2 (10 bytes): mirrors the C version's three-part MSB2/MSB4 moves
        let value2 = [
            mem[7], mem[6], // MSB2(*(u16*)(mem+6))
            mem[11], mem[10], mem[9], mem[8], // MSB4(*(u32*)(mem+8))
            mem[15], mem[14], mem[13], mem[12], // MSB4(*(u32*)(mem+12))
        ];

        Some(CdKeyW3 { product, value1, value2 })
    }

    /// getProduct() (W3: MSB4(product) = product, because product is already handled inside decode)
    pub fn product(&self) -> u32 {
        self.product
    }

    /// getVal1() (W3: MSB4(value1) = value1.swap_bytes())
    pub fn value1(&self) -> u32 {
        self.value1.swap_bytes()
    }

    /// CD key hash for SID_AUTH_CHECK (standard SHA-1, 26-byte input).
    /// Mirrors the W3 branch of the C version's calculateHash.
    pub fn calculate_hash(&self, client_token: u32, server_token: u32) -> [u8; 20] {
        let mut input = Vec::with_capacity(26);
        input.extend_from_slice(&client_token.to_le_bytes());
        input.extend_from_slice(&server_token.to_le_bytes());
        input.extend_from_slice(&self.product().to_le_bytes());
        input.extend_from_slice(&self.value1().to_le_bytes());
        input.extend_from_slice(&self.value2);

        let mut hasher = Sha1::new();
        hasher.update(&input);
        hasher.finalize().into()
    }
}

/// bignum multiply-accumulate: values = values * x + carry (x=5, 4 limbs, big-endian carry).
/// Mirrors the C version `mult(4, 5, values+3, dcByte)`.
fn mult(values: &mut [u32; 4], x: u64, digit: u32) {
    let mut carry = digit;
    for i in (0..4).rev() {
        let edxeax = (values[i] as u64) * x;
        let low = (edxeax & 0xFFFF_FFFF) as u32;
        values[i] = carry.wrapping_add(low);
        carry = (edxeax >> 32) as u32;
    }
}

/// Mirrors the C version `decodeKeyTable`, performing two bit-reshuffle passes over the 4 32-bit limbs.
fn decode_key_table(key_table: &mut [u32; 4]) {
    // ---- pass 1 ----
    let mut var8: u32 = 29;
    let mut i: i32 = 464;

    loop {
        let esi = (var8 & 7) << 2;
        let var4 = var8 >> 3;
        let mut var_c = key_table[(3 - var4) as usize];
        var_c &= 0xF << esi;
        var_c >>= esi;

        if i < 464 {
            let mut j: i32 = 29;
            while (j as u32) > var8 {
                let ecx = ((j & 7) << 2) as u32;
                let mut ebp = key_table[(3 - (j >> 3)) as usize];
                ebp &= 0xF << ecx;
                ebp >>= ecx;
                let idx = ebp
                    ^ (W3_TRANSLATE_MAP[(var_c as i32 + i) as usize] as u32)
                        .wrapping_add(i as u32);
                var_c = W3_TRANSLATE_MAP[idx as usize] as u32;
                j -= 1;
            }
        }

        var8 = var8.wrapping_sub(1);
        let mut j: i32 = var8 as i32;
        while j >= 0 {
            let ecx = ((j & 7) << 2) as u32;
            let mut ebp = key_table[(3 - (j >> 3)) as usize];
            ebp &= 0xF << ecx;
            ebp >>= ecx;
            let idx = ebp
                ^ (W3_TRANSLATE_MAP[(var_c as i32 + i) as usize] as u32).wrapping_add(i as u32);
            var_c = W3_TRANSLATE_MAP[idx as usize] as u32;
            j -= 1;
        }

        let jj = (3 - var4) as usize;
        let ebx = ((W3_TRANSLATE_MAP[(var_c as i32 + i) as usize] as u32) & 0xF) << esi;
        key_table[jj] = ebx | (!(0xF << esi) & key_table[jj]);

        i -= 16;
        if i < 0 {
            break;
        }
    }

    // ---- pass 2 ----
    let copy: [u32; 4] = *key_table;
    let mut esi: u32 = 0;

    for edi in 0..120u32 {
        let eax = edi & 0x1F;
        let ecx = esi & 0x1F;
        let edx = (3 - (edi >> 5)) as usize;

        // location = 12 - ((esi>>5)<<2) → copy[3 - (esi>>5)]
        let ebp_word = copy[(3 - (esi >> 5)) as usize];
        let ebp = (ebp_word & (1 << ecx)) >> ecx;

        let ckt_temp = key_table[edx];
        let mut ckt = ebp & 1;
        ckt <<= eax;
        ckt |= !(1u32 << eax) & ckt_temp;
        key_table[edx] = ckt;

        esi += 0x0B;
        if esi >= 120 {
            esi -= 120;
        }
    }
}

/// Assemble the 36-byte keyinfo for SID_AUTH_CHECK (mirrors GHost `CreateKeyInfo`).
/// Layout: [key_len u32 LE][product u32 LE][value1 u32 LE][0u32][sha1 20 bytes].
/// Returns an empty Vec when the key is invalid.
pub fn create_key_info(key: &str, client_token: u32, server_token: u32) -> Vec<u8> {
    match CdKeyW3::decode(key) {
        Some(decoder) => {
            let mut info = Vec::with_capacity(36);
            info.extend_from_slice(&(key.len() as u32).to_le_bytes());
            info.extend_from_slice(&decoder.product().to_le_bytes());
            info.extend_from_slice(&decoder.value1().to_le_bytes());
            info.extend_from_slice(&[0u8; 4]);
            info.extend_from_slice(&decoder.calculate_hash(client_token, server_token));
            info
        }
        None => Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_bad_length() {
        assert!(CdKeyW3::decode("TOOSHORT").is_none());
        assert!(CdKeyW3::decode("").is_none());
    }

    #[test]
    fn rejects_non_alnum() {
        // 26 characters but contains an illegal character
        assert!(CdKeyW3::decode("AAAAAAAAAAAAAAAAAAAAAAAA!!").is_none());
    }

    /// The W3 key's valid alphabet is "246789BCDEFGHJKMNPRTVWXYZ" (excludes easily-confused characters like 0/1/A/I/L/O/S/U).
    /// This 26-character key is drawn entirely from the valid alphabet, so it runs the full decode flow (not a real serial, tests structure only).
    const VALID_KEY: &str = "2468BCDEFGHJKMNPRTVWXYZ246";

    #[test]
    fn keyinfo_len_is_36_for_valid_key() {
        let info = create_key_info(VALID_KEY, 0xDEADBEEF, 0x12345678);
        assert_eq!(info.len(), 36);
        // key_len field = 26
        assert_eq!(u32::from_le_bytes([info[0], info[1], info[2], info[3]]), 26);
    }

    #[test]
    fn rejects_invalid_alphabet() {
        // 'A' is not in the W3 alphabet, so it should be rejected even at 26 characters
        assert!(CdKeyW3::decode("AAAAAAAAAAAAAAAAAAAAAAAAAA").is_none());
    }

    #[test]
    fn hash_is_deterministic() {
        let d = CdKeyW3::decode(VALID_KEY).unwrap();
        assert_eq!(
            d.calculate_hash(1, 2),
            d.calculate_hash(1, 2),
            "same input must hash consistently"
        );
        assert_ne!(d.calculate_hash(1, 2), d.calculate_hash(3, 4));
    }

    /// Regression vector: 36-byte keyinfo for a fixed key + fixed tokens (fully deterministic, reproducible).
    /// First 4 bytes = key length 26 (0x1A); this decode path is indirectly validated by real-machine PVPGN login.
    /// If cdkey decoding / keyinfo assembly is broken in the future, this test goes red immediately.
    #[test]
    fn regression_vector_keyinfo() {
        const EXPECTED: [u8; 36] = [
            0x1A, 0x00, 0x00, 0x00, 0xDD, 0x6B, 0x00, 0x00, 0x1D, 0x4A, 0x13, 0x00, 0x00, 0x00,
            0x00, 0x00, 0x33, 0xD3, 0x57, 0x5A, 0xAF, 0x75, 0x48, 0xE5, 0xEF, 0x69, 0x77, 0x3D,
            0xFC, 0x7E, 0x5B, 0x72, 0xC6, 0xD4, 0xA7, 0xA1,
        ];
        // Fixed non-secret input: valid-alphabet key + fixed client/server token
        assert_eq!(
            create_key_info(VALID_KEY, 0xDEAD_BEEF, 0x1234_5678),
            EXPECTED.to_vec(),
            "keyinfo regression value mismatch — cdkey decoding or keyinfo assembly may have been broken"
        );
    }
}
