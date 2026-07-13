//! Phase 6: database abstraction layer (mirrors the C++ CGHostDB base class + backend subclasses).
//!
//! Design: a domain-method trait (`GhostDb`) + per-backend implementations, with a single `db_url` switching by scheme:
//! - `sqlite://ghost.db` (default; a bare filename also works): sqlx + SQLite, zero-config single file
//! - `postgres://user:pass@host/db`: sqlx + PostgreSQL
//!
//! Names are always stored/compared in lowercase (mirrors C++'s case-insensitive behavior).

use std::sync::Arc;

use async_trait::async_trait;

mod sql;

pub type DbResult<T> = Result<T, DbError>;

#[derive(Debug, thiserror::Error)]
pub enum DbError {
    #[error("db backend error: {0}")]
    Backend(String),
    #[error("unsupported db_url scheme [{0}] (supported: sqlite:// or postgres://)")]
    UnsupportedBackend(String),
}

/// A single ban record (mirrors C++ CDBBan)
#[derive(Debug, Clone, Default)]
pub struct BanRecord {
    pub server: String,
    pub name: String,
    pub ip: String,
    /// Creation time (display string)
    pub date: String,
    pub game_name: String,
    pub admin: String,
    pub reason: String,
}

/// A single game record (mirrors the C++ games table, trimmed down)
#[derive(Debug, Clone, Default)]
pub struct GameRecord {
    pub server: String,
    pub map: String,
    pub datetime: String,
    pub game_name: String,
    pub owner_name: String,
    /// Game duration (seconds, counted from when everyone finished loading)
    pub duration: u32,
}

/// One player's participation record (mirrors the full C++ gameplayers table columns + an extra retained leftcode)
#[derive(Debug, Clone, Default)]
pub struct GamePlayerRecord {
    pub name: String,
    pub ip: String,
    /// Passed spoofcheck (always 0 until implemented)
    pub spoofed: u8,
    /// Reserved-slot player (always 0 until implemented)
    pub reserved: u8,
    /// Time taken to load the map (ms)
    pub loading_time_ms: u32,
    /// Time of leaving (seconds counted from game start; 0 for leaving in the lobby)
    pub left_secs: u32,
    /// Reason for leaving (human-readable, mirrors the C++ language.cfg style)
    pub left_reason: String,
    /// Leave code (W3GS PLAYERLEAVE_*, retained as extra)
    pub left_code: u32,
    pub team: u8,
    pub colour: u8,
    /// The realm the player belongs to (filled with the bnet server before spoofcheck)
    pub spoofed_realm: String,
}

/// Database domain interface (mirrors C++ CGHostDB's AdminAdd/BanAdd/GameAdd, etc.)
#[async_trait]
pub trait GhostDb: Send + Sync {
    /// Backend description (for logging)
    fn description(&self) -> String;

    async fn admin_add(&self, server: &str, name: &str) -> DbResult<bool>;
    async fn admin_remove(&self, server: &str, name: &str) -> DbResult<bool>;
    async fn admin_check(&self, server: &str, name: &str) -> DbResult<bool>;
    async fn admin_list(&self, server: &str) -> DbResult<Vec<String>>;

    async fn ban_add(&self, ban: &BanRecord) -> DbResult<bool>;
    async fn ban_remove(&self, server: &str, name: &str) -> DbResult<bool>;
    /// Check by name or IP (if ip is an empty string, only the name is queried)
    async fn ban_check(&self, server: &str, name: &str, ip: &str) -> DbResult<Option<BanRecord>>;
    async fn ban_list(&self, server: &str) -> DbResult<Vec<BanRecord>>;

    /// Write a game and its player roster, returning the game id (backends without auto-increment ids, e.g. Redis, return a serial number)
    async fn game_add(&self, game: &GameRecord, players: &[GamePlayerRecord]) -> DbResult<u64>;
}

/// Establish a backend connection from a single `db_url`, with the scheme selecting the backend:
/// - `sqlite://ghost.db` (or a bare file path with no scheme) → SQLite
/// - `postgres://user:pass@host/dbname` → PostgreSQL
pub async fn connect(db_url: &str) -> DbResult<Arc<dyn GhostDb>> {
    let url = db_url.trim();
    if url.starts_with("postgres://") || url.starts_with("postgresql://") {
        Ok(Arc::new(sql::PgDb::connect(url).await?))
    } else if let Some(file) = url
        .strip_prefix("sqlite://")
        .or_else(|| url.strip_prefix("sqlite:"))
    {
        Ok(Arc::new(sql::SqliteDb::connect(file).await?))
    } else if url.contains("://") {
        Err(DbError::UnsupportedBackend(url.to_string()))
    } else {
        // No scheme: treat it as a SQLite file path
        Ok(Arc::new(sql::SqliteDb::connect(url).await?))
    }
}
