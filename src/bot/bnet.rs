//! BnetActor: a tokio actor for a single battle.net / PVPGN connection.
//!
//! Replaces the synchronous 50ms polling of legacy `core::bnet`:
//! - One `Framed<TcpStream, FrameCodec::bncs()>` reads/writes BNCS packets
//! - `tokio::select!` concurrently handles: incoming packets, BotCore's commands, the flood-protection send timer, and the 60s NULL
//! - Reconnects after a disconnect per reconnect_delay (PVPGN 90s / official 240s)
//! - Login state machine mirrors the C++ bnet.cpp login sequence
//!
//! Events go up as `BotEvent::Bnet{server_id, ...}`, commands come down as `BnetCommand`.

use std::net::{SocketAddr, SocketAddrV4};
use std::sync::Arc;
use std::time::Duration;

use config::Config;
use futures::{SinkExt, StreamExt};
use tokio::net::TcpStream;
use tokio::sync::mpsc;
use tokio::time::{interval, timeout, Instant, MissedTickBehavior};
use tokio_util::codec::Framed;
use tracing::{debug, info, warn};

use crate::core::bncsutilinterface::BNCSUtilInterface;
use crate::core::bnetprotocol::*;
use crate::core::gamemap::{MAPGAMETYPE_PRIVATEGAME, MAPGAMETYPE_UNKNOWN0};
use crate::core::gameprotocol::GAME_PRIVATE;
use crate::core::GameMap;
use crate::net::codec::FrameCodec;
use crate::util::{get_command_and_payload, get_ipv4_address, util_extract_numbers};

use super::messages::{BnetCommand, BnetEvent, BotEvent};

/// Config for a single bnet connection (mirrors the config read by the legacy CBNET constructor).
/// Config keys always use the `bnet_` prefix (mirrors the original GHost++ config file format).
#[derive(Debug, Clone)]
pub struct BnetConfig {
    /// Connection target (host:port); bnet_server itself includes the port
    pub server_addr: String,
    /// Local hosting port (bot_hostport), used for SID_NETGAMEPORT
    pub host_port: u16,
    /// War3 install path (bot_war3path), used by checkRevision to compute exe version/hash
    pub war3_path: String,
    pub alias: String,
    pub cdkey_roc: String,
    pub cdkey_tft: String,
    pub country_abbrev: String,
    pub country: String,
    pub locale_id: u32,
    pub user_name: String,
    pub user_password: String,
    pub first_channel: String,
    pub command_trigger: String,
    pub war3_version: u8,
    pub exe_version: Vec<u8>,
    pub exe_version_hash: Vec<u8>,
    pub root_admins: Vec<String>,
    pub tft: bool,
    pub is_pvpgn: bool,
    /// bot_reconnect: when GProxy++ reconnect is enabled, STARTADVEX3 advertises with the [192,7] size signal
    pub reconnect: bool,
}

impl BnetConfig {
    /// Load from the config file (with the `bnet_` prefix). Returns None when server / credentials are missing (skip this connection).
    pub fn load(config: &Config) -> Option<Self> {
        Self::load_with_prefix(config, "bnet_")
    }

    pub fn load_with_prefix(config: &Config, p: &str) -> Option<Self> {
        let get = |k: &str| config.get_string(&format!("{p}{k}")).unwrap_or_default();

        let mut server_addr = get("server");
        if server_addr.is_empty() {
            warn!("[BNET] {p}server not found in config, skipping");
            return None;
        }
        // bnet_server usually already includes the port; if not, append the default 6112
        if !server_addr.contains(':') {
            server_addr.push_str(":6112");
        }

        let mut alias = get("serveralias");
        if alias.is_empty() {
            alias = server_addr.clone();
        }

        let cdkey_roc = get("cdkeyroc").replace('-', "").to_uppercase();
        let cdkey_tft = get("cdkeytft").replace('-', "").to_uppercase();

        let user_name = get("username");
        let user_password = get("password");
        if user_name.is_empty() || user_password.is_empty() {
            warn!("[BNET: {alias}] missing {p}username/{p}password, skipping");
            return None;
        }

        let mut command_trigger = get("commandtrigger");
        if command_trigger.is_empty() {
            command_trigger = "!".to_string();
        }

        let password_hash_type = get("custom_passwordhashtype");
        let exe_version = util_extract_numbers(&get("custom_exeversion"), 4);
        let exe_version_hash = util_extract_numbers(&get("custom_exeversionhash"), 4);
        let is_pvpgn = password_hash_type == "pvpgn";

        let root_admins = get("rootadmin")
            .split_whitespace()
            .map(|s| s.to_lowercase())
            .collect();

        let country_abbrev = {
            let v = get("countryabbrev");
            if v.is_empty() { "USA".to_string() } else { v }
        };
        let country = {
            let v = get("country");
            if v.is_empty() { "United States".to_string() } else { v }
        };
        let first_channel = {
            let v = get("firstchannel");
            if v.is_empty() { "The Void".to_string() } else { v }
        };

        Some(Self {
            server_addr,
            host_port: config.get_int("bot_hostport").map(|v| v as u16).unwrap_or(6112),
            war3_path: config.get_string("bot_war3path").unwrap_or_default(),
            alias,
            cdkey_roc,
            cdkey_tft,
            country_abbrev,
            country,
            locale_id: config.get_int(&format!("{p}locale")).map(|v| v as u32).unwrap_or(1033),
            user_name,
            user_password,
            first_channel,
            command_trigger,
            war3_version: Self::war3_version_from(config, p),
            exe_version,
            exe_version_hash,
            root_admins,
            tft: config.get_bool("bot_tft").unwrap_or(true),
            is_pvpgn,
            reconnect: config.get_bool("bot_reconnect").unwrap_or(true),
        })
    }

    fn war3_version_from(config: &Config, p: &str) -> u8 {
        // bnet_custom_war3version is often a string in the config (e.g. "28"), so try string parsing first, then fall back to get_int
        config
            .get_string(&format!("{p}custom_war3version"))
            .ok()
            .and_then(|s| s.trim().parse::<u8>().ok())
            .or_else(|| config.get_int(&format!("{p}custom_war3version")).ok().map(|v| v as u8))
            .unwrap_or(26)
    }

    fn reconnect_delay(&self) -> Duration {
        Duration::from_secs(if self.is_pvpgn { 90 } else { 240 })
    }

    pub fn is_root_admin(&self, name: &str) -> bool {
        let name = name.to_lowercase();
        self.root_admins.iter().any(|a| a == &name)
    }
}

/// Start a BnetActor task, returning a sender for issuing commands.
pub fn spawn(
    server_id: usize,
    cfg: BnetConfig,
    event_tx: mpsc::Sender<BotEvent>,
) -> mpsc::Sender<BnetCommand> {
    let (cmd_tx, cmd_rx) = mpsc::channel::<BnetCommand>(256);
    let actor = BnetActor::new(server_id, cfg, event_tx, cmd_rx);
    tokio::spawn(actor.run());
    cmd_tx
}

struct BnetActor {
    server_id: usize,
    cfg: BnetConfig,
    event_tx: mpsc::Sender<BotEvent>,
    cmd_rx: mpsc::Receiver<BnetCommand>,
    protocol: BNetProtocol,
    bncs: BNCSUtilInterface,
    /// Flood-protection outgoing packet queue
    out_packets: std::collections::VecDeque<Vec<u8>>,
    last_out_packet: Instant,
    last_out_packet_size: usize,
    logged_in: bool,
    /// The game currently advertised on this server (retained across reconnects)
    advertised: Option<AdvertisedGame>,
}

/// A game currently being advertised on battle.net (the content source for STARTADVEX3)
#[derive(Clone)]
struct AdvertisedGame {
    game_state: u8,
    game_name: String,
    map: Arc<GameMap>,
    host_counter: u32,
}

/// The reason a single connection ended
enum Ended {
    /// Peer closed / socket error → reconnect per reconnect_delay
    Disconnected,
    /// Received a Shutdown command → terminate the actor
    Shutdown,
}

impl BnetActor {
    fn new(
        server_id: usize,
        cfg: BnetConfig,
        event_tx: mpsc::Sender<BotEvent>,
        cmd_rx: mpsc::Receiver<BnetCommand>,
    ) -> Self {
        Self {
            server_id,
            cfg,
            event_tx,
            cmd_rx,
            protocol: BNetProtocol::new(),
            bncs: BNCSUtilInterface::new("", ""),
            out_packets: Default::default(),
            last_out_packet: Instant::now(),
            last_out_packet_size: 0,
            logged_in: false,
            advertised: None,
        }
    }

    async fn emit(&self, event: BnetEvent) {
        let _ = self
            .event_tx
            .send(BotEvent::Bnet {
                server_id: self.server_id,
                event,
            })
            .await;
    }

    async fn run(mut self) {
        loop {
            match self.connect_and_serve().await {
                Ended::Shutdown => break,
                Ended::Disconnected => {
                    self.emit(BnetEvent::Disconnected).await;
                    let delay = self.cfg.reconnect_delay();
                    info!(
                        "[BNET: {}] waiting {}s to reconnect",
                        self.cfg.alias,
                        delay.as_secs()
                    );
                    // Still receive commands while waiting (to avoid blocking the sender); terminate on Shutdown
                    if self.wait_reconnect(delay).await {
                        break;
                    }
                }
            }
        }
        info!("[BNET: {}] actor stopped", self.cfg.alias);
    }

    /// Reconnect wait; returns true if a Shutdown was received
    async fn wait_reconnect(&mut self, delay: Duration) -> bool {
        let deadline = Instant::now() + delay;
        loop {
            tokio::select! {
                _ = tokio::time::sleep_until(deadline) => return false,
                cmd = self.cmd_rx.recv() => match cmd {
                    Some(BnetCommand::Shutdown) | None => return true,
                    // Discard other commands during disconnection (BotCore re-drives them after reconnect)
                    Some(_) => {}
                }
            }
        }
    }

    async fn connect_and_serve(&mut self) -> Ended {
        // Reset per-connection state
        self.protocol = BNetProtocol::new();
        self.bncs.reset(&self.cfg.user_name, &self.cfg.user_password);
        self.out_packets.clear();
        self.logged_in = false;

        let addr: SocketAddrV4 = match get_ipv4_address(&self.cfg.server_addr) {
            Ok(a) => a,
            Err(e) => {
                warn!("[BNET: {}] DNS resolve failed ({}): {e}", self.cfg.alias, self.cfg.server_addr);
                return Ended::Disconnected;
            }
        };

        info!("[BNET: {}] connecting to {}", self.cfg.alias, addr);
        let stream = match timeout(Duration::from_secs(15), TcpStream::connect(SocketAddr::V4(addr))).await
        {
            Ok(Ok(s)) => s,
            Ok(Err(e)) => {
                warn!("[BNET: {}] connect error: {e}", self.cfg.alias);
                return Ended::Disconnected;
            }
            Err(_) => {
                warn!("[BNET: {}] connect timed out", self.cfg.alias);
                return Ended::Disconnected;
            }
        };
        let _ = stream.set_nodelay(true);

        let mut framed = Framed::new(stream, FrameCodec::bncs());
        info!("[BNET: {}] connected", self.cfg.alias);
        self.emit(BnetEvent::Connected).await;

        // Send the protocol selection byte (0x01) + SID_AUTH_INFO
        if framed
            .send(self.protocol.send_protocol_initialize_selector())
            .await
            .is_err()
        {
            return Ended::Disconnected;
        }
        let auth_info = self.protocol.send_sid_auth_info(
            self.cfg.war3_version,
            self.cfg.tft,
            self.cfg.locale_id,
            &self.cfg.country_abbrev,
            &self.cfg.country,
        );
        if framed.send(auth_info).await.is_err() {
            return Ended::Disconnected;
        }

        self.last_out_packet = Instant::now();

        // Timers: flood-protection queue check (100ms), 60s NULL keepalive, 3s public-game refresh
        let mut flood_tick = interval(Duration::from_millis(100));
        flood_tick.set_missed_tick_behavior(MissedTickBehavior::Delay);
        let mut null_tick = interval(Duration::from_secs(60));
        null_tick.set_missed_tick_behavior(MissedTickBehavior::Delay);
        null_tick.tick().await; // swallow the immediately-firing first tick
        let mut refresh_tick = interval(Duration::from_secs(3));
        refresh_tick.set_missed_tick_behavior(MissedTickBehavior::Delay);
        refresh_tick.tick().await; // swallow the immediate first tick

        loop {
            tokio::select! {
                // Incoming packets
                frame = framed.next() => match frame {
                    Some(Ok(f)) => {
                        if let Err(reason) = self.handle_frame(&mut framed, &f.data).await {
                            // Login/protocol error: show the reason with warn (visible at the default INFO level)
                            warn!("[BNET: {}] {reason}", self.cfg.alias);
                            return Ended::Disconnected;
                        }
                    }
                    Some(Err(e)) => {
                        warn!("[BNET: {}] frame error: {e}", self.cfg.alias);
                        return Ended::Disconnected;
                    }
                    None => {
                        info!("[BNET: {}] disconnected", self.cfg.alias);
                        return Ended::Disconnected;
                    }
                },

                // BotCore commands
                cmd = self.cmd_rx.recv() => match cmd {
                    Some(BnetCommand::Shutdown) | None => return Ended::Shutdown,
                    Some(c) => self.handle_command(c),
                },

                // Flood-protection packet send
                _ = flood_tick.tick() => {
                    if self.flush_one_packet(&mut framed).await.is_err() {
                        return Ended::Disconnected;
                    }
                }

                // 60s NULL keepalive
                _ = null_tick.tick() => {
                    if framed.send(self.protocol.send_sid_null()).await.is_err() {
                        return Ended::Disconnected;
                    }
                    self.last_out_packet = Instant::now();
                }

                // Public games refresh every 3 seconds (private games are not refreshed, mirrors C++ game_base.cpp:550)
                _ = refresh_tick.tick() => {
                    let is_public = self
                        .advertised
                        .as_ref()
                        .map(|a| a.game_state != GAME_PRIVATE)
                        .unwrap_or(false);
                    if is_public {
                        self.queue_game_refresh();
                    }
                }
            }
        }
    }

    /// Determine the wait time based on "the previous packet's size" (mirrors legacy bnet.cpp:522)
    fn wait_ticks(&self) -> Duration {
        let ms = match self.last_out_packet_size {
            s if s < 10 => 1300,
            s if s < 30 => 3400,
            s if s < 50 => 3600,
            s if s < 100 => 3900,
            _ => 5500,
        };
        Duration::from_millis(ms)
    }

    async fn flush_one_packet(
        &mut self,
        framed: &mut Framed<TcpStream, FrameCodec>,
    ) -> Result<(), ()> {
        if self.out_packets.is_empty() {
            return Ok(());
        }
        if self.last_out_packet.elapsed() < self.wait_ticks() {
            return Ok(());
        }
        if self.out_packets.len() > 7 {
            warn!(
                "[BNET: {}] packet queue warning - {} packets waiting",
                self.cfg.alias,
                self.out_packets.len()
            );
        }
        if let Some(packet) = self.out_packets.pop_front() {
            if packet.get(1) == Some(&SID_STARTADVEX3) {
                debug!("[BNET: {}] sending STARTADVEX3", self.cfg.alias);
            }
            self.last_out_packet_size = packet.len();
            self.last_out_packet = Instant::now();
            if framed.send(packet).await.is_err() {
                return Err(());
            }
        }
        Ok(())
    }

    fn queue_chat(&mut self, message: &str) {
        if message.is_empty() || !self.logged_in {
            return;
        }
        let max = if self.cfg.is_pvpgn { 200 } else { 255 };
        let truncated = crate::util::util_truncate_str(message, max);
        self.out_packets
            .push_back(self.protocol.send_sid_chatcommand(truncated));
    }

    fn handle_command(&mut self, cmd: BnetCommand) {
        match cmd {
            BnetCommand::QueueChat(msg) => self.queue_chat(&msg),
            BnetCommand::QueueWhisper { to, message } => {
                self.queue_chat(&format!("/w {to} {message}"))
            }
            BnetCommand::CreateGame { game_state, game_name, map, host_counter } => {
                self.advertised = Some(AdvertisedGame { game_state, game_name, map, host_counter });
                self.queue_game_refresh();
            }
            BnetCommand::RefreshGame { game_state, game_name, map, host_counter } => {
                self.advertised = Some(AdvertisedGame { game_state, game_name, map, host_counter });
                self.queue_game_refresh();
            }
            BnetCommand::UncreateGame => {
                self.advertised = None;
                if self.logged_in {
                    // Remove any pending refresh, send STOPADV, then re-enter chat to return to the channel
                    // (mirrors C++: QueueGameUncreate() is always immediately followed by QueueEnterChat()).
                    // The server's response to SID_ENTERCHAT triggers the existing handler to resend JOINCHANNEL.
                    self.out_packets.retain(|p| p.get(1) != Some(&SID_STARTADVEX3));
                    self.out_packets.push_back(self.protocol.send_sid_stopadv());
                    self.out_packets.push_back(self.protocol.send_sid_enterchat());
                }
            }
            BnetCommand::EnterChat => {
                if self.logged_in {
                    self.out_packets.push_back(self.protocol.send_sid_enterchat());
                }
            }
            BnetCommand::JoinChannel(channel) => {
                if self.logged_in {
                    self.out_packets.push_back(self.protocol.send_sid_joinchannel(&channel));
                }
            }
            BnetCommand::Shutdown => {} // handled by the select branch
        }
    }

    /// Build STARTADVEX3 from the current advertised game, replacing the old refresh in the queue before enqueuing.
    /// Mirrors legacy CBNET::QueueGameRefresh + UnqueueGameRefreshes.
    fn queue_game_refresh(&mut self) {
        if !self.logged_in {
            return;
        }
        let adv = match &self.advertised {
            Some(a) => a.clone(),
            None => return,
        };
        let pkt = self.build_startadvex3(&adv);
        if pkt.is_empty() {
            warn!(
                "[BNET: {}] STARTADVEX3 build failed (map field length mismatch), game will not be listed",
                self.cfg.alias
            );
            return;
        }
        debug!(
            "[BNET: {}] queued STARTADVEX3 for [{}] ({} bytes)",
            self.cfg.alias,
            adv.game_name,
            pkt.len()
        );
        // Keep only the single most recent STARTADVEX3 in the queue
        self.out_packets.retain(|p| p.get(1) != Some(&SID_STARTADVEX3));
        self.out_packets.push_back(pkt);
    }

    fn build_startadvex3(&self, adv: &AdvertisedGame) -> Vec<u8> {
        let mut map_game_type = adv.map.get_map_game_type();
        map_game_type |= MAPGAMETYPE_UNKNOWN0;
        if adv.game_state == GAME_PRIVATE {
            map_game_type |= MAPGAMETYPE_PRIVATEGAME;
        }
        let mgt = map_game_type.to_le_bytes();
        let flags = adv.map.get_map_game_flags();
        // Using the map's [real size] = a non-reconnectable game.
        // When GProxy reconnect is enabled, advertise with the [192,7] (=1984) size signal;
        // upon seeing it, the GProxy client sends GPS_INIT on joining to start the reconnect handshake (mirrors C++ bnet.cpp)
        let (map_width, map_height) = if self.cfg.reconnect {
            (vec![192u8, 7], vec![192u8, 7])
        } else {
            (
                adv.map.get_map_width().clone(),
                adv.map.get_map_height().clone(),
            )
        };

        self.protocol.send_sid_startadvex3(
            adv.game_state,
            &mgt,
            &flags,
            &map_width,
            &map_height,
            &adv.game_name,
            &self.cfg.user_name,
            0,
            adv.map.get_map_path(),
            adv.map.get_map_crc(),
            adv.map.get_map_sha1(),
            adv.host_counter & 0x0FFF_FFFF, // host_counter_id = 0
        )
    }

    /// Handle one received BNCS packet. Err(reason) means the connection should be dropped.
    async fn handle_frame(
        &mut self,
        framed: &mut Framed<TcpStream, FrameCodec>,
        data: &[u8],
    ) -> Result<(), String> {
        let id = data[1];
        // Diagnostics: show non-routine packets sent by the server (excluding NULL/PING/CHATEVENT to avoid log spam)
        if !matches!(id, SID_NULL | SID_PING | SID_CHATEVENT) {
            debug!("[BNET: {}] << received packet id=0x{:02X} ({} bytes)", self.cfg.alias, id, data.len());
        }
        match id {
            SID_NULL => {}
            SID_PING => {
                let ping = self.protocol.receive_sid_ping(data);
                framed
                    .send(self.protocol.send_sid_ping(&ping))
                    .await
                    .map_err(|e| format!("send ping: {e}"))?;
            }
            SID_AUTH_INFO => {
                if !self.protocol.receive_sid_auth_info(data) {
                    return Err("bad SID_AUTH_INFO".into());
                }
                let ct = self.protocol.get_client_token().to_vec();
                let st = self.protocol.get_server_token().to_vec();
                // Clone them out to avoid tangling with the &mut self.bncs borrow
                let formula = self.protocol.get_value_string_formula().to_string();
                let ix86 = self.protocol.get_ix86_ver_file_name().to_string();

                // Compute the CD key keyinfo, and (if bot_war3path is set) use checkRevision to compute exe version/hash/info
                let ok = self.bncs.help_sid_auth_check(
                    &self.cfg.war3_path,
                    &self.cfg.cdkey_roc,
                    &self.cfg.cdkey_tft,
                    &formula,
                    &ix86,
                    &ct,
                    &st,
                    self.cfg.war3_version,
                );
                if !ok {
                    return Err("bncsutil key hash failed (check CD keys)".into());
                }

                // The config's custom exe version/hash override if provided (mirrors GHost behavior)
                if self.cfg.exe_version.len() == 4 {
                    self.bncs.set_exe_version(&self.cfg.exe_version);
                }
                if self.cfg.exe_version_hash.len() == 4 {
                    self.bncs.set_exe_version_hash(&self.cfg.exe_version_hash);
                }

                // By this point both exe version + hash must be present (from checkRevision or config)
                if self.bncs.get_exe_version().len() != 4 || self.bncs.get_exe_version_hash().len() != 4 {
                    return Err(format!(
                        "missing exe version/hash. Choose one:\
                         (a) set bot_war3path to a War3 1.28 install directory containing [Warcraft III.exe] (bot computes them automatically), or \
                         (b) manually set bnet_custom_exeversion and bnet_custom_exeversionhash (4 numbers each).\
                         current version={:?} hash={:?}",
                        self.bncs.get_exe_version(),
                        self.bncs.get_exe_version_hash()
                    ));
                }
                info!(
                    "[BNET: {}] exe version = {:?}, hash = {:?}",
                    self.cfg.alias,
                    self.bncs.get_exe_version(),
                    self.bncs.get_exe_version_hash()
                );

                let packet = self.protocol.send_sid_auth_check(
                    self.cfg.tft,
                    self.protocol.get_client_token(),
                    self.bncs.get_exe_version(),
                    self.bncs.get_exe_version_hash(),
                    self.bncs.get_key_info_roc(),
                    self.bncs.get_key_info_tft(),
                    self.bncs.get_exe_info(),
                    &self.cfg.user_name,
                );
                if packet.is_empty() {
                    return Err("failed to build SID_AUTH_CHECK".into());
                }
                framed.send(packet).await.map_err(|e| format!("send auth_check: {e}"))?;
            }
            SID_AUTH_CHECK => {
                if !self.protocol.receive_sid_auth_check(data) {
                    let ks = crate::util::util_byte_array_to_u32(self.protocol.get_key_state(), false, 0);
                    let reason = match ks {
                        KR_ROC_KEY_IN_USE => "ROC CD key in use",
                        KR_TFT_KEY_IN_USE => "TFT CD key in use",
                        KR_OLD_GAME_VERSION => "game version too old",
                        KR_INVALID_VERSION => "game version invalid",
                        _ => "CD keys not accepted",
                    };
                    return Err(format!("logon failed - {reason}"));
                }
                info!("[BNET: {}] cd keys accepted", self.cfg.alias);
                self.bncs.help_sid_auth_accountlogon();
                framed
                    .send(
                        self.protocol
                            .send_sid_auth_accountlogon(self.bncs.get_client_key(), &self.cfg.user_name),
                    )
                    .await
                    .map_err(|e| format!("send accountlogon: {e}"))?;
            }
            SID_AUTH_ACCOUNTLOGON => {
                if !self.protocol.receive_sid_auth_accountlogon(data) {
                    return Err("logon failed - invalid username".into());
                }
                info!("[BNET: {}] username [{}] accepted", self.cfg.alias, self.cfg.user_name);
                let proof = if self.cfg.is_pvpgn {
                    self.bncs.help_pvpg_password_hash(&self.cfg.user_password);
                    self.bncs.get_pvpg_password_hash().to_vec()
                } else {
                    // The official bnet's SRP M1 is not yet ported (see bncsutilinterface)
                    self.bncs.help_sid_auth_accountlogonproof(
                        self.protocol.get_salt(),
                        self.protocol.get_server_public_key(),
                    );
                    self.bncs.get_m1().to_vec()
                };
                framed
                    .send(self.protocol.send_sid_auth_accountlogonproof(&proof))
                    .await
                    .map_err(|e| format!("send logonproof: {e}"))?;
            }
            SID_AUTH_ACCOUNTLOGONPROOF => {
                if !self.protocol.receive_sid_auth_accountlogonproof(data) {
                    return Err("logon failed - invalid password".into());
                }
                info!("[BNET: {}] logon successful", self.cfg.alias);
                self.logged_in = true;
                self.emit(BnetEvent::LoggedIn).await;
                // Send netgameport / enterchat / friends / clan
                for p in [
                    self.protocol.send_sid_netgameport(self.cfg.host_port),
                    self.protocol.send_sid_enterchat(),
                    self.protocol.send_sid_friendlist(),
                    self.protocol.send_sid_clanmemberlist(),
                ] {
                    framed.send(p).await.map_err(|e| format!("post-login send: {e}"))?;
                }
            }
            SID_ENTERCHAT => {
                if self.protocol.receive_sid_enterchat(data) {
                    info!(
                        "[BNET: {}] joining channel [{}]",
                        self.cfg.alias, self.cfg.first_channel
                    );
                    framed
                        .send(self.protocol.send_sid_joinchannel(&self.cfg.first_channel))
                        .await
                        .map_err(|e| format!("send joinchannel: {e}"))?;
                }
            }
            SID_CHATEVENT => {
                if let Some(ev) = self.protocol.receive_sid_chatevent(data) {
                    self.handle_chat_event(ev).await;
                }
            }
            SID_STARTADVEX3 => {
                if self.protocol.receive_sid_startadvex3(data) {
                    self.emit(BnetEvent::GameRefreshed).await;
                } else {
                    self.emit(BnetEvent::GameRefreshFailed).await;
                }
            }
            SID_CHECKAD => {}
            SID_FRIENDLIST => {
                let _friends = self.protocol.receive_sid_friendlist(data);
            }
            SID_CLANMEMBERLIST => {
                let _clan = self.protocol.receive_sid_clanmemberlist(data);
            }
            other => debug!("[BNET: {}] unhandled packet id 0x{other:02X}", self.cfg.alias),
        }
        Ok(())
    }

    async fn handle_chat_event(&self, ev: IncomingChatEvent) {
        let whisper = ev.chat_event == EID_WHISPER;

        match ev.chat_event {
            EID_WHISPER | EID_TALK => {
                if whisper {
                    info!("[WHISPER: {}] [{}] {}", self.cfg.alias, ev.user, ev.message);
                } else {
                    info!("[LOCAL: {}] [{}] {}", self.cfg.alias, ev.user, ev.message);
                }

                // spoofcheck: whisper "s" / "sc" / "spoofcheck" (GProxy sends it automatically when joining a game)
                // The whisper is authenticated by the PVPGN server, so the name cannot be forged (mirrors the spoof check in C++ bnet.cpp)
                if whisper {
                    let m = ev.message.trim().to_lowercase();
                    if m == "s" || m == "sc" || m == "spoofcheck" {
                        self.emit(BnetEvent::SpoofCheck { user: ev.user }).await;
                        return;
                    }
                }

                // Command dispatch
                if ev.message.starts_with(&self.cfg.command_trigger) {
                    let (command, payload) = get_command_and_payload(&ev.message);
                    self.emit(BnetEvent::Command {
                        user: ev.user,
                        command,
                        payload,
                        whisper,
                    })
                    .await;
                } else {
                    self.emit(BnetEvent::ChatEvent {
                        user: ev.user,
                        message: ev.message,
                        whisper,
                    })
                    .await;
                }
            }
            EID_INFO | EID_ERROR => {
                debug!("[BNET: {}] info/error: {}", self.cfg.alias, ev.message);
            }
            EID_CHANNEL => {
                info!("[BNET: {}] joined channel [{}]", self.cfg.alias, ev.message);
            }
            _ => {}
        }
    }
}
