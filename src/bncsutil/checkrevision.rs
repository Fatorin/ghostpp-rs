//! CheckRevision: compute the exe version hash from local War3 files (mirrors bncsutil `checkrevision.cpp`).
//!
//! PVPGN / battle.net's SID_AUTH_CHECK requires:
//! - exe_version (4 bytes): PE product version (see [`get_exe_version`])
//! - exe_version_hash (4 bytes): [`check_revision`] hashes the exe file contents using the "value string formula"
//! - exe_info string: filename + modification time (UTC) + file size (see [`get_exe_info`])
//!
//! The formula / ix86ver filename are provided by the server in SID_AUTH_INFO.
//!
//! Numeric arithmetic: the C version uses uint64, but only the low 32 bits of values[2] are taken in the end;
//! and the low 32 bits of +,-,^,* depend only on the low 32 bits of the operands, so u32 wrapping yields the
//! same result (see the comment below).

use std::path::{Path, PathBuf};

/// checkrevision MPQ seed table (mirrors the C version's initialize_checkrevision_seeds)
pub const MPQ_SEEDS: [u32; 8] = [
    0xE7F4CB62, 0xF6A14FFC, 0xAA5504AF, 0x871FCDC2, 0x11BF6A18, 0xC57292E6, 0x7927D27E, 0x2FEC8733,
];

/// Extract the MPQ number (1 here) from the ix86ver filename (e.g. "ver-IX86-1.mpq").
/// Mirrors the C version's extractMPQNumber: after finding '.', parse the integer starting one character before it.
pub fn extract_mpq_number(mpq_name: &str) -> Option<usize> {
    let dot = mpq_name.find('.')?;
    if dot == 0 {
        return None;
    }
    // C version atoi(n-1): parse starting from the character before '.' (usually a single digit)
    let start = dot - 1;
    let digits: String = mpq_name[start..dot]
        .chars()
        .take_while(|c| c.is_ascii_digit())
        .collect();
    digits.parse::<usize>().ok()
}

// Variable index: 'S' → 3, the rest 'A'/'B'/'C' → ch - 'A'
fn bucr_getnum(ch: u8) -> Option<usize> {
    let v = if ch == b'S' { 3 } else { (ch as i32) - (b'A' as i32) };
    if (0..=3).contains(&v) {
        Some(v as usize)
    } else {
        None
    }
}

struct Op {
    dest: usize,
    s1: usize,
    op: u8,
    s2: usize,
}

/// Parse the value string formula → (initial values, operation sequence).
fn parse_formula(formula: &str) -> Option<([u32; 4], Vec<Op>)> {
    let mut values = [0u32; 4];
    let mut ops: Vec<Op> = Vec::new();
    let bytes = formula.as_bytes();
    let mut i = 0usize;

    while i < bytes.len() {
        // Require token[1] == '='
        if i + 1 < bytes.len() && bytes[i + 1] == b'=' {
            let variable = bucr_getnum(bytes[i])?;
            let rhs = i + 2;
            if rhs < bytes.len() && bytes[rhs].is_ascii_digit() {
                // Constant assignment: A=1234
                let digits: String = formula[rhs..]
                    .chars()
                    .take_while(|c| c.is_ascii_digit())
                    .collect();
                values[variable] = digits.parse::<u64>().ok()? as u32;
            } else if rhs + 2 < bytes.len() {
                // Operation: dest = s1 <op> s2 (at most 4)
                if ops.len() > 3 {
                    return None;
                }
                ops.push(Op {
                    dest: variable,
                    s1: bucr_getnum(bytes[rhs])?,
                    op: bytes[rhs + 1],
                    s2: bucr_getnum(bytes[rhs + 2])?,
                });
            }
        }

        // Advance past the next space
        while i < bytes.len() && bytes[i] != b' ' {
            i += 1;
        }
        while i < bytes.len() && bytes[i] == b' ' {
            i += 1;
        }
    }

    Some((values, ops))
}

/// Mirrors the C version's checkRevision: hash a set of file contents using formula + mpq seed.
/// Returns the exe_version_hash (u32); returns None if any step fails.
pub fn check_revision(formula: &str, files: &[PathBuf], mpq_number: usize) -> Option<u32> {
    if files.is_empty() || mpq_number >= MPQ_SEEDS.len() {
        return None;
    }

    let (mut values, ops) = parse_formula(formula)?;

    // "hash A by the hashcode"
    values[0] ^= MPQ_SEEDS[mpq_number];

    for path in files {
        let mut data = std::fs::read(path).ok()?;

        // Pad to a multiple of 1024: padding starts at 0xFF and decrements each byte
        let remainder = data.len() % 1024;
        if remainder != 0 {
            let extra = 1024 - remainder;
            let mut pad: u8 = 0xFF;
            for _ in 0..extra {
                data.push(pad);
                pad = pad.wrapping_sub(1);
            }
        }

        // Process 4 bytes (LE) at a time
        for chunk in data.chunks_exact(4) {
            values[3] = u32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]);
            for op in &ops {
                let a = values[op.s1];
                let b = values[op.s2];
                // The low 32 bits of +,-,^,* depend only on the low 32 bits of the operands; u32 wrapping is equivalent to the C version's uint64
                values[op.dest] = match op.op {
                    b'+' => a.wrapping_add(b),
                    b'-' => a.wrapping_sub(b),
                    b'^' => a ^ b,
                    b'*' => a.wrapping_mul(b),
                    b'/' => {
                        if b == 0 {
                            return None;
                        }
                        a / b
                    }
                    _ => return None,
                };
            }
        }
    }

    Some(values[2])
}

/// Select the list of files to hash, based on which files actually exist (do not fully trust war3_version).
/// - Single-file layout (1.29+ / some 1.28): `Warcraft III.exe`
/// - Three-file layout (1.26~1.28 classic / FateAnother etc.): exe + Storm.dll + game.dll,
///   in the order exe → Storm.dll → game.dll (mirrors file1/2/3 in the C version's checkRevisionFlat)
///
/// Note: when the three files exist (especially game.dll), **prefer** the three-file path even if a single-file exe is also present.
pub fn select_war3_files(war3_path: &str) -> Option<Vec<PathBuf>> {
    let base = Path::new(war3_path);

    // Main exe candidates (clients name it differently: war3.exe / warcraft.exe / Warcraft III.exe)
    let exe = first_existing(
        base,
        &[
            "war3.exe",
            "War3.exe",
            "warcraft.exe",
            "Warcraft.exe",
            "Warcraft III.exe",
        ],
    )?;
    let storm = first_existing(base, &["Storm.dll", "storm.dll"]);
    let game = first_existing(base, &["game.dll", "Game.dll"]);

    // Three-file layout: exe + Storm.dll + game.dll are all present
    if let (Some(storm), Some(game)) = (storm, game) {
        return Some(vec![exe, storm, game]);
    }

    // Otherwise single-file (e.g. 1.29+'s "Warcraft III.exe")
    Some(vec![exe])
}

fn first_existing(base: &Path, names: &[&str]) -> Option<PathBuf> {
    names.iter().map(|n| base.join(n)).find(|p| p.exists())
}

/// Scan the PE file for the VS_FIXEDFILEINFO signature (0xFEEF04BD), read out the product version and pack it into
/// bncsutil's 4-byte exe version. Returns None if not found.
pub fn get_exe_version(exe_path: &Path) -> Option<u32> {
    let data = std::fs::read(exe_path).ok()?;
    // VS_FIXEDFILEINFO signature 0xFEEF04BD, stored LE in the file: BD 04 EF FE
    let sig = [0xBDu8, 0x04, 0xEF, 0xFE];
    let pos = data.windows(4).position(|w| w == sig)?;
    // Layout: sig(0) strucVer(4) fileVerMS(8) fileVerLS(12) prodVerMS(16) prodVerLS(20)
    let read_u32 = |off: usize| -> Option<u32> {
        let p = pos + off;
        data.get(p..p + 4)
            .map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    };
    let prod_ms = read_u32(16)?;
    let prod_ls = read_u32(20)?;
    // Mirrors getExeInfo: ((MS>>16 &0xFF)<<24)|((MS&0xFF)<<16)|((LS>>16 &0xFF)<<8)|(LS&0xFF)
    let version = ((prod_ms >> 16) & 0xFF) << 24
        | (prod_ms & 0xFF) << 16
        | ((prod_ls >> 16) & 0xFF) << 8
        | (prod_ls & 0xFF);
    Some(version)
}

/// Produce the exe_info string: "<basename> MM/DD/YY HH:MM:SS <size>" (UTC modification time).
/// Mirrors the string output of getExeInfo (excludes the PE version, which is provided separately by get_exe_version).
pub fn get_exe_info(exe_path: &Path) -> Option<String> {
    let meta = std::fs::metadata(exe_path).ok()?;
    let size = meta.len();
    let basename = exe_path.file_name()?.to_string_lossy().to_string();

    let modified = meta.modified().ok()?;
    let secs = modified
        .duration_since(std::time::UNIX_EPOCH)
        .ok()?
        .as_secs();
    let (year, mon, day, hour, min, sec) = civil_from_unix(secs);

    Some(format!(
        "{basename} {:02}/{:02}/{:02} {:02}:{:02}:{:02} {size}",
        mon,
        day,
        year % 100,
        hour,
        min,
        sec
    ))
}

/// Unix seconds → UTC (year, month, day, hour, min, sec).
/// Uses Howard Hinnant's civil-from-days algorithm, no external crate needed.
pub fn civil_from_unix(secs: u64) -> (i64, u32, u32, u32, u32, u32) {
    let days = (secs / 86400) as i64;
    let rem = (secs % 86400) as u32;
    let hour = rem / 3600;
    let min = (rem % 3600) / 60;
    let sec = rem % 60;

    // days since 1970-01-01 → civil date
    let z = days + 719468;
    let era = if z >= 0 { z } else { z - 146096 } / 146097;
    let doe = z - era * 146097; // [0, 146096]
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365; // [0, 399]
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100); // [0, 365]
    let mp = (5 * doy + 2) / 153; // [0, 11]
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32; // [1, 31]
    let m = (if mp < 10 { mp + 3 } else { mp - 9 }) as u32; // [1, 12]
    let year = if m <= 2 { y + 1 } else { y };

    (year, m, d, hour, min, sec)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extract_mpq() {
        assert_eq!(extract_mpq_number("ver-IX86-1.mpq"), Some(1));
        assert_eq!(extract_mpq_number("ver-IX86-0.mpq"), Some(0));
        assert_eq!(extract_mpq_number("IX86ver7.mpq"), Some(7));
        assert_eq!(extract_mpq_number("noext"), None);
    }

    #[test]
    fn parse_and_hash_xor_formula() {
        // Verify formula VM parsing: constant assignment + operation sequence
        let (vals, ops) = parse_formula("A=5 B=6 C=7 4 A=A^S C=C^A").unwrap();
        assert_eq!(vals[0], 5);
        assert_eq!(vals[1], 6);
        assert_eq!(vals[2], 7);
        assert_eq!(ops.len(), 2);
        assert_eq!(ops[0].op, b'^');
    }

    #[test]
    fn civil_epoch() {
        // 1970-01-01 00:00:00
        assert_eq!(civil_from_unix(0), (1970, 1, 1, 0, 0, 0));
        // 2000-01-01 00:00:00 = 946684800
        assert_eq!(civil_from_unix(946684800), (2000, 1, 1, 0, 0, 0));
    }
}
