//! Password hashing and opaque-token helpers.
//!
//! Passwords are hashed with Argon2id and a server-wide "pepper" injected as
//! Argon2's secret parameter — so a leaked password_hash column alone cannot
//! be brute-forced without also leaking the pepper.
//!
//! Refresh tokens are random 32-byte values returned to the client
//! base64url-encoded. Server side we store only an HMAC-SHA256 of the raw
//! token (keyed with the pepper), so the DB never sees the plaintext.

use crate::{Error, Result};
use argon2::{
    password_hash::{rand_core::OsRng, PasswordHash, PasswordHasher, PasswordVerifier, SaltString},
    Algorithm, Argon2, Params, Version,
};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use hmac::{Hmac, Mac};
use rand::RngCore;
use sha2::Sha256;
use subtle::ConstantTimeEq;

const REFRESH_TOKEN_BYTES: usize = 32;

fn argon2_with_pepper(pepper: &[u8]) -> Result<Argon2<'_>> {
    // Defaults in argon2 v0.5 already match OWASP 2026 recommendations
    // (m=19456, t=2, p=1). We only need to inject the pepper.
    Argon2::new_with_secret(
        pepper,
        Algorithm::Argon2id,
        Version::V0x13,
        Params::default(),
    )
    .map_err(|e| Error::Config(format!("argon2 config: {e}")))
}

/// Hash a user-supplied password. Returns the PHC-string encoding, which
/// includes the algorithm, parameters, and salt — fit for direct storage.
pub fn hash_password(password: &str, pepper: &[u8]) -> Result<String> {
    let salt = SaltString::generate(&mut OsRng);
    let argon2 = argon2_with_pepper(pepper)?;
    argon2
        .hash_password(password.as_bytes(), &salt)
        .map(|h| h.to_string())
        .map_err(|e| Error::Other(anyhow::anyhow!("argon2 hash: {e}")))
}

/// Constant-time verification against a previously hashed password.
pub fn verify_password(password: &str, hash: &str, pepper: &[u8]) -> Result<bool> {
    let parsed =
        PasswordHash::new(hash).map_err(|e| Error::Other(anyhow::anyhow!("argon2 parse: {e}")))?;
    let argon2 = argon2_with_pepper(pepper)?;
    Ok(argon2.verify_password(password.as_bytes(), &parsed).is_ok())
}

/// Fresh random refresh token. The returned string is what goes to the
/// client; never store it directly — use [`hash_refresh_token`] first.
pub fn new_refresh_token() -> String {
    let mut buf = [0u8; REFRESH_TOKEN_BYTES];
    OsRng.fill_bytes(&mut buf);
    URL_SAFE_NO_PAD.encode(buf)
}

/// HMAC-SHA256 hex digest of a refresh token, keyed with the pepper.
/// Deterministic — same input always produces the same output, which is what
/// lets us look a token up by hash.
pub fn hash_refresh_token(token: &str, pepper: &[u8]) -> Result<String> {
    let mut mac = <Hmac<Sha256> as Mac>::new_from_slice(pepper)
        .map_err(|e| Error::Other(anyhow::anyhow!("hmac init: {e}")))?;
    mac.update(token.as_bytes());
    Ok(hex_lower(&mac.finalize().into_bytes()))
}

/// Constant-time equality on two hex-encoded hashes. Useful when comparing
/// refresh token hashes to avoid timing side-channels.
pub fn ct_eq_str(a: &str, b: &str) -> bool {
    let a = a.as_bytes();
    let b = b.as_bytes();
    if a.len() != b.len() {
        return false;
    }
    a.ct_eq(b).into()
}

fn hex_lower(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        out.push(HEX[(b >> 4) as usize] as char);
        out.push(HEX[(b & 0x0f) as usize] as char);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const PEPPER: &[u8] = b"unit-test-pepper";

    #[test]
    fn password_round_trip() {
        let hash = hash_password("correct horse battery staple", PEPPER).unwrap();
        assert!(verify_password("correct horse battery staple", &hash, PEPPER).unwrap());
        assert!(!verify_password("wrong password", &hash, PEPPER).unwrap());
    }

    #[test]
    fn pepper_is_required() {
        let hash = hash_password("pw", PEPPER).unwrap();
        assert!(!verify_password("pw", &hash, b"different-pepper").unwrap());
    }

    #[test]
    fn refresh_tokens_are_unique_and_hashable() {
        let a = new_refresh_token();
        let b = new_refresh_token();
        assert_ne!(a, b);
        let ha = hash_refresh_token(&a, PEPPER).unwrap();
        let hb = hash_refresh_token(&b, PEPPER).unwrap();
        assert_ne!(ha, hb);
        // Deterministic for same input.
        assert_eq!(ha, hash_refresh_token(&a, PEPPER).unwrap());
        // 64 hex chars = 256 bits.
        assert_eq!(ha.len(), 64);
    }

    #[test]
    fn ct_eq_str_matches() {
        assert!(ct_eq_str("abc", "abc"));
        assert!(!ct_eq_str("abc", "abd"));
        assert!(!ct_eq_str("abc", "abcd"));
    }
}
