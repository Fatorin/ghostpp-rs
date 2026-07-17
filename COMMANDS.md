# ghostpp-rs Command Reference

English | [繁體中文](COMMANDS.zh-TW.md)

This document is derived from the source code (`src/bot/mod.rs`, `src/game/actor.rs`,
`src/bot/bnet.rs`, `src/bot/console.rs`) and lists the **actually implemented** commands.
Commands from the original GHost++ that are intentionally not implemented are listed in the
"Not implemented" section at the end.

## Trigger character and permissions

- **Trigger character**: default `!`.
  - On battle.net it is set per connection via `bnet_commandtrigger` (default `!`).
  - In game it is set via `bot_commandtrigger` (default `!`).
- **Permission levels**:
  - **root admin**: config `bnet_rootadmin` (whitespace-separated accounts, matched case-insensitively).
  - **admin**: a root admin, **or** a database admin (added with `!addadmin`, scoped to that server/realm).
  - **regular player**: neither of the above.
- **Syntax notation**: `<>` required, `[]` optional.

### In-game commands require spoofcheck first

In game (lobby or in progress), apart from a few self-query commands, every command requires the sender to:

1. **Pass spoofcheck**: **whisper** `sc` to the bot account (see "Special mechanics"). The whisper is
   authenticated by the battle.net/PVPGN server, so the name cannot be spoofed; GProxy++ clients send it
   automatically when joining a game.
2. Once spoofchecked, the sender's admin status is checked against the **verified realm**
   (root admin, or a db admin on that realm).

Sending a command without spoofcheck → the bot publicly asks the player to spoofcheck; spoofchecked but
not an admin → silently ignored. The only exceptions are the self-query commands `!checkme` / `!version` /
`!stats` / `!statsdota` (no spoofcheck, no admin required).

---

## 1. battle.net whisper / channel commands

Source: `handle_bnet_command` (`src/bot/mod.rs`).

> These commands **all require admin** (the code returns early via `if !is_admin { return; }` before dispatch).
> Those marked "root" additionally require root admin. Replies go by whisper or channel to match the
> original message. The lobby-control ones (open/close/swap/kick/start/latency/synclimit/unhost) act on the
> **current lobby** game; in-progress games can only be controlled via `!saygame` / `!saygames` or in-game chat.

| Syntax | Permission | Description |
|--------|------------|-------------|
| `!addadmin <name>` | root | Add `<name>` as a db admin for this server. Reports already-exists / failure. |
| `!deladmin <name>` | root | Remove a db admin from this server. |
| `!checkadmin <name>` | admin | Check whether `<name>` is an admin on this server. |
| `!addban <name> [reason]`, `!ban <name> [reason]` | admin | Add a ban by name (records the issuing admin, date, reason; IP left blank). |
| `!delban <name>`, `!unban <name>` | admin | Remove the ban for that name. |
| `!checkban <name>` | admin | Query ban info (admin, date, reason). |
| `!autohost [on\|off]` | admin | `on` enables autohost (requires `auto_host_game_name` set) and tries to host immediately; `off` disables; no argument shows status (state, game name, max games, auto-start players). |
| `!say <text>` | admin | Broadcast text to **all bnet channels** (not into the game). |
| `!pub <name>` | admin | Create a **public** game (name length must be 1–31). |
| `!priv <name>` | admin | Create a **private** game (name length must be 1–31). |
| `!unhost` | admin | Unhost the current lobby game. |
| `!open <slot>` | admin | Open the given lobby slot (**1-based**, converted to 0-based internally). |
| `!close <slot>` | admin | Close the given lobby slot (1-based). |
| `!swap <s1> <s2>` | admin | Swap two slots (1-based; needs exactly two numbers). |
| `!kick <name\|slot>` | admin | Kick: a pure number is a slot index (1-based), otherwise partial name match (case-insensitive). |
| `!start` | admin | Start the lobby countdown. |
| `!latency [n]` | admin | No argument queries; sets action interval in ms, **clamped 5~500** (see Special mechanics). |
| `!synclimit [n]` | admin | No argument queries; `n` is the **lag batch count** (converted to a time window, see Special mechanics). |
| `!exit`, `!quit` | **root** | Sends a shutdown reply, then triggers a full program shutdown. Rejected for non-root. |
| `!disable` | admin | Disable game creation (including autohost). |
| `!enable` | admin | Re-enable game creation and try autohost. |
| `!downloads <0\|1\|2>` | admin | Set map download mode: `0` disabled (players without the map are kicked) / `1` enabled / `2` conditional. Other values show usage. |
| `!getgames` | admin | Summary: lobby (0/1), in-progress count, max games, plus each game's name. |
| `!getgame` | admin | Show the current lobby game's name and host_counter; reports none if empty. |
| `!saygames <text>` | admin | Broadcast text to the lobby and **all in-progress** games. |
| `!saygame <host_counter> <text>` | admin | Broadcast text to the game with the given host_counter. |
| `!countadmins` | admin | Count admins for this server. |
| `!countbans` | admin | Count bans for this server. |
| `!dbstatus` | admin | Show the database backend description. |
| `!channel <name>` | admin | Make the bot join the given channel. |
| `!map [pattern]`, `!load [pattern]` | admin | Without a pattern: report the current map. With a pattern: case-insensitive partial match against .w3x/.w3m files in maps/ — a unique match is loaded and becomes the hosting map (applies to newly hosted games; existing games are unaffected); multiple matches list the first 5. |

---

## 2. In-game lobby / in-progress commands

Source: `dispatch_lobby_command` (BotCore side, `src/bot/mod.rs`) + `handle_admin_command`
(GameActor side, `src/game/actor.rs`). See "In-game commands require spoofcheck first" above.

### Available to regular players (no spoofcheck, no admin)

| Syntax | Permission | Description |
|--------|------------|-------------|
| `!checkme` | regular player | Whisper-reply with your own info: ping, whether spoofed, realm. |
| `!version` | regular player | Whisper-reply with the version string. |
| `!stats` | regular player | **Whitelisted but not implemented** (W3MMD stats unfinished); currently no response. |
| `!statsdota` | regular player | Same as above, no response. |

### Require admin + spoofcheck

Handled directly by BotCore (lobby control):

| Syntax | Permission | Description |
|--------|------------|-------------|
| `!say <text>` | admin | Broadcast to everyone as the host (lobby flag 16 / in-game flag 32). |
| `!open <slot>` | admin | Open a slot (1-based). No effect after game start. |
| `!close <slot>` | admin | Close a slot (1-based). If a human occupies it, they are kicked first. |
| `!swap <s1> <s2>` | admin | Swap two slots (1-based). Behavior depends on map options (fixed settings / custom forces). |
| `!kick <name\|slot>` | admin | Kick: number = slot (1-based), otherwise partial name match. |
| `!start` | admin | Start the countdown; rejected if anyone is still downloading the map. |
| `!latency [n]` | admin | Query / set action interval in ms (clamped 5~500). |
| `!synclimit [n]` | admin | Query / set lag tolerance (entered as batch count, stored internally as a time window). |
| `!unhost` | admin | Unhost the lobby game. **Note**: the implementation acts on the "current lobby", not the sender's own game (see the inconsistency list). |

Handled by GameActor (`handle_admin_command`):

| Syntax | Permission | Description |
|--------|------------|-------------|
| `!abort`, `!a` | admin | Cancel the start countdown; whispers a note if none is running. |
| `!openall` | admin | Open all currently closed slots. |
| `!closeall` | admin | Close all currently open slots. |
| `!sp` | admin | Shuffle players (randomly reassign occupied human players across occupied slots). |
| `!hold <name> [name...]` | admin | Reserve names: add names (lowercased) to the hold list; consumed once on join. |
| `!mute <name>` | admin | Mute a player (their messages are not relayed). Partial name match. |
| `!unmute <name>` | admin | Unmute. |
| `!muteall` | admin | Mute all: in game only blocks "all"-scope public messages (flag 32, mode 0); team / private still pass. |
| `!unmuteall` | admin | Undo mute-all. |
| `!check [name]` | admin | Whisper-reply with player info (ping, spoofed, realm); omitting the name queries yourself. |
| `!trigger` | admin | Whisper-reply with the current trigger character (always replies `!`). |
| `!from` | admin | List each player's IP (country lookup needs GeoIP, not implemented, so IP only). |
| `!ping [n]` | admin | No argument: whisper-list everyone's ping; with a number `n`: kick players whose average ping > `n` ms. |
| `!drop` | admin | While in game and someone is lagging, drop all lagging players; otherwise whispers that no one is lagging. |
| `!end` | admin | Force-end the current game. |
| `!autostart [n\|off]` | admin | Auto-start at `n` players (countdown only once everyone has confirmed the map); `off` or no argument disables. |
| `!announce [secs msg \| off]` | admin | Broadcast `msg` in the lobby every `secs`; `off` or no argument disables. |
| `!hcl [str]` | admin | No argument shows the current HCL string; setting checks game-started / length / allowed chars (see Special mechanics). |
| `!clearhcl` | admin | Clear the HCL string (rejected if the game has started). |

> Any other string is silently ignored by GameActor (debug log only).

---

## 3. console (stdin) commands

Source: `console.rs` reads each line → `BotEvent::ConsoleInput` → `handle_event` (`src/bot/mod.rs`).
This is the local operator console, with **no permission checks**.

| Syntax | Description |
|--------|-------------|
| `exit`, `quit` | Shut down the whole program. |
| `unhost` | Unhost the current lobby game. |
| `start` | Start the current lobby game's countdown. |
| `say <text>` | Broadcast text to all bnet channels. |
| `pub <name>` | Create a public game. |
| `priv <name>` | Create a private game. |

> Note: console commands do **not** use the `!` trigger; type the keyword directly. Unknown input just logs a warning.

---

## Special mechanics

### spoofcheck flow and the `sc` whisper

- A player **whispers** the bot account on battle.net; if the message (trimmed and lowercased) is exactly
  `s`, `sc`, or `spoofcheck`, it triggers spoofcheck (`handle_chat_event` in `bnet.rs`).
- The whisper is server-authenticated, so `user` is the real account and cannot be forged. BnetActor emits
  `BnetEvent::SpoofCheck`.
- BotCore then sends `GameCommand::SpoofCheck { name, realm }` to the same-named player in the **current lobby**;
  GameActor marks that player `spoofed=true`, records `spoofed_realm`, and publicly announces "spoofcheck accepted".
- That player's subsequent in-game commands are then authorized against `spoofed_realm`.
- **GProxy++** clients send this whisper automatically on join, so regular players usually need not do it manually.

### autohost behavior

- Enabled (initial value) when: `auto_host_game_name` is non-empty, `auto_host_maximum_games > 0`, and
  `auto_host_auto_start_players > 0`. Toggle at runtime with `!autohost on/off`.
- `try_autohost` fires on: bnet login complete, game start (lobby freed), and game deletion.
- It is blocked when: `!disable` is active, autohost is off, a lobby game already exists, in-progress count
  reaches `auto_host_maximum_games`, the map is invalid, or there are no bnet connections.
- Hosted names are `"<name> #N"` (N is an incrementing counter). When the game reaches
  `auto_host_auto_start_players` and everyone has confirmed the map, the countdown starts automatically
  (`maybe_autostart`).

### `!hcl` character / length limits

- Allowed character set: `abcdefghijklmnopqrstuvwxyz0123456789 -=,.` (constant `HCL_ALLOWED_CHARS`).
- Length must not exceed the **current number of occupied slots** (`occupied_slot_count`), else "too long".
- Invalid characters → "invalid chars". If the game has started (`started`) → modification rejected.
- On game start the HCL string is encoded into each occupied slot's handicap field for the map to decode
  and pick a mode; the initial value comes from the map default HCL (`map_defaulthcl`).

### `!latency` / `!synclimit` (lag tolerance model)

- `latency_ms` initial value comes from config `bot_latency` (default 100), **clamped 5~500**
  (`LATENCY_MIN`/`LATENCY_MAX`).
- Lag tolerance uses a "time window" `sync_window_ms` (initial `SYNC_TOLERANCE_MS = 5000` ms). The actual
  lag-screen trigger batch count = `sync_window_ms / latency_ms` (at least 1), auto-derived from latency.
- `!synclimit <n>`: converts the user's **batch count** back into a time window `n × latency`, clamped to
  `SYNC_WINDOW_MIN_MS (500)` ~ `SYNC_WINDOW_MAX_MS (30000)` ms.

### `!ping` RTT source and `lc_pings`

- RTT is computed from `W3GS_PONG_TO_HOST`: the host stores `get_ticks` when sending PING, and on pong
  `RTT = now ticks − pong`. The first pong (usually 1) is dropped; RTT ≥ 60000 ms is treated as abnormal and
  filtered; the last 10 samples per player are kept and averaged for display.
- When `lc_pings` (config `bot_lcpings`) is true, the displayed value is **halved** (one-way estimate).

---

## Not implemented (compared with original GHost++, intentionally omitted)

| Command / feature | Reason not implemented |
|-------------------|------------------------|
| `!savegame` family (load/host saved game) | Save/load resume flow not ported. |
| `!hostsg` / admin game (admin-only game) | Dedicated admin-game interface not ported. |
| matchmaking (beyond `!pub`/`!priv`) | ELO/matchmaking system not ported. |
| warden (anti-cheat module) | Warden challenge/response not ported. |
| `!votekick` | Vote-kick not ported. |
| comp family (`!comp`/`!compcolour`/`!comprace`/`!comphandicap`/`!compteam`) | Adding computer players not ported. |
| `!owner` / `!lock` / `!unlock` | Game-owner lock mechanism not ported (authorization is now spoofcheck + admin). |
| `!stats` / `!statsdota` | W3MMD stats unfinished; the commands are whitelisted but currently return nothing. |
| `!from` country lookup | GeoIP not integrated; `!from` shows IP only. |
| `!reload` | Config file reload not implemented (use `!map <pattern>` to switch maps). |
| `!sendlan` | LAN (UDP) broadcast not ported. |
