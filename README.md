# ghostpp-rs

**A Rust rewrite of the classic Warcraft III hosting bot [GHost++](https://github.com/uakfdotb/ghostpp).**

[![Release](https://github.com/Fatorin/ghostpp-rs/actions/workflows/release.yml/badge.svg)](https://github.com/Fatorin/ghostpp-rs/actions/workflows/release.yml)
[![License](https://img.shields.io/badge/license-Apache--2.0-blue.svg)](LICENSE)

English | [繁體中文](README.zh-TW.md)

ghostpp-rs targets **Warcraft III 1.26–1.28 on PVPGN servers** (private realms / gaming platforms). The original C++ single-threaded 50 ms select loop is replaced with a tokio async actor architecture, while the protocol layer is ported byte-for-byte against the C++ original.

## Features

- **PVPGN login** — CD key decoding, XSHA1 password hashing and checkRevision (exe hash) implemented in pure Rust (embedded bncsutil port), no external C libraries
- **Hosting & lobby** — STARTADVEX3 advertising with 3-second refresh, player joins / slot management, free team/colour/race/handicap switching, HCL mode-string encoding
- **Map downloads** — MAPCHECK / STARTDOWNLOAD / MAPPART sliding-window transfer with live progress in the lobby
- **Full in-game loop** — load synchronization, action batching with adjustable latency (5–500 ms), keepalive desync detection, lag screen (tolerance auto-scaled from latency)
- **Autohost** — continuous auto-hosting and auto-start when the lobby fills
- **GProxy++ reliable reconnects** — send buffering with ACK trimming, players kept alive through disconnects, buffered resend on reconnect, safe removal on timeout
- **Spoofcheck** — identity verification via the `sc` whisper (sent automatically by GProxy); in-game admin commands always require verification and check permissions against the verified realm
- **Database** — both SQLite (default, zero-config) and PostgreSQL built in, switched by a single `db_url`; admins / bans / game & player records
- **Replay saving** — every game is saved as a `.w3g` (zlib segmented packed container, playable directly in the W3 client)
- **i18n** — all user-visible messages go through a language catalog (Traditional Chinese and English built in, switched by one `bot_language` line)
- **Commands** — 31 battle.net whisper commands + 30+ in-game commands, see [COMMANDS.md](COMMANDS.md)

## Architecture

```
main ─┬─ BotCore     (event loop: command dispatch, permissions, autohost, db)
      ├─ BnetActor   (one per PVPGN connection: login state machine, anti-flood queue, refresh)
      ├─ GameActor   (one per game: lobby / downloads / in-game loop / GProxy buffers / replay)
      │    └─ PlayerConn (one per player: read/write task pair, framed codec)
      ├─ listener (host_port) and reconnect listener (GProxy)
      └─ console (stdin commands)
```

Actors communicate exclusively via `mpsc` messages (events up, commands down) with no shared mutable state. The protocol codecs (`src/core/`) are ported function-by-function against the C++ source, with the critical traps (string null terminators, integer widths, packet ordering) annotated with their corresponding C++ locations.

## Downloads

Prebuilt binaries for every tagged release are published on the [Releases](https://github.com/Fatorin/ghostpp-rs/releases) page:

| Platform | Artifact | Notes |
|---|---|---|
| Windows x64 | `ghostpp-rs-windows-x64.exe` | |
| macOS (Apple Silicon) | `ghostpp-rs-macos-arm64` | |
| Linux x64 | `ghostpp-rs-linux-x64` | glibc distros (Debian, Ubuntu, Fedora, …) |
| Linux ARM64 | `ghostpp-rs-linux-arm64-musl` | fully static — runs on any ARM64 distro, including Alpine |

## Building from source

All four platforms above are built and verified by CI.

Requirements: Rust (stable) and CMake ≥ 4.1 (stormlib-sys uses it to build the bundled StormLib C++ source), plus a platform C/C++ toolchain:

- **Windows** — MSVC build tools. If building with the VS2026 toolchain, also run `cargo update -p cmake` (older versions of the cmake crate aren't compatible with VS2026).
- **Linux** — C/C++ toolchain and the zlib / bzip2 dev packages (linked by StormLib).

  Debian/Ubuntu:
  ```
  sudo apt install build-essential cmake zlib1g-dev libbz2-dev
  ```
  Fedora/RHEL:
  ```
  sudo dnf install gcc-c++ cmake zlib-devel bzip2-devel
  ```
- **macOS** — Xcode Command Line Tools and CMake (`brew install cmake`); zlib and bzip2 ship with the SDK. Verified on Apple Silicon by CI.

```
cargo build --release
```

## Runtime setup

For copyright reasons the repository contains **no** Blizzard files. To run, supply locally:

1. **`lib/`** — War3 installation files (`war3.exe`/`warcraft.exe`, `Storm.dll`, `game.dll`) used by checkRevision to compute the exe hash
2. **`maps/`** — the map files to host (`.w3x`/`.w3m`)
3. **`config/`**
   - `ghost.toml` — main settings (ports, latency, replays, database, language file, …)
   - `bnet.toml` — PVPGN server, account credentials, CD keys, root admins (see `bnet.toml.example`; this file holds secrets and is gitignored)
   - `map.toml` — current map settings

```
cargo run --release
```

On startup the bot logs in to PVPGN and (if autohost is enabled) starts hosting automatically; or host manually by whispering `!pub <game name>`. See [COMMANDS.md](COMMANDS.md) for command usage.

## Key settings (config/ghost.toml)

| Key | Description |
|---|---|
| `db_url` | `sqlite://ghost.db` (default) or `postgres://user:pass@host/db` |
| `bot_language` | language file; `config/language.toml` (zh-TW) / `config/language_en.toml` |
| `bot_latency` | action interval in ms (5–500; adjustable at runtime with `!latency`) |
| `bot_reconnect` / `bot_reconnectport` | GProxy++ reconnect toggle and port |
| `bot_savereplays` / `bot_replaypath` | automatic replay saving |
| `autohost_gamename` / `autohost_maxgames` / `autohost_startplayers` | autohosting |

## Credits & license

This project is a rewrite of [GHost++](https://github.com/uakfdotb/ghostpp) (originally by Trevor Hogan); all protocol knowledge and behavioral semantics derive from the original project. Licensed under the [Apache License 2.0](LICENSE).
