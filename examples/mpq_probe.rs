//! Probe a map: open the MPQ and try to read the files the bot needs.
//! Usage: cargo run --example mpq_probe -- <map.w3x>
use stormlib::{Archive, OpenArchiveFlags};

fn main() {
    let path = std::env::args().nth(1).expect("usage: mpq_probe <map.w3x>");
    let flags = OpenArchiveFlags::MPQ_OPEN_NO_LISTFILE | OpenArchiveFlags::MPQ_OPEN_NO_ATTRIBUTES;
    let mut archive = match Archive::open(&path, flags) {
        Ok(a) => {
            println!("open: OK");
            a
        }
        Err(e) => {
            println!("open: FAILED: {e:?}");
            return;
        }
    };
    let names = [
        "war3map.w3i",
        "war3map.j",
        r"scripts\war3map.j",
        "war3map.w3e",
        "war3map.wpm",
        "war3map.doo",
        "war3map.w3u",
        "war3map.w3b",
        "war3map.w3d",
        "war3map.w3a",
        "war3map.w3q",
        r"Scripts\common.j",
        r"Scripts\blizzard.j",
        "kkmap.jc",
        "kkmap.jd",
        "war3map.lua",
        "(listfile)",
    ];
    for name in names {
        match archive.open_file(name) {
            Ok(mut f) => match f.read_all() {
                Ok(d) => {
                    let head: String = d
                        .iter()
                        .take(120)
                        .map(|&b| if (0x20..0x7f).contains(&b) { b as char } else { '.' })
                        .collect();
                    println!("{name}: {} bytes  head={head:?}", d.len());
                }
                Err(e) => println!("{name}: open OK, read FAILED: {e:?}"),
            },
            Err(e) => println!("{name}: not found ({e:?})"),
        }
    }
}
