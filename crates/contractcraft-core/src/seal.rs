//! Tamper evidence for contract versions: SHA-256 of the exact text bytes plus
//! an Ed25519 signature over (contract number, version, hash, timestamp).
//!
//! This proves "this text was sealed by this installation at this time and has
//! not changed since". It is NOT an electronic signature with legal effect
//! (no QES, no ID Austria / A-Trust); parties still sign the contract.

use ed25519_dalek::{Signature, Signer, SigningKey, VerifyingKey};
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::path::Path;

pub const SEAL_DOMAIN: &str = "contractcraft-seal-v1";

pub fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

pub fn unhex(s: &str) -> Result<Vec<u8>, String> {
    let t = s.trim();
    if !t.len().is_multiple_of(2) || !t.chars().all(|c| c.is_ascii_hexdigit()) {
        return Err("not a hex string".into());
    }
    (0..t.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&t[i..i + 2], 16).map_err(|e| e.to_string()))
        .collect()
}

pub fn sha256_hex(bytes: &[u8]) -> String {
    hex(&Sha256::digest(bytes))
}

/// The exact bytes that are signed.
pub fn message(number: &str, version: i64, sha256: &str, sealed_at: &str) -> String {
    format!("{SEAL_DOMAIN}\n{number}\n{version}\n{sha256}\n{sealed_at}")
}

/// Load the installation's seal key, creating it on first use. The file holds
/// the 32-byte secret as hex; keep it out of backups you share.
pub fn load_or_create_key(path: &Path) -> Result<SigningKey, String> {
    if path.exists() {
        let text = std::fs::read_to_string(path)
            .map_err(|e| format!("seal key {}: {e}", path.display()))?;
        let bytes: [u8; 32] = unhex(&text)?
            .try_into()
            .map_err(|_| format!("seal key {} is not 32 bytes", path.display()))?;
        return Ok(SigningKey::from_bytes(&bytes));
    }
    let mut secret = [0u8; 32];
    getrandom::fill(&mut secret).map_err(|e| format!("random: {e}"))?;
    if let Some(dir) = path.parent() {
        if !dir.as_os_str().is_empty() {
            std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        }
    }
    std::fs::write(path, format!("{}\n", hex(&secret)))
        .map_err(|e| format!("seal key {}: {e}", path.display()))?;
    Ok(SigningKey::from_bytes(&secret))
}

pub struct Sealed {
    pub sha256: String,
    pub signature: String,
    pub public_key: String,
}

pub fn seal(key: &SigningKey, number: &str, version: i64, text: &[u8], sealed_at: &str) -> Sealed {
    let sha256 = sha256_hex(text);
    let sig = key.sign(message(number, version, &sha256, sealed_at).as_bytes());
    Sealed {
        sha256,
        signature: hex(&sig.to_bytes()),
        public_key: hex(key.verifying_key().as_bytes()),
    }
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct Verdict {
    pub hash_matches: bool,
    pub signature_valid: bool,
    pub expected_sha256: String,
    pub actual_sha256: String,
}

impl Verdict {
    pub fn ok(&self) -> bool {
        self.hash_matches && self.signature_valid
    }
}

/// Check text bytes against a stored seal.
pub fn verify(
    number: &str,
    version: i64,
    text: &[u8],
    sha256: &str,
    sealed_at: &str,
    signature: &str,
    public_key: &str,
) -> Result<Verdict, String> {
    let actual = sha256_hex(text);
    let pk: [u8; 32] = unhex(public_key)?
        .try_into()
        .map_err(|_| "public key is not 32 bytes".to_string())?;
    let sig: [u8; 64] = unhex(signature)?
        .try_into()
        .map_err(|_| "signature is not 64 bytes".to_string())?;
    let vk = VerifyingKey::from_bytes(&pk).map_err(|e| format!("public key: {e}"))?;
    let signature_valid = vk
        .verify_strict(
            message(number, version, sha256, sealed_at).as_bytes(),
            &Signature::from_bytes(&sig),
        )
        .is_ok();
    Ok(Verdict {
        hash_matches: actual == sha256,
        signature_valid,
        expected_sha256: sha256.into(),
        actual_sha256: actual,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn seal_roundtrip_and_tamper_detection() {
        let path =
            std::env::temp_dir().join(format!("contractcraft-key-{}.hex", std::process::id()));
        let _ = std::fs::remove_file(&path);
        let k1 = load_or_create_key(&path).unwrap();
        let k2 = load_or_create_key(&path).unwrap();
        assert_eq!(k1.verifying_key(), k2.verifying_key(), "key persists");
        let s = seal(
            &k1,
            "C-2026-001",
            2,
            "Vertragstext".as_bytes(),
            "2026-11-09T10:00:00Z",
        );
        assert_eq!(s.sha256.len(), 64);
        let ok = verify(
            "C-2026-001",
            2,
            "Vertragstext".as_bytes(),
            &s.sha256,
            "2026-11-09T10:00:00Z",
            &s.signature,
            &s.public_key,
        )
        .unwrap();
        assert!(ok.ok());
        let text_changed = verify(
            "C-2026-001",
            2,
            "Vertragstext!".as_bytes(),
            &s.sha256,
            "2026-11-09T10:00:00Z",
            &s.signature,
            &s.public_key,
        )
        .unwrap();
        assert!(!text_changed.hash_matches && text_changed.signature_valid);
        let ts_changed = verify(
            "C-2026-001",
            2,
            "Vertragstext".as_bytes(),
            &s.sha256,
            "2026-11-10T10:00:00Z",
            &s.signature,
            &s.public_key,
        )
        .unwrap();
        assert!(
            !ts_changed.signature_valid,
            "timestamp is covered by the signature"
        );
        let other_version = verify(
            "C-2026-001",
            3,
            "Vertragstext".as_bytes(),
            &s.sha256,
            "2026-11-09T10:00:00Z",
            &s.signature,
            &s.public_key,
        )
        .unwrap();
        assert!(!other_version.signature_valid);
        assert!(verify("C", 1, b"x", "00", "t", "zz", &s.public_key).is_err());
        assert_eq!(
            sha256_hex(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        let _ = std::fs::remove_file(&path);
    }
}
