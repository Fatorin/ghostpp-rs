//! bncsutil: Rust port of the cryptography required for Battle.net / PVPGN login (embedded module).
//!
//! Mirrors the original C library bncsutil, porting the parts used by the GHost++ PVPGN path:
//! - [`bsha1`]: Broken SHA-1 (XSHA1), PVPGN password proof
//! - [`cdkey`]: W3 CD key decode + 36-byte keyinfo for SID_AUTH_CHECK
//! - [`checkrevision`]: compute exe version/hash from local War3 files (no need to hand-fill exeversionhash)
//! - [`keytables`]: CD key decode lookup tables
//!
//! The boundary stays self-contained: this module does not depend on the rest of ghostpp-rs, so it can be extracted into a standalone crate at any time.

pub mod bsha1;
pub mod cdkey;
pub mod checkrevision;
pub mod keytables;

pub use bsha1::hash_password;
pub use cdkey::{create_key_info, CdKeyW3};
pub use checkrevision::{check_revision, get_exe_info, get_exe_version, select_war3_files};
