//! GHost++ Rust — entry point (ROADMAP Phase 1: tokio event loop replaces the 50ms select polling).
//!
//! See ghostpp-rs/ROADMAP.md §2 for the architecture:
//!   main → BotCore (event loop) + listener task + console task + BnetActor
//!   Phase 4/5 adds GameActor.

pub mod bncsutil;
pub mod bot;
pub mod core;
pub mod db;
pub mod error;
pub mod game;
pub mod lang;
pub mod net;
pub mod util;

use std::sync::Arc;

use config::Config;
use tracing::info;
use tracing_subscriber::EnvFilter;

use crate::bot::{BotConfig, BotCore};
use crate::core::GameMap;
use crate::error::GhostError;

#[tokio::main]
async fn main() -> Result<(), GhostError> {
    // RUST_LOG can override; defaults to info
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")))
        .init();

    info!("[GHOST] starting up");

    let settings = Config::builder()
        .add_source(config::File::with_name("config/ghost"))
        .add_source(config::File::with_name("config/bnet"))
        .add_source(config::File::with_name("config/map"))
        .build()?;

    // i18n: load the user-visible message catalog (mirrors C++ language.cfg).
    // bot_language points to the TOML language file; falls back to built-in English defaults when missing/broken.
    lang::load(
        &settings
            .get_string("bot_language")
            .unwrap_or_else(|_| "config/language.toml".into()),
    );

    let bot_config = BotConfig::load(&settings);

    // Load the default map (config/map.toml); when invalid, !pub hosting will be refused
    let mut map = GameMap::new();
    map.load(&settings);
    if map.is_valid {
        info!("[GHOST] map loaded and valid: {}", map.get_map_path());
    } else {
        info!("[GHOST] warning - default map is not valid; hosting will be refused until fixed");
    }
    let map = Arc::new(map);

    // Connect to the database (the scheme of db_url selects the backend; abort on failure to avoid silently running without ban checks)
    let db_url = settings
        .get_string("db_url")
        .unwrap_or_else(|_| "sqlite://ghost.db".into());
    let database = db::connect(&db_url)
        .await
        .map_err(|e| GhostError::Other(format!("database connection failed: {e}")))?;
    info!("[GHOST] database connected: {}", database.description());

    let (mut core, event_tx) = BotCore::new(bot_config, map, database);

    // Load and spawn the battle.net / PVPGN connection
    // The config currently uses a single bnet_ section; multi-server support awaits a config format extension
    if let Some(bnet_cfg) = bot::BnetConfig::load(&settings) {
        let server_id = 0usize;
        info!("[GHOST] found battle.net connection for {}", bnet_cfg.alias);
        let tx = bot::bnet::spawn(server_id, bnet_cfg.clone(), event_tx.clone());
        core.add_bnet(server_id, tx, bnet_cfg);
    } else {
        info!("[GHOST] no valid battle.net connection configured");
    }

    // stdin commands (exit / quit / say <msg>)
    let _console = bot::console::spawn(event_tx.clone());

    // Centralized listener on host_port (ROADMAP §2: BotCore routes to the lobby GameActor)
    let _listener = bot::listener::spawn(
        &core.config().bind_address.clone(),
        core.config().host_port,
        event_tx.clone(),
    )
    .await?;

    // GProxy++ reconnect listener (when bot_reconnect is enabled)
    let _reconnect_listener = if core.config().reconnect {
        Some(
            bot::listener::spawn_reconnect(
                &core.config().bind_address.clone(),
                core.config().reconnect_port,
                event_tx.clone(),
            )
            .await?,
        )
    } else {
        None
    };

    core.run().await;

    Ok(())
}
