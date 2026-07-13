//! i18n: user-visible message catalog (mirrors C++ language.cfg).
//!
//! Design:
//!   - Built-in English defaults live in the `DEFAULTS` constant table (compile-time); `defaults()` builds a HashMap from it.
//!   - At startup `load()` uses the config crate to read the TOML language file (`key = "template"`),
//!     merging with defaults: file values override defaults, missing keys use defaults; on read failure it logs a warn and uses English.
//!   - `t()` fetches the template and performs `{name}` placeholder substitution; falls back to the built-in defaults when not loaded / key missing.
//!
//! Template placeholder format: `{name}`, matching the call site `t("key", &[("name", value)])`.
//! Countdown "{n}. . .", packet protocol strings, log macros, and db record strings are not part of this catalog (see task exemptions).

use std::collections::HashMap;
use std::path::Path;
use std::sync::OnceLock;

use tracing::{info, warn};

static CATALOG: OnceLock<HashMap<String, String>> = OnceLock::new();
static DEFAULTS_CACHE: OnceLock<HashMap<String, String>> = OnceLock::new();

/// Built-in defaults (English). See the DEFAULTS constant table below for keys/templates.
/// The set of keys in the TOML language file must match this exactly.
const DEFAULTS: &[(&str, &str)] = &[
    // ---- src/game/actor.rs: in-game chat ----
    ("latency_current", "Current game latency is {latency} ms (lag tolerance {window} ms = {batches} batches behind)."),
    ("latency_set_clamped", "Latency is limited to {min}~{max}, set to {latency} ms (lag {batches} batches behind)."),
    ("latency_set", "Game latency set to {latency} ms (lag {batches} batches behind)."),
    ("synclimit_current", "Current lag tolerance is {batches} batches behind (= {window} ms)."),
    ("synclimit_set", "Lag tolerance set to {batches} batches behind (= {window} ms)."),
    ("spoofcheck_accepted", "[{name}] has passed the spoof check."),
    ("countdown_aborted", "Countdown aborted."),
    ("countdown_none", "There is no countdown in progress."),
    ("players_shuffled", "Players have been shuffled."),
    ("hold_usage", "Usage: !hold <name>"),
    ("hold_reserved", "Reserved a slot for: {names}"),
    ("player_muted", "[{name}] has been muted."),
    ("player_not_found", "Player not found."),
    ("player_unmuted", "[{name}] has been unmuted."),
    ("muteall_on", "Global chat has been muted (team/private still allowed)."),
    ("muteall_off", "Global chat has been unmuted."),
    ("player_check_info", "[{name}] ping={ping}, spoofed={spoofed}, realm={realm}"),
    ("player_not_found_named", "Player [{name}] not found."),
    ("version", "GHost++ Rust rewrite (ghostpp-rs)"),
    ("command_trigger", "The command trigger is {trigger}"),
    ("players_from", "Player sources: {list}"),
    ("ping_kicked", "{name}'s ping {ping}ms is too high, kicked."),
    ("ping_list", "Ping - {list}"),
    ("drop_laggers", "Dropped all lagging players."),
    ("drop_none", "There are no lagging players right now."),
    ("game_ended_admin", "The game has been ended by an admin."),
    ("autostart_off", "Autostart has been disabled."),
    ("autostart_set", "Will auto-start when {count} players have joined."),
    ("autostart_usage", "Usage: !autostart <count> or off"),
    ("announce_off", "Announcements have been disabled."),
    ("announce_set", "Announcement set (every {seconds} seconds)."),
    ("announce_usage", "Usage: !announce <seconds> <message>"),
    ("hcl_current", "The current HCL command string is [{hcl}]"),
    ("hcl_game_started", "The game has already started, HCL cannot be changed."),
    ("hcl_too_long", "Cannot set HCL: string too long (must not exceed the number of occupied slots)."),
    ("hcl_invalid_chars", "Cannot set HCL: contains invalid characters."),
    ("hcl_set", "HCL command string has been set to [{hcl}]"),
    ("hcl_cleared", "HCL command string has been cleared."),
    ("load_shortest", "Shortest load by player [{name}] was {seconds} seconds."),
    ("load_longest", "Longest load by player [{name}] was {seconds} seconds."),
    ("load_your_time", "Your loading time was {seconds} seconds."),
    ("desync_detected", "Warning! Desync detected!"),
    ("reconnect_timeout", "{name} timed out on reconnect and has left the game."),
    ("gproxy_disconnected_waiting", "{name} has disconnected, waiting for reconnect (up to {seconds}s)..."),
    ("gproxy_reconnected", "{name} has reconnected!"),
    ("autostart_full", "Player count reached ({count}), auto-starting!"),
    ("no_map_downloads_disabled", "A player has no map, but map downloads are disabled."),
    ("player_downloaded_map", "{name} has downloaded the map."),
    ("countdown_downloading", "A player is still downloading the map, cannot start."),
    ("countdown_cancelled_left", "A player left, the countdown has been cancelled."),

    // ---- src/bot/mod.rs: battle.net / PVPGN replies and broadcasts ----
    ("game_create_disabled", "Creating new games is currently disabled (!enable to re-enable)."),
    ("game_name_length", "Game name must be 1~31 characters."),
    ("game_map_invalid", "Cannot host: the current map is invalid (check config/map.toml and maps/)."),
    ("game_already_hosted", "There is already a game in the lobby, please !unhost first."),
    ("game_kind_public", "public"),
    ("game_kind_private", "private"),
    ("bnet_creating_game", "Creating {kind} game [{name}]"),
    ("only_root_addadmin", "Only a root admin can add admins."),
    ("addadmin_usage", "Usage: !addadmin <name>"),
    ("admin_added", "Admin [{name}] has been added."),
    ("admin_already", "[{name}] is already an admin."),
    ("admin_add_failed", "Failed to add: {error}"),
    ("only_root_deladmin", "Only a root admin can remove admins."),
    ("admin_removed", "Admin [{name}] has been removed."),
    ("admin_not_admin", "[{name}] is not an admin."),
    ("admin_remove_failed", "Failed to remove: {error}"),
    ("admin_is", "[{name}] is an admin."),
    ("query_failed", "Query failed: {error}"),
    ("ban_usage", "Usage: !ban <name> [reason]"),
    ("ban_added", "[{name}] has been banned."),
    ("ban_add_failed", "Ban failed: {error}"),
    ("ban_removed", "Ban on [{name}] has been removed."),
    ("ban_not_banned", "[{name}] is not on the ban list."),
    ("ban_remove_failed", "Failed to remove ban: {error}"),
    ("ban_info", "[{name}] is banned: by [{admin}] on {date}, reason: {reason}"),
    ("ban_reason_none", "(none)"),
    ("autohost_off", "Autohost has been disabled (existing games are unaffected)."),
    ("autohost_no_gamename", "No autohost_gamename in config, cannot enable."),
    ("autohost_on", "Autohost has been enabled."),
    ("autohost_status", "Autohost: {state} (name=[{name}], maxgames={maxgames}, startplayers={startplayers}); usage: !autohost on|off"),
    ("state_on", "enabled"),
    ("state_off", "disabled"),
    ("only_root_exit", "Only a root admin can shut down the bot."),
    ("bnet_shutting_down", "shutting down..."),
    ("games_disabled_msg", "Creating new games has been disabled."),
    ("games_enabled_msg", "Creating new games has been enabled."),
    ("downloads_disabled", "Map downloads: disabled"),
    ("downloads_enabled", "Map downloads: enabled"),
    ("downloads_conditional", "Map downloads: conditional"),
    ("downloads_usage", "Usage: !downloads <0|1|2>"),
    ("games_summary", "Games: lobby {lobby} / in progress {active} (max {max})"),
    ("games_lobby_entry", "[lobby] {name}"),
    ("games_active_entry", "[in progress] {name}"),
    ("getgame_info", "Lobby game: [{name}] host_counter={hc}"),
    ("getgame_none", "There is no lobby game right now."),
    ("saygames_done", "Broadcast to all games."),
    ("saygame_done", "Sent."),
    ("saygame_usage", "Usage: !saygame <host_counter> <message>"),
    ("admin_count", "[{server}] has {count} admin(s)."),
    ("ban_count", "[{server}] has {count} ban(s)."),
    ("db_status", "Database: {description}"),
    ("channel_usage", "Usage: !channel <channel name>"),
    ("channel_joining", "Joining channel [{channel}]"),
    ("map_current", "Current map: {path}"),
    ("map_invalid", "The current map is invalid (check config/map.toml)."),
    ("spoofcheck_required", "{name}: please whisper the bot 'sc' to verify your identity (/w botname sc) before using commands."),
];

/// Built-in defaults (English). See DEFAULTS for the keys.
fn defaults() -> HashMap<String, String> {
    DEFAULTS
        .iter()
        .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
        .collect()
}

/// Cached built-in defaults (fallback source for t() when not loaded / key missing, avoids rebuilding).
fn defaults_cached() -> &'static HashMap<String, String> {
    DEFAULTS_CACHE.get_or_init(defaults)
}

/// Load the language file at startup (TOML: `key = "template"`); use built-in English when the file is missing/broken.
/// Reads into a HashMap<String,String> via the config crate, then merges with defaults
/// (file values override defaults, missing keys use defaults). Should be called only once.
pub fn load(path: &str) {
    let mut catalog = defaults();
    match config::Config::builder()
        .add_source(config::File::from(Path::new(path)))
        .build()
        .and_then(|c| c.try_deserialize::<HashMap<String, String>>())
    {
        Ok(overrides) => {
            let n = overrides.len();
            for (k, v) in overrides {
                catalog.insert(k, v);
            }
            info!("[LANG] loaded {n} message(s) from '{path}'");
        }
        Err(e) => {
            warn!("[LANG] could not load language file '{path}': {e}; using built-in English defaults");
        }
    }
    if CATALOG.set(catalog).is_err() {
        warn!("[LANG] language catalog already initialised; ignoring second load()");
    }
}

/// Fetch a message and perform placeholder substitution: templates use the `{name}` form, args are (placeholder name, value).
/// Falls back to built-in defaults when not loaded; a key missing even from defaults is returned as-is.
pub fn t(key: &str, args: &[(&str, &str)]) -> String {
    let catalog = CATALOG.get().unwrap_or_else(|| defaults_cached());
    let template = catalog.get(key).map(String::as_str).unwrap_or(key);
    let mut out = template.to_string();
    for (name, value) in args {
        out = out.replace(&format!("{{{name}}}"), value);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_have_no_duplicate_keys() {
        let mut seen = std::collections::HashSet::new();
        for (k, _) in DEFAULTS {
            assert!(seen.insert(*k), "duplicate default key: {k}");
        }
    }

    #[test]
    fn placeholder_substitution() {
        // t() falls back to defaults when not loaded, substituting placeholders one by one
        let out = t("spoofcheck_accepted", &[("name", "Alice")]);
        assert_eq!(out, "[Alice] has passed the spoof check.");
    }

    #[test]
    fn unknown_key_returns_key() {
        assert_eq!(t("no_such_key_xyz", &[]), "no_such_key_xyz");
    }
}
