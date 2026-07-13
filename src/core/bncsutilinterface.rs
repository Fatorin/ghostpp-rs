//! bncsutil interface (mirrors C++ CBNCSUtilInterface).
//! Phase 2/3b: hooked up to the embedded `crate::bncsutil` module.
//!
//! Used by the PVPGN path:
//! - `help_sid_auth_check`: compute the 36-byte keyinfo from the ROC/TFT CD key;
//!   if war3_path is set, also use checkRevision to compute exe version/hash/info from local War3 files
//! - `help_pvpgn_password_hash`: XSHA1(password) as the proof for SID_AUTH_ACCOUNTLOGONPROOF
//! - `help_sid_auth_accountlogon`: provide the 32-byte client key (A)
//!
//! Not yet ported (only needed by official battle.net, deferred per ROADMAP §4):
//! - The real NLS/SRP M1 (`help_sid_auth_accountlogonproof`). PVPGN's pvpgn-hash
//!   accounts do not verify SRP, so sending a random 32-byte A is enough to log in; official bnet needs full NLS.

use std::path::Path;

use tracing::warn;

use crate::bncsutil::cdkey::create_key_info;
use crate::bncsutil::checkrevision::{check_revision, get_exe_info, get_exe_version, select_war3_files};
use crate::bncsutil::hash_password;

use crate::util::{util_byte_array_to_u32, util_create_byte_array};

#[derive(Debug)]
pub struct BNCSUtilInterface {
    exe_version: Vec<u8>,        // set in help_sid_auth_check (or overridden by config)
    exe_version_hash: Vec<u8>,   // set in help_sid_auth_check (or overridden by config)
    key_info_roc: Vec<u8>,       // set in help_sid_auth_check
    key_info_tft: Vec<u8>,       // set in help_sid_auth_check
    client_key: Vec<u8>,         // set in help_sid_auth_accountlogon
    m1: Vec<u8>,                 // set in help_sid_auth_accountlogonproof (official bnet)
    pvpg_password_hash: Vec<u8>, // set in help_pvpgn_password_hash
    exe_info: String,            // set in help_sid_auth_check (or overridden by config)
}

impl BNCSUtilInterface {
    pub fn new(_user_name: &str, _user_password: &str) -> Self {
        Self {
            exe_version: Vec::new(),
            exe_version_hash: Vec::new(),
            key_info_roc: Vec::new(),
            key_info_tft: Vec::new(),
            client_key: Vec::new(),
            m1: Vec::new(),
            pvpg_password_hash: Vec::new(),
            exe_info: String::new(),
        }
    }

    pub fn get_exe_version(&self) -> &[u8] {
        &self.exe_version
    }

    pub fn get_exe_version_hash(&self) -> &[u8] {
        &self.exe_version_hash
    }

    pub fn get_exe_info(&self) -> &str {
        &self.exe_info
    }

    pub fn get_key_info_roc(&self) -> &[u8] {
        &self.key_info_roc
    }

    pub fn get_key_info_tft(&self) -> &[u8] {
        &self.key_info_tft
    }

    pub fn get_client_key(&self) -> &[u8] {
        &self.client_key
    }

    pub fn get_m1(&self) -> &[u8] {
        &self.m1
    }

    pub fn get_pvpg_password_hash(&self) -> &[u8] {
        &self.pvpg_password_hash
    }

    pub fn set_exe_version(&mut self, exe_version: &[u8]) {
        self.exe_version = exe_version.to_vec();
    }

    pub fn set_exe_version_hash(&mut self, exe_version_hash: &[u8]) {
        self.exe_version_hash = exe_version_hash.to_vec();
    }

    pub fn set_exe_info(&mut self, exe_info: String) {
        self.exe_info = exe_info;
    }

    pub fn reset(&mut self, _user_name: &str, _user_password: &str) {
        self.exe_version.clear();
        self.exe_version_hash.clear();
        self.key_info_roc.clear();
        self.key_info_tft.clear();
        self.client_key.clear();
        self.m1.clear();
        self.pvpg_password_hash.clear();
        self.exe_info.clear();
    }

    /// Compute each 36-byte keyinfo from the ROC/TFT CD key (mirrors the keyinfo part of C++ HELP_SID_AUTH_CHECK).
    /// Additionally: if `war3_path` is set and War3 files are found, use checkRevision to compute exe version/hash/info.
    ///
    /// `_war3_version` is a legacy-interface compatibility parameter; actual file selection is based on the files present on disk.
    pub fn help_sid_auth_check(
        &mut self,
        war3_path: &str,
        key_roc: &str,
        key_tft: &str,
        value_string_formula: &str,
        mpq_file_name: &str,
        client_token: &[u8],
        server_token: &[u8],
        _war3_version: u8,
    ) -> bool {
        let ct = util_byte_array_to_u32(client_token, false, 0);
        let st = util_byte_array_to_u32(server_token, false, 0);

        // 1) CD key → keyinfo
        self.key_info_roc = create_key_info(&key_roc.to_uppercase().replace('-', ""), ct, st);
        self.key_info_tft = create_key_info(&key_tft.to_uppercase().replace('-', ""), ct, st);

        let roc_ok = self.key_info_roc.len() == 36;
        let tft_ok = self.key_info_tft.len() == 36;

        if !roc_ok {
            warn!("[BNCSUI] unable to create ROC key info - invalid ROC key");
        }
        if !tft_ok {
            warn!("[BNCSUI] unable to create TFT key info - invalid TFT key");
        }

        // 2) Compute exe version / hash / info from local War3 files (checkRevision)
        //    If war3_path is unset or files are not found, leave empty and let the caller override with config custom values
        if !war3_path.is_empty() {
            if let Some(files) = select_war3_files(war3_path) {
                let mpq_number = crate::bncsutil::checkrevision::extract_mpq_number(mpq_file_name);
                if let Some(mpq_number) = mpq_number {
                    if let Some(hash) = check_revision(value_string_formula, &files, mpq_number) {
                        self.exe_version_hash = util_create_byte_array(hash, false);
                    } else {
                        warn!("[BNCSUI] checkRevision failed (formula/seed/file issue)");
                    }
                } else {
                    warn!("[BNCSUI] failed to parse MPQ number from ix86ver filename: {mpq_file_name}");
                }

                // Main exe (single-file or war3.exe) used to get the version and info string
                if let Some(primary) = files.first() {
                    if let Some(version) = get_exe_version(primary) {
                        self.exe_version = util_create_byte_array(version, false);
                    }
                    if let Some(info) = get_exe_info(Path::new(primary)) {
                        self.exe_info = info;
                    }
                }
            } else {
                warn!(
                    "[BNCSUI] Warcraft III.exe not found in {war3_path} (or war3.exe/Storm.dll/game.dll)"
                );
            }
        }

        roc_ok && tft_ok
    }

    /// Generate the 32-byte client key (SRP's A).
    /// PVPGN's pvpgn-hash accounts do not verify A, so a random value is fine; official bnet needs NLS (deferred).
    pub fn help_sid_auth_accountlogon(&mut self) -> bool {
        use rand::Rng;
        let mut key = vec![0u8; 32];
        rand::rng().fill(&mut key[..]);
        self.client_key = key;
        true
    }

    /// Official battle.net's SRP M1 (needs NLS, not yet ported).
    /// The PVPGN path never reaches here (uses the pvpgn password hash instead).
    pub fn help_sid_auth_accountlogonproof(&mut self, _salt: &[u8], _server_key: &[u8]) -> bool {
        // TODO(Phase 3b): port nls.c's SRP-6 M1 computation (needs 256-bit modular exponentiation)
        self.m1 = vec![0u8; 20];
        false
    }

    /// PVPGN password proof: XSHA1(plaintext password), 20 bytes.
    /// Mirrors C++ HELP_PvPGNPasswordHash.
    pub fn help_pvpg_password_hash(&mut self, user_password: &str) -> bool {
        self.pvpg_password_hash = hash_password(user_password).to_vec();
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pvpgn_password_hash_is_20_bytes() {
        let mut b = BNCSUtilInterface::new("user", "pass");
        assert!(b.help_pvpg_password_hash("pass"));
        assert_eq!(b.get_pvpg_password_hash().len(), 20);
    }

    #[test]
    fn account_logon_client_key_is_32_bytes() {
        let mut b = BNCSUtilInterface::new("user", "pass");
        assert!(b.help_sid_auth_accountlogon());
        assert_eq!(b.get_client_key().len(), 32);
    }

    #[test]
    fn auth_check_with_valid_key_yields_36_byte_keyinfo() {
        let mut b = BNCSUtilInterface::new("user", "pass");
        // All drawn from the W3 valid alphabet (not a real serial, tests structure only)
        let key = "2468BCDEFGHJKMNPRTVWXYZ246";
        let ct = [1u8, 2, 3, 4];
        let st = [5u8, 6, 7, 8];
        assert!(b.help_sid_auth_check("", key, key, "", "", &ct, &st, 26));
        assert_eq!(b.get_key_info_roc().len(), 36);
        assert_eq!(b.get_key_info_tft().len(), 36);
    }

    #[test]
    fn auth_check_rejects_invalid_key() {
        let mut b = BNCSUtilInterface::new("user", "pass");
        // 'A' is not in the W3 alphabet
        let key = "AAAAAAAAAAAAAAAAAAAAAAAAAA";
        assert!(!b.help_sid_auth_check("", key, key, "", "", &[1, 2, 3, 4], &[5, 6, 7, 8], 26));
    }
}
