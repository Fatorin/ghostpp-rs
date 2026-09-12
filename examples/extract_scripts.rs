//! Extract Scripts\common.j and Scripts\blizzard.j from a Warcraft III install into bot_mapcfgpath.
//! Usage: cargo run --example extract_scripts -- "<war3 dir>" [out dir = config]
use std::path::Path;
use stormlib::{Archive, OpenArchiveFlags};

fn main() {
    let mut args = std::env::args().skip(1);
    let war3 = args.next().expect("usage: extract_scripts <war3 dir> [out dir]");
    let out = args.next().unwrap_or_else(|| "config".to_string());
    let flags = OpenArchiveFlags::MPQ_OPEN_NO_LISTFILE | OpenArchiveFlags::MPQ_OPEN_NO_ATTRIBUTES;

    for script in ["common.j", "blizzard.j"] {
        let mut done = false;
        // Same priority as the game itself: patch archive overrides the base ones
        for mpq in ["War3Patch.mpq", "War3x.mpq", "War3.mpq"] {
            let path = Path::new(&war3).join(mpq);
            let Ok(mut archive) = Archive::open(&path, flags) else { continue };
            let Ok(mut file) = archive.open_file(&format!("Scripts\\{script}")) else { continue };
            let Ok(data) = file.read_all() else { continue };
            let dest = Path::new(&out).join(script);
            std::fs::write(&dest, &data).expect("write failed");
            println!("{script}: {} bytes from {mpq} -> {}", data.len(), dest.display());
            done = true;
            break;
        }
        if !done {
            eprintln!("{script}: not found in any archive under {war3}");
        }
    }
}
