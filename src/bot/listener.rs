//! Centralized listener on host_port (ROADMAP §2).
//!
//! Unlike the original where "each CBaseGame binds host_port itself": here a single listener accepts,
//! handing the TcpStream to BotCore for routing (from Phase 4 on, forwarded to the current lobby's GameActor).
//! Behavior is equivalent — a game that has started does not accept new connections anyway, and GProxy reconnects use a separate reconnect port.

use std::io;
use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tokio::time::timeout;
use tracing::{debug, error, info};

use super::messages::BotEvent;
use crate::core::gpsprotocol;

/// Bind host_port and start the accept loop.
/// When `bind_address` is an empty string, bind all interfaces (mirrors bot_bindaddress).
pub async fn spawn(
    bind_address: &str,
    port: u16,
    event_tx: mpsc::Sender<BotEvent>,
) -> io::Result<JoinHandle<()>> {
    let addr = if bind_address.is_empty() {
        format!("0.0.0.0:{port}")
    } else {
        format!("{bind_address}:{port}")
    };

    let listener = TcpListener::bind(&addr).await?;
    info!("[GHOST] listening for game connections on {addr}");

    Ok(tokio::spawn(async move {
        loop {
            match listener.accept().await {
                Ok((stream, peer)) => {
                    if event_tx
                        .send(BotEvent::NewConnection { stream, peer })
                        .await
                        .is_err()
                    {
                        // BotCore has shut down, so the listener terminates too
                        break;
                    }
                }
                Err(e) => {
                    error!("[GHOST] accept error: {e}");
                }
            }
        }
    }))
}

/// GProxy++ reconnect listener (bot_reconnectport).
/// Each connection first reads the 13-byte GPS_RECONNECT handshake (5-second timeout);
/// only when valid does it report to BotCore for routing; otherwise it replies GPSS_REJECT and closes.
pub async fn spawn_reconnect(
    bind_address: &str,
    port: u16,
    event_tx: mpsc::Sender<BotEvent>,
) -> io::Result<JoinHandle<()>> {
    let addr = if bind_address.is_empty() {
        format!("0.0.0.0:{port}")
    } else {
        format!("{bind_address}:{port}")
    };

    let listener = TcpListener::bind(&addr).await?;
    info!("[GHOST] listening for GProxy++ reconnects on {addr}");

    Ok(tokio::spawn(async move {
        loop {
            match listener.accept().await {
                Ok((mut stream, peer)) => {
                    let tx = event_tx.clone();
                    tokio::spawn(async move {
                        let _ = stream.set_nodelay(true);
                        // GPS_RECONNECT is a fixed 13 bytes: [F8][02][len u16][pid][key u32][last u32]
                        let mut buf = [0u8; 13];
                        let read =
                            timeout(Duration::from_secs(5), stream.read_exact(&mut buf)).await;
                        match read {
                            Ok(Ok(_)) => {
                                if let Some(rc) = gpsprotocol::receive_gps_reconnect(&buf) {
                                    debug!(
                                        "[GHOST] GProxy reconnect from {peer}: pid={} key={:08X}",
                                        rc.pid, rc.key
                                    );
                                    let _ = tx
                                        .send(BotEvent::GProxyReconnect {
                                            stream,
                                            pid: rc.pid,
                                            key: rc.key,
                                            last_packet: rc.last_packet,
                                        })
                                        .await;
                                } else {
                                    let _ = stream
                                        .write_all(&gpsprotocol::send_gpss_reject(
                                            gpsprotocol::REJECTGPS_INVALID as u32,
                                        ))
                                        .await;
                                }
                            }
                            _ => {
                                debug!("[GHOST] GProxy reconnect from {peer}: handshake timeout/read failure");
                            }
                        }
                    });
                }
                Err(e) => {
                    error!("[GHOST] reconnect accept error: {e}");
                }
            }
        }
    }))
}
