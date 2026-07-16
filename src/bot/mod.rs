//! BotCore: global coordinator (mirrors C++ CGHost).
//!
//! - A single `mpsc<BotEvent>` event loop replaces the 50ms select polling
//! - Routes player connections coming in from the listener to the current lobby's GameActor
//! - Holds the BnetActor handles (HashMap<usize, mpsc::Sender<BnetCommand>>)
//! - Command dispatch / permissions / autohost / database live here

pub mod config;
pub mod bnet;
pub mod console;
pub mod listener;
pub mod messages;

use std::collections::HashMap;
use std::sync::Arc;

use tokio::sync::mpsc;
use tracing::{debug, info, warn};

use crate::core::gameprotocol::{GameProtocol, GAME_PRIVATE, GAME_PUBLIC, W3GS_REQJOIN};
use crate::core::GameMap;
use crate::game::{self, GameConfig, GameHandle};
use crate::net::codec::{FrameCodec, W3GS_HEADER_CONSTANT};
use crate::net::conn::{self, ConnEvent, ConnHandle, ConnId, DEFAULT_RECV_TIMEOUT};

// Note: self:: is required, otherwise there is path ambiguity with the external `config` crate (E0659)
pub use self::bnet::BnetConfig;
pub use self::config::BotConfig;
use self::messages::{BnetCommand, BnetEvent, BotEvent, GameCommand, GameEvent};

/// BotCore event queue depth
const EVENT_QUEUE_DEPTH: usize = 1024;

/// The db server primary key: takes the host part of the bnet server (mirrors C++ m_Server)
fn server_key(cfg: &BnetConfig) -> String {
    cfg.server_addr
        .split(':')
        .next()
        .unwrap_or("")
        .to_string()
}

/// Handle for one bnet connection: the sender for issuing commands + its config (root admin checks, etc.)
struct BnetHandle {
    tx: mpsc::Sender<BnetCommand>,
    cfg: BnetConfig,
}

pub struct BotCore {
    cfg: BotConfig,
    /// The currently loaded map (used for hosting)
    map: Arc<GameMap>,
    event_rx: mpsc::Receiver<BotEvent>,
    /// The event entry point for listener / console / actor
    event_tx: mpsc::Sender<BotEvent>,

    /// Player connections held temporarily when there is no game yet (fallback, log only)
    conns: HashMap<ConnId, ConnHandle>,
    next_conn_id: ConnId,

    /// battle.net / PVPGN connections (indexed by server_id)
    bnets: HashMap<usize, BnetHandle>,

    /// The game currently in the lobby (single lobby)
    current_game: Option<GameHandle>,
    /// Games in progress (moved in from current_game once started; removed when they end)
    games: HashMap<u32, GameHandle>,
    /// Monotonically increasing unique host counter
    host_counter: u32,
    /// Database (admins / bans / games)
    db: Arc<dyn crate::db::GhostDb>,
    /// Autohost switch (toggleable at runtime via !autohost off/on; initial value depends on config validity)
    autohost_enabled: bool,
    /// Number of games autohost has created (named "name #N")
    autohost_counter: u32,
    /// GProxy reconnect key → host_counter (reconnect routing; cleaned up when a game is deleted)
    gproxy_keys: HashMap<u32, u32>,
    /// Group B: !disable turns off hosting (including autohost)
    games_disabled: bool,
    /// Group B: map download mode (0=disabled / 1=enabled / 2=conditional; mirrors bot_downloads)
    download_mode: u8,
}

impl BotCore {
    pub fn new(
        cfg: BotConfig,
        map: Arc<GameMap>,
        db: Arc<dyn crate::db::GhostDb>,
    ) -> (Self, mpsc::Sender<BotEvent>) {
        let (event_tx, event_rx) = mpsc::channel(EVENT_QUEUE_DEPTH);

        let autohost_enabled = !cfg.auto_host_game_name.is_empty()
            && cfg.auto_host_maximum_games > 0
            && cfg.auto_host_auto_start_players > 0;

        let core = Self {
            cfg,
            map,
            event_rx,
            event_tx: event_tx.clone(),
            conns: HashMap::new(),
            next_conn_id: 1,
            bnets: HashMap::new(),
            current_game: None,
            games: HashMap::new(),
            host_counter: 0,
            db,
            autohost_enabled,
            autohost_counter: 0,
            gproxy_keys: HashMap::new(),
            games_disabled: false,
            download_mode: 1,
        };

        (core, event_tx)
    }

    /// Automatically host the next game when conditions are met (mirrors the autohost section of C++ CGHost::Update).
    /// Trigger points: login completed, game started (lobby freed up), game deleted.
    async fn try_autohost(&mut self) {
        if self.games_disabled
            || !self.autohost_enabled
            || self.current_game.is_some()
            || self.games.len() >= self.cfg.auto_host_maximum_games as usize
            || !self.map.is_valid
            || self.bnets.is_empty()
        {
            return;
        }
        self.autohost_counter += 1;
        let name = format!("{} #{}", self.cfg.auto_host_game_name, self.autohost_counter);
        info!("[GHOST] autohost: hosting [{name}] (game #{})", self.autohost_counter);
        self.create_game(
            GAME_PUBLIC,
            name,
            None,
            self.cfg.auto_host_auto_start_players,
        )
        .await;
    }

    /// Create a game (mirrors C++ CGHost::CreateGame).
    /// creator: (server_id, creator name, whether whisper) — used for reply messages; None means console-created.
    async fn create_game(
        &mut self,
        game_state: u8,
        game_name: String,
        creator: Option<(usize, String, bool)>,
        autostart_players: u8,
    ) {
        if self.games_disabled {
            self.reply_creator(&creator, &crate::lang::t("game_create_disabled", &[])).await;
            return;
        }
        if game_name.is_empty() || game_name.len() > 31 {
            self.reply_creator(&creator, &crate::lang::t("game_name_length", &[])).await;
            return;
        }
        if !self.map.is_valid {
            self.reply_creator(&creator, &crate::lang::t("game_map_invalid", &[])).await;
            return;
        }
        if self.current_game.is_some() {
            self.reply_creator(&creator, &crate::lang::t("game_already_hosted", &[])).await;
            return;
        }

        self.host_counter += 1;
        let hc = self.host_counter;

        let gcfg = GameConfig {
            host_counter: hc,
            game_name: game_name.clone(),
            game_state,
            map: Arc::clone(&self.map),
            virtual_host_name: self.cfg.virtual_host_name.clone(),
            hide_ip: self.cfg.hide_ip_addresses,
            lc_pings: self.cfg.lc_pings,
            latency_ms: self.cfg.latency.max(1),
            reconnect_enabled: self.cfg.reconnect,
            reconnect_port: self.cfg.reconnect_port,
            // Mirrors C++: empty_actions = reconnect_wait_time - 1, capped at 9 (wait = (N+1)×60 seconds)
            gproxy_empty_actions: if self.cfg.reconnect {
                self.cfg.reconnect_wait_time.saturating_sub(1).min(9) as u8
            } else {
                0
            },
            save_replays: self.cfg.save_replays,
            replay_path: self.cfg.replay_path.clone(),
            replay_war3_version: self.cfg.replay_war3_version,
            replay_build_number: self.cfg.replay_build_number,
            download_mode: self.download_mode,
            db: Arc::clone(&self.db),
            servers: self
                .bnets
                .values()
                .map(|h| server_key(&h.cfg))
                .collect(),
            // Game record owner = bnet account name (bnet_username)
            owner_name: self
                .bnets
                .values()
                .next()
                .map(|h| h.cfg.user_name.clone())
                .unwrap_or_default(),
            autostart_players,
        };
        self.current_game = Some(game::spawn(gcfg, self.event_tx.clone()));
        info!("[GHOST] creating game [{game_name}] (host_counter={hc})");

        // Notify all bnets to broadcast this game (STARTADVEX3)
        let private = game_state == GAME_PRIVATE;
        let kind = if private {
            crate::lang::t("game_kind_private", &[])
        } else {
            crate::lang::t("game_kind_public", &[])
        };
        for h in self.bnets.values() {
            let _ = h
                .tx
                .send(BnetCommand::QueueChat(crate::lang::t(
                    "bnet_creating_game",
                    &[("kind", &kind), ("name", &game_name)],
                )))
                .await;
            let _ = h
                .tx
                .send(BnetCommand::CreateGame {
                    game_state,
                    game_name: game_name.clone(),
                    map: Arc::clone(&self.map),
                    host_counter: hc,
                })
                .await;
        }
    }

    /// Unhost the game currently in the lobby
    async fn unhost_game(&mut self) {
        if let Some(g) = self.current_game.take() {
            let _ = g.tx.send(GameCommand::Close).await;
            for h in self.bnets.values() {
                let _ = h.tx.send(BnetCommand::UncreateGame).await;
            }
            info!("[GHOST] unhosted game [{}]", g.game_name);
        }
    }

    /// Send a command to the game currently in the lobby (ignored if there is no game)
    async fn send_game(&self, cmd: GameCommand) {
        if let Some(g) = &self.current_game {
            let _ = g.tx.send(cmd).await;
        }
    }

    /// Search bot_mappath for map files (.w3x/.w3m) whose name contains the pattern
    /// (case-insensitive; mirrors the C++ !map partial match). Results are sorted.
    fn find_map_files(&self, pattern: &str) -> Vec<String> {
        let pattern = pattern.to_lowercase();
        let dir = if self.cfg.map_path.is_empty() {
            "maps".to_string()
        } else {
            self.cfg.map_path.clone()
        };
        let mut found: Vec<String> = Vec::new();
        if let Ok(entries) = std::fs::read_dir(&dir) {
            for entry in entries.flatten() {
                let name = entry.file_name().to_string_lossy().to_string();
                let lower = name.to_lowercase();
                if (lower.ends_with(".w3x") || lower.ends_with(".w3m"))
                    && lower.contains(&pattern)
                {
                    found.push(name);
                }
            }
        }
        found.sort();
        found
    }

    /// Load a map file at runtime and swap it in as the current hosting map.
    /// Heavy work (MPQ parse + SHA1 over the whole file) runs on a blocking thread.
    /// Existing games keep their own Arc of the old map; only newly hosted games use the new one.
    async fn load_map_file(&mut self, file: &str) -> String {
        let map_dir = if self.cfg.map_path.is_empty() {
            "maps".to_string()
        } else {
            self.cfg.map_path.clone()
        };
        // Build a minimal map config: the W3 internal path uses the Maps\Download\ convention
        // (mirrors what C++ !map generates for downloaded maps)
        let built = ::config::Config::builder()
            .set_override("bot_mappath", map_dir)
            .and_then(|b| b.set_override("map_localpath", file.to_string()))
            .and_then(|b| b.set_override("map_path", format!("Maps\\Download\\{file}")))
            .map(|b| b.build());
        let cfg = match built {
            Ok(Ok(c)) => c,
            Ok(Err(e)) | Err(e) => {
                warn!("[GHOST] failed to build map config for [{file}]: {e}");
                return crate::lang::t("map_load_failed", &[("file", file)]);
            }
        };

        let loaded = tokio::task::spawn_blocking(move || {
            let mut m = GameMap::new();
            m.load(&cfg);
            m
        })
        .await;

        match loaded {
            Ok(m) if m.is_valid => {
                info!("[GHOST] map switched to [{}]", m.get_map_path());
                self.map = Arc::new(m);
                crate::lang::t("map_loaded", &[("file", file)])
            }
            Ok(_) => {
                warn!("[GHOST] map [{file}] loaded but is not valid");
                crate::lang::t("map_load_failed", &[("file", file)])
            }
            Err(e) => {
                warn!("[GHOST] map load task failed for [{file}]: {e}");
                crate::lang::t("map_load_failed", &[("file", file)])
            }
        }
    }

    /// Find a game by host_counter (lobby or in progress) and send it a command — in-game commands must be routed back
    /// to that specific game, not to current_game (after a game starts, the lobby may already be autohost's new game)
    async fn send_game_to(&self, host_counter: u32, cmd: GameCommand) {
        let tx = if self.current_game.as_ref().map(|g| g.host_counter) == Some(host_counter) {
            self.current_game.as_ref().map(|g| g.tx.clone())
        } else {
            self.games.get(&host_counter).map(|g| g.tx.clone())
        };
        if let Some(tx) = tx {
            let _ = tx.send(cmd).await;
        }
    }

    /// Command typed by a player in the lobby (root admin already verified). Replies go through in-game chat.
    /// Command typed by an in-game player (spoofcheck + admin already verified).
    /// Routed by host_counter back to "the game that issued the command" (lobby or in progress).
    async fn dispatch_lobby_command(
        &mut self,
        host_counter: u32,
        requester: &str,
        command: &str,
        payload: String,
    ) {
        let hc = host_counter;
        match command {
            "say" => {
                if !payload.trim().is_empty() {
                    self.send_game_to(hc, GameCommand::Say(payload)).await;
                }
            }
            "open" => {
                if let Ok(n) = payload.trim().parse::<usize>() {
                    if n >= 1 {
                        self.send_game_to(hc, GameCommand::OpenSlot(n - 1)).await;
                    }
                }
            }
            "close" => {
                if let Ok(n) = payload.trim().parse::<usize>() {
                    if n >= 1 {
                        self.send_game_to(hc, GameCommand::CloseSlot(n - 1)).await;
                    }
                }
            }
            "swap" => {
                let nums: Vec<usize> = payload
                    .split_whitespace()
                    .filter_map(|s| s.parse().ok())
                    .collect();
                if nums.len() == 2 && nums[0] >= 1 && nums[1] >= 1 {
                    self.send_game_to(hc, GameCommand::SwapSlots(nums[0] - 1, nums[1] - 1)).await;
                }
            }
            "kick" => {
                if !payload.trim().is_empty() {
                    self.send_game_to(hc, GameCommand::Kick(payload.trim().to_string())).await;
                }
            }
            "start" => self.send_game_to(hc, GameCommand::Start).await,
            "latency" => {
                let value = payload.trim().parse::<u32>().ok();
                self.send_game_to(hc, GameCommand::SetLatency(value)).await;
            }
            "synclimit" => {
                let value = payload.trim().parse::<u32>().ok();
                self.send_game_to(hc, GameCommand::SetSyncLimit(value)).await;
            }
            "unhost" => {
                // Only unhost when the game the issuer is in is the current lobby,
                // otherwise (after a game started, autohost has opened a new game) it would wrongly tear down someone else's new lobby
                if self.current_game.as_ref().map(|g| g.host_counter) == Some(hc) {
                    self.unhost_game().await;
                }
            }
            // The remaining in-game admin commands (Group A: abort/openall/closeall/sp/hold/mute/…)
            // are forwarded to GameActor to handle itself (the state all lives there)
            other => {
                self.send_game_to(
                    hc,
                    GameCommand::AdminCommand {
                        requester: requester.to_string(),
                        command: other.to_string(),
                        payload,
                    },
                )
                .await;
            }
        }
    }

    async fn reply_creator(&self, creator: &Option<(usize, String, bool)>, msg: &str) {
        match creator {
            Some((server_id, user, whisper)) => {
                if let Some(h) = self.bnets.get(server_id) {
                    let _ = h
                        .tx
                        .send(if *whisper {
                            BnetCommand::QueueWhisper { to: user.clone(), message: msg.to_string() }
                        } else {
                            BnetCommand::QueueChat(msg.to_string())
                        })
                        .await;
                }
            }
            None => info!("[GHOST] {msg}"),
        }
    }

    pub fn config(&self) -> &BotConfig {
        &self.cfg
    }

    /// Get the event entry point (for main.rs to spawn bnet/listener)
    pub fn event_sender(&self) -> mpsc::Sender<BotEvent> {
        self.event_tx.clone()
    }

    /// Register an already-spawned bnet connection
    pub fn add_bnet(&mut self, server_id: usize, tx: mpsc::Sender<BnetCommand>, cfg: BnetConfig) {
        self.bnets.insert(server_id, BnetHandle { tx, cfg });
    }

    /// Broadcast a chat message to all bnets
    async fn broadcast_chat(&self, message: &str) {
        for h in self.bnets.values() {
            let _ = h.tx.send(BnetCommand::QueueChat(message.to_string())).await;
        }
    }

    /// Main event loop. All mutable state lives in this task, so no locks are needed.
    pub async fn run(mut self) {
        info!("[GHOST] BotCore started (command trigger = {})", self.cfg.command_trigger);

        while let Some(event) = self.event_rx.recv().await {
            if !self.handle_event(event).await {
                break;
            }
        }

        info!("[GHOST] BotCore shutting down");
    }

    /// Returning false means the main loop should terminate
    async fn handle_event(&mut self, event: BotEvent) -> bool {
        match event {
            BotEvent::GProxyReconnect { stream, pid, key, last_packet } => {
                // Look up which game the key belongs to (lobby or in progress)
                let target = self.gproxy_keys.get(&key).copied().and_then(|hc| {
                    if self.current_game.as_ref().map(|g| g.host_counter) == Some(hc) {
                        self.current_game.as_ref().map(|g| g.tx.clone())
                    } else {
                        self.games.get(&hc).map(|g| g.tx.clone())
                    }
                });
                match target {
                    Some(tx) => {
                        let _ = tx
                            .send(GameCommand::GProxyReconnect { stream, pid, key, last_packet })
                            .await;
                    }
                    None => {
                        warn!("[GHOST] GProxy reconnect key={key:08X} has no matching game, rejecting");
                        tokio::spawn(async move {
                            use tokio::io::AsyncWriteExt;
                            let mut s = stream;
                            let _ = s
                                .write_all(&crate::core::gpsprotocol::send_gpss_reject(
                                    crate::core::gpsprotocol::REJECTGPS_NOTFOUND as u32,
                                ))
                                .await;
                        });
                    }
                }
            }

            BotEvent::NewConnection { stream, peer } => {
                // If there is a lobby game, route the connection to it; otherwise take the fallback (log only)
                if let Some(g) = &self.current_game {
                    info!(%peer, "[GHOST] player connected, routing to lobby [{}]", g.game_name);
                    if g.tx.send(GameCommand::NewConnection { stream, peer }).await.is_err() {
                        // GameActor has ended but not yet been cleaned up; drop the connection
                        debug!(%peer, "[GHOST] game actor gone, dropping connection");
                    }
                } else {
                    // No game: accept it briefly to see if it is a REQJOIN, purely for logging (fallback)
                    let id = self.next_conn_id;
                    self.next_conn_id += 1;
                    debug!(conn = id, %peer, "[GHOST] connection but no game hosted");
                    let conn_event_tx = self.event_tx.clone();
                    let (tx, mut rx) = mpsc::channel::<ConnEvent>(64);
                    tokio::spawn(async move {
                        while let Some(ev) = rx.recv().await {
                            if conn_event_tx.send(BotEvent::Conn(ev)).await.is_err() {
                                break;
                            }
                        }
                    });
                    let handle = conn::spawn(id, stream, FrameCodec::w3gs(), tx, DEFAULT_RECV_TIMEOUT);
                    self.conns.insert(id, handle);
                }
            }

            BotEvent::Conn(ConnEvent::Frame(id, frame)) => {
                // When a real War3 client connects, the first packet is W3GS_REQJOIN;
                // being able to see the player's name in the log means listener → codec → channel is fully working end to end
                if frame.magic == W3GS_HEADER_CONSTANT && frame.id == W3GS_REQJOIN {
                    if let Some(join) = GameProtocol::receive_w3gs_reqjoin(&frame.data) {
                        info!(
                            conn = id,
                            "[GHOST] W3GS_REQJOIN from player [{}] (host_counter={}) - no lobby to route to",
                            join.name,
                            join.host_counter
                        );
                        return true;
                    }
                }

                debug!(
                    conn = id,
                    magic = frame.magic,
                    packet_id = frame.id,
                    len = frame.data.len(),
                    "[GHOST] frame received"
                );
            }

            BotEvent::Conn(ConnEvent::Closed(id, reason)) => {
                info!(conn = id, "[GHOST] connection closed: {reason:?}");
                self.conns.remove(&id);
            }

            BotEvent::Bnet { server_id, event } => {
                self.handle_bnet_event(server_id, event).await;
            }

            BotEvent::Game { host_counter, event } => {
                self.handle_game_event(host_counter, event).await;
            }

            BotEvent::ConsoleInput(line) => {
                match line.as_str() {
                    "exit" | "quit" => {
                        info!("[GHOST] shutting down (console)");
                        return false;
                    }
                    "unhost" => self.unhost_game().await,
                    "start" => self.send_game(GameCommand::Start).await,
                    _ => {
                        // console test commands: say / pub / priv <name>
                        if let Some(msg) = line.strip_prefix("say ") {
                            self.broadcast_chat(msg).await;
                        } else if let Some(name) = line.strip_prefix("pub ") {
                            self.create_game(GAME_PUBLIC, name.to_string(), None, 0).await;
                        } else if let Some(name) = line.strip_prefix("priv ") {
                            self.create_game(GAME_PRIVATE, name.to_string(), None, 0).await;
                        } else {
                            warn!("[GHOST] unknown console command: {line}");
                        }
                    }
                }
            }
        }

        true
    }

    async fn handle_bnet_event(&mut self, server_id: usize, event: BnetEvent) {
        match event {
            BnetEvent::LoggedIn => {
                info!(server_id, "[BNET] logged in");
                self.try_autohost().await;
            }
            BnetEvent::Connected => debug!(server_id, "[BNET] connected"),
            BnetEvent::Disconnected => debug!(server_id, "[BNET] disconnected"),
            BnetEvent::JoinedChannel(ch) => info!(server_id, "[BNET] joined channel [{ch}]"),
            BnetEvent::GameRefreshed => debug!(server_id, "[BNET] server accepted game refresh"),
            BnetEvent::GameRefreshFailed => warn!(server_id, "[BNET] server rejected game hosting (STARTADVEX3 failed)"),
            BnetEvent::ChatEvent { .. } => { /* general chat: can be forwarded into the game lobby */ }
            BnetEvent::Command { user, command, payload, whisper } => {
                self.handle_bnet_command(server_id, user, command, payload, whisper)
                    .await;
            }
            BnetEvent::SpoofCheck { user } => {
                // Whisper sc is server-authenticated → mark the same-named player in the current lobby as passed
                let realm = self
                    .bnets
                    .get(&server_id)
                    .map(|h| server_key(&h.cfg))
                    .unwrap_or_default();
                info!(server_id, "[BNET] spoofcheck from [{user}] ({realm})");
                if let Some(g) = &self.current_game {
                    let _ = g
                        .tx
                        .send(GameCommand::SpoofCheck { name: user, realm })
                        .await;
                }
            }
        }
    }

    /// In-game / channel command dispatch
    async fn handle_bnet_command(
        &mut self,
        server_id: usize,
        user: String,
        command: String,
        payload: String,
        whisper: bool,
    ) {
        let is_root = self
            .bnets
            .get(&server_id)
            .map(|h| h.cfg.is_root_admin(&user))
            .unwrap_or(false);
        let server = self
            .bnets
            .get(&server_id)
            .map(|h| server_key(&h.cfg))
            .unwrap_or_default();
        // Either a root admin (config file) or a db admin can use commands
        let is_admin = is_root
            || self
                .db
                .admin_check(&server, &user)
                .await
                .unwrap_or(false);

        info!(server_id, "[BNET] command from [{user}] (root={is_root}, admin={is_admin}): !{command} {payload}");

        if !is_admin {
            return;
        }

        let who = Some((server_id, user.clone(), whisper));
        match command.as_str() {
            // ---- admin / ban management ----
            "addadmin" => {
                if !is_root {
                    self.reply_creator(&who, &crate::lang::t("only_root_addadmin", &[])).await;
                } else {
                    let name = payload.trim().to_string();
                    if name.is_empty() {
                        self.reply_creator(&who, &crate::lang::t("addadmin_usage", &[])).await;
                    } else {
                        let msg = match self.db.admin_add(&server, &name).await {
                            Ok(true) => crate::lang::t("admin_added", &[("name", &name)]),
                            Ok(false) => crate::lang::t("admin_already", &[("name", &name)]),
                            Err(e) => crate::lang::t("admin_add_failed", &[("error", &e.to_string())]),
                        };
                        self.reply_creator(&who, &msg).await;
                    }
                }
            }
            "deladmin" => {
                if !is_root {
                    self.reply_creator(&who, &crate::lang::t("only_root_deladmin", &[])).await;
                } else {
                    let name = payload.trim().to_string();
                    let msg = match self.db.admin_remove(&server, &name).await {
                        Ok(true) => crate::lang::t("admin_removed", &[("name", &name)]),
                        Ok(false) => crate::lang::t("admin_not_admin", &[("name", &name)]),
                        Err(e) => crate::lang::t("admin_remove_failed", &[("error", &e.to_string())]),
                    };
                    self.reply_creator(&who, &msg).await;
                }
            }
            "checkadmin" => {
                let name = payload.trim().to_string();
                let msg = match self.db.admin_check(&server, &name).await {
                    Ok(true) => crate::lang::t("admin_is", &[("name", &name)]),
                    Ok(false) => crate::lang::t("admin_not_admin", &[("name", &name)]),
                    Err(e) => crate::lang::t("query_failed", &[("error", &e.to_string())]),
                };
                self.reply_creator(&who, &msg).await;
            }
            "addban" | "ban" => {
                let mut it = payload.trim().splitn(2, ' ');
                let name = it.next().unwrap_or("").to_string();
                let reason = it.next().unwrap_or("").to_string();
                if name.is_empty() {
                    self.reply_creator(&who, &crate::lang::t("ban_usage", &[])).await;
                } else {
                    let ban = crate::db::BanRecord {
                        server: server.clone(),
                        name: name.clone(),
                        ip: String::new(),
                        date: crate::util::now_datetime_string(),
                        game_name: String::new(),
                        admin: user.clone(),
                        reason,
                    };
                    let msg = match self.db.ban_add(&ban).await {
                        Ok(_) => crate::lang::t("ban_added", &[("name", &name)]),
                        Err(e) => crate::lang::t("ban_add_failed", &[("error", &e.to_string())]),
                    };
                    self.reply_creator(&who, &msg).await;
                }
            }
            "delban" | "unban" => {
                let name = payload.trim().to_string();
                let msg = match self.db.ban_remove(&server, &name).await {
                    Ok(true) => crate::lang::t("ban_removed", &[("name", &name)]),
                    Ok(false) => crate::lang::t("ban_not_banned", &[("name", &name)]),
                    Err(e) => crate::lang::t("ban_remove_failed", &[("error", &e.to_string())]),
                };
                self.reply_creator(&who, &msg).await;
            }
            "checkban" => {
                let name = payload.trim().to_string();
                let msg = match self.db.ban_check(&server, &name, "").await {
                    Ok(Some(b)) => {
                        let reason = if b.reason.is_empty() {
                            crate::lang::t("ban_reason_none", &[])
                        } else {
                            b.reason.clone()
                        };
                        crate::lang::t(
                            "ban_info",
                            &[
                                ("name", &name),
                                ("admin", &b.admin),
                                ("date", &b.date),
                                ("reason", &reason),
                            ],
                        )
                    }
                    Ok(None) => crate::lang::t("ban_not_banned", &[("name", &name)]),
                    Err(e) => crate::lang::t("query_failed", &[("error", &e.to_string())]),
                };
                self.reply_creator(&who, &msg).await;
            }
            "autohost" => {
                match payload.trim() {
                    "off" => {
                        self.autohost_enabled = false;
                        self.reply_creator(&who, &crate::lang::t("autohost_off", &[])).await;
                    }
                    "on" => {
                        if self.cfg.auto_host_game_name.is_empty() {
                            self.reply_creator(&who, &crate::lang::t("autohost_no_gamename", &[])).await;
                        } else {
                            self.autohost_enabled = true;
                            self.reply_creator(&who, &crate::lang::t("autohost_on", &[])).await;
                            self.try_autohost().await;
                        }
                    }
                    _ => {
                        let state = if self.autohost_enabled {
                            crate::lang::t("state_on", &[])
                        } else {
                            crate::lang::t("state_off", &[])
                        };
                        let msg = crate::lang::t(
                            "autohost_status",
                            &[
                                ("state", &state),
                                ("name", &self.cfg.auto_host_game_name),
                                ("maxgames", &self.cfg.auto_host_maximum_games.to_string()),
                                ("startplayers", &self.cfg.auto_host_auto_start_players.to_string()),
                            ],
                        );
                        self.reply_creator(&who, &msg).await;
                    }
                }
            }
            "say" => self.broadcast_chat(&payload).await,
            "pub" => {
                self.create_game(GAME_PUBLIC, payload, Some((server_id, user, whisper)), 0)
                    .await
            }
            "priv" => {
                self.create_game(GAME_PRIVATE, payload, Some((server_id, user, whisper)), 0)
                    .await
            }
            "unhost" => self.unhost_game().await,
            // ---- lobby commands (slot numbers are 1-based, same as the GHost convention) ----
            "open" => {
                if let Ok(n) = payload.trim().parse::<usize>() {
                    if n >= 1 {
                        self.send_game(GameCommand::OpenSlot(n - 1)).await;
                    }
                }
            }
            "close" => {
                if let Ok(n) = payload.trim().parse::<usize>() {
                    if n >= 1 {
                        self.send_game(GameCommand::CloseSlot(n - 1)).await;
                    }
                }
            }
            "swap" => {
                let nums: Vec<usize> = payload
                    .split_whitespace()
                    .filter_map(|s| s.parse().ok())
                    .collect();
                if nums.len() == 2 && nums[0] >= 1 && nums[1] >= 1 {
                    self.send_game(GameCommand::SwapSlots(nums[0] - 1, nums[1] - 1)).await;
                }
            }
            "kick" => {
                if !payload.trim().is_empty() {
                    self.send_game(GameCommand::Kick(payload.trim().to_string())).await;
                }
            }
            "start" => self.send_game(GameCommand::Start).await,
            "latency" => {
                let value = payload.trim().parse::<u32>().ok();
                self.send_game(GameCommand::SetLatency(value)).await;
            }
            "synclimit" => {
                let value = payload.trim().parse::<u32>().ok();
                self.send_game(GameCommand::SetSyncLimit(value)).await;
            }
            "exit" | "quit" => {
                if !is_root {
                    self.reply_creator(&who, &crate::lang::t("only_root_exit", &[])).await;
                    return;
                }
                if let Some(h) = self.bnets.get(&server_id) {
                    let reply = crate::lang::t("bnet_shutting_down", &[]);
                    let _ = h
                        .tx
                        .send(if whisper {
                            BnetCommand::QueueWhisper { to: user, message: reply }
                        } else {
                            BnetCommand::QueueChat(reply)
                        })
                        .await;
                }
                // Trigger overall shutdown
                let _ = self.event_tx.send(BotEvent::ConsoleInput("exit".into())).await;
            }
            // ---- Group B: global management ----
            "disable" => {
                self.games_disabled = true;
                self.reply_creator(&who, &crate::lang::t("games_disabled_msg", &[])).await;
            }
            "enable" => {
                self.games_disabled = false;
                self.reply_creator(&who, &crate::lang::t("games_enabled_msg", &[])).await;
                self.try_autohost().await;
            }
            "downloads" => match payload.trim() {
                "0" => { self.download_mode = 0; self.reply_creator(&who, &crate::lang::t("downloads_disabled", &[])).await; }
                "1" => { self.download_mode = 1; self.reply_creator(&who, &crate::lang::t("downloads_enabled", &[])).await; }
                "2" => { self.download_mode = 2; self.reply_creator(&who, &crate::lang::t("downloads_conditional", &[])).await; }
                _ => self.reply_creator(&who, &crate::lang::t("downloads_usage", &[])).await,
            },
            "getgames" => {
                let mut lines = vec![crate::lang::t(
                    "games_summary",
                    &[
                        ("lobby", if self.current_game.is_some() { "1" } else { "0" }),
                        ("active", &self.games.len().to_string()),
                        ("max", &self.cfg.max_games.to_string()),
                    ],
                )];
                if let Some(g) = &self.current_game {
                    lines.push(crate::lang::t("games_lobby_entry", &[("name", &g.game_name)]));
                }
                for g in self.games.values() {
                    lines.push(crate::lang::t("games_active_entry", &[("name", &g.game_name)]));
                }
                self.reply_creator(&who, &lines.join(" | ")).await;
            }
            "getgame" => {
                let msg = if let Some(g) = &self.current_game {
                    crate::lang::t(
                        "getgame_info",
                        &[("name", &g.game_name), ("hc", &g.host_counter.to_string())],
                    )
                } else {
                    crate::lang::t("getgame_none", &[])
                };
                self.reply_creator(&who, &msg).await;
            }
            "saygames" => {
                if !payload.trim().is_empty() {
                    self.send_game(GameCommand::Say(payload.clone())).await;
                    for g in self.games.values() {
                        let _ = g.tx.send(GameCommand::Say(payload.clone())).await;
                    }
                    self.reply_creator(&who, &crate::lang::t("saygames_done", &[])).await;
                }
            }
            "saygame" => {
                // !saygame <host_counter> <text>
                let mut it = payload.trim().splitn(2, ' ');
                let hc = it.next().and_then(|s| s.parse::<u32>().ok());
                let text = it.next().unwrap_or("").to_string();
                match hc {
                    Some(hc) if !text.is_empty() => {
                        self.send_game_to(hc, GameCommand::Say(text)).await;
                        self.reply_creator(&who, &crate::lang::t("saygame_done", &[])).await;
                    }
                    _ => self.reply_creator(&who, &crate::lang::t("saygame_usage", &[])).await,
                }
            }
            "countadmins" => {
                let msg = match self.db.admin_list(&server).await {
                    Ok(list) => crate::lang::t(
                        "admin_count",
                        &[("server", &server), ("count", &list.len().to_string())],
                    ),
                    Err(e) => crate::lang::t("query_failed", &[("error", &e.to_string())]),
                };
                self.reply_creator(&who, &msg).await;
            }
            "countbans" => {
                let msg = match self.db.ban_list(&server).await {
                    Ok(list) => crate::lang::t(
                        "ban_count",
                        &[("server", &server), ("count", &list.len().to_string())],
                    ),
                    Err(e) => crate::lang::t("query_failed", &[("error", &e.to_string())]),
                };
                self.reply_creator(&who, &msg).await;
            }
            "dbstatus" => {
                self.reply_creator(&who, &crate::lang::t("db_status", &[("description", &self.db.description())])).await;
            }
            "channel" => {
                let ch = payload.trim().to_string();
                if ch.is_empty() {
                    self.reply_creator(&who, &crate::lang::t("channel_usage", &[])).await;
                } else if let Some(h) = self.bnets.get(&server_id) {
                    let _ = h.tx.send(BnetCommand::JoinChannel(ch.clone())).await;
                    self.reply_creator(&who, &crate::lang::t("channel_joining", &[("channel", &ch)])).await;
                }
            }
            "map" | "load" => {
                let pattern = payload.trim().to_string();
                if pattern.is_empty() {
                    // No pattern: report the current map
                    let msg = if self.map.is_valid {
                        crate::lang::t("map_current", &[("path", self.map.get_map_path())])
                    } else {
                        crate::lang::t("map_invalid", &[])
                    };
                    self.reply_creator(&who, &msg).await;
                } else {
                    // Pattern given: search bot_mappath and load on a unique match
                    // (mirrors the C++ !map partial-match behavior)
                    let matches = self.find_map_files(&pattern);
                    match matches.len() {
                        0 => {
                            self.reply_creator(
                                &who,
                                &crate::lang::t("map_search_none", &[("pattern", &pattern)]),
                            )
                            .await;
                        }
                        1 => {
                            let file = matches.into_iter().next().unwrap();
                            self.reply_creator(
                                &who,
                                &crate::lang::t("map_loading", &[("file", &file)]),
                            )
                            .await;
                            let msg = self.load_map_file(&file).await;
                            self.reply_creator(&who, &msg).await;
                        }
                        n => {
                            // Multiple matches: list the first few
                            let list = matches
                                .iter()
                                .take(5)
                                .cloned()
                                .collect::<Vec<_>>()
                                .join(", ");
                            self.reply_creator(
                                &who,
                                &crate::lang::t(
                                    "map_search_multi",
                                    &[
                                        ("count", &n.to_string()),
                                        ("pattern", &pattern),
                                        ("list", &list),
                                    ],
                                ),
                            )
                            .await;
                        }
                    }
                }
            }
            other => debug!(server_id, "[BNET] unhandled command !{other}"),
        }
    }

    async fn handle_game_event(&mut self, host_counter: u32, event: GameEvent) {
        match event {
            GameEvent::PlayerJoined { name } => info!(host_counter, "[GAME] player joined: {name}"),
            GameEvent::PlayerLeft { name } => info!(host_counter, "[GAME] player left: {name}"),
            GameEvent::GProxyRegistered { key } => {
                debug!(host_counter, "[GAME] GProxy key registered {key:08X}");
                self.gproxy_keys.insert(key, host_counter);
            }
            GameEvent::PlayerChat { name, message, spoofed, spoofed_realm } => {
                info!(host_counter, "[GAME] [{name}] {message}");
                // In-game command permission: must pass spoofcheck (whisper sc, a server-authenticated identity),
                // then match root admin / db admin against the "verified realm".
                // (Replaces the earlier stopgap that trusted the client's self-reported name — impersonators cannot pass)
                if message.starts_with(&self.cfg.command_trigger) {
                    let (command, payload) = crate::util::get_command_and_payload(&message);
                    // Commands usable by ordinary players (no admin/spoofcheck required) (mirrors COMMAND.txt "can be used
                    // by non admins"; they only view their own info and do not affect the game): checkme / check / version / stats
                    let non_admin = matches!(
                        command.as_str(),
                        "checkme" | "version" | "stats" | "statsdota"
                    );
                    if non_admin {
                        self.dispatch_lobby_command(host_counter, &name, &command, payload).await;
                    } else if !spoofed {
                        debug!(host_counter, "[GAME] command from non-spoofchecked player, rejecting: [{name}] !{command}");
                        self.send_game_to(
                            host_counter,
                            GameCommand::Say(crate::lang::t(
                                "spoofcheck_required",
                                &[("name", &name)],
                            )),
                        )
                        .await;
                    } else {
                        let is_admin = self
                            .bnets
                            .values()
                            .any(|h| {
                                server_key(&h.cfg) == spoofed_realm && h.cfg.is_root_admin(&name)
                            })
                            || self
                                .db
                                .admin_check(&spoofed_realm, &name)
                                .await
                                .unwrap_or(false);
                        if is_admin {
                            self.dispatch_lobby_command(host_counter, &name, &command, payload).await;
                        } else {
                            debug!(host_counter, "[GAME] command from non-admin, ignoring: [{name}] !{command}");
                        }
                    }
                }
            }
            GameEvent::GameStarted => {
                info!(host_counter, "[GAME] started - removing from game list (UncreateGame)");
                for h in self.bnets.values() {
                    let _ = h.tx.send(BnetCommand::UncreateGame).await;
                }
                // After starting, move the game from the lobby into the in-progress set, freeing the lobby to !pub a new game
                if self
                    .current_game
                    .as_ref()
                    .map(|g| g.host_counter == host_counter)
                    .unwrap_or(false)
                {
                    if let Some(g) = self.current_game.take() {
                        self.games.insert(host_counter, g);
                    }
                }
                // The lobby is now free: autohost proceeds to host the next game
                self.try_autohost().await;
            }
            GameEvent::GameEnded { record, players } => {
                info!(
                    host_counter,
                    "[GAME] ended: [{}] duration={}s players={}",
                    record.game_name,
                    record.duration,
                    players.len()
                );
                match self.db.game_add(&record, &players).await {
                    Ok(id) => info!(host_counter, "[GAME] written to database game_id={id}"),
                    Err(e) => warn!(host_counter, "[GAME] failed to write game record: {e}"),
                }
            }
            GameEvent::Deleted => {
                // Could be the lobby game (!unhost) or an in-progress game (finished / everyone left)
                if self
                    .current_game
                    .as_ref()
                    .map(|g| g.host_counter == host_counter)
                    .unwrap_or(false)
                {
                    self.current_game = None;
                    for h in self.bnets.values() {
                        let _ = h.tx.send(BnetCommand::UncreateGame).await;
                    }
                    info!(host_counter, "[GAME] deleted; lobby cleared");
                } else if self.games.remove(&host_counter).is_some() {
                    info!(host_counter, "[GAME] in-progress game ended and removed ({} still in progress)", self.games.len());
                }
                // Clean up that game's GProxy reconnect keys
                self.gproxy_keys.retain(|_, hc| *hc != host_counter);
                // Host a replacement if there is room (autohost)
                self.try_autohost().await;
            }
        }
    }
}
