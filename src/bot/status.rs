//! Read-only status endpoint: which games exist and who is in them.
//!
//! Built so that an external poller (e.g. a bot manager) can never slow a game down:
//! - Each GameActor *pushes* a snapshot into the [`StatusBoard`] from its existing 1-second
//!   tick, and only when something visible changed (a cheap hash decides). A request never
//!   reaches a GameActor, so however often the endpoint is polled the action beat is untouched.
//! - The board keeps only the latest snapshot per game (a couple of KB), removed when the game closes.
//! - The HTTP side is a minimal GET-only HTTP/1.1 responder on tokio, with a peer IP allowlist,
//!   a cap on concurrent connections and timeouts on every read/write.
//!
//! Built to keep the poller's traffic to a minimum:
//! - Connections are kept alive, so a poll does not pay for a TCP handshake each time.
//! - The board carries a revision bumped on every change, served as the `ETag`. A poll that
//!   sends it back in `If-None-Match` gets a bodiless `304` until something actually changes -
//!   which, with rosters that rarely move mid-game, is almost every poll.
//! - When the body does have to go out, it is gzipped if the client accepts it.

use std::collections::hash_map::DefaultHasher;
use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::io::{self, Write};
use std::net::IpAddr;
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use flate2::write::GzEncoder;
use flate2::Compression;
use serde::Serialize;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::Semaphore;
use tokio::task::JoinHandle;
use tokio::time::{timeout, timeout_at, Instant};
use tracing::{debug, error, info, warn};

/// Connections served at once (idle keep-alive ones included); anything beyond is closed straight away
const MAX_CONNECTIONS: usize = 8;
/// Time allowed to receive a request head once it is expected, and to write a response
const IO_TIMEOUT: Duration = Duration::from_secs(3);
/// How long a kept-alive connection may sit idle before the next request
const KEEPALIVE_IDLE: Duration = Duration::from_secs(60);
/// The idle time advertised to clients in `Keep-Alive`. Kept below [`KEEPALIVE_IDLE`] so a
/// client retires the connection before the server does, rather than reusing one that is
/// being closed under it.
const KEEPALIVE_ADVERTISED_SECS: u64 = 55;
/// Largest request head accepted (a GET needs a few hundred bytes at most)
const MAX_REQUEST_BYTES: usize = 4096;

/// One player as seen by the status endpoint
#[derive(Debug, Clone, Serialize)]
pub struct PlayerSnapshot {
    pub name: String,
    /// 0-based slot index
    pub slot: u8,
    pub team: u8,
    pub colour: u8,
    pub observer: bool,
    /// GProxy++ player whose connection dropped, held while waiting for a reconnect
    pub reconnecting: bool,
}

/// One game as seen by the status endpoint
#[derive(Debug, Clone, Serialize)]
pub struct GameSnapshot {
    pub host_counter: u32,
    pub name: String,
    pub map: String,
    /// "lobby" / "loading" / "playing"
    pub phase: &'static str,
    /// Unix seconds
    pub created_at: u64,
    /// Unix seconds the game left the lobby (None while still in the lobby)
    pub started_at: Option<u64>,
    /// Slots still open for joining
    pub open_slots: u8,
    pub players: Vec<PlayerSnapshot>,
}

#[derive(Default)]
struct BoardState {
    games: HashMap<u32, Arc<GameSnapshot>>,
    /// Bumped on every change to `games`; the ETag is built from it
    revision: u64,
}

/// The latest snapshot of every live game, shared between the GameActors (writers) and the
/// HTTP endpoint (reader). The lock is only ever held to swap or clone an `Arc`.
pub struct StatusBoard {
    /// Unix seconds the bot started. Part of the ETag, so a restarted bot (whose revision
    /// starts over) can never answer 304 to a tag handed out by the previous run.
    started_at: u64,
    state: Mutex<BoardState>,
}

impl StatusBoard {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            started_at: unix_now(),
            state: Mutex::new(BoardState::default()),
        })
    }

    fn put(&self, snapshot: GameSnapshot) {
        let snapshot = Arc::new(snapshot);
        let mut state = self.lock();
        state.games.insert(snapshot.host_counter, snapshot);
        state.revision += 1;
    }

    fn remove(&self, host_counter: u32) {
        let mut state = self.lock();
        if state.games.remove(&host_counter).is_some() {
            state.revision += 1;
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, BoardState> {
        // A panic while holding the lock cannot leave the state half-written (every critical
        // section is a single insert/remove/clone), so a poisoned lock is safe to keep using
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn etag(&self, revision: u64) -> String {
        format!("\"{:x}-{revision}\"", self.started_at)
    }

    /// The ETag of the board as it is right now (no serialization)
    fn current_etag(&self) -> String {
        let revision = self.lock().revision;
        self.etag(revision)
    }

    /// Serialize the whole board together with the ETag of exactly what was serialized.
    /// The lock is released before any serialization happens.
    fn to_json(&self) -> (String, Vec<u8>) {
        let (revision, mut games) = {
            let state = self.lock();
            let games: Vec<Arc<GameSnapshot>> = state.games.values().cloned().collect();
            (state.revision, games)
        };
        games.sort_unstable_by_key(|g| g.host_counter);

        let count = |phase: &str| -> usize {
            games
                .iter()
                .filter(|g| g.phase == phase)
                .map(|g| g.players.len())
                .sum()
        };

        // Everything here must only change when the revision does, or the ETag would lie
        #[derive(Serialize)]
        struct Body<'a> {
            version: &'static str,
            /// Unix seconds the bot started
            started_at: u64,
            lobby_players: usize,
            ingame_players: usize,
            games: &'a [Arc<GameSnapshot>],
        }
        let body = Body {
            version: env!("CARGO_PKG_VERSION"),
            started_at: self.started_at,
            lobby_players: count("lobby"),
            ingame_players: count("loading") + count("playing"),
            games: &games,
        };
        (self.etag(revision), serde_json::to_vec(&body).unwrap_or_default())
    }
}

/// A GameActor's link to the board. Remembers what it last published so an unchanged game
/// costs one hash per second, and removes the game from the board when dropped - including
/// when the actor task unwinds from a panic, so a crashed game cannot linger as a ghost entry.
pub struct StatusPublisher {
    board: Arc<StatusBoard>,
    host_counter: u32,
    created_at: u64,
    started_at: Option<u64>,
    last_hash: Option<u64>,
}

impl StatusPublisher {
    pub fn new(board: Arc<StatusBoard>, host_counter: u32) -> Self {
        Self {
            board,
            host_counter,
            created_at: unix_now(),
            started_at: None,
            last_hash: None,
        }
    }

    /// Publish the game's state if it changed since the last call.
    ///
    /// `fingerprint` must hash everything `build` puts into the snapshot (apart from the
    /// timestamps, which the publisher tracks itself); `build` only runs on a change.
    pub fn update(
        &mut self,
        phase: &'static str,
        fingerprint: impl FnOnce(&mut DefaultHasher),
        build: impl FnOnce() -> (String, String, u8, Vec<PlayerSnapshot>),
    ) {
        let mut hasher = DefaultHasher::new();
        phase.hash(&mut hasher);
        fingerprint(&mut hasher);
        let hash = hasher.finish();
        if self.last_hash == Some(hash) {
            return;
        }
        self.last_hash = Some(hash);

        if phase != "lobby" && self.started_at.is_none() {
            self.started_at = Some(unix_now());
        }
        let (name, map, open_slots, players) = build();
        self.board.put(GameSnapshot {
            host_counter: self.host_counter,
            name,
            map,
            phase,
            created_at: self.created_at,
            started_at: self.started_at,
            open_slots,
            players,
        });
    }
}

impl Drop for StatusPublisher {
    fn drop(&mut self) {
        self.board.remove(self.host_counter);
    }
}

/// Who may read the endpoint: single addresses and CIDR blocks
#[derive(Debug, Clone)]
pub struct AllowList(Vec<(IpAddr, u8)>);

impl AllowList {
    /// Parse a comma-separated list such as "127.0.0.1, 10.0.0.0/24, ::1".
    /// An empty list allows loopback only. Invalid entries are logged and skipped.
    pub fn parse(spec: &str) -> Self {
        let mut entries = Vec::new();
        for item in spec.split(',').map(str::trim).filter(|s| !s.is_empty()) {
            let (addr, prefix) = match item.split_once('/') {
                Some((a, p)) => (a, p.parse::<u8>().ok()),
                None => (item, None),
            };
            match addr.parse::<IpAddr>() {
                Ok(ip) => {
                    let max = if ip.is_ipv4() { 32 } else { 128 };
                    match prefix {
                        Some(p) if p <= max => entries.push((ip, p)),
                        None => entries.push((ip, max)),
                        _ => warn!("[STATUS] ignoring invalid status_allow entry [{item}]"),
                    }
                }
                Err(_) => warn!("[STATUS] ignoring invalid status_allow entry [{item}]"),
            }
        }
        if entries.is_empty() {
            entries.push(("127.0.0.1".parse().unwrap(), 32));
            entries.push(("::1".parse().unwrap(), 128));
        }
        Self(entries)
    }

    pub fn allows(&self, ip: IpAddr) -> bool {
        // A dual-stack listener reports IPv4 peers as ::ffff:a.b.c.d
        let ip = ip.to_canonical();
        self.0.iter().any(|&(net, prefix)| match (net, ip) {
            (IpAddr::V4(n), IpAddr::V4(i)) => {
                let mask = u32::MAX.checked_shl(32 - prefix as u32).unwrap_or(0);
                u32::from(n) & mask == u32::from(i) & mask
            }
            (IpAddr::V6(n), IpAddr::V6(i)) => {
                let mask = u128::MAX.checked_shl(128 - prefix as u32).unwrap_or(0);
                u128::from(n) & mask == u128::from(i) & mask
            }
            _ => false,
        })
    }
}

/// Bind the status endpoint and start serving `GET /status`
pub async fn spawn(
    bind: &str,
    allow: AllowList,
    board: Arc<StatusBoard>,
) -> io::Result<JoinHandle<()>> {
    let listener = TcpListener::bind(bind).await?;
    info!("[STATUS] status endpoint listening on {bind} (allow: {:?})", allow.0);
    let slots = Arc::new(Semaphore::new(MAX_CONNECTIONS));

    Ok(tokio::spawn(async move {
        loop {
            let (stream, peer) = match listener.accept().await {
                Ok(c) => c,
                Err(e) => {
                    error!("[STATUS] accept error: {e}");
                    // Usually fd exhaustion: back off instead of spinning on the error
                    tokio::time::sleep(Duration::from_millis(100)).await;
                    continue;
                }
            };
            if !allow.allows(peer.ip()) {
                debug!("[STATUS] rejected connection from {peer} (not in status_allow)");
                continue; // dropping the stream closes it without a reply
            }
            let Ok(permit) = Arc::clone(&slots).try_acquire_owned() else {
                debug!("[STATUS] too many connections, dropping {peer}");
                continue;
            };
            let board = Arc::clone(&board);
            tokio::spawn(async move {
                if let Err(e) = serve(stream, &board).await {
                    debug!("[STATUS] connection from {peer} failed: {e}");
                }
                drop(permit);
            });
        }
    }))
}

/// The parts of a request head the endpoint cares about
struct Request {
    method: String,
    path: String,
    keep_alive: bool,
    if_none_match: Option<String>,
    accepts_gzip: bool,
    has_body: bool,
}

impl Request {
    fn parse(head: &[u8]) -> Self {
        let head = String::from_utf8_lossy(head);
        let mut lines = head.split("\r\n");
        let mut first = lines.next().unwrap_or("").split(' ');
        let method = first.next().unwrap_or("").to_string();
        // Ignore any query string, so pollers may append cache busters
        let path = first.next().unwrap_or("").split('?').next().unwrap_or("").to_string();
        // HTTP/1.1 keeps the connection open unless told otherwise; HTTP/1.0 only when asked
        let http11 = first.next() == Some("HTTP/1.1");

        let mut req = Request {
            method,
            path,
            keep_alive: http11,
            if_none_match: None,
            accepts_gzip: false,
            has_body: false,
        };
        for line in lines {
            let Some((name, value)) = line.split_once(':') else {
                continue;
            };
            let value = value.trim();
            match name.trim().to_ascii_lowercase().as_str() {
                "connection" => {
                    for token in value.split(',').map(str::trim) {
                        if token.eq_ignore_ascii_case("close") {
                            req.keep_alive = false;
                        } else if token.eq_ignore_ascii_case("keep-alive") {
                            req.keep_alive = true;
                        }
                    }
                }
                "if-none-match" => req.if_none_match = Some(value.to_string()),
                "accept-encoding" => req.accepts_gzip = accepts_gzip(value),
                "content-length" => req.has_body |= value != "0",
                "transfer-encoding" => req.has_body = true,
                _ => {}
            }
        }
        req
    }

    /// Whether the client already holds the representation tagged `etag`
    fn has_current(&self, etag: &str) -> bool {
        self.if_none_match.as_deref().is_some_and(|tags| {
            tags.split(',').map(str::trim).any(|t| {
                // Weak comparison, as RFC 9110 requires for If-None-Match
                t == "*" || t.strip_prefix("W/").unwrap_or(t) == etag
            })
        })
    }
}

/// `Accept-Encoding` lists gzip without turning it off with `q=0`
fn accepts_gzip(value: &str) -> bool {
    value.split(',').any(|coding| {
        let mut params = coding.split(';').map(str::trim);
        let name = params.next().unwrap_or("");
        let refused = params.any(|p| {
            p.strip_prefix("q=")
                .and_then(|q| q.parse::<f32>().ok())
                .is_some_and(|q| q == 0.0)
        });
        (name.eq_ignore_ascii_case("gzip") || name == "*") && !refused
    })
}

/// Serve requests on one connection until the client closes it, asks to close, goes idle
/// for [`KEEPALIVE_IDLE`], or sends something the endpoint will not keep the connection open for
async fn serve(mut stream: TcpStream, board: &StatusBoard) -> io::Result<()> {
    let mut buf = Vec::with_capacity(1024);
    let mut first = true;
    loop {
        // The first request is expected promptly; later ones may follow a long idle gap
        let wait = if first { IO_TIMEOUT } else { KEEPALIVE_IDLE };
        let Some(head_len) = read_head(&mut stream, &mut buf, Instant::now() + wait).await? else {
            return Ok(()); // closed (or went idle) cleanly between requests
        };
        let req = Request::parse(&buf[..head_len]);
        buf.drain(..head_len);
        first = false;

        // A body is never expected; rather than skip over one, stop reading the connection
        let keep_alive = req.keep_alive && !req.has_body;
        let response = respond_to(&req, board, keep_alive);
        timeout(IO_TIMEOUT, stream.write_all(&response))
            .await
            .map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "response timed out"))??;
        if !keep_alive {
            let _ = stream.shutdown().await;
            return Ok(());
        }
    }
}

/// Read until `buf` holds a complete request head, returning its length (terminator included).
/// `None` when the connection closed - or the deadline passed - with nothing pending,
/// which is just the client finishing with a kept-alive connection.
async fn read_head(
    stream: &mut TcpStream,
    buf: &mut Vec<u8>,
    deadline: Instant,
) -> io::Result<Option<usize>> {
    let mut chunk = [0u8; 512];
    loop {
        if let Some(pos) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
            return Ok(Some(pos + 4));
        }
        if buf.len() > MAX_REQUEST_BYTES {
            return Err(io::Error::new(io::ErrorKind::InvalidData, "request head too large"));
        }
        let n = match timeout_at(deadline, stream.read(&mut chunk)).await {
            Ok(r) => r?,
            Err(_) if buf.is_empty() => return Ok(None),
            Err(_) => return Err(io::Error::new(io::ErrorKind::TimedOut, "request timed out")),
        };
        if n == 0 {
            if buf.is_empty() {
                return Ok(None);
            }
            return Err(io::Error::new(io::ErrorKind::UnexpectedEof, "closed mid-request"));
        }
        buf.extend_from_slice(&chunk[..n]);
    }
}

fn respond_to(req: &Request, board: &StatusBoard, keep_alive: bool) -> Vec<u8> {
    match (req.method.as_str(), req.path.as_str()) {
        ("GET", "/status") => {
            // Checking the tag costs one lock; nothing is serialized for an unchanged board
            let etag = board.current_etag();
            if req.has_current(&etag) {
                return response("304 Not Modified", &[("ETag", &etag)], None, keep_alive);
            }
            let (etag, json) = board.to_json();
            if req.accepts_gzip {
                if let Some(gz) = gzip(&json) {
                    return response(
                        "200 OK",
                        &[("ETag", &etag), ("Content-Encoding", "gzip")],
                        Some(("application/json", &gz)),
                        keep_alive,
                    );
                }
            }
            response(
                "200 OK",
                &[("ETag", &etag)],
                Some(("application/json", &json)),
                keep_alive,
            )
        }
        ("GET", _) => response("404 Not Found", &[], Some(("text/plain", b"not found")), keep_alive),
        _ => response(
            "405 Method Not Allowed",
            &[("Allow", "GET")],
            Some(("text/plain", b"method not allowed")),
            keep_alive,
        ),
    }
}

/// Build a response. `body` is (content type, bytes); a 304 passes None and carries no body.
fn response(
    status: &str,
    headers: &[(&str, &str)],
    body: Option<(&str, &[u8])>,
    keep_alive: bool,
) -> Vec<u8> {
    let mut head = format!("HTTP/1.1 {status}\r\n");
    for (name, value) in headers {
        head.push_str(&format!("{name}: {value}\r\n"));
    }
    // no-cache, not no-store: clients may keep the body, but must revalidate it with the ETag
    head.push_str("Cache-Control: no-cache\r\nVary: Accept-Encoding\r\n");
    if keep_alive {
        head.push_str(&format!(
            "Connection: keep-alive\r\nKeep-Alive: timeout={KEEPALIVE_ADVERTISED_SECS}\r\n"
        ));
    } else {
        head.push_str("Connection: close\r\n");
    }
    let body = match body {
        Some((content_type, bytes)) => {
            head.push_str(&format!(
                "Content-Type: {content_type}; charset=utf-8\r\nContent-Length: {}\r\n",
                bytes.len()
            ));
            bytes
        }
        None => &[],
    };
    head.push_str("\r\n");
    let mut out = head.into_bytes();
    out.extend_from_slice(body);
    out
}

fn gzip(data: &[u8]) -> Option<Vec<u8>> {
    let mut encoder = GzEncoder::new(Vec::with_capacity(data.len() / 4), Compression::default());
    encoder.write_all(data).ok()?;
    encoder.finish().ok()
}

fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read;

    #[test]
    fn allow_list_matches_addresses_and_blocks() {
        let allow = AllowList::parse("10.0.0.5, 192.168.1.0/24, bogus, 1.2.3.4/40");
        assert!(allow.allows("10.0.0.5".parse().unwrap()));
        assert!(!allow.allows("10.0.0.6".parse().unwrap()));
        assert!(allow.allows("192.168.1.200".parse().unwrap()));
        assert!(!allow.allows("192.168.2.1".parse().unwrap()));
        // IPv4 peer seen through a dual-stack socket
        assert!(allow.allows("::ffff:10.0.0.5".parse().unwrap()));
        // Invalid entries are skipped rather than widening access
        assert!(!allow.allows("1.2.3.4".parse().unwrap()));
        assert!(!allow.allows("127.0.0.1".parse().unwrap()));
    }

    #[test]
    fn empty_allow_list_means_loopback_only() {
        let allow = AllowList::parse("");
        assert!(allow.allows("127.0.0.1".parse().unwrap()));
        assert!(allow.allows("::1".parse().unwrap()));
        assert!(!allow.allows("10.0.0.1".parse().unwrap()));
    }

    #[test]
    fn publisher_skips_unchanged_state_and_cleans_up_on_drop() {
        let board = StatusBoard::new();
        let mut builds = 0;
        {
            let mut publisher = StatusPublisher::new(Arc::clone(&board), 7);
            for _ in 0..3 {
                publisher.update(
                    "lobby",
                    |h| "alice".hash(h),
                    || {
                        builds += 1;
                        ("g".into(), "m".into(), 3, vec![])
                    },
                );
            }
            assert_eq!(builds, 1, "an unchanged game must not be rebuilt");
            assert_eq!(board.lock().revision, 1, "an unchanged game must not bump the revision");
            publisher.update("playing", |h| "alice".hash(h), || ("g".into(), "m".into(), 0, vec![]));
            let snap = board.lock().games.get(&7).cloned().unwrap();
            assert_eq!(snap.phase, "playing");
            assert!(snap.started_at.is_some());
        }
        let state = board.lock();
        assert!(state.games.is_empty(), "a closed game must leave the board");
        assert_eq!(state.revision, 3, "closing a game is a change too");
    }

    #[test]
    fn json_counts_players_by_phase() {
        let board = StatusBoard::new();
        let player = |name: &str| PlayerSnapshot {
            name: name.into(),
            slot: 0,
            team: 0,
            colour: 0,
            observer: false,
            reconnecting: false,
        };
        let mut a = StatusPublisher::new(Arc::clone(&board), 1);
        a.update("lobby", |h| 1.hash(h), || ("a".into(), "m".into(), 9, vec![player("x")]));
        let mut b = StatusPublisher::new(Arc::clone(&board), 2);
        b.update("playing", |h| 2.hash(h), || {
            ("b".into(), "m".into(), 0, vec![player("y"), player("z")])
        });
        let (_, body) = board.to_json();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["lobby_players"], 1);
        assert_eq!(json["ingame_players"], 2);
        assert_eq!(json["games"][1]["players"][1]["name"], "z");
    }

    #[test]
    fn gzip_and_if_none_match_parsing() {
        assert!(accepts_gzip("gzip, deflate, br"));
        assert!(accepts_gzip("br;q=1.0, GZIP;q=0.5"));
        assert!(accepts_gzip("*"));
        assert!(!accepts_gzip("gzip;q=0, deflate"));
        assert!(!accepts_gzip("deflate, br"));

        let req = Request::parse(b"GET /status HTTP/1.1\r\nIf-None-Match: \"a\", W/\"b-1\"\r\n\r\n");
        assert!(req.has_current("\"b-1\""));
        assert!(!req.has_current("\"b-2\""));
        assert!(req.keep_alive);
        let req = Request::parse(b"GET /status HTTP/1.0\r\n\r\n");
        assert!(!req.keep_alive, "HTTP/1.0 closes unless it asks to keep alive");
    }

    /// A test client on one kept-alive connection
    struct Client {
        stream: TcpStream,
        buf: Vec<u8>,
    }

    impl Client {
        /// Start a server task for a single connection and connect to it
        async fn connect(board: Arc<StatusBoard>) -> (Self, JoinHandle<io::Result<()>>) {
            let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
            let addr = listener.local_addr().unwrap();
            let server = tokio::spawn(async move {
                let (stream, _) = listener.accept().await.unwrap();
                serve(stream, &board).await
            });
            let stream = TcpStream::connect(addr).await.unwrap();
            (Self { stream, buf: Vec::new() }, server)
        }

        /// Send a request and read exactly one response: (head, body)
        async fn request(&mut self, request: &str) -> (String, Vec<u8>) {
            self.stream.write_all(request.as_bytes()).await.unwrap();
            let len = read_head(&mut self.stream, &mut self.buf, Instant::now() + IO_TIMEOUT)
                .await
                .unwrap()
                .expect("connection closed before a response");
            let head = String::from_utf8(self.buf.drain(..len).collect()).unwrap();
            let body_len = head
                .lines()
                .find_map(|l| l.strip_prefix("Content-Length: "))
                .map_or(0, |v| v.parse().unwrap());
            while self.buf.len() < body_len {
                let mut chunk = [0u8; 4096];
                let n = self.stream.read(&mut chunk).await.unwrap();
                assert!(n > 0, "connection closed mid-body");
                self.buf.extend_from_slice(&chunk[..n]);
            }
            (head, self.buf.drain(..body_len).collect())
        }

        /// The server closed the connection (EOF on the next read)
        async fn closed(&mut self) -> bool {
            let mut byte = [0u8; 1];
            matches!(timeout(IO_TIMEOUT, self.stream.read(&mut byte)).await, Ok(Ok(0)))
        }
    }

    fn header<'a>(head: &'a str, name: &str) -> Option<&'a str> {
        head.lines()
            .find_map(|l| l.split_once(": ").filter(|(n, _)| n.eq_ignore_ascii_case(name)))
            .map(|(_, v)| v)
    }

    #[tokio::test]
    async fn keep_alive_connection_revalidates_with_etag() {
        let board = StatusBoard::new();
        let mut game = StatusPublisher::new(Arc::clone(&board), 3);
        game.update("lobby", |h| 0.hash(h), || ("dota #3".into(), "m".into(), 10, vec![]));

        let (mut client, server) = Client::connect(Arc::clone(&board)).await;

        // First poll: the full body and its tag
        let (head, body) = client.request("GET /status?t=1 HTTP/1.1\r\nHost: x\r\n\r\n").await;
        assert!(head.starts_with("HTTP/1.1 200 OK\r\n"), "{head}");
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["games"][0]["name"], "dota #3");
        assert_eq!(json["games"][0]["open_slots"], 10);
        let etag = header(&head, "ETag").unwrap().to_string();

        // Same connection, nothing changed: 304 with no body
        let poll = format!("GET /status HTTP/1.1\r\nIf-None-Match: {etag}\r\n\r\n");
        let (head, body) = client.request(&poll).await;
        assert!(head.starts_with("HTTP/1.1 304 Not Modified\r\n"), "{head}");
        assert!(body.is_empty());
        assert_eq!(header(&head, "ETag"), Some(etag.as_str()));

        // The game changes: the old tag no longer matches
        game.update("lobby", |h| 1.hash(h), || ("dota #3".into(), "m".into(), 9, vec![]));
        let (head, body) = client.request(&poll).await;
        assert!(head.starts_with("HTTP/1.1 200 OK\r\n"), "{head}");
        assert_ne!(header(&head, "ETag"), Some(etag.as_str()));
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["games"][0]["open_slots"], 9);

        // Unknown paths and other methods are answered too; a 405 still keeps the connection
        let (head, _) = client.request("GET /other HTTP/1.1\r\n\r\n").await;
        assert!(head.starts_with("HTTP/1.1 404"), "{head}");
        let (head, _) = client.request("DELETE /status HTTP/1.1\r\n\r\n").await;
        assert!(head.starts_with("HTTP/1.1 405"), "{head}");

        // Asking to close ends the connection after the response
        let (head, _) = client.request("GET /status HTTP/1.1\r\nConnection: close\r\n\r\n").await;
        assert_eq!(header(&head, "Connection"), Some("close"));
        assert!(client.closed().await);
        server.await.unwrap().unwrap();
    }

    #[tokio::test]
    async fn gzip_body_when_accepted() {
        let board = StatusBoard::new();
        let mut game = StatusPublisher::new(Arc::clone(&board), 1);
        game.update("lobby", |h| 0.hash(h), || ("g".into(), "m".into(), 5, vec![]));
        let (mut client, _server) = Client::connect(board).await;

        let (head, body) = client
            .request("GET /status HTTP/1.1\r\nAccept-Encoding: gzip, deflate\r\n\r\n")
            .await;
        assert_eq!(header(&head, "Content-Encoding"), Some("gzip"));
        let mut json = String::new();
        flate2::read::GzDecoder::new(body.as_slice())
            .read_to_string(&mut json)
            .unwrap();
        let json: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(json["games"][0]["name"], "g");

        // Without Accept-Encoding the body is plain
        let (head, body) = client.request("GET /status HTTP/1.1\r\n\r\n").await;
        assert_eq!(header(&head, "Content-Encoding"), None);
        assert!(serde_json::from_slice::<serde_json::Value>(&body).is_ok());
    }

    #[tokio::test]
    async fn http10_and_request_bodies_close_the_connection() {
        let board = StatusBoard::new();
        let (mut client, server) = Client::connect(Arc::clone(&board)).await;
        let (head, _) = client.request("GET /status HTTP/1.0\r\n\r\n").await;
        assert!(head.starts_with("HTTP/1.1 200 OK\r\n"), "{head}");
        assert!(client.closed().await);
        server.await.unwrap().unwrap();

        // A body is never read, so the connection cannot be reused after one
        let (mut client, server) = Client::connect(board).await;
        let (head, _) = client
            .request("POST /status HTTP/1.1\r\nContent-Length: 2\r\n\r\n{}")
            .await;
        assert!(head.starts_with("HTTP/1.1 405"), "{head}");
        assert!(client.closed().await);
        server.await.unwrap().unwrap();
    }
}
