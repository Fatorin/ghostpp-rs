//! Inter-actor message types.
//! Principle: events go up (XxxEvent → BotEvent → BotCore), commands go down (XxxCommand).
//! Thoroughly replaces the C++ `m_Game->m_GHost->...` back-pointer chain.
#![allow(dead_code)]

use std::net::SocketAddr;
use std::sync::Arc;

use tokio::net::TcpStream;

use crate::core::GameMap;
use crate::net::conn::ConnEvent;

/// All events sent into the BotCore main loop
#[derive(Debug)]
pub enum BotEvent {
    /// Listener received a new TCP connection (routed to the current lobby's GameActor)
    NewConnection { stream: TcpStream, peer: SocketAddr },

    /// The reconnect listener received a valid GPS_RECONNECT handshake, pending routing to the corresponding game
    GProxyReconnect {
        stream: TcpStream,
        pid: u8,
        key: u32,
        last_packet: u32,
    },

    /// Player connection event (fallback path when no game exists; normally handled inside GameActor)
    Conn(ConnEvent),

    /// An event from some battle.net connection (server_id = the index in the config file)
    Bnet { server_id: usize, event: BnetEvent },

    /// An event from some game (identified by host_counter)
    Game { host_counter: u32, event: GameEvent },

    /// A line of stdin input (originally the ghost.cpp input thread)
    ConsoleInput(String),
}

/// Events reported by BnetActor
#[derive(Debug)]
pub enum BnetEvent {
    Connected,
    LoggedIn,
    Disconnected,
    /// Entered a channel
    JoinedChannel(String),
    /// Channel/whisper chat (chat event details already stripped out)
    ChatEvent { user: String, message: String, whisper: bool },
    /// A command starting with command_trigger
    Command { user: String, command: String, payload: String, whisper: bool },
    /// Whisper "sc": spoofcheck (the whisper is server-authenticated, so user is the real account; GProxy sends it automatically on joining a game)
    SpoofCheck { user: String },
    /// STARTADVEX3 succeeded
    GameRefreshed,
    /// STARTADVEX3 failed (mirrors CGHost::EventBNETGameRefreshFailed)
    GameRefreshFailed,
}

/// Commands issued by BotCore to BnetActor
#[derive(Debug)]
pub enum BnetCommand {
    /// Enqueue chat/command (goes through the flood-protection queue)
    QueueChat(String),
    QueueWhisper { to: String, message: String },
    /// Join the specified channel (!channel)
    JoinChannel(String),
    /// Create a game (send STARTADVEX3 and leave the channel)
    CreateGame {
        game_state: u8,
        game_name: String,
        map: Arc<GameMap>,
        host_counter: u32,
    },
    /// Periodic refresh (public game, every 3 seconds while there is an open slot)
    RefreshGame {
        game_state: u8,
        game_name: String,
        map: Arc<GameMap>,
        host_counter: u32,
    },
    /// Uncreate the game (SID_STOPADV)
    UncreateGame,
    /// Return to the chat channel
    EnterChat,
    /// End this connection task
    Shutdown,
}

/// Events reported by GameActor
#[derive(Debug)]
pub enum GameEvent {
    PlayerJoined { name: String },
    PlayerLeft { name: String },
    /// Lobby chat (BotCore parses commands from this; the name comes from the server-side pid mapping, not the packet content)
    PlayerChat {
        name: String,
        message: String,
        /// Passed spoofcheck (identity verified via whisper sc)
        spoofed: bool,
        /// The verified realm (server host)
        spoofed_realm: String,
    },
    /// Player enabled GProxy and registered a reconnect key (BotCore records key→host_counter for reconnect routing)
    GProxyRegistered { key: u32 },
    /// Countdown finished, loading started (triggers bnet to stop refreshing)
    GameStarted,
    /// Game ended (everyone left); includes game and player records for BotCore to write to the db
    GameEnded {
        record: crate::db::GameRecord,
        players: Vec<crate::db::GamePlayerRecord>,
    },
    /// The game task has ended, BotCore should remove the record (mirrors EventGameDeleted)
    Deleted,
}

/// Commands issued by BotCore to GameActor
#[derive(Debug)]
pub enum GameCommand {
    /// New player connection routed in by the Listener
    NewConnection { stream: TcpStream, peer: SocketAddr },
    /// GProxy reconnect (the reconnect listener has already read the GPS_RECONNECT handshake)
    GProxyReconnect {
        stream: TcpStream,
        pid: u8,
        key: u32,
        last_packet: u32,
    },
    /// Broadcast a message to the whole game (!saygames etc.)
    Say(String),
    /// Open a slot (0-based)
    OpenSlot(usize),
    /// Close a slot (0-based)
    CloseSlot(usize),
    /// Swap two slots (0-based)
    SwapSlots(usize, usize),
    /// Kick by name (partial match)
    Kick(String),
    /// Begin the start countdown
    Start,
    /// Adjust/query the action send interval (!latency; None = query, Some(n) = set, 5~500 ms)
    SetLatency(Option<u32>),
    /// Adjust/query the lag tolerance batch count (!synclimit; None = query, Some(n) = set)
    SetSyncLimit(Option<u32>),
    /// spoofcheck passed (name verified its identity via whisper on realm)
    SpoofCheck { name: String, realm: String },
    /// Generic in-game admin command (BotCore has verified permissions; GameActor parses and executes it itself).
    /// requester = the name of the player who issued the command (GameActor uses it to find the pid for a private reply).
    AdminCommand {
        requester: String,
        command: String,
        payload: String,
    },
    /// Close this game
    Close,
}
