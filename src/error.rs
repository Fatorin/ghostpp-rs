//! Project-wide unified error type.
//! Gradually replaces the `io::Error::new(ErrorKind::InvalidData, ...)` scattered everywhere.

use thiserror::Error;

#[derive(Debug, Error)]
pub enum GhostError {
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),

    #[error("config error: {0}")]
    Config(#[from] config::ConfigError),

    /// Packet framing error (see net::codec)
    #[error("frame error: {0}")]
    Frame(#[from] crate::net::codec::FrameError),

    /// Config file is missing a required field (e.g. bnet_server, bnet_cdkeyroc)
    #[error("missing config key: {0}")]
    MissingConfig(&'static str),

    #[error("map error: {0}")]
    Map(String),

    #[error("{0}")]
    Other(String),
}

pub type Result<T> = std::result::Result<T, GhostError>;
