//! GameActor implementation (mirrors the lobby portion of C++ CBaseGame).

use std::collections::HashMap;
use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::mpsc;
use tokio::time::{interval, MissedTickBehavior};
use tracing::{debug, info, warn};

use crate::bot::messages::{BotEvent, GameCommand, GameEvent};
use crate::core::gameprotocol::*;
use crate::core::gameslot::*;
use crate::core::gamemap::{
    MAPFLAG_RANDOMRACES, MAPGAMETYPE_PRIVATEGAME, MAPGAMETYPE_UNKNOWN0, MAPOBS_ALLOWED,
    MAPOBS_REFEREES, MAPOPT_CUSTOMFORCES, MAPOPT_FIXEDPLAYERSETTINGS,
};
use crate::core::GameMap;
use crate::net::codec::FrameCodec;
use crate::net::conn::{self, CloseReason, ConnEvent, ConnHandle, ConnId, DEFAULT_RECV_TIMEOUT};
use crate::util::{get_ticks, util_byte_array_to_u32, util_encode_stat_string};

/// The set of legal characters for the HCL command string (mirrors C++ game_base.cpp)
pub const HCL_ALLOWED_CHARS: &str = "abcdefghijklmnopqrstuvwxyz0123456789 -=,.";

/// Lower bound for the action send interval in ms. C++ uses 20; loosened here to 5 for low-latency LAN use.
/// Note: 5ms = 200 action batches per second, with higher CPU/bandwidth cost, suitable only for LAN or extremely low ping.
pub const LATENCY_MIN: u32 = 5;
pub const LATENCY_MAX: u32 = 500;
/// Lag tolerance time window (ms): the lag screen only triggers once a player falls behind by more than this "time".
/// A fixed constant = controlled solely by latency; the actual trigger batch count = SYNC_TOLERANCE_MS / latency, auto-derived
/// (mirrors the GHost convention "sync_limit ≈ 5000 / latency"). Players can temporarily override it with !synclimit.
pub const SYNC_TOLERANCE_MS: u32 = 5_000;
/// Lower/upper bounds (ms) for the time window when overridden by !synclimit, to avoid extreme values.
pub const SYNC_WINDOW_MIN_MS: u32 = 500;
pub const SYNC_WINDOW_MAX_MS: u32 = 30_000;

/// The config needed to create a game (assembled by BotCore from BotConfig + map)
pub struct GameConfig {
    pub host_counter: u32,
    pub game_name: String,
    /// GAME_PUBLIC(16) / GAME_PRIVATE(17)
    pub game_state: u8,
    pub map: Arc<GameMap>,
    pub virtual_host_name: String,
    /// Hide IPs from players (bot_hideipaddresses)
    pub hide_ip: bool,
    /// LC-style ping (halved)
    pub lc_pings: bool,
    /// Action send interval in ms (bot_latency, defaults to 100)
    pub latency_ms: u32,
    /// Database (join-time ban check, game records)
    pub db: Arc<dyn crate::db::GhostDb>,
    /// All bnet server names (ban check compares each one, mirrors the C++ loop over m_BNETs)
    pub servers: Vec<String>,
    /// The owner for the game record (bnet account name, not the virtual host name)
    pub owner_name: String,
    /// Player count that auto-starts the countdown (0 = no auto-start; games created by autohost carry a value)
    pub autostart_players: u8,
    /// GProxy++ reconnect enabled
    pub reconnect_enabled: bool,
    /// The reconnect port advertised to the client via GPSS_INIT
    pub reconnect_port: u16,
    /// GProxy empty action count (= reconnect_wait_time-1, capped at 9; wait time = (N+1)×60 seconds)
    pub gproxy_empty_actions: u8,
    /// Replay: auto-save a .w3g per game (bot_savereplays)
    pub save_replays: bool,
    /// Directory where replays are stored
    pub replay_path: String,
    /// Replay header version number / build (replay_war3version / replay_buildnumber)
    pub replay_war3_version: u32,
    pub replay_build_number: u16,
    /// Map download mode (0=disabled / 1=enabled / 2=conditional; !downloads)
    pub download_mode: u8,
}

/// The game handle held by BotCore
pub struct GameHandle {
    pub host_counter: u32,
    pub game_name: String,
    pub tx: mpsc::Sender<GameCommand>,
}

/// Start a game actor, returning its handle.
pub fn spawn(cfg: GameConfig, event_tx: mpsc::Sender<BotEvent>) -> GameHandle {
    let host_counter = cfg.host_counter;
    let game_name = cfg.game_name.clone();
    let (cmd_tx, cmd_rx) = mpsc::channel(256);
    let actor = GameActor::new(cfg, event_tx, cmd_rx);
    tokio::spawn(actor.run());
    GameHandle {
        host_counter,
        game_name,
        tx: cmd_tx,
    }
}

/// A single real player in the lobby
struct LobbyPlayer {
    pid: u8,
    name: String,
    conn_id: ConnId,
    handle: ConnHandle,
    /// Internal IP reported by REQJOIN (4 bytes)
    internal_ip: Vec<u8>,
    /// The peer's external IP (4 bytes; 0 when hide_ip)
    external_ip: Vec<u8>,

    // ---- map download (mirrors the C++ CGamePlayer download fields) ----
    download_started: bool,
    download_finished: bool,
    /// The map offset already sent (the next part starts here)
    last_part_sent: u32,
    /// The offset the client has acknowledged receiving (MAPSIZE size with flag!=1 / MAPPARTOK)
    last_part_acked: u32,

    // ---- gameplay ----
    /// Whether loading has finished (GAMELOADED_SELF)
    finished_loading: bool,
    /// The tick when loading finished (ticks; used to compute loadingtime)
    finished_loading_ticks: u64,
    /// Number of keepalives received (compared with the game's sync_counter to detect falling behind)
    sync_counter: u32,
    /// Keepalive checksums not yet compared (desync detection)
    checksums: std::collections::VecDeque<u32>,
    /// Whether the player is in the lag screen list
    lagging: bool,
    /// The ticks when lag started (used to compute lag duration)
    started_lagging_ticks: u64,

    // ---- GProxy++ (mirrors C++ CGamePlayer) ----
    /// The client has reported using GProxy (received a GPS_INIT)
    gproxy: bool,
    /// Disconnected and waiting for reconnect (the socket is dead but the player is retained)
    gproxy_disconnected: bool,
    /// Reconnect verification key (randomly assigned on join)
    gproxy_key: u32,
    /// Send buffer (all W3GS packets after game_loaded; resent on reconnect)
    gproxy_buffer: std::collections::VecDeque<Vec<u8>>,
    gproxy_buffer_bytes: usize,
    /// Total packets sent counted from connection start (mirrors m_TotalPacketsSent; GPS packets not counted)
    total_sent: u32,
    /// Total packets received (counted from REQJOIN = initial value 1; mirrors m_TotalPacketsReceived)
    total_received: u32,
    /// Disconnect time (seconds; formally removed after (empty_actions+1)×60 timeout)
    disconnect_time: u64,

    // ---- spoofcheck ----
    /// Passed spoofcheck (bnet whisper sc, a server-authenticated identity)
    spoofed: bool,
    /// The verified realm (server host)
    spoofed_realm: String,

    /// Muted (!mute: messages are not forwarded to others)
    muted: bool,
    /// The most recent RTTs (ms; computed when a PONG returns, averaged by !ping)
    pings: std::collections::VecDeque<u32>,
}

struct GameActor {
    cfg: GameConfig,
    event_tx: mpsc::Sender<BotEvent>,
    cmd_rx: mpsc::Receiver<GameCommand>,
    /// Event bus for all player connections (each conn read task → here)
    conn_event_tx: mpsc::Sender<ConnEvent>,
    conn_event_rx: mpsc::Receiver<ConnEvent>,

    /// slot layout (copied from the map)
    slots: Vec<GameSlot>,
    /// Connection handles that are established but not yet / already joined
    conns: HashMap<ConnId, ConnHandle>,
    /// Joined connection → PID
    conn_pid: HashMap<ConnId, u8>,
    /// PID → player
    players: HashMap<u8, LobbyPlayer>,

    next_conn_id: ConnId,
    virtual_host_pid: u8,
    random_seed: u32,
    started: bool,
    /// Total map size (bytes, cached from the map_size field)
    map_size: u32,
    /// !start countdown: Some(remaining count), decremented every 500ms, 0 → start (mirrors C++ m_CountDownCounter)
    countdown: Option<u8>,
    /// slot info pending broadcast (for download progress, throttled to once per second; mirrors C++ m_SlotInfoChanged)
    slot_info_changed: bool,

    // ---- gameplay ----
    /// Current action send interval in ms (initially from cfg, dynamically adjustable via !latency, 5~500)
    latency_ms: u32,
    /// Lag tolerance time window (ms). The actual trigger batch count = sync_window_ms / latency_ms, auto-derived with latency
    sync_window_ms: u32,
    /// Loading (after COUNTDOWN_END, before everyone has GAMELOADED)
    game_loading: bool,
    /// Game in progress (the action loop is running)
    game_loaded: bool,
    /// Player actions pending batched send (mirrors C++ m_Actions)
    actions: std::collections::VecDeque<IncomingAction>,
    /// Number of action batches already sent (mirrors C++ m_SyncCounter)
    sync_counter: u32,
    /// Whether the lag screen is active
    lagging: bool,
    /// The ticks of the last lag screen resend (the W3 lag screen auto-dismisses after ~65s, so it is re-shown every 60s)
    last_lag_screen_reset: u64,
    /// Already warned about desync (to avoid log spam)
    desync_warned: bool,
    /// Participation records of players who left (written to db when the game ends)
    player_records: Vec<crate::db::GamePlayerRecord>,
    /// The time everyone finished loading (seconds; used to compute game duration)
    loaded_time: u64,
    /// The ticks when loading started (COUNTDOWN_END; used to compute each player's loadingtime)
    start_loading_ticks: u64,
    /// Replay recorder (created at game start when bot_savereplays, compressed and written to file when the game ends)
    replay: Option<crate::core::replay::ReplayRecorder>,

    /// !muteall: mute the whole game (only team/private allowed; only blocks when flag != 16)
    mute_all: bool,
    /// !autostart: auto-start at this player count (0 = off; adjustable at runtime, distinct from cfg.autostart_players)
    autostart_override: Option<u8>,
    /// !announce: (interval seconds, message); None = off
    announce: Option<(u64, String)>,
    /// The second-clock of the last announce broadcast
    last_announce: u64,
    /// Names reserved by !hold (lowercase; prioritized on join, and let into a full lobby if on the list)
    held_names: Vec<String>,
    /// HCL command string (initially = the map's map_defaulthcl; overridden by !hcl, encoded into slot handicaps at game start)
    hcl_string: String,
}

impl GameActor {
    fn new(
        cfg: GameConfig,
        event_tx: mpsc::Sender<BotEvent>,
        cmd_rx: mpsc::Receiver<GameCommand>,
    ) -> Self {
        let (conn_event_tx, conn_event_rx) = mpsc::channel(512);
        let slots = cfg.map.get_slots().clone();
        let map_size = util_byte_array_to_u32(cfg.map.get_map_size(), false, 0);
        let cfg_latency = cfg.latency_ms.clamp(LATENCY_MIN, LATENCY_MAX);
        // Lag tolerance uses a fixed "time window", converted to a batch count solely via latency (no sync_limit setting needed).
        let sync_window_ms = SYNC_TOLERANCE_MS;
        // The HCL initial value comes from the map default (overridable by !hcl)
        let hcl_string = cfg.map.get_map_default_hcl().to_string();
        Self {
            cfg,
            event_tx,
            cmd_rx,
            conn_event_tx,
            conn_event_rx,
            slots,
            conns: HashMap::new(),
            conn_pid: HashMap::new(),
            players: HashMap::new(),
            next_conn_id: 1,
            // The virtual host occupies PID 1 (without taking a slot), so the lobby shows a host and it serves as the host chat source
            virtual_host_pid: 1,
            random_seed: get_ticks() as u32,
            started: false,
            map_size,
            countdown: None,
            slot_info_changed: false,
            latency_ms: cfg_latency,
            sync_window_ms,
            game_loading: false,
            game_loaded: false,
            actions: Default::default(),
            sync_counter: 0,
            lagging: false,
            last_lag_screen_reset: 0,
            desync_warned: false,
            player_records: Vec::new(),
            loaded_time: 0,
            start_loading_ticks: 0,
            replay: None,
            mute_all: false,
            autostart_override: None,
            announce: None,
            last_announce: 0,
            held_names: Vec::new(),
            hcl_string,
        }
    }

    async fn run(mut self) {
        info!(
            "[GAME: {}] lobby created (host_counter={})",
            self.cfg.game_name, self.cfg.host_counter
        );

        // The lobby pings players every 5 seconds
        let mut ping_tick = interval(Duration::from_secs(5));
        ping_tick.set_missed_tick_behavior(MissedTickBehavior::Delay);
        // Map download cadence (mirrors the C++ 100ms download loop) + slot progress throttle (every 1s)
        let mut download_tick = interval(Duration::from_millis(100));
        download_tick.set_missed_tick_behavior(MissedTickBehavior::Delay);
        let mut slot_info_tick = interval(Duration::from_secs(1));
        slot_info_tick.set_missed_tick_behavior(MissedTickBehavior::Delay);
        // !start countdown: one step per second, "5 4 3 2 1" = a real 5 seconds
        // (C++ uses 500ms giving only 2.5 seconds, too fast; changed here to real seconds)
        let mut countdown_tick = interval(Duration::from_secs(1));
        countdown_tick.set_missed_tick_behavior(MissedTickBehavior::Delay);
        // Action batch beat. Uses sleep_until rather than interval,
        // so a dynamic !latency adjustment takes effect on the very next beat
        let mut next_action =
            tokio::time::Instant::now() + Duration::from_millis(self.latency_ms as u64);

        loop {
            tokio::select! {
                cmd = self.cmd_rx.recv() => match cmd {
                    Some(c) => {
                        if !self.handle_command(c).await {
                            break;
                        }
                    }
                    None => break, // BotCore dropped the handle
                },
                ev = self.conn_event_rx.recv() => {
                    if let Some(ev) = ev {
                        self.handle_conn_event(ev).await;
                    }
                }
                _ = ping_tick.tick() => {
                    // C++ pings every 5 seconds regardless of phase (also sent in-game, for the ping display)
                    self.send_all(GameProtocol::send_w3gs_ping_from_host()).await;
                    // GProxy: periodically report the received packet count so the client can trim its resend buffer
                    // (GPS packets are sent directly and not counted; mirrors the C++ SEND_GPSS_ACK every 10 seconds)
                    let acks: Vec<(ConnHandle, u32)> = self
                        .players
                        .values()
                        .filter(|p| p.gproxy && !p.gproxy_disconnected)
                        .map(|p| (p.handle.clone(), p.total_received))
                        .collect();
                    for (h, received) in acks {
                        let _ = h.send(crate::core::gpsprotocol::send_gpss_ack(received)).await;
                    }
                }
                _ = download_tick.tick() => {
                    if !self.started {
                        self.pump_downloads().await;
                    }
                }
                _ = slot_info_tick.tick() => {
                    if self.slot_info_changed && !self.started {
                        self.slot_info_changed = false;
                        self.send_all_slot_info().await;
                    }
                    // !announce: broadcast once every interval seconds in the lobby
                    if !self.started {
                        if let Some((interval_s, msg)) = self.announce.clone() {
                            let now = crate::util::get_time();
                            if now.saturating_sub(self.last_announce) >= interval_s {
                                self.last_announce = now;
                                self.send_all_chat(&msg).await;
                            }
                        }
                    }
                }
                _ = countdown_tick.tick() => {
                    self.tick_countdown().await;
                }
                _ = tokio::time::sleep_until(next_action) => {
                    next_action = tokio::time::Instant::now()
                        + Duration::from_millis(self.latency_ms as u64);
                    if self.game_loaded {
                        self.game_tick().await;
                    }
                }
            }

            // Game-over detection: after starting, all players left → write records and close this game
            if self.started && self.players.is_empty() {
                info!("[GAME: {}] all players left, game over", self.cfg.game_name);

                // Replay: compress and write to file (mirrors C++ CBaseGame's BuildReplay + Compress + Save on destruction)
                if let Some(rep) = self.replay.take() {
                    self.save_replay(rep);
                }
                let duration = if self.loaded_time > 0 {
                    crate::util::get_time().saturating_sub(self.loaded_time) as u32
                } else {
                    0
                };
                let record = crate::db::GameRecord {
                    server: self.cfg.servers.first().cloned().unwrap_or_default(),
                    map: self.cfg.map.get_map_path().to_string(),
                    datetime: crate::util::now_datetime_string(),
                    game_name: self.cfg.game_name.clone(),
                    owner_name: self.cfg.owner_name.clone(),
                    duration,
                };
                let players = std::mem::take(&mut self.player_records);
                let _ = self
                    .event_tx
                    .send(BotEvent::Game {
                        host_counter: self.cfg.host_counter,
                        event: GameEvent::GameEnded { record, players },
                    })
                    .await;
                break;
            }
        }

        info!("[GAME: {}] closed", self.cfg.game_name);
        let _ = self
            .event_tx
            .send(BotEvent::Game {
                host_counter: self.cfg.host_counter,
                event: GameEvent::Deleted,
            })
            .await;
    }

    /// Returning false means this game should be closed
    async fn handle_command(&mut self, cmd: GameCommand) -> bool {
        match cmd {
            GameCommand::NewConnection { stream, peer } => {
                let id = self.next_conn_id;
                self.next_conn_id += 1;
                info!("[GAME: {}] new connection from {peer} (conn={id}), waiting for REQJOIN", self.cfg.game_name);
                let handle = conn::spawn(
                    id,
                    stream,
                    FrameCodec::w3gs(),
                    self.conn_event_tx.clone(),
                    DEFAULT_RECV_TIMEOUT,
                );
                self.conns.insert(id, handle);
                true
            }
            GameCommand::Say(msg) => {
                // Host broadcast to the whole game (from = virtual host)
                self.send_all_chat(&msg).await;
                true
            }
            GameCommand::OpenSlot(sid) => {
                self.set_slot_open(sid, true).await;
                true
            }
            GameCommand::CloseSlot(sid) => {
                self.set_slot_open(sid, false).await;
                true
            }
            GameCommand::SwapSlots(a, b) => {
                self.swap_slots(a, b).await;
                true
            }
            GameCommand::Kick(name) => {
                self.kick_player(&name).await;
                true
            }
            GameCommand::Start => {
                self.start_countdown().await;
                true
            }
            GameCommand::SetLatency(value) => {
                match value {
                    None => {
                        let msg = crate::lang::t(
                            "latency_current",
                            &[
                                ("latency", &self.latency_ms.to_string()),
                                ("window", &self.sync_window_ms.to_string()),
                                ("batches", &self.effective_sync_limit().to_string()),
                            ],
                        );
                        self.send_all_chat(&msg).await;
                    }
                    Some(n) => {
                        let clamped = n.clamp(LATENCY_MIN, LATENCY_MAX);
                        self.latency_ms = clamped;
                        // The lag time window is unchanged → the trigger batch count auto-derives from the new latency, so it does not become overly sensitive at low latency
                        let msg = if clamped != n {
                            crate::lang::t(
                                "latency_set_clamped",
                                &[
                                    ("min", &LATENCY_MIN.to_string()),
                                    ("max", &LATENCY_MAX.to_string()),
                                    ("latency", &clamped.to_string()),
                                    ("batches", &self.effective_sync_limit().to_string()),
                                ],
                            )
                        } else {
                            crate::lang::t(
                                "latency_set",
                                &[
                                    ("latency", &clamped.to_string()),
                                    ("batches", &self.effective_sync_limit().to_string()),
                                ],
                            )
                        };
                        info!(
                            "[GAME: {}] latency={clamped}ms, effective sync_limit={}",
                            self.cfg.game_name,
                            self.effective_sync_limit()
                        );
                        self.send_all_chat(&msg).await;
                    }
                }
                true
            }
            GameCommand::SetSyncLimit(value) => {
                match value {
                    None => {
                        let msg = crate::lang::t(
                            "synclimit_current",
                            &[
                                ("batches", &self.effective_sync_limit().to_string()),
                                ("window", &self.sync_window_ms.to_string()),
                            ],
                        );
                        self.send_all_chat(&msg).await;
                    }
                    Some(batches) => {
                        // The user sets it as a "batch count" → convert back to a time window (which can still be auto-adjusted afterward by !latency)
                        let batches = batches.max(1);
                        self.sync_window_ms =
                            (batches * self.latency_ms).clamp(SYNC_WINDOW_MIN_MS, SYNC_WINDOW_MAX_MS);
                        let msg = crate::lang::t(
                            "synclimit_set",
                            &[
                                ("batches", &self.effective_sync_limit().to_string()),
                                ("window", &self.sync_window_ms.to_string()),
                            ],
                        );
                        info!(
                            "[GAME: {}] sync_window={}ms, effective sync_limit={}",
                            self.cfg.game_name,
                            self.sync_window_ms,
                            self.effective_sync_limit()
                        );
                        self.send_all_chat(&msg).await;
                    }
                }
                true
            }
            GameCommand::GProxyReconnect { stream, pid, key, last_packet } => {
                self.handle_gproxy_reconnect(stream, pid, key, last_packet).await;
                true
            }
            GameCommand::SpoofCheck { name, realm } => {
                // First complete the marking in an isolated scope (ending the mutable borrow), then broadcast
                let verified_name: Option<String> = {
                    match self
                        .players
                        .values_mut()
                        .find(|p| p.name.eq_ignore_ascii_case(&name))
                    {
                        Some(p) if !p.spoofed => {
                            p.spoofed = true;
                            p.spoofed_realm = realm.clone();
                            Some(p.name.clone())
                        }
                        Some(_) => None, // already verified, ignore
                        None => {
                            debug!(
                                "[GAME: {}] spoofcheck: player [{name}] is not in this game",
                                self.cfg.game_name
                            );
                            None
                        }
                    }
                };
                if let Some(verified_name) = verified_name {
                    info!(
                        "[GAME: {}] [{}] spoof check accepted ({realm})",
                        self.cfg.game_name, verified_name
                    );
                    self.send_all_chat(&crate::lang::t(
                        "spoofcheck_accepted",
                        &[("name", &verified_name)],
                    ))
                    .await;
                }
                true
            }
            GameCommand::AdminCommand { requester, command, payload } => {
                self.handle_admin_command(&requester, &command, &payload).await
            }
            GameCommand::Close => false,
        }
    }

    /// In-game admin command (permissions already verified by BotCore). Returns false = close the game.
    /// requester = the name of the player who issued the command (for private replies).
    async fn handle_admin_command(&mut self, requester: &str, command: &str, payload: &str) -> bool {
        let req_pid = self
            .players
            .values()
            .find(|p| p.name.eq_ignore_ascii_case(requester))
            .map(|p| p.pid);
        let arg = payload.trim();

        match command {
            // ---- countdown ----
            "abort" | "a" => {
                if self.countdown.take().is_some() {
                    self.send_all_chat(&crate::lang::t("countdown_aborted", &[])).await;
                } else {
                    self.reply_to(req_pid, &crate::lang::t("countdown_none", &[])).await;
                }
            }
            // ---- slot batch operations ----
            "openall" => {
                let n = self.slots.len();
                for sid in 0..n {
                    if self.slots[sid].slot_status == SLOTSTATUS_CLOSED {
                        self.set_slot_open(sid, true).await;
                    }
                }
            }
            "closeall" => {
                let n = self.slots.len();
                for sid in 0..n {
                    if self.slots[sid].slot_status == SLOTSTATUS_OPEN {
                        self.set_slot_open(sid, false).await;
                    }
                }
            }
            "sp" => {
                self.shuffle_players().await;
                self.send_all_chat(&crate::lang::t("players_shuffled", &[])).await;
            }
            "hold" => {
                // !hold <name> ...: reserve a slot (record the list, prioritizing that name on join; simplified here to an immediate acknowledgement)
                if arg.is_empty() {
                    self.reply_to(req_pid, &crate::lang::t("hold_usage", &[])).await;
                } else {
                    self.held_names.extend(arg.split_whitespace().map(|s| s.to_lowercase()));
                    self.reply_to(req_pid, &crate::lang::t("hold_reserved", &[("names", arg)])).await;
                }
            }
            // ---- mute ----
            "mute" => {
                let name = self.set_muted(arg, true);
                match name {
                    Some(n) => self.send_all_chat(&crate::lang::t("player_muted", &[("name", &n)])).await,
                    None => self.reply_to(req_pid, &crate::lang::t("player_not_found", &[])).await,
                }
            }
            "unmute" => {
                let name = self.set_muted(arg, false);
                match name {
                    Some(n) => self.send_all_chat(&crate::lang::t("player_unmuted", &[("name", &n)])).await,
                    None => self.reply_to(req_pid, &crate::lang::t("player_not_found", &[])).await,
                }
            }
            "muteall" => {
                self.mute_all = true;
                self.send_all_chat(&crate::lang::t("muteall_on", &[])).await;
            }
            "unmuteall" => {
                self.mute_all = false;
                self.send_all_chat(&crate::lang::t("muteall_off", &[])).await;
            }
            // ---- info (private replies) ----
            "check" | "checkme" => {
                let target = if command == "checkme" || arg.is_empty() {
                    requester.to_string()
                } else {
                    arg.to_string()
                };
                let msg = match self.players.values().find(|p| p.name.eq_ignore_ascii_case(&target)) {
                    Some(p) => {
                        let ping = match self.player_ping_ms(p) {
                            Some(ms) => format!("{ms}ms"),
                            None => "-".to_string(),
                        };
                        crate::lang::t(
                            "player_check_info",
                            &[
                                ("name", &p.name),
                                ("ping", &ping),
                                ("spoofed", if p.spoofed { "yes" } else { "no" }),
                                ("realm", if p.spoofed_realm.is_empty() { "-" } else { p.spoofed_realm.as_str() }),
                            ],
                        )
                    }
                    None => crate::lang::t("player_not_found_named", &[("name", &target)]),
                };
                self.reply_to(req_pid, &msg).await;
            }
            "version" => {
                self.reply_to(req_pid, &crate::lang::t("version", &[])).await;
            }
            "trigger" => {
                self.reply_to(req_pid, &crate::lang::t("command_trigger", &[("trigger", "!")])).await;
            }
            "from" => {
                // Show each player's IP (country lookup needs GeoIP; show the IP for now)
                let list: Vec<String> = self
                    .players
                    .values()
                    .map(|p| format!("{}: {}", p.name, p.handle.peer.ip()))
                    .collect();
                self.reply_to(req_pid, &crate::lang::t("players_from", &[("list", &list.join(", "))])).await;
            }
            "ping" => {
                // !ping <n>: kick players with ping above n ms; !ping: show everyone
                if let Ok(threshold) = arg.parse::<u32>() {
                    let over: Vec<(u8, String, u32)> = self
                        .players
                        .values()
                        .filter_map(|p| self.player_ping_ms(p).map(|ms| (p.pid, p.name.clone(), ms)))
                        .filter(|(_, _, ms)| *ms > threshold)
                        .collect();
                    for (pid, name, ms) in over {
                        self.send_all_chat(&crate::lang::t(
                            "ping_kicked",
                            &[("name", &name), ("ping", &ms.to_string())],
                        ))
                        .await;
                        self.remove_player(pid, PLAYERLEAVE_LOBBY as u32).await;
                    }
                } else {
                    let list: Vec<String> = self
                        .players
                        .values()
                        .map(|p| match self.player_ping_ms(p) {
                            Some(ms) => format!("{}: {ms}ms", p.name),
                            None => format!("{}: -", p.name),
                        })
                        .collect();
                    self.reply_to(req_pid, &crate::lang::t("ping_list", &[("list", &list.join(", "))])).await;
                }
            }
            // ---- in-game ----
            "drop" => {
                if self.game_loaded && self.lagging {
                    let laggers: Vec<u8> =
                        self.players.values().filter(|p| p.lagging).map(|p| p.pid).collect();
                    for pid in laggers {
                        self.remove_player(pid, PLAYERLEAVE_DISCONNECT as u32).await;
                    }
                    self.send_all_chat(&crate::lang::t("drop_laggers", &[])).await;
                } else {
                    self.reply_to(req_pid, &crate::lang::t("drop_none", &[])).await;
                }
            }
            "end" => {
                info!("[GAME: {}] admin [{requester}] force-ended the game", self.cfg.game_name);
                self.send_all_chat(&crate::lang::t("game_ended_admin", &[])).await;
                return false;
            }
            // ---- operations ----
            "autostart" => {
                if arg.is_empty() || arg.eq_ignore_ascii_case("off") {
                    self.autostart_override = Some(0);
                    self.send_all_chat(&crate::lang::t("autostart_off", &[])).await;
                } else if let Ok(n) = arg.parse::<u8>() {
                    self.autostart_override = Some(n);
                    self.send_all_chat(&crate::lang::t("autostart_set", &[("count", &n.to_string())])).await;
                    self.maybe_autostart().await;
                } else {
                    self.reply_to(req_pid, &crate::lang::t("autostart_usage", &[])).await;
                }
            }
            "announce" => {
                let low = arg.to_lowercase();
                if arg.is_empty() || low == "off" {
                    self.announce = None;
                    self.send_all_chat(&crate::lang::t("announce_off", &[])).await;
                } else {
                    // !announce <seconds> <message>
                    let mut it = arg.splitn(2, ' ');
                    let secs = it.next().and_then(|s| s.parse::<u64>().ok());
                    let msg = it.next().unwrap_or("").to_string();
                    match (secs, msg.is_empty()) {
                        (Some(s), false) if s > 0 => {
                            self.announce = Some((s, msg));
                            self.last_announce = crate::util::get_time();
                            self.send_all_chat(&crate::lang::t("announce_set", &[("seconds", &s.to_string())])).await;
                        }
                        _ => self.reply_to(req_pid, &crate::lang::t("announce_usage", &[])).await,
                    }
                }
            }
            // ---- HCL mode string (mirrors C++ !hcl / !clearhcl) ----
            "hcl" => {
                if arg.is_empty() {
                    self.send_all_chat(&crate::lang::t("hcl_current", &[("hcl", &self.hcl_string)])).await;
                } else if self.started {
                    self.reply_to(req_pid, &crate::lang::t("hcl_game_started", &[])).await;
                } else if arg.len() > self.occupied_slot_count() {
                    self.send_all_chat(&crate::lang::t("hcl_too_long", &[])).await;
                } else if !arg.chars().all(|c| HCL_ALLOWED_CHARS.contains(c)) {
                    self.send_all_chat(&crate::lang::t("hcl_invalid_chars", &[])).await;
                } else {
                    self.hcl_string = arg.to_string();
                    self.send_all_chat(&crate::lang::t("hcl_set", &[("hcl", &self.hcl_string)])).await;
                }
            }
            "clearhcl" => {
                if self.started {
                    self.reply_to(req_pid, &crate::lang::t("hcl_game_started", &[])).await;
                } else {
                    self.hcl_string.clear();
                    self.send_all_chat(&crate::lang::t("hcl_cleared", &[])).await;
                }
            }
            // Unknown or unimplemented command: stay silent (debug log only, no reply to the player)
            other => {
                debug!("[GAME: {}] unhandled in-game command !{other}", self.cfg.game_name);
            }
        }
        true
    }

    /// The number of currently occupied (non-computer) slots
    fn occupied_slot_count(&self) -> usize {
        self.slots
            .iter()
            .filter(|s| s.slot_status == SLOTSTATUS_OCCUPIED)
            .count()
    }

    /// Private reply to the given pid (ignored if None)
    async fn reply_to(&mut self, pid: Option<u8>, message: &str) {
        if let Some(pid) = pid {
            self.send_private_chat(pid, message).await;
        }
    }

    /// Set/clear mute, returning the player's display name (None if not found). The borrow ends here so the caller can borrow self again.
    fn set_muted(&mut self, name: &str, muted: bool) -> Option<String> {
        let pid = self.find_player_pid(name)?;
        let p = self.players.get_mut(&pid)?;
        p.muted = muted;
        Some(p.name.clone())
    }

    /// The player's average RTT (ms); shows one-way (÷2) when lc_pings. Returns None if there is no data.
    fn player_ping_ms(&self, p: &LobbyPlayer) -> Option<u32> {
        if p.pings.is_empty() {
            return None;
        }
        let avg = p.pings.iter().sum::<u32>() / p.pings.len() as u32;
        Some(if self.cfg.lc_pings { avg / 2 } else { avg })
    }

    /// Find a pid by name (case-insensitive, partial match)
    fn find_player_pid(&self, name: &str) -> Option<u8> {
        let n = name.to_lowercase();
        self.players
            .values()
            .find(|p| p.name.to_lowercase().contains(&n))
            .map(|p| p.pid)
    }

    /// Shuffle: randomly rearrange occupied players' pids among the occupied slots (mirrors C++ ShuffleSlots)
    async fn shuffle_players(&mut self) {
        if self.started {
            return;
        }
        let occupied: Vec<usize> = (0..self.slots.len())
            .filter(|&i| self.slots[i].slot_status == SLOTSTATUS_OCCUPIED && self.slots[i].computer == 0)
            .collect();
        // Collect these slots' (pid, download_status), shuffle randomly, and fill back
        let mut movable: Vec<(u8, u8)> =
            occupied.iter().map(|&i| (self.slots[i].pid, self.slots[i].download_status)).collect();
        // Fisher-Yates
        for i in (1..movable.len()).rev() {
            let j = (rand::random::<u32>() as usize) % (i + 1);
            movable.swap(i, j);
        }
        for (idx, &sid) in occupied.iter().enumerate() {
            self.slots[sid].pid = movable[idx].0;
            self.slots[sid].download_status = movable[idx].1;
        }
        self.send_all_slot_info().await;
    }

    async fn handle_conn_event(&mut self, ev: ConnEvent) {
        match ev {
            ConnEvent::Frame(conn_id, frame) => {
                let data = frame.data.to_vec();
                if let Some(&pid) = self.conn_pid.get(&conn_id) {
                    // magic 0xF7=W3GS, 0xF8=GProxy(GPS)
                    debug!("[GAME: {}] pid={pid} frame magic=0x{:02X}", self.cfg.game_name, frame.magic);
                    self.handle_player_frame(pid, frame.id, &data).await;
                } else if frame.id == W3GS_REQJOIN {
                    self.handle_join(conn_id, &data).await;
                }
                // A not-yet-joined connection only cares about REQJOIN; the rest are ignored
            }
            ConnEvent::Closed(conn_id, reason) => {
                self.handle_conn_closed(conn_id, reason).await;
            }
        }
    }

    /// Packet dispatch for a joined player
    async fn handle_player_frame(&mut self, pid: u8, id: u8, data: &[u8]) {
        debug!("[GAME: {}] pid={pid} sent packet 0x{id:02X} ({} bytes)", self.cfg.game_name, data.len());

        // GPS (magic 0xF8) packets are handled separately; their ids live in a different namespace from W3GS.
        // Note: GPS packets are "not counted" toward total_received (mirrors C++ ExtractPackets, which only
        // does ++m_TotalPacketsReceived on W3GS_HEADER_CONSTANT). Over-counting makes the reconnect handshake
        // report too high → the client over-trims its own resend buffer → missing packets → desync.
        if data.first() == Some(&crate::core::gpsprotocol::GPS_HEADER_CONSTANT) {
            self.handle_gps_frame(pid, id, data).await;
            return;
        }
        // W3GS received-packet count (the resend baseline for GProxy reconnect)
        if let Some(p) = self.players.get_mut(&pid) {
            p.total_received = p.total_received.wrapping_add(1);
        }

        match id {
            W3GS_CHAT_TO_HOST => {
                let owned = data.to_vec();
                if let Some(chat) = GameProtocol::receive_w3gs_chat_to_host(&owned) {
                    match chat.flag {
                        // 16 = lobby text message, 32 = in-game text message: report to BotCore for command parsing and forwarding
                        16 | 32 => {
                            if let Some(p) = self.players.get(&pid) {
                                let _ = self
                                    .event_tx
                                    .send(BotEvent::Game {
                                        host_counter: self.cfg.host_counter,
                                        event: GameEvent::PlayerChat {
                                            name: p.name.clone(),
                                            message: chat.message.clone(),
                                            spoofed: p.spoofed,
                                            spoofed_realm: p.spoofed_realm.clone(),
                                        },
                                    })
                                    .await;
                            }
                            // Mirrors C++ EventPlayerChatToHost (game_base.cpp:2956/2989):
                            // during loading (GameLoading) chat is never forwarded — in-game messages with
                            // extra flags are only forwarded once m_GameLoaded; lobby messages are also not
                            // forwarded during GameLoading/GameLoaded. A modified client receiving a chat packet
                            // on the loading screen may trigger a "click to enter" pause behavior.
                            if !self.game_loading {
                                // Replay: record in-game chat (mirrors C++ game_base.cpp:2983)
                                if self.game_loaded && chat.flag == 32 {
                                    if let Some(rep) = &mut self.replay {
                                        let mode = if chat.extra_flags.len() >= 4 {
                                            util_byte_array_to_u32(&chat.extra_flags, false, 0)
                                        } else {
                                            0
                                        };
                                        rep.add_chat(chat.from_pid, chat.flag, mode, &chat.message);
                                    }
                                }
                                self.relay_chat(chat).await;
                            }
                        }
                        // 17-20 = the player changes their own team/colour/race/handicap in the lobby
                        17 => self.change_team(pid, chat.byte).await,
                        18 => self.change_colour(pid, chat.byte).await,
                        19 => self.change_race(pid, chat.byte).await,
                        20 => self.change_handicap(pid, chat.byte).await,
                        _ => {}
                    }
                }
            }
            W3GS_MAPSIZE => {
                let owned = data.to_vec();
                if let Some(ms) = GameProtocol::receive_w3gs_mapsize(&owned) {
                    self.handle_map_size(pid, ms.size_flag, ms.map_size).await;
                }
            }
            W3GS_MAPPARTOK => {
                let owned = data.to_vec();
                let acked = GameProtocol::receive_w3gs_mappartok(&owned);
                if let Some(p) = self.players.get_mut(&pid) {
                    if acked > p.last_part_acked {
                        p.last_part_acked = acked;
                    }
                }
                self.send_map_parts(pid).await;
            }
            W3GS_PONG_TO_HOST => {
                // pong = the get_ticks we embedded when sending PING; RTT = now - pong (mirrors C++)
                // The first pong is often 1, discard it; also filter out overly large RTTs (wrapping/anomalies)
                let owned = data.to_vec();
                let pong = GameProtocol::receive_w3gs_pong_to_host(&owned);
                if pong != 1 {
                    let rtt = (get_ticks() as u32).wrapping_sub(pong);
                    if rtt < 60_000 {
                        if let Some(p) = self.players.get_mut(&pid) {
                            p.pings.push_back(rtt);
                            while p.pings.len() > 10 {
                                p.pings.pop_front();
                            }
                        }
                    }
                }
            }
            W3GS_GAMELOADED_SELF => {
                let owned = data.to_vec();
                if GameProtocol::receive_w3gs_gameloaded_self(&owned) {
                    self.handle_player_loaded(pid).await;
                }
            }
            W3GS_OUTGOING_ACTION => {
                if self.game_loaded {
                    let owned = data.to_vec();
                    if let Some(action) = GameProtocol::receive_w3gs_outgoing_action(&owned, pid) {
                        self.actions.push_back(action);
                    }
                }
            }
            W3GS_OUTGOING_KEEPALIVE => {
                let owned = data.to_vec();
                let checksum = GameProtocol::receive_w3gs_outgoing_keepalive(&owned);
                let mut first = false;
                if let Some(p) = self.players.get_mut(&pid) {
                    p.sync_counter += 1;
                    first = p.sync_counter == 1;
                    p.checksums.push_back(checksum);
                }
                if first {
                    // The first keepalive = the objective moment the client actually enters the game (used to diagnose "stuck on loading screen")
                    let name = self.players.get(&pid).map(|p| p.name.clone()).unwrap_or_default();
                    info!(
                        "[GAME: {}] [{}] client actually entered the game (first keepalive)",
                        self.cfg.game_name, name
                    );
                }
                self.check_desync().await;
            }
            W3GS_LEAVEGAME => {
                let owned = data.to_vec();
                let reason = GameProtocol::receive_w3gs_leavegame(&owned);
                info!("[GAME: {}] pid={pid} sent LEAVEGAME (reason={reason})", self.cfg.game_name);
                self.remove_player(pid, reason).await;
            }
            _ => debug!("[GAME: {}] unhandled player packet 0x{id:02X}", self.cfg.game_name),
        }
    }

    /// Player finished loading (mirrors C++ EventPlayerLoaded + the all-players check in Update)
    async fn handle_player_loaded(&mut self, pid: u8) {
        if !self.game_loading {
            return;
        }
        let name = match self.players.get_mut(&pid) {
            Some(p) if !p.finished_loading => {
                p.finished_loading = true;
                p.finished_loading_ticks = get_ticks();
                p.name.clone()
            }
            _ => return,
        };
        info!("[GAME: {}] [{}] finished loading", self.cfg.game_name, name);
        // Immediately broadcast that this player has loaded (mirrors C++ EventPlayerLoaded)
        self.send_all(GameProtocol::send_w3gs_gameloaded_others(pid)).await;
        self.check_loading_complete().await;
    }

    /// Everyone finished loading → start the action loop
    async fn check_loading_complete(&mut self) {
        if !self.game_loading || self.players.is_empty() {
            return;
        }
        if self.players.values().all(|p| p.finished_loading) {
            self.game_loading = false;
            self.game_loaded = true;
            self.loaded_time = crate::util::get_time();
            info!(
                "[GAME: {}] all players loaded, game started (latency={}ms, lag tolerance={}ms={} batches)",
                self.cfg.game_name,
                self.latency_ms,
                self.sync_window_ms,
                self.effective_sync_limit()
            );

            // Mirrors C++ EventGameLoaded (game_base.cpp:3543-3569): once everyone finishes loading,
            // immediately broadcast the two shortest/longest load-time messages and privately whisper each player their own load time.
            // C++ always sends these in-game chat packets (flag 32), and before the first batch of actions;
            // a modified client (e.g. FateAnother 1.28) may use them as an "auto-enter" trigger, so they cannot be omitted.
            let mut shortest: Option<(String, u64)> = None;
            let mut longest: Option<(String, u64)> = None;
            for p in self.players.values() {
                let t = p.finished_loading_ticks;
                if shortest.as_ref().map_or(true, |s| t < s.1) {
                    shortest = Some((p.name.clone(), t));
                }
                if longest.as_ref().map_or(true, |l| t > l.1) {
                    longest = Some((p.name.clone(), t));
                }
            }
            let base = self.start_loading_ticks;
            let secs = move |t: u64| format!("{:.2}", t.saturating_sub(base) as f64 / 1000.0);
            if let (Some(s), Some(l)) = (shortest, longest) {
                self.send_all_chat(&crate::lang::t(
                    "load_shortest",
                    &[("name", &s.0), ("seconds", &secs(s.1))],
                ))
                .await;
                self.send_all_chat(&crate::lang::t(
                    "load_longest",
                    &[("name", &l.0), ("seconds", &secs(l.1))],
                ))
                .await;
            }
            // Privately whisper each player their own load time (mirrors C++ game_base.cpp:3568-3569 SendChat)
            let times: Vec<(u8, u64)> = self
                .players
                .values()
                .map(|p| (p.pid, p.finished_loading_ticks))
                .collect();
            for (pid, t) in times {
                self.send_private_chat(
                    pid,
                    &crate::lang::t("load_your_time", &[("seconds", &secs(t))]),
                )
                .await;
            }
        }
    }

    /// Desync detection: when every player has a checksum pending comparison, compare them one by one (mirrors C++ CheckSyncs)
    async fn check_desync(&mut self) {
        while !self.players.is_empty()
            && self.players.values().all(|p| !p.checksums.is_empty())
        {
            let mut first: Option<u32> = None;
            let mut mismatch = false;
            for p in self.players.values_mut() {
                let c = p.checksums.pop_front().unwrap();
                match first {
                    None => first = Some(c),
                    Some(f) if f != c => mismatch = true,
                    _ => {}
                }
            }
            if mismatch && !self.desync_warned {
                self.desync_warned = true;
                warn!("[GAME: {}] desync detected! player game states have diverged", self.cfg.game_name);
                self.send_all_chat(&crate::lang::t("desync_detected", &[])).await;
            }
        }
    }

    /// Convert the lag time window and current latency into "the number of batches behind that triggers the lag screen" (at least 1)
    fn effective_sync_limit(&self) -> u32 {
        (self.sync_window_ms / self.latency_ms).max(1)
    }

    /// The game beat every latency ms: lag check + batched action send (mirrors the m_GameLoaded section of C++ Update)
    async fn game_tick(&mut self) {
        let sync_limit = self.effective_sync_limit();

        // 0) GProxy maintenance: remove on reconnect timeout + kick on buffer overflow
        if self.cfg.reconnect_enabled {
            let now_secs = crate::util::get_time();
            let wait_secs = (self.cfg.gproxy_empty_actions as u64 + 1) * 60;
            let expired: Vec<u8> = self
                .players
                .values()
                .filter(|p| {
                    (p.gproxy_disconnected && now_secs.saturating_sub(p.disconnect_time) > wait_secs)
                        || p.gproxy_buffer_bytes > 8 * 1024 * 1024
                })
                .map(|p| p.pid)
                .collect();
            for pid in expired {
                let name = self.players.get(&pid).map(|p| p.name.clone()).unwrap_or_default();
                info!("[GAME: {}] [{}] reconnect timeout/buffer overflow, removing for good", self.cfg.game_name, name);
                self.send_all_chat(&crate::lang::t("reconnect_timeout", &[("name", &name)])).await;
                self.remove_player(pid, PLAYERLEAVE_DISCONNECT as u32).await;
            }
        }

        // 1) lag state management
        if !self.lagging {
            // Someone is more than sync_limit batches behind → open the lag screen
            let now = get_ticks();
            let laggers: Vec<(u8, u32)> = self
                .players
                .values()
                .filter(|p| self.sync_counter.saturating_sub(p.sync_counter) > sync_limit)
                .map(|p| (p.pid, 0u32))
                .collect();
            if !laggers.is_empty() {
                self.lagging = true;
                self.last_lag_screen_reset = now;
                for (pid, _) in &laggers {
                    if let Some(p) = self.players.get_mut(pid) {
                        p.lagging = true;
                        p.started_lagging_ticks = now;
                    }
                }
                let names: Vec<String> = laggers
                    .iter()
                    .filter_map(|(pid, _)| self.players.get(pid).map(|p| p.name.clone()))
                    .collect();
                info!("[GAME: {}] lag screen started: {:?}", self.cfg.game_name, names);
                self.send_all(GameProtocol::send_w3gs_start_lag_pids(&laggers)).await;
            }
        } else {
            let now = get_ticks();
            // Clear caught-up players one by one (mirrors C++: cleared once the sync difference < sync_limit)
            let recovered: Vec<(u8, u32)> = self
                .players
                .values()
                .filter(|p| {
                    p.lagging
                        && self.sync_counter.saturating_sub(p.sync_counter) < sync_limit
                })
                .map(|p| (p.pid, (now - p.started_lagging_ticks) as u32))
                .collect();
            for (pid, lag_ms) in recovered {
                if let Some(p) = self.players.get_mut(&pid) {
                    p.lagging = false;
                }
                self.send_all(GameProtocol::send_w3gs_stop_lag_pid(pid, lag_ms)).await;
            }
            // Everyone cleared → close the lag screen
            if !self.players.values().any(|p| p.lagging) {
                self.lagging = false;
                info!("[GAME: {}] lag screen cleared", self.cfg.game_name);
            } else if now - self.last_lag_screen_reset >= 60_000 {
                // W3's lag screen disconnects after ~65 seconds without receiving an action, so re-show it every 60 seconds.
                // Mirrors the C++ lag screen reset: for each recipient → STOP_LAG (each lagger) →
                // (non-GProxy recipients get empty_actions empty actions) → 1 empty action → START_LAG.
                // A GProxy client inserts empty actions itself, so only non-GProxy players are supplemented,
                // ensuring every client's W3 sees exactly the same data stream (to avoid desync).
                self.last_lag_screen_reset = now;
                let laggers: Vec<(u8, u32)> = self
                    .players
                    .values()
                    .filter(|p| p.lagging)
                    .map(|p| (p.pid, (now - p.started_lagging_ticks) as u32))
                    .collect();
                if !laggers.is_empty() {
                    let using_gproxy = self.players.values().any(|p| p.gproxy);
                    let empty_n = self.cfg.gproxy_empty_actions;
                    let recipients: Vec<(u8, bool)> =
                        self.players.values().map(|p| (p.pid, p.gproxy)).collect();
                    for (rpid, is_gproxy) in recipients {
                        for (lpid, lag_ms) in &laggers {
                            self.send_to_pid(
                                rpid,
                                GameProtocol::send_w3gs_stop_lag_pid(*lpid, *lag_ms),
                            )
                            .await;
                        }
                        if using_gproxy && !is_gproxy {
                            for _ in 0..empty_n {
                                let mut empty = std::collections::VecDeque::new();
                                self.send_to_pid(
                                    rpid,
                                    GameProtocol::send_w3gs_incoming_action(&mut empty, 0),
                                )
                                .await;
                            }
                        }
                        let mut empty = std::collections::VecDeque::new();
                        self.send_to_pid(
                            rpid,
                            GameProtocol::send_w3gs_incoming_action(&mut empty, 0),
                        )
                        .await;
                        self.send_to_pid(rpid, GameProtocol::send_w3gs_start_lag_pids(&laggers))
                            .await;
                    }
                    // Replay: record these empty actions in sync (mirrors the C++ lag reset's AddTimeSlot(0, ...))
                    if let Some(rep) = &mut self.replay {
                        if using_gproxy {
                            for _ in 0..empty_n {
                                rep.add_time_slot(0, &[]);
                            }
                        }
                        rep.add_time_slot(0, &[]);
                    }
                }
            }
        }

        // 2) pause sending actions while lagging (mirrors C++: SendAllActions only when !m_Lagging)
        if self.lagging {
            return;
        }
        self.send_all_actions().await;
    }

    /// Batch-send the queued actions (mirrors C++ SendAllActions: split into INCOMING_ACTION2 when >1452 bytes)
    async fn send_all_actions(&mut self) {
        self.sync_counter += 1;

        // Slice the queue into chunks by the 1452-byte cap (each subpacket item = 1(pid) + 2(len) + action bytes)
        let mut chunks: Vec<std::collections::VecDeque<IncomingAction>> = vec![Default::default()];
        let mut current_len = 0usize;
        while let Some(action) = self.actions.pop_front() {
            let len = action.action.len() + 3;
            if current_len + len > 1452 && !chunks.last().unwrap().is_empty() {
                chunks.push(Default::default());
                current_len = 0;
            }
            current_len += len;
            chunks.last_mut().unwrap().push_back(action);
        }

        // Use INCOMING_ACTION2 for all but the last chunk; the last chunk (which may be empty = advance game time) uses INCOMING_ACTION
        let last = chunks.len() - 1;
        for (i, mut chunk) in chunks.into_iter().enumerate() {
            // Replay: record a TimeSlot (mirrors the AddTimeSlot/AddTimeSlot2 in C++ SendAllActions)
            if let Some(rep) = &mut self.replay {
                let acts: Vec<(u8, Vec<u8>)> =
                    chunk.iter().map(|a| (a.pid, a.action.clone())).collect();
                if i == last {
                    rep.add_time_slot(self.latency_ms as u16, &acts);
                } else {
                    rep.add_time_slot2(&acts);
                }
            }
            let pkt = if i == last {
                GameProtocol::send_w3gs_incoming_action(&mut chunk, self.latency_ms as u16)
            } else {
                GameProtocol::send_w3gs_incoming_action2(&mut chunk)
            };
            self.send_all(pkt).await;
        }
    }

    async fn handle_conn_closed(&mut self, conn_id: ConnId, reason: CloseReason) {
        self.conns.remove(&conn_id);
        if let Some(pid) = self.conn_pid.remove(&conn_id) {
            info!("[GAME: {}] pid={pid} connection closed (not a voluntary leave), reason: {reason:?}", self.cfg.game_name);

            // A GProxy player disconnecting mid-game → retain them, wait for reconnect
            // (the lag screen triggers naturally because they stop sending keepalives; the timeout is checked by game_tick)
            if self.game_loaded && self.cfg.reconnect_enabled {
                let keep = self
                    .players
                    .get_mut(&pid)
                    .map(|p| {
                        if p.gproxy && !p.gproxy_disconnected {
                            p.gproxy_disconnected = true;
                            p.disconnect_time = crate::util::get_time();
                            true
                        } else {
                            false
                        }
                    })
                    .unwrap_or(false);
                if keep {
                    let name = self.players.get(&pid).map(|p| p.name.clone()).unwrap_or_default();
                    let wait = (self.cfg.gproxy_empty_actions as u32 + 1) * 60;
                    info!(
                        "[GAME: {}] [{}] disconnected but using GProxy++, holding {wait}s waiting for reconnect",
                        self.cfg.game_name, name
                    );
                    self.send_all_chat(&crate::lang::t(
                        "gproxy_disconnected_waiting",
                        &[("name", &name), ("seconds", &wait.to_string())],
                    ))
                    .await;
                    return;
                }
            }

            self.remove_player(pid, PLAYERLEAVE_DISCONNECT as u32).await;
        }
    }

    /// GPS (GProxy++) packet handling (mirrors the GPS branch in C++ gameplayer.cpp)
    async fn handle_gps_frame(&mut self, pid: u8, id: u8, data: &[u8]) {
        use crate::core::gpsprotocol as gps;
        match id {
            gps::GPS_INIT => {
                if !self.cfg.reconnect_enabled {
                    return;
                }
                let (key, name, handle) = match self.players.get_mut(&pid) {
                    Some(p) => {
                        p.gproxy = true;
                        (p.gproxy_key, p.name.clone(), p.handle.clone())
                    }
                    None => return,
                };
                // GPS packets are sent directly, not counted and not buffered (mirrors C++ using PutBytes rather than Send)
                let _ = handle
                    .send(gps::send_gpss_init(
                        self.cfg.reconnect_port,
                        pid,
                        key,
                        self.cfg.gproxy_empty_actions,
                    ))
                    .await;
                info!("[GAME: {}] [{}] is using GProxy++", self.cfg.game_name, name);
                // Register the key → BotCore (used for routing on reconnect)
                let _ = self
                    .event_tx
                    .send(BotEvent::Game {
                        host_counter: self.cfg.host_counter,
                        event: GameEvent::GProxyRegistered { key },
                    })
                    .await;
            }
            gps::GPS_ACK => {
                if let Some(last) = gps::receive_gps_ack(data) {
                    if let Some(p) = self.players.get_mut(&pid) {
                        trim_gproxy_buffer(p, last);
                    }
                }
            }
            // GPS_RECONNECT goes through the reconnect listener and does not appear on the game socket
            _ => {}
        }
    }

    /// GProxy reconnect (the reconnect listener has already validated the GPS_RECONNECT format)
    async fn handle_gproxy_reconnect(
        &mut self,
        stream: tokio::net::TcpStream,
        pid: u8,
        key: u32,
        last_packet: u32,
    ) {
        use crate::core::gpsprotocol as gps;

        // Validate pid + key
        let valid = self
            .players
            .get(&pid)
            .map(|p| p.gproxy && p.gproxy_key == key)
            .unwrap_or(false);
        if !valid {
            warn!(
                "[GAME: {}] GProxy reconnect validation failed pid={pid} key={key:08X}",
                self.cfg.game_name
            );
            // Reply reject directly, then discard
            let mut s = stream;
            use tokio::io::AsyncWriteExt;
            let _ = s
                .write_all(&gps::send_gpss_reject(gps::REJECTGPS_NOTFOUND as u32))
                .await;
            return;
        }

        // Swap in the new connection
        let old_conn_id = self.players.get(&pid).map(|p| p.conn_id).unwrap_or(0);
        self.conns.remove(&old_conn_id);
        self.conn_pid.remove(&old_conn_id);

        let id = self.next_conn_id;
        self.next_conn_id += 1;
        let handle = conn::spawn(
            id,
            stream,
            FrameCodec::w3gs(),
            self.conn_event_tx.clone(),
            DEFAULT_RECV_TIMEOUT,
        );
        self.conns.insert(id, handle.clone());
        self.conn_pid.insert(id, pid);

        let (total_received, name) = {
            let p = match self.players.get_mut(&pid) {
                Some(p) => p,
                None => return,
            };
            p.conn_id = id;
            p.handle = handle.clone();
            p.gproxy_disconnected = false;
            p.disconnect_time = 0;
            // Trim to the position the client has already received
            trim_gproxy_buffer(p, last_packet);
            (p.total_received, p.name.clone())
        };

        // Handshake response (tells the client how much we received, so it resends its own buffer accordingly)
        let _ = handle.send(gps::send_gpss_reconnect(total_received)).await;

        // Resend the buffer (kept in the buffer, trimmed later by ACK; mirrors C++ EventGProxyReconnect)
        let pending: Vec<Vec<u8>> = self
            .players
            .get(&pid)
            .map(|p| p.gproxy_buffer.iter().cloned().collect())
            .unwrap_or_default();
        info!(
            "[GAME: {}] [{}] GProxy reconnect succeeded, resending {} packets",
            self.cfg.game_name,
            name,
            pending.len()
        );
        for pkt in pending {
            let _ = handle.send(pkt).await;
        }
        self.send_all_chat(&crate::lang::t("gproxy_reconnected", &[("name", &name)])).await;
    }

    /// REQJOIN join flow (mirrors C++ CBaseGame::EventPlayerJoined)
    async fn handle_join(&mut self, conn_id: ConnId, data: &[u8]) {
        let join = match GameProtocol::receive_w3gs_reqjoin(data) {
            Some(j) => j,
            None => {
                warn!("[GAME: {}] REQJOIN parse failed", self.cfg.game_name);
                return;
            }
        };
        info!("[GAME: {}] received REQJOIN from [{}]", self.cfg.game_name, join.name);
        let handle = match self.conns.get(&conn_id) {
            Some(h) => h.clone(),
            None => return,
        };

        // The game has already started
        if self.started {
            let _ = handle
                .send(GameProtocol::send_w3gs_rejectjoin(REJECTJOIN_STARTED as u32))
                .await;
            self.conns.remove(&conn_id);
            return;
        }

        // Duplicate-name check
        if self
            .players
            .values()
            .any(|p| p.name.eq_ignore_ascii_case(&join.name))
        {
            info!("[GAME: {}] rejecting [{}] - name already taken", self.cfg.game_name, join.name);
            let _ = handle
                .send(GameProtocol::send_w3gs_rejectjoin(REJECTJOIN_FULL as u32))
                .await;
            self.conns.remove(&conn_id);
            return;
        }

        // Ban check (name + IP, compared against each bnet server; mirrors C++ EventPlayerJoined)
        let peer_ip = handle.peer.ip().to_string();
        for server in &self.cfg.servers {
            match self.cfg.db.ban_check(server, &join.name, &peer_ip).await {
                Ok(Some(ban)) => {
                    info!(
                        "[GAME: {}] rejecting [{}] ({peer_ip}) - banned on [{server}] by [{}] ({})",
                        self.cfg.game_name, join.name, ban.admin, ban.reason
                    );
                    let _ = handle
                        .send(GameProtocol::send_w3gs_rejectjoin(REJECTJOIN_FULL as u32))
                        .await;
                    self.conns.remove(&conn_id);
                    return;
                }
                Ok(None) => {}
                Err(e) => warn!("[GAME: {}] ban check failed (allowing): {e}", self.cfg.game_name),
            }
        }

        // !hold: if the joiner is on the reserved list, consume the reservation and log it (mirrors the C++ reserved concept)
        let jname = join.name.to_lowercase();
        if let Some(idx) = self.held_names.iter().position(|n| *n == jname) {
            self.held_names.remove(idx);
            info!("[GAME: {}] [{}] claimed reserved slot", self.cfg.game_name, join.name);
        }

        // Find an empty slot and a PID
        let sid = match self.empty_slot() {
            Some(s) => s,
            None => {
                info!("[GAME: {}] rejecting [{}] - game is full", self.cfg.game_name, join.name);
                let _ = handle
                    .send(GameProtocol::send_w3gs_rejectjoin(REJECTJOIN_FULL as u32))
                    .await;
                self.conns.remove(&conn_id);
                return;
            }
        };
        let pid = match self.new_pid() {
            Some(p) => p,
            None => {
                let _ = handle
                    .send(GameProtocol::send_w3gs_rejectjoin(REJECTJOIN_FULL as u32))
                    .await;
                self.conns.remove(&conn_id);
                return;
            }
        };

        let (external_ip, external_port) = addr_bytes(handle.peer, self.cfg.hide_ip);
        // GProxy counting: the directly-sent SLOTINFOJOIN + vhost PLAYERINFO + existing players' PLAYERINFO + MAPCHECK below
        // total 3 + N packets, counted into total_sent's initial value when the player record is created (mirrors C++ counting everything from connection start)
        let pre_sent = 3 + self.players.len() as u32;

        // Occupy the slot
        self.slots[sid].pid = pid;
        self.slots[sid].slot_status = SLOTSTATUS_OCCUPIED;
        self.slots[sid].computer = 0;
        self.slots[sid].download_status = 255; // download not yet confirmed; becomes 100 after MAPSIZE

        // ---- send to the joiner ----
        // 1) SLOTINFOJOIN (includes their own PID + the full slot layout)
        let _ = handle
            .send(GameProtocol::send_w3gs_slotinfojoin(
                pid,
                external_port.clone(),
                external_ip.clone(),
                &self.slots,
                self.random_seed,
                self.cfg.map.get_map_layout_style(),
                self.cfg.map.get_map_nuplayers() as u8,
            ))
            .await;

        // 2) the virtual host's PLAYERINFO (so the lobby shows a host)
        let _ = handle
            .send(GameProtocol::send_w3gs_playerinfo(
                self.virtual_host_pid,
                self.cfg.virtual_host_name.clone(),
                vec![0, 0, 0, 0],
                vec![0, 0, 0, 0],
            ))
            .await;

        // 3) existing players' PLAYERINFO
        for p in self.players.values() {
            let _ = handle
                .send(GameProtocol::send_w3gs_playerinfo(
                    p.pid,
                    p.name.clone(),
                    p.external_ip.clone(),
                    p.internal_ip.clone(),
                ))
                .await;
        }

        // 4) MAPCHECK (lets the client compare the map / trigger a download)
        let _ = handle
            .send(GameProtocol::send_w3gs_mapcheck(
                self.cfg.map.get_map_path().to_string(),
                self.cfg.map.get_map_size().clone(),
                self.cfg.map.get_map_info().clone(),
                self.cfg.map.get_map_crc().clone(),
                self.cfg.map.get_map_sha1().clone(),
            ))
            .await;

        // Create the player record
        self.players.insert(
            pid,
            LobbyPlayer {
                pid,
                name: join.name.clone(),
                conn_id,
                handle,
                internal_ip: join.internal_ip.clone(),
                external_ip: external_ip.clone(),
                download_started: false,
                download_finished: false,
                last_part_sent: 0,
                last_part_acked: 0,
                finished_loading: false,
                finished_loading_ticks: 0,
                sync_counter: 0,
                checksums: Default::default(),
                lagging: false,
                started_lagging_ticks: 0,
                gproxy: false,
                gproxy_disconnected: false,
                gproxy_key: rand::random::<u32>(),
                gproxy_buffer: Default::default(),
                gproxy_buffer_bytes: 0,
                total_sent: pre_sent,
                // REQJOIN was already received before the player was created (mirrors the C++ hackhack comment)
                total_received: 1,
                disconnect_time: 0,
                spoofed: false,
                spoofed_realm: String::new(),
                muted: false,
                pings: Default::default(),
            },
        );
        self.conn_pid.insert(conn_id, pid);

        info!(
            "[GAME: {}] player [{}] joined as pid={} slot={}",
            self.cfg.game_name, join.name, pid, sid
        );

        // ---- send to everyone else ----
        // 5) broadcast the new player's PLAYERINFO to the other players
        let info = GameProtocol::send_w3gs_playerinfo(
            pid,
            join.name.clone(),
            external_ip,
            join.internal_ip.clone(),
        );
        self.send_to_others(pid, info).await;

        // 6) broadcast the updated slot info to the whole game
        self.send_all_slot_info().await;

        let _ = self
            .event_tx
            .send(BotEvent::Game {
                host_counter: self.cfg.host_counter,
                event: GameEvent::PlayerJoined { name: join.name },
            })
            .await;

        self.maybe_autostart().await;
    }

    /// autohost auto-start when full (only counts down once the player count is reached + everyone has confirmed the map)
    async fn maybe_autostart(&mut self) {
        let n = self
            .autostart_override
            .unwrap_or(self.cfg.autostart_players) as usize;
        if n == 0 || self.started || self.countdown.is_some() || self.players.len() < n {
            return;
        }
        // Only start once everyone has confirmed the map (slot download_status == 100), to avoid someone still downloading / not yet reported
        let all_ready = self.players.keys().all(|pid| {
            self.slot_index_of_pid(*pid)
                .map(|sid| self.slots[sid].download_status == 100)
                .unwrap_or(false)
        });
        if all_ready {
            info!(
                "[GAME: {}] autohost: player count reached {n}, auto-starting countdown",
                self.cfg.game_name
            );
            self.send_all_chat(&crate::lang::t("autostart_full", &[("count", &n.to_string())])).await;
            self.start_countdown().await;
        }
    }

    /// MAPSIZE handling (mirrors C++ EventPlayerMapSize)
    async fn handle_map_size(&mut self, pid: u8, size_flag: u8, client_size: u32) {
        if self.started {
            return;
        }
        let our_size = self.map_size;

        if size_flag != 1 || client_size != our_size {
            // The player does not have the (complete) map
            // !downloads 0: downloads disabled → kick immediately if no map (mirrors C++ m_AllowDownloads==0)
            if self.cfg.download_mode == 0 {
                warn!(
                    "[GAME: {}] pid={pid} has no map and downloads are disabled (!downloads 0), kicking",
                    self.cfg.game_name
                );
                self.send_all_chat(&crate::lang::t("no_map_downloads_disabled", &[])).await;
                self.remove_player(pid, PLAYERLEAVE_LOBBY as u32).await;
                return;
            }
            if self.cfg.map.get_map_data().is_empty() {
                warn!(
                    "[GAME: {}] pid={pid} has no map and no local map file to send, kicking",
                    self.cfg.game_name
                );
                self.remove_player(pid, PLAYERLEAVE_LOBBY as u32).await;
                return;
            }

            let start_download = {
                let p = match self.players.get_mut(&pid) {
                    Some(p) => p,
                    None => return,
                };
                if !p.download_started && size_flag == 1 {
                    // Tell the client we are willing to send the map
                    p.download_started = true;
                    p.last_part_sent = 0;
                    p.last_part_acked = 0;
                    true
                } else {
                    // Downloading: the client reports the size received so far via MAPSIZE
                    p.last_part_acked = client_size;
                    false
                }
            };

            if start_download {
                info!("[GAME: {}] pid={pid} started map download", self.cfg.game_name);
                let pkt = GameProtocol::send_w3gs_startdownload(self.virtual_host_pid);
                self.send_to_pid(pid, pkt).await;
            }
            self.send_map_parts(pid).await;
        } else {
            // The player already has the complete map
            let finished_now = {
                let p = match self.players.get_mut(&pid) {
                    Some(p) => p,
                    None => return,
                };
                let f = p.download_started && !p.download_finished;
                if f {
                    p.download_finished = true;
                }
                f
            };
            if finished_now {
                info!("[GAME: {}] pid={pid} finished map download", self.cfg.game_name);
                let name = self.players.get(&pid).map(|p| p.name.clone()).unwrap_or_default();
                self.send_all_chat(&crate::lang::t("player_downloaded_map", &[("name", &name)])).await;
            }
        }

        // Update slot download progress (throttled: mark it, slot_info_tick broadcasts once per second)
        let mut status = if our_size > 0 {
            ((client_size as u64) * 100 / (our_size as u64)) as u8
        } else {
            100
        };
        if status > 100 {
            status = 100;
        }
        if let Some(sid) = self.slot_index_of_pid(pid) {
            if self.slots[sid].download_status != status {
                self.slots[sid].download_status = status;
                self.slot_info_changed = true;
            }
        }

        // The map confirmation may be the last piece before auto-start
        self.maybe_autostart().await;
    }

    /// Feed map parts to a single player (sliding window: send at most 100 parts beyond acked; mirrors the C++ download loop)
    async fn send_map_parts(&mut self, pid: u8) {
        const PART: u32 = 1442;
        const WINDOW: u32 = PART * 100;

        let map_data = self.cfg.map.get_map_data();
        if map_data.is_empty() {
            return;
        }
        let (mut sent, acked, downloading) = match self.players.get(&pid) {
            Some(p) => (p.last_part_sent, p.last_part_acked, p.download_started && !p.download_finished),
            None => return,
        };
        if !downloading {
            return;
        }

        let mut packets = Vec::new();
        while sent < self.map_size && sent < acked.saturating_add(WINDOW) {
            packets.push(GameProtocol::send_w3gs_mappart(
                self.virtual_host_pid,
                pid,
                sent as usize,
                map_data,
            ));
            sent = sent.saturating_add(PART).min(self.map_size);
        }

        if let Some(p) = self.players.get_mut(&pid) {
            p.last_part_sent = sent;
        }
        for pkt in packets {
            self.send_to_pid(pid, pkt).await;
        }
    }

    /// 100ms download cadence: refill the window for all downloading players
    async fn pump_downloads(&mut self) {
        let pids: Vec<u8> = self
            .players
            .values()
            .filter(|p| p.download_started && !p.download_finished)
            .map(|p| p.pid)
            .collect();
        for pid in pids {
            self.send_map_parts(pid).await;
        }
    }

    /// !start: begin the countdown (mirrors C++ StartCountDown)
    async fn start_countdown(&mut self) {
        if self.started || self.countdown.is_some() {
            return;
        }
        // Do not start if someone is still downloading (mirrors the C++ check)
        if self.players.values().any(|p| p.download_started && !p.download_finished) {
            self.send_all_chat(&crate::lang::t("countdown_downloading", &[])).await;
            return;
        }
        info!("[GAME: {}] countdown started", self.cfg.game_name);
        self.countdown = Some(5);
    }

    /// Every 500ms: decrement the countdown, 0 → start the game
    async fn tick_countdown(&mut self) {
        let c = match self.countdown {
            Some(c) => c,
            None => return,
        };
        if c > 0 {
            self.send_all_chat(&format!("{c}. . .")).await;
            self.countdown = Some(c - 1);
        } else {
            self.countdown = None;
            self.event_game_started().await;
        }
    }

    /// Start the game (mirrors C++ EventGameStarted: SlotInfo → COUNTDOWN_START → delete virtual host → COUNTDOWN_END)
    async fn event_game_started(&mut self) {
        if self.started {
            return;
        }
        info!(
            "[GAME: {}] started loading with {} players",
            self.cfg.game_name,
            self.players.len()
        );
        self.started = true;

        // HCL: encode the command string into the occupied slots' handicaps (mirrors C++ EventGameStarted,
        // game_base.cpp:3345-3400). HCL is the bot's only channel for passing a mode string to the map
        // (decoded from the handicap once the map finishes loading); maps like FateAnother/DotA use it to auto-select a mode.
        // Uses this game's hcl_string (initial value = map default, overridable by !hcl)
        let hcl = self.hcl_string.clone();
        if !hcl.is_empty() {
            const HCL_CHARS: &str = HCL_ALLOWED_CHARS;
            let occupied = self
                .slots
                .iter()
                .filter(|s| s.slot_status == SLOTSTATUS_OCCUPIED)
                .count();
            if hcl.len() > occupied {
                warn!(
                    "[GAME: {}] HCL [{hcl}] encoding failed: not enough occupied slots (cf. C++ game_base.cpp:3399)",
                    self.cfg.game_name
                );
            } else if !hcl.chars().all(|c| HCL_CHARS.contains(c)) {
                warn!(
                    "[GAME: {}] HCL [{hcl}] encoding failed: contains invalid characters (cf. C++ game_base.cpp:3396)",
                    self.cfg.game_name
                );
            } else {
                // EncodingMap: skip the 7 "valid handicap values" 0/50/60/70/80/90/100,
                // so the encoded handicap is always an invalid value, by which the map side recognizes and decodes it
                // (mirrors C++ game_base.cpp:3367-3378)
                let mut encoding_map = [0u8; 256];
                let mut j: u32 = 0;
                for e in encoding_map.iter_mut() {
                    if matches!(j, 0 | 50 | 60 | 70 | 80 | 90 | 100) {
                        j += 1;
                    }
                    *e = j as u8;
                    j += 1;
                }
                let mut cur = 0usize;
                for ch in hcl.chars() {
                    while self.slots[cur].slot_status != SLOTSTATUS_OCCUPIED {
                        cur += 1;
                    }
                    let handicap_index =
                        (self.slots[cur].handicap.saturating_sub(50) / 10) as usize;
                    let char_index = HCL_CHARS.find(ch).unwrap();
                    self.slots[cur].handicap = encoding_map[handicap_index + char_index * 6];
                    cur += 1;
                }
                info!(
                    "[GAME: {}] successfully encoded HCL command string [{hcl}]",
                    self.cfg.game_name
                );
                // The encoded handicaps are broadcast all at once by send_all_slot_info below
                // (C++ makes an extra SendAllSlotInfo call at the encoding site; the final state the client sees is the same)
            }
        }

        // Replay initialization (mirrors C++ EventGameStarted 3454-3489: after HCL encoding, when slots are finalized)
        if self.cfg.save_replays {
            // stat string (mirrors game_base.cpp:156-167; host name fixed as "GHost++")
            let mut ss: Vec<u8> = Vec::new();
            ss.extend(self.cfg.map.get_map_game_flags());
            ss.push(0);
            ss.extend(self.cfg.map.get_map_width());
            ss.extend(self.cfg.map.get_map_height());
            ss.extend(self.cfg.map.get_map_crc());
            ss.extend(self.cfg.map.get_map_path().as_bytes());
            ss.push(0);
            ss.extend(b"GHost++");
            ss.push(0);
            ss.push(0);
            ss.extend(self.cfg.map.get_map_sha1());
            let stat_string = util_encode_stat_string(&ss);

            let mut game_type = self.cfg.map.get_map_game_type() | MAPGAMETYPE_UNKNOWN0;
            if self.cfg.game_state == GAME_PRIVATE {
                game_type |= MAPGAMETYPE_PRIVATEGAME;
            }
            // The replay host = the real player with the smallest PID (the virtual host is about to be deleted; mirrors C++ m_Players[0])
            let host = self
                .players
                .keys()
                .copied()
                .min()
                .and_then(|pid| self.players.get(&pid).map(|p| (pid, p.name.clone())))
                .unwrap_or((1, self.cfg.virtual_host_name.clone()));
            let players: Vec<(u8, String)> =
                self.players.values().map(|p| (p.pid, p.name.clone())).collect();
            self.replay = Some(crate::core::replay::ReplayRecorder::new(
                host.0,
                host.1,
                players,
                self.slots.clone(),
                self.random_seed,
                self.cfg.map.get_map_layout_style(),
                self.cfg.map.get_map_nuplayers() as u8,
                game_type,
                self.cfg.game_name.clone(),
                stat_string,
            ));
        }

        self.send_all_slot_info().await;
        self.send_all(GameProtocol::send_w3gs_countdown_start()).await;
        // Delete the virtual host (the client removes it from the player list)
        self.send_all(GameProtocol::send_w3gs_playerleave_others(
            self.virtual_host_pid,
            PLAYERLEAVE_LOBBY as u32,
        ))
        .await;
        self.send_all(GameProtocol::send_w3gs_countdown_end()).await;

        // Enter the loading state, waiting for all players to GAMELOADED_SELF
        self.game_loading = true;
        self.start_loading_ticks = get_ticks();
        info!(
            "[GAME: {}] waiting for {} players to load the map...",
            self.cfg.game_name,
            self.players.len()
        );

        let _ = self
            .event_tx
            .send(BotEvent::Game {
                host_counter: self.cfg.host_counter,
                event: GameEvent::GameStarted,
            })
            .await;
    }

    /// Open/close a slot (sid is 0-based; kicks the player out when occupied)
    async fn set_slot_open(&mut self, sid: usize, open: bool) {
        if sid >= self.slots.len() || self.started {
            return;
        }
        if self.slots[sid].slot_status == SLOTSTATUS_OCCUPIED && self.slots[sid].computer == 0 {
            let pid = self.slots[sid].pid;
            self.remove_player(pid, PLAYERLEAVE_LOBBY as u32).await;
        }
        self.slots[sid].slot_status = if open { SLOTSTATUS_OPEN } else { SLOTSTATUS_CLOSED };
        self.slots[sid].pid = 0;
        self.slots[sid].computer = 0;
        self.slots[sid].download_status = 255;
        self.send_all_slot_info().await;
    }

    /// Swap two slots (mirrors C++ SwapSlots: swaps the whole set of fields)
    async fn swap_slots(&mut self, sid1: usize, sid2: usize) {
        if sid1 >= self.slots.len() || sid2 >= self.slots.len() || sid1 == sid2 || self.started {
            return;
        }
        let opts = self.cfg.map.get_map_options();
        let s1 = self.slots[sid1].clone();
        let s2 = self.slots[sid2].clone();

        if opts & MAPOPT_FIXEDPLAYERSETTINGS != 0 {
            // Fixed player settings: team/colour/race/handicap are tied to the "slot position", so only swap the player and status
            let mut n1 = s2.clone();
            n1.team = s1.team;
            n1.colour = s1.colour;
            n1.race = s1.race;
            n1.handicap = s1.handicap;
            let mut n2 = s1.clone();
            n2.team = s2.team;
            n2.colour = s2.colour;
            n2.race = s2.race;
            n2.handicap = s2.handicap;
            self.slots[sid1] = n1;
            self.slots[sid2] = n2;
        } else if opts & MAPOPT_CUSTOMFORCES != 0 {
            // Custom forces: swap the whole slot, but keep team in place (otherwise the player would be moved to the wrong team)
            let mut n1 = s2.clone();
            let mut n2 = s1.clone();
            n1.team = s1.team;
            n2.team = s2.team;
            self.slots[sid1] = n1;
            self.slots[sid2] = n2;
        } else {
            // Normal: swap the whole slot
            self.slots[sid1] = s2;
            self.slots[sid2] = s1;
        }
        self.send_all_slot_info().await;
    }

    /// A player changes their own team (mirrors C++ EventPlayerChangeTeam)
    async fn change_team(&mut self, pid: u8, team: u8) {
        if self.started {
            return;
        }
        let opts = self.cfg.map.get_map_options();
        let num_players = self.cfg.map.get_map_nuplayers() as u8;
        let observers = self.cfg.map.get_map_observers();

        if opts & MAPOPT_CUSTOMFORCES != 0 {
            // Custom forces: changing team = moving to an empty slot on that team
            let old_sid = match self.slot_index_of_pid(pid) {
                Some(s) => s,
                None => return,
            };
            if let Some(new_sid) = self.get_empty_slot_on_team(team, pid) {
                self.swap_slots(old_sid, new_sid).await;
            }
            return;
        }

        // Non-custom forces: change the team field directly
        if team > MAX_SLOTS as u8 {
            return;
        }
        if team == MAX_SLOTS as u8 {
            // Observer team: only allowed if the map permits observers
            if observers != MAPOBS_ALLOWED && observers != MAPOBS_REFEREES {
                return;
            }
        } else {
            if team >= num_players {
                return;
            }
            // Ensure the team will not exceed the player cap
            let others = self
                .slots
                .iter()
                .filter(|s| {
                    s.slot_status == SLOTSTATUS_OCCUPIED
                        && s.team != MAX_SLOTS as u8
                        && s.pid != pid
                })
                .count() as u8;
            if others >= num_players {
                return;
            }
        }
        if let Some(sid) = self.slot_index_of_pid(pid) {
            self.slots[sid].team = team;
            if team == MAX_SLOTS as u8 {
                self.slots[sid].colour = MAX_SLOTS as u8;
            } else if self.slots[sid].colour == MAX_SLOTS as u8 {
                self.slots[sid].colour = self.get_new_colour();
            }
            self.send_all_slot_info().await;
        }
    }

    /// A player changes their own colour (mirrors C++ EventPlayerChangeColour + ColourSlot)
    async fn change_colour(&mut self, pid: u8, colour: u8) {
        if self.started || self.cfg.map.get_map_options() & MAPOPT_FIXEDPLAYERSETTINGS != 0 {
            return;
        }
        if colour >= MAX_SLOTS as u8 {
            return;
        }
        let sid = match self.slot_index_of_pid(pid) {
            Some(s) => s,
            None => return,
        };
        // Observers cannot change colour
        if self.slots[sid].team == MAX_SLOTS as u8 {
            return;
        }
        // Find the slot currently holding this colour
        let taken = self.slots.iter().position(|s| s.colour == colour);
        match taken {
            Some(tsid) if self.slots[tsid].slot_status != SLOTSTATUS_OCCUPIED => {
                // "Held" by an unused slot → give that slot the player's old colour to avoid a duplicate
                let old = self.slots[sid].colour;
                self.slots[tsid].colour = old;
                self.slots[sid].colour = colour;
                self.send_all_slot_info().await;
            }
            None => {
                self.slots[sid].colour = colour;
                self.send_all_slot_info().await;
            }
            _ => {} // Held by a player who is present → do not allow the change
        }
    }

    /// A player changes their own race (mirrors C++ EventPlayerChangeRace)
    async fn change_race(&mut self, pid: u8, race: u8) {
        if self.started
            || self.cfg.map.get_map_options() & MAPOPT_FIXEDPLAYERSETTINGS != 0
            || self.cfg.map.get_map_flags() & MAPFLAG_RANDOMRACES != 0
        {
            return;
        }
        let base = race & !SLOTRACE_SELECTABLE;
        if base != SLOTRACE_HUMAN
            && base != SLOTRACE_ORC
            && base != SLOTRACE_NIGHTELF
            && base != SLOTRACE_UNDEAD
            && base != SLOTRACE_RANDOM
        {
            return;
        }
        if let Some(sid) = self.slot_index_of_pid(pid) {
            self.slots[sid].race = base | SLOTRACE_SELECTABLE;
            self.send_all_slot_info().await;
        }
    }

    /// A player changes their own handicap (mirrors C++ EventPlayerChangeHandicap)
    async fn change_handicap(&mut self, pid: u8, handicap: u8) {
        if self.started || self.cfg.map.get_map_options() & MAPOPT_FIXEDPLAYERSETTINGS != 0 {
            return;
        }
        if ![50, 60, 70, 80, 90, 100].contains(&handicap) {
            return;
        }
        if let Some(sid) = self.slot_index_of_pid(pid) {
            self.slots[sid].handicap = handicap;
            self.send_all_slot_info().await;
        }
    }

    /// Find an empty slot on the team, searching circularly from the player's current position (mirrors C++ GetEmptySlot(team, pid))
    fn get_empty_slot_on_team(&self, team: u8, pid: u8) -> Option<usize> {
        let start = match self.slot_index_of_pid(pid) {
            Some(s) if self.slots[s].team == team => s, // same team → start from self
            _ => 0,                                     // changing team → search from the start
        };
        let n = self.slots.len();
        for k in 0..n {
            let i = (start + k) % n;
            if self.slots[i].slot_status == SLOTSTATUS_OPEN && self.slots[i].team == team {
                return Some(i);
            }
        }
        None
    }

    /// Find an unused colour (mirrors C++ GetNewColour)
    fn get_new_colour(&self) -> u8 {
        for test in 0..MAX_SLOTS as u8 {
            if !self.slots.iter().any(|s| s.colour == test) {
                return test;
            }
        }
        MAX_SLOTS as u8
    }

    /// Kick: a bare number is treated as a slot number (1-based), otherwise match by name (case-insensitive, partial match)
    async fn kick_player(&mut self, name: &str) {
        // slot number mode
        if let Ok(n) = name.trim().parse::<usize>() {
            if n >= 1 && n <= self.slots.len() {
                let slot = &self.slots[n - 1];
                if slot.slot_status == SLOTSTATUS_OCCUPIED && slot.computer == 0 {
                    let pid = slot.pid;
                    self.remove_player(pid, PLAYERLEAVE_LOBBY as u32).await;
                }
            }
            return;
        }
        // name mode
        let target = name.to_lowercase();
        let pid = self
            .players
            .values()
            .find(|p| p.name.to_lowercase().contains(&target))
            .map(|p| p.pid);
        match pid {
            Some(pid) => self.remove_player(pid, PLAYERLEAVE_LOBBY as u32).await,
            None => debug!("[GAME: {}] kick: player [{name}] not found", self.cfg.game_name),
        }
    }

    /// Remove a player (leave / disconnect), free the slot and notify the whole game
    async fn remove_player(&mut self, pid: u8, left_code: u32) {
        let player = match self.players.remove(&pid) {
            Some(p) => p,
            None => return,
        };
        self.conn_pid.remove(&player.conn_id);
        self.conns.remove(&player.conn_id);

        // Grab the slot's team/colour first (once freed, the pid is zeroed and can no longer be looked up)
        let (team, colour) = self
            .slot_index_of_pid(pid)
            .map(|sid| (self.slots[sid].team, self.slots[sid].colour))
            .unwrap_or((0, 0));

        // Free the slot (keep team/colour/race/handicap, set status back to open)
        if let Some(sid) = self.slot_index_of_pid(pid) {
            self.slots[sid].pid = 0;
            self.slots[sid].slot_status = SLOTSTATUS_OPEN;
            self.slots[sid].computer = 0;
            self.slots[sid].download_status = 255;
        }

        info!("[GAME: {}] player [{}] (pid={pid}) left", self.cfg.game_name, player.name);

        // Replay: record the departure (loading blocks are placed during loading; mirrors C++ EventPlayerDeleted)
        if self.game_loading || self.game_loaded {
            let during_loading = self.game_loading;
            if let Some(rep) = &mut self.replay {
                rep.add_leave(pid, left_code, during_loading);
            }
        }

        // Record participation info (written to db when the game ends; fields mirror C++ gameplayers)
        let now = crate::util::get_time();
        self.player_records.push(crate::db::GamePlayerRecord {
            name: player.name.clone(),
            ip: player.handle.peer.ip().to_string(),
            spoofed: player.spoofed as u8,
            reserved: 0, // reserved slot not implemented
            loading_time_ms: player
                .finished_loading_ticks
                .saturating_sub(self.start_loading_ticks) as u32,
            // Time of leaving = seconds counted from game (all-loaded) start; 0 for leaving in the lobby / during loading
            left_secs: if self.loaded_time > 0 {
                now.saturating_sub(self.loaded_time) as u32
            } else {
                0
            },
            left_reason: left_reason_text(left_code).to_string(),
            left_code,
            team,
            colour,
            // Only write the realm if spoofcheck passed (mirrors C++ having spoofedrealm only when GetSpoofed)
            spoofed_realm: player.spoofed_realm.clone(),
        });

        // Notify the other players + update slots (once the game has started, no more slot info is sent)
        self.send_all(GameProtocol::send_w3gs_playerleave_others(pid, left_code))
            .await;
        if !self.started {
            self.send_all_slot_info().await;
        }

        // Someone left during the countdown → abort it and return to the lobby (a client cannot be forced to stay, so we "cancel the start if anyone leaves")
        if self.countdown.is_some() {
            self.countdown = None;
            info!("[GAME: {}] a player left, countdown cancelled", self.cfg.game_name);
            self.send_all_chat(&crate::lang::t("countdown_cancelled_left", &[])).await;
        }

        // Someone left during loading: the remaining players may now all have finished loading
        if self.game_loading {
            self.check_loading_complete().await;
        }

        let _ = self
            .event_tx
            .send(BotEvent::Game {
                host_counter: self.cfg.host_counter,
                event: GameEvent::PlayerLeft { name: player.name },
            })
            .await;
    }

    /// Send hub: all W3GS packets must go through here to reach joined players.
    /// - Counts total_sent (mirrors C++ CGamePlayer::Send; GPS packets bypass this and are not counted)
    /// - After game_loaded, everything for GProxy players goes into the buffer (resent on reconnect; trimmed by ACK)
    /// - For those disconnected and awaiting reconnect, only buffer, do not actually send
    async fn send_to_pid(&mut self, pid: u8, data: Vec<u8>) {
        let game_loaded = self.game_loaded;
        let handle = match self.players.get_mut(&pid) {
            Some(p) => {
                p.total_sent = p.total_sent.wrapping_add(1);
                if p.gproxy && game_loaded {
                    p.gproxy_buffer_bytes += data.len();
                    p.gproxy_buffer.push_back(data.clone());
                }
                if p.gproxy_disconnected {
                    None
                } else {
                    Some(p.handle.clone())
                }
            }
            None => return,
        };
        if let Some(h) = handle {
            let _ = h.send(data).await;
        }
    }

    /// Relay lobby chat: forward CHAT_TO_HOST as CHAT_FROM_HOST to the to_pids
    async fn relay_chat(&mut self, chat: IncomingChatPlayer) {
        // !mute: a muted sender's message is not forwarded
        let sender_muted = self
            .players
            .get(&chat.from_pid)
            .map(|p| p.muted)
            .unwrap_or(false);
        if sender_muted {
            return;
        }
        // !muteall: in-game, block "global" public messages (flag 32 and extra_flags[0]==0 = all);
        // team (>=2) / private messages still pass (mirrors C++ MuteAll blocking only global)
        if self.mute_all && chat.flag == 32 {
            let mode = chat.extra_flags.first().copied().unwrap_or(0);
            if mode == 0 {
                return;
            }
        }
        for &to in &chat.to_pids {
            let pkt = GameProtocol::send_w3gs_chat_from_host(
                chat.from_pid,
                vec![to],
                chat.flag,
                chat.extra_flags.clone(),
                chat.message.clone(),
            );
            self.send_to_pid(to, pkt).await;
        }
    }

    /// Compress the replay and write it to a file: <replay_path>/<datetime> <game name>.w3g
    fn save_replay(&self, rep: crate::core::replay::ReplayRecorder) {
        let dir = if self.cfg.replay_path.is_empty() {
            "replays".to_string()
        } else {
            self.cfg.replay_path.clone()
        };
        // Sanitize the filename (mirrors C++ UTIL_FileSafeName)
        let safe_name: String = self
            .cfg
            .game_name
            .chars()
            .map(|c| match c {
                '\\' | '/' | ':' | '*' | '?' | '"' | '<' | '>' | '|' => '_',
                c => c,
            })
            .collect();
        let datetime = crate::util::now_datetime_string().replace(':', "-");
        let path = format!("{dir}/{datetime} {safe_name}.w3g");

        match rep.build_and_compress(self.cfg.replay_war3_version, self.cfg.replay_build_number) {
            Ok(bytes) => {
                if let Err(e) = std::fs::create_dir_all(&dir) {
                    warn!("[GAME: {}] failed to create replay directory: {e}", self.cfg.game_name);
                    return;
                }
                match std::fs::write(&path, &bytes) {
                    Ok(()) => info!(
                        "[GAME: {}] replay saved: {path} ({} bytes, {:.1} minutes)",
                        self.cfg.game_name,
                        bytes.len(),
                        rep.replay_length_ms() as f64 / 60000.0
                    ),
                    Err(e) => warn!("[GAME: {}] replay write failed: {e}", self.cfg.game_name),
                }
            }
            Err(e) => warn!("[GAME: {}] replay compression failed: {e}", self.cfg.game_name),
        }
    }

    /// The source PID for chat messages (mirrors C++ GetHostPID, game_base.cpp:3782-3811).
    /// At game start the virtual host has already been removed via PLAYERLEAVE, and the client ignores chat
    /// packets from a nonexistent PID, so after the game starts a real player must be used: prefer the owner, otherwise the smallest PID.
    fn host_pid(&self) -> u8 {
        if !self.started {
            return self.virtual_host_pid;
        }
        if let Some(p) = self
            .players
            .values()
            .find(|p| p.name.eq_ignore_ascii_case(&self.cfg.owner_name))
        {
            return p.pid;
        }
        self.players.keys().copied().min().unwrap_or(255)
    }

    /// The host speaks to the whole game (from = host_pid: virtual host in the lobby, a real player after the game starts).
    /// The lobby uses flag 16; in-game must use flag 32 + extra_flags [0;4] (mirrors C++ SendChat),
    /// otherwise the in-game client will not display it.
    async fn send_all_chat(&mut self, message: &str) {
        let from = self.host_pid();
        let (flag, extra) = if self.game_loading || self.game_loaded {
            (32u8, vec![0u8, 0, 0, 0])
        } else {
            (16u8, vec![])
        };
        // Replay: record the bot's in-game broadcast (mirrors C++ game_base.cpp:1256)
        if flag == 32 {
            if let Some(rep) = &mut self.replay {
                rep.add_chat(from, 32, 0, message);
            }
        }
        let pids: Vec<u8> = self.players.keys().copied().collect();
        for pid in pids {
            let pkt = GameProtocol::send_w3gs_chat_from_host(
                from,
                vec![pid],
                flag,
                extra.clone(),
                message.to_string(),
            );
            self.send_to_pid(pid, pkt).await;
        }
    }

    /// Private message to a single player (mirrors C++ SendChat, game_base.cpp:1186-1216).
    /// Lobby: flag 16; in-game: flag 32 + extra_flags[0] = 3 + the recipient's colour
    /// (the client uses this to label it [Private]).
    async fn send_private_chat(&mut self, pid: u8, message: &str) {
        let from = self.host_pid();
        let (flag, extra) = if self.game_loading || self.game_loaded {
            let colour = self
                .slot_index_of_pid(pid)
                .map(|sid| self.slots[sid].colour)
                .unwrap_or(0);
            (32u8, vec![3u8.wrapping_add(colour), 0, 0, 0])
        } else {
            (16u8, vec![])
        };
        let pkt = GameProtocol::send_w3gs_chat_from_host(
            from,
            vec![pid],
            flag,
            extra,
            message.to_string(),
        );
        self.send_to_pid(pid, pkt).await;
    }

    async fn send_all_slot_info(&mut self) {
        let pkt = GameProtocol::send_w3gs_slotinfo(
            &self.slots,
            self.random_seed,
            self.cfg.map.get_map_layout_style(),
            self.cfg.map.get_map_nuplayers() as u8,
        );
        self.send_all(pkt).await;
    }

    /// Send to all players
    async fn send_all(&mut self, data: Vec<u8>) {
        let pids: Vec<u8> = self.players.keys().copied().collect();
        for pid in pids {
            self.send_to_pid(pid, data.clone()).await;
        }
    }

    /// Send to all players except except_pid
    async fn send_to_others(&mut self, except_pid: u8, data: Vec<u8>) {
        let pids: Vec<u8> = self.players.keys().copied().collect();
        for pid in pids {
            if pid != except_pid {
                self.send_to_pid(pid, data.clone()).await;
            }
        }
    }

    /// Find the first open, non-computer slot
    fn empty_slot(&self) -> Option<usize> {
        self.slots
            .iter()
            .position(|s| s.slot_status == SLOTSTATUS_OPEN && s.computer == 0)
    }

    /// Find the slot index occupied by that PID
    fn slot_index_of_pid(&self, pid: u8) -> Option<usize> {
        self.slots.iter().position(|s| s.pid == pid)
    }

    /// Allocate the smallest unused PID (1..=255, excluding the virtual host and existing players)
    fn new_pid(&self) -> Option<u8> {
        (1u8..=255).find(|&pid| pid != self.virtual_host_pid && !self.players.contains_key(&pid))
    }
}

/// Leave code → human-readable text (mirrors the leftreason style of C++ language.cfg)
fn left_reason_text(left_code: u32) -> &'static str {
    match left_code as u8 {
        PLAYERLEAVE_DISCONNECT => "has lost the connection",
        PLAYERLEAVE_LOST => "has left the game voluntarily",
        PLAYERLEAVE_LOSTBUILDINGS => "has left the game (lost buildings)",
        PLAYERLEAVE_WON => "has left the game (won)",
        PLAYERLEAVE_DRAW => "has left the game (draw)",
        PLAYERLEAVE_OBSERVER => "has left the game (observer)",
        PLAYERLEAVE_LOBBY => "has left the game (lobby)",
        PLAYERLEAVE_GPROXY => "was unrecoverably dropped from GProxy++",
        _ => "has left the game",
    }
}

/// GProxy buffer trim: the client reports having received last_packet packets, so drop the acknowledged ones from the front
/// (mirrors the C++ GPS_ACK handling: PacketsAlreadyUnqueued = TotalSent - buffer.size())
fn trim_gproxy_buffer(p: &mut LobbyPlayer, last_packet: u32) {
    let already = p.total_sent.wrapping_sub(p.gproxy_buffer.len() as u32);
    if last_packet > already {
        let mut n = (last_packet - already) as usize;
        n = n.min(p.gproxy_buffer.len());
        for _ in 0..n {
            if let Some(pkt) = p.gproxy_buffer.pop_front() {
                p.gproxy_buffer_bytes = p.gproxy_buffer_bytes.saturating_sub(pkt.len());
            }
        }
    }
}

/// SocketAddr → (external_ip 4 bytes, port 2 bytes).
/// The port uses big-endian (network order), matching the bytes sent by C++ CSocket::GetPort.
fn addr_bytes(peer: SocketAddr, hide: bool) -> (Vec<u8>, Vec<u8>) {
    let ip = match peer.ip() {
        IpAddr::V4(v4) if !hide => v4.octets().to_vec(),
        _ => vec![0, 0, 0, 0],
    };
    let port = peer.port().to_be_bytes().to_vec();
    (ip, port)
}
