//! Network layer (ROADMAP Phase 1): the packet framing codec and connection task template.
//! Replaces the old manual buffers of `core::gamesocket` / `core::commandpacket`.

pub mod codec;
pub mod conn;
