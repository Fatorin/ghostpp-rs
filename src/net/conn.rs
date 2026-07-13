//! Player connection template (ROADMAP §2 PlayerConn).
//!
//! Each TCP connection is split into two tasks:
//! - **read task**: `FramedRead` + [`FrameCodec`] deframes → each [`Frame`] is sent as
//!   [`ConnEvent::Frame`] into the owner's (GameActor / temporarily BotCore in Phase 1) mpsc.
//!   Receiving no data for 30 seconds is treated as a disconnect (mirrors the 30s timeout in C++ gameplayer.cpp;
//!   the lobby pings every 5 seconds, and an in-game client sends at least one keepalive per second, so there is always data within 30 seconds).
//! - **write task**: mpsc receives bytes → `write_all`. The owner merely drops bytes into the channel
//!   and is never blocked by a slow client.
//!
//! The owner holds a [`ConnHandle`]; dropping the handle ⇒ the write channel closes ⇒ the write task ends
//! ⇒ the socket write side closes. The read task emits [`ConnEvent::Closed`] on EOF / timeout / deframing error.
//! This design converts C++'s `player → game` back-pointer calls entirely into a one-way event stream.

use std::net::SocketAddr;
use std::time::Duration;

use futures::StreamExt;
use tokio::io::AsyncWriteExt;
use tokio::net::TcpStream;
use tokio::sync::mpsc;
use tokio::time::timeout;
use tokio_util::codec::FramedRead;
use tracing::{debug, warn};

use super::codec::{Frame, FrameCodec};

/// Connection identifier (assigned by the owner, unique within the process)
pub type ConnId = u64;

/// By default, no data for 30 seconds is treated as a disconnect
pub const DEFAULT_RECV_TIMEOUT: Duration = Duration::from_secs(30);

/// Write channel depth: exceeding it means the client cannot keep up; send applies backpressure as it keeps piling up
const WRITE_QUEUE_DEPTH: usize = 256;

#[derive(Debug)]
pub enum ConnEvent {
    /// Received a complete packet
    Frame(ConnId, Frame),
    /// Connection ended (no further events for this ConnId afterward)
    Closed(ConnId, CloseReason),
}

#[derive(Debug)]
pub enum CloseReason {
    /// Peer closed normally
    Eof,
    /// No data at all beyond the recv timeout
    Timeout,
    /// Deframing failure or IO error
    Error(String),
    /// The owner deliberately dropped the ConnHandle
    Dropped,
}

/// The connection's control handle: send data, look up the peer address. Dropping it closes the write side.
#[derive(Debug, Clone)]
pub struct ConnHandle {
    pub id: ConnId,
    pub peer: SocketAddr,
    write_tx: mpsc::Sender<Vec<u8>>,
}

impl ConnHandle {
    /// Send complete packet bytes (the output of the `send_*` family of functions).
    /// Returns false if the connection is already closed.
    pub async fn send(&self, data: Vec<u8>) -> bool {
        self.write_tx.send(data).await.is_ok()
    }

    /// Non-blocking queued version: returns false if the queue is full or the connection is closed (does not wait)
    pub fn try_send(&self, data: Vec<u8>) -> bool {
        self.write_tx.try_send(data).is_ok()
    }
}

/// Start the read/write tasks for an already-accepted TCP connection.
///
/// * `codec` — player connections use `FrameCodec::w3gs()` (accepts W3GS+GPS)
/// * `event_tx` — events are sent to the owner; the read task ends naturally when the owner drops the receiver
pub fn spawn(
    id: ConnId,
    stream: TcpStream,
    codec: FrameCodec,
    event_tx: mpsc::Sender<ConnEvent>,
    recv_timeout: Duration,
) -> ConnHandle {
    let peer = stream
        .peer_addr()
        .unwrap_or_else(|_| SocketAddr::from(([0, 0, 0, 0], 0)));

    // Mirrors C++'s tcp_nodelay setting; failure is not fatal
    if let Err(e) = stream.set_nodelay(true) {
        debug!(conn = id, "set_nodelay failed: {e}");
    }

    let (read_half, mut write_half) = stream.into_split();
    let (write_tx, mut write_rx) = mpsc::channel::<Vec<u8>>(WRITE_QUEUE_DEPTH);

    // ---- write task ----
    tokio::spawn(async move {
        while let Some(data) = write_rx.recv().await {
            if let Err(e) = write_half.write_all(&data).await {
                debug!(conn = id, "write error: {e}");
                break;
            }
        }

        // Channel closed (all ConnHandles dropped) or a write error: close the write side
        let _ = write_half.shutdown().await;
    });

    // ---- read task ----
    tokio::spawn(async move {
        let mut framed = FramedRead::new(read_half, codec);

        let reason = loop {
            match timeout(recv_timeout, framed.next()).await {
                // A complete packet
                Ok(Some(Ok(frame))) => {
                    if event_tx.send(ConnEvent::Frame(id, frame)).await.is_err() {
                        // The owner is gone, wrap up silently
                        break CloseReason::Dropped;
                    }
                }
                // Deframing error / IO error
                Ok(Some(Err(e))) => {
                    warn!(conn = id, %peer, "frame error: {e}");
                    break CloseReason::Error(e.to_string());
                }
                // EOF
                Ok(None) => break CloseReason::Eof,
                // recv timeout
                Err(_) => break CloseReason::Timeout,
            }
        };

        let _ = event_tx.send(ConnEvent::Closed(id, reason)).await;
    });

    ConnHandle { id, peer, write_tx }
}
