//! GameActor: a tokio actor for one game (lobby → in progress).
//!
//! Replaces C++ CBaseGame/CGame. One task per game, owning its own slots/players,
//! so no locks are needed. Each player connection has its own read/write task (net::conn), and events flow one-way into GameActor.
//!
//! Covers the full game lifecycle: lobby (joins, slots, chat, map downloads),
//! start countdown, in-game action loop, GProxy reconnects, and replay recording.

pub mod actor;

pub use actor::{spawn, GameConfig, GameHandle};
