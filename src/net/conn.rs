//! Player connection template (PlayerConn).
//!
//! Each TCP connection is split into two tasks:
//! - **read task**: `FramedRead` + [`FrameCodec`] deframes → each [`Frame`] is sent as
//!   [`ConnEvent::Frame`] into the owner's (GameActor, or BotCore as fallback) mpsc.
//!   Receiving no data for 30 seconds is treated as a disconnect (mirrors the 30s timeout in C++ gameplayer.cpp;
//!   the lobby pings every 5 seconds, and an in-game client sends at least one keepalive per second, so there is always data within 30 seconds).
//! - **write task**: mpsc receives bytes → `write_all`. The owner merely drops bytes into the channel
//!   and is never blocked by a slow client.
//!
//! The owner holds a [`ConnHandle`]; dropping every handle ⇒ the write channel closes ⇒ the write
//! task ends ⇒ the socket write side closes. Note the write task first pushes whatever is still
//! queued, which on a stalled socket can take as long as the kernel retransmits; call
//! [`ConnHandle::close`] to tear it down immediately instead. The read task emits [`ConnEvent::Closed`] on EOF / timeout / deframing error.
//! This design converts C++'s `player → game` back-pointer calls entirely into a one-way event stream.

use std::net::SocketAddr;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
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

/// How many bytes may sit unsent for one connection before the owner gives up on that client.
///
/// Counted in bytes rather than packets on purpose: a packet count that is a comfortable
/// backlog at `bot_latency` 100 is only a few seconds at 5, so any packet-based limit silently
/// changes meaning with the latency setting. Must be at least as large as the GProxy++ replay
/// buffer (`game::actor::GPROXY_BUFFER_MAX_BYTES`), otherwise a client reconnecting after a
/// long hold could never be sent its backlog and would be stuck reconnecting forever.
pub const WRITE_QUEUE_MAX_BYTES: usize = 8 * 1024 * 1024;

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
///
/// There is deliberately no awaiting send. The queue is unbounded and guarded by a byte
/// budget instead, because an actor that serves many connections must never be parked on one
/// of them: a client whose TCP has stalled would block it for as long as the kernel keeps
/// retransmitting - minutes - freezing every other player in the game.
#[derive(Debug, Clone)]
pub struct ConnHandle {
    pub id: ConnId,
    pub peer: SocketAddr,
    write_tx: mpsc::UnboundedSender<Vec<u8>>,
    /// Bytes handed over but not yet written to the socket
    queued: Arc<AtomicUsize>,
    /// Lets close() stop the write task without waiting for it to drain
    write_task: Arc<tokio::task::AbortHandle>,
}

impl ConnHandle {
    /// Queue complete packet bytes (the output of the `send_*` family of functions).
    /// Never blocks.
    ///
    /// Returns false if the connection is closed, or if the peer has stopped draining and
    /// [`WRITE_QUEUE_MAX_BYTES`] is already outstanding. Callers should treat false as "this
    /// client is gone" - the packet was not queued, and for an exact protocol stream like
    /// W3GS there is no way to skip one and carry on.
    pub fn try_send(&self, data: Vec<u8>) -> bool {
        let len = data.len();
        if self.queued.load(Ordering::Relaxed).saturating_add(len) > WRITE_QUEUE_MAX_BYTES {
            return false;
        }
        self.queued.fetch_add(len, Ordering::Relaxed);
        if self.write_tx.send(data).is_err() {
            self.queued.fetch_sub(len, Ordering::Relaxed);
            return false;
        }
        true
    }

    /// Bytes still waiting to reach the socket (diagnostics)
    pub fn queued_bytes(&self) -> usize {
        self.queued.load(Ordering::Relaxed)
    }

    /// Tear the connection down now, discarding anything still queued.
    ///
    /// Merely dropping every handle also ends the write task, but only after it has pushed the
    /// whole backlog through - on a stalled socket that is however long the kernel keeps
    /// retransmitting. Use this when the owner has decided the peer is gone and wants the
    /// socket closed promptly, so the peer's TCP sees it and can act (a GProxy++ client
    /// reconnects on close rather than waiting out its own idle timer).
    ///
    /// Aborting mid-write can leave a partial packet on the wire; that is fine for a
    /// connection being abandoned, since anything the peer still needs is replayed in full
    /// over the new connection.
    pub fn close(&self) {
        self.write_task.abort();
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
    let (write_tx, mut write_rx) = mpsc::unbounded_channel::<Vec<u8>>();
    let queued = Arc::new(AtomicUsize::new(0));

    // ---- write task ----
    let write_queued = Arc::clone(&queued);
    let write_task = tokio::spawn(async move {
        while let Some(data) = write_rx.recv().await {
            let len = data.len();
            let result = write_half.write_all(&data).await;
            // release the budget whether or not the write succeeded, so a dying connection
            // cannot leave phantom bytes charged against it
            write_queued.fetch_sub(len, Ordering::Relaxed);
            if let Err(e) = result {
                debug!(conn = id, "write error: {e}");
                break;
            }
        }

        // A write error abandons whatever is still queued, so give the budget those bytes
        // back rather than leaving them charged against a handle that will never send again
        write_rx.close();
        while let Ok(data) = write_rx.try_recv() {
            write_queued.fetch_sub(data.len(), Ordering::Relaxed);
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

    ConnHandle {
        id,
        peer,
        write_tx,
        queued,
        write_task: Arc::new(write_task.abort_handle()),
    }
}

impl ConnHandle {
    /// A handle backed by a plain channel instead of a socket, so a test can hold the receiver
    /// and decline to drain it. That reproduces a stalled peer exactly, without the kernel's
    /// loopback buffering - which happily absorbs tens of megabytes and would stop the byte
    /// budget from ever being reached.
    #[cfg(test)]
    pub fn new_undrained_for_test() -> (Self, mpsc::UnboundedReceiver<Vec<u8>>) {
        let (write_tx, write_rx) = mpsc::unbounded_channel::<Vec<u8>>();
        let idle = tokio::spawn(std::future::ready(()));
        (
            Self {
                id: 0,
                peer: SocketAddr::from(([127, 0, 0, 1], 0)),
                write_tx,
                queued: Arc::new(AtomicUsize::new(0)),
                write_task: Arc::new(idle.abort_handle()),
            },
            write_rx,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A peer that stops draining must be refused once WRITE_QUEUE_MAX_BYTES is outstanding,
    /// and must be refused without ever blocking the caller. This is the backstop the whole
    /// stalled-client fix rests on: `try_send` returning false is what tells GameActor to drop
    /// that player instead of parking the entire game on them.
    #[tokio::test]
    async fn try_send_refuses_once_the_byte_budget_is_exhausted() {
        let (handle, mut rx) = ConnHandle::new_undrained_for_test();
        let packet = vec![0u8; 64 * 1024];

        let mut accepted = 0usize;
        // Bound the loop well past the budget so a regression shows up as a failed assert
        // rather than an infinite loop
        for _ in 0..(WRITE_QUEUE_MAX_BYTES / packet.len()) * 4 {
            if !handle.try_send(packet.clone()) {
                break;
            }
            accepted += packet.len();
        }

        assert_eq!(
            accepted, WRITE_QUEUE_MAX_BYTES,
            "should accept exactly the budget before refusing"
        );
        assert_eq!(handle.queued_bytes(), WRITE_QUEUE_MAX_BYTES);
        assert!(!handle.try_send(vec![0u8; 1]), "still refused while nothing drains");

        // Draining frees the budget again: the accounting tracks what is outstanding, not a
        // high-water mark
        let taken = rx.recv().await.expect("queued packet");
        assert_eq!(taken.len(), packet.len());
    }

    /// A closed connection refuses sends rather than growing the queue forever.
    #[tokio::test]
    async fn try_send_refuses_after_the_receiver_is_gone() {
        let (handle, rx) = ConnHandle::new_undrained_for_test();
        assert!(handle.try_send(vec![1, 2, 3]));
        drop(rx);
        assert!(!handle.try_send(vec![4, 5, 6]), "closed channel must refuse");
        assert_eq!(
            handle.queued_bytes(),
            3,
            "a refused send must not be charged against the budget"
        );
    }
}
