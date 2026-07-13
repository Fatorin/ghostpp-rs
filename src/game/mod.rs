//! GameActor: a tokio actor for one game (lobby → in progress) (ROADMAP Phase 4/5).
//!
//! Replaces C++ CBaseGame/CGame. One task per game, owning its own slots/players,
//! so no locks are needed. Each player connection has its own read/write task (net::conn), and events flow one-way into GameActor.
//!
//! Stage 4a scope: the lobby — player joins, slot display, virtual host, chat forwarding, 5s ping.
//! Stage 4b (later): map downloads, slot operation commands, start countdown.

pub mod actor;

pub use actor::{spawn, GameConfig, GameHandle};
