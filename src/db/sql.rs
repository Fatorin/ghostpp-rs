//! SQL backend (shared implementation for SQLite + PostgreSQL).
//!
//! The query syntax is fully shared between the two: `$N` placeholders, `ON CONFLICT`, and `RETURNING id`
//! are all supported by modern SQLite (sqlx bundles 3.4x) and PostgreSQL.
//! The only difference is the auto-increment primary key DDL (AUTOINCREMENT vs BIGSERIAL), injected via the `{ID}` placeholder.
//! The method bodies are expanded over the two pool types with a macro (monomorphized, no generic bound).

use async_trait::async_trait;
use sqlx::Row;

use super::{BanRecord, DbError, DbResult, GamePlayerRecord, GameRecord, GhostDb};

fn e(err: sqlx::Error) -> DbError {
    DbError::Backend(err.to_string())
}

/// Shared schema (`{ID}` = each backend's auto-increment primary key column definition)
const SCHEMA: [&str; 4] = [
    "CREATE TABLE IF NOT EXISTS admins (
        server TEXT NOT NULL,
        name   TEXT NOT NULL,
        PRIMARY KEY (server, name)
    )",
    "CREATE TABLE IF NOT EXISTS bans (
        server   TEXT NOT NULL,
        name     TEXT NOT NULL,
        ip       TEXT NOT NULL DEFAULT '',
        date     TEXT NOT NULL DEFAULT '',
        gamename TEXT NOT NULL DEFAULT '',
        admin    TEXT NOT NULL DEFAULT '',
        reason   TEXT NOT NULL DEFAULT '',
        PRIMARY KEY (server, name)
    )",
    "CREATE TABLE IF NOT EXISTS games (
        {ID},
        server    TEXT NOT NULL,
        map       TEXT NOT NULL,
        datetime  TEXT NOT NULL,
        gamename  TEXT NOT NULL,
        ownername TEXT NOT NULL,
        duration  BIGINT NOT NULL
    )",
    // Mirrors the gameplayers of C++ ghost.sql, with an extra leftcode column
    // ("left" is a reserved word in PostgreSQL, so it is quoted; SQLite accepts it too)
    "CREATE TABLE IF NOT EXISTS gameplayers (
        {ID},
        botid        BIGINT NOT NULL DEFAULT 1,
        gameid       BIGINT NOT NULL,
        name         TEXT NOT NULL,
        ip           TEXT NOT NULL DEFAULT '',
        spoofed      BIGINT NOT NULL DEFAULT 0,
        reserved     BIGINT NOT NULL DEFAULT 0,
        loadingtime  BIGINT NOT NULL DEFAULT 0,
        \"left\"       BIGINT NOT NULL DEFAULT 0,
        leftreason   TEXT NOT NULL DEFAULT '',
        leftcode     BIGINT NOT NULL DEFAULT 0,
        team         BIGINT NOT NULL DEFAULT 0,
        colour       BIGINT NOT NULL DEFAULT 0,
        spoofedrealm TEXT NOT NULL DEFAULT ''
    )",
];

// ---- Shared queries (syntax identical for both backends) ----
const Q_ADMIN_ADD: &str =
    "INSERT INTO admins (server, name) VALUES ($1, $2) ON CONFLICT DO NOTHING";
const Q_ADMIN_DEL: &str = "DELETE FROM admins WHERE server = $1 AND name = $2";
const Q_ADMIN_CHECK: &str = "SELECT 1 FROM admins WHERE server = $1 AND name = $2";
const Q_ADMIN_LIST: &str = "SELECT name FROM admins WHERE server = $1 ORDER BY name";

const Q_BAN_ADD: &str = "INSERT INTO bans (server, name, ip, date, gamename, admin, reason)
     VALUES ($1, $2, $3, $4, $5, $6, $7)
     ON CONFLICT (server, name) DO UPDATE SET
       ip = EXCLUDED.ip, date = EXCLUDED.date, gamename = EXCLUDED.gamename,
       admin = EXCLUDED.admin, reason = EXCLUDED.reason";
const Q_BAN_DEL: &str = "DELETE FROM bans WHERE server = $1 AND name = $2";
const Q_BAN_SELECT: &str = "SELECT server, name, ip, date, gamename, admin, reason FROM bans";
const Q_GAME_ADD: &str =
    "INSERT INTO games (server, map, datetime, gamename, ownername, duration)
     VALUES ($1, $2, $3, $4, $5, $6) RETURNING id";
const Q_PLAYER_ADD: &str = "INSERT INTO gameplayers
     (botid, gameid, name, ip, spoofed, reserved, loadingtime, \"left\", leftreason, leftcode, team, colour, spoofedrealm)
     VALUES (1, $1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12)";

/// Expand the full GhostDb implementation for a given pool type (the method bodies are verbatim identical for both backends)
macro_rules! impl_sql_ghost_db {
    ($name:ident, $pool:ty) => {
        pub struct $name {
            pool: $pool,
            info: String,
        }

        impl $name {
            /// Create tables + wrap (id_column = that backend's auto-increment primary key DDL)
            async fn init(pool: $pool, info: String, id_column: &str) -> DbResult<Self> {
                for ddl in SCHEMA {
                    let ddl = ddl.replace("{ID}", id_column);
                    sqlx::query(&ddl).execute(&pool).await.map_err(e)?;
                }
                Ok(Self { pool, info })
            }
        }

        #[async_trait]
        impl GhostDb for $name {
            fn description(&self) -> String {
                self.info.clone()
            }

            async fn admin_add(&self, server: &str, name: &str) -> DbResult<bool> {
                let r = sqlx::query(Q_ADMIN_ADD)
                    .bind(server)
                    .bind(name.to_lowercase())
                    .execute(&self.pool)
                    .await
                    .map_err(e)?;
                Ok(r.rows_affected() > 0)
            }

            async fn admin_remove(&self, server: &str, name: &str) -> DbResult<bool> {
                let r = sqlx::query(Q_ADMIN_DEL)
                    .bind(server)
                    .bind(name.to_lowercase())
                    .execute(&self.pool)
                    .await
                    .map_err(e)?;
                Ok(r.rows_affected() > 0)
            }

            async fn admin_check(&self, server: &str, name: &str) -> DbResult<bool> {
                let row = sqlx::query(Q_ADMIN_CHECK)
                    .bind(server)
                    .bind(name.to_lowercase())
                    .fetch_optional(&self.pool)
                    .await
                    .map_err(e)?;
                Ok(row.is_some())
            }

            async fn admin_list(&self, server: &str) -> DbResult<Vec<String>> {
                let rows = sqlx::query(Q_ADMIN_LIST)
                    .bind(server)
                    .fetch_all(&self.pool)
                    .await
                    .map_err(e)?;
                Ok(rows.iter().map(|r| r.get::<String, _>(0)).collect())
            }

            async fn ban_add(&self, ban: &BanRecord) -> DbResult<bool> {
                let r = sqlx::query(Q_BAN_ADD)
                    .bind(&ban.server)
                    .bind(ban.name.to_lowercase())
                    .bind(&ban.ip)
                    .bind(&ban.date)
                    .bind(&ban.game_name)
                    .bind(&ban.admin)
                    .bind(&ban.reason)
                    .execute(&self.pool)
                    .await
                    .map_err(e)?;
                Ok(r.rows_affected() > 0)
            }

            async fn ban_remove(&self, server: &str, name: &str) -> DbResult<bool> {
                let r = sqlx::query(Q_BAN_DEL)
                    .bind(server)
                    .bind(name.to_lowercase())
                    .execute(&self.pool)
                    .await
                    .map_err(e)?;
                Ok(r.rows_affected() > 0)
            }

            async fn ban_check(
                &self,
                server: &str,
                name: &str,
                ip: &str,
            ) -> DbResult<Option<BanRecord>> {
                let row = if ip.is_empty() {
                    sqlx::query(&format!("{Q_BAN_SELECT} WHERE server = $1 AND name = $2"))
                        .bind(server)
                        .bind(name.to_lowercase())
                        .fetch_optional(&self.pool)
                        .await
                } else {
                    sqlx::query(&format!(
                        "{Q_BAN_SELECT} WHERE server = $1 AND (name = $2 OR ip = $3)"
                    ))
                    .bind(server)
                    .bind(name.to_lowercase())
                    .bind(ip)
                    .fetch_optional(&self.pool)
                    .await
                }
                .map_err(e)?;

                Ok(row.map(|r| BanRecord {
                    server: r.get(0),
                    name: r.get(1),
                    ip: r.get(2),
                    date: r.get(3),
                    game_name: r.get(4),
                    admin: r.get(5),
                    reason: r.get(6),
                }))
            }

            async fn ban_list(&self, server: &str) -> DbResult<Vec<BanRecord>> {
                let rows =
                    sqlx::query(&format!("{Q_BAN_SELECT} WHERE server = $1 ORDER BY name"))
                        .bind(server)
                        .fetch_all(&self.pool)
                        .await
                        .map_err(e)?;
                Ok(rows
                    .iter()
                    .map(|r| BanRecord {
                        server: r.get(0),
                        name: r.get(1),
                        ip: r.get(2),
                        date: r.get(3),
                        game_name: r.get(4),
                        admin: r.get(5),
                        reason: r.get(6),
                    })
                    .collect())
            }

            async fn game_add(
                &self,
                game: &GameRecord,
                players: &[GamePlayerRecord],
            ) -> DbResult<u64> {
                let row = sqlx::query(Q_GAME_ADD)
                    .bind(&game.server)
                    .bind(&game.map)
                    .bind(&game.datetime)
                    .bind(&game.game_name)
                    .bind(&game.owner_name)
                    .bind(game.duration as i64)
                    .fetch_one(&self.pool)
                    .await
                    .map_err(e)?;
                let game_id: i64 = row.get(0);

                for p in players {
                    sqlx::query(Q_PLAYER_ADD)
                        .bind(game_id)
                        .bind(p.name.to_lowercase())
                        .bind(&p.ip)
                        .bind(p.spoofed as i64)
                        .bind(p.reserved as i64)
                        .bind(p.loading_time_ms as i64)
                        .bind(p.left_secs as i64)
                        .bind(&p.left_reason)
                        .bind(p.left_code as i64)
                        .bind(p.team as i64)
                        .bind(p.colour as i64)
                        .bind(&p.spoofed_realm)
                        .execute(&self.pool)
                        .await
                        .map_err(e)?;
                }
                Ok(game_id as u64)
            }
        }
    };
}

impl_sql_ghost_db!(SqliteDb, sqlx::SqlitePool);
impl_sql_ghost_db!(PgDb, sqlx::PgPool);

impl SqliteDb {
    pub async fn connect(file: &str) -> DbResult<Self> {
        let opts = sqlx::sqlite::SqliteConnectOptions::new()
            .filename(file)
            .create_if_missing(true);
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(4)
            .connect_with(opts)
            .await
            .map_err(e)?;
        Self::init(
            pool,
            format!("SQLite [{file}]"),
            "id INTEGER PRIMARY KEY AUTOINCREMENT",
        )
        .await
    }
}

impl PgDb {
    pub async fn connect(url: &str) -> DbResult<Self> {
        let pool = sqlx::postgres::PgPoolOptions::new()
            .max_connections(4)
            .connect(url)
            .await
            .map_err(e)?;
        Self::init(pool, "PostgreSQL".to_string(), "id BIGSERIAL PRIMARY KEY").await
    }
}
