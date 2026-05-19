//! AEAD wrapper for secret storage.
//!
//! ChaCha20-Poly1305 with a 32-byte key derived from `Config.password_pepper`
//! by SHA-256 over `pepper || <domain-tag>`. The wire format is
//! `base64url(nonce(12) || ciphertext_with_tag)`. Decrypting a forged blob
//! fails authentication and returns an `Unauthorized` error so the calling
//! handler degrades to "please reconnect" / clears the broken secret.
//!
//! The domain tag binds a ciphertext to its purpose so a YT refresh-token
//! blob cannot be replayed as an LLM API key or vice-versa. Each consumer
//! gets its own constant; never reuse one across kinds of secrets.

use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use chacha20poly1305::{
    aead::{Aead, KeyInit, OsRng},
    ChaCha20Poly1305, Key, Nonce,
};
use listenai_core::{Error, Result};
use rand::RngCore;
use sha2::{Digest, Sha256};

/// Domain tag for the YouTube refresh-token column (`refresh_token_enc`).
const YT_REFRESH_DOMAIN: &[u8] = b"yt-refresh-token-v1";
/// Domain tag for the per-LLM-row OpenAI-compat API key (`api_key_enc`).
pub const LLM_API_KEY_DOMAIN: &[u8] = b"llm-api-key-v1";
const NONCE_LEN: usize = 12;

fn derive_key(pepper: &[u8], domain: &[u8]) -> Key {
    let mut h = Sha256::new();
    h.update(pepper);
    h.update(domain);
    let bytes = h.finalize();
    *Key::from_slice(&bytes)
}

/// Encrypt a plaintext secret. Returns `base64url(nonce || ct||tag)`. The
/// `domain` tag binds the ciphertext to its column so secrets from
/// different tables cannot be cross-replayed even when they share a pepper.
pub fn encrypt_with_domain(plaintext: &str, pepper: &[u8], domain: &[u8]) -> Result<String> {
    let key = derive_key(pepper, domain);
    let cipher = ChaCha20Poly1305::new(&key);

    let mut nonce_bytes = [0u8; NONCE_LEN];
    OsRng.fill_bytes(&mut nonce_bytes);
    let nonce = Nonce::from_slice(&nonce_bytes);

    let ct = cipher
        .encrypt(nonce, plaintext.as_bytes())
        .map_err(|e| Error::Other(anyhow::anyhow!("encrypt: {e}")))?;

    let mut out = Vec::with_capacity(NONCE_LEN + ct.len());
    out.extend_from_slice(&nonce_bytes);
    out.extend_from_slice(&ct);
    Ok(URL_SAFE_NO_PAD.encode(&out))
}

/// Decrypt a previously [`encrypt_with_domain`]ed secret. Tampered or
/// wrong-key inputs surface as `Error::Unauthorized` (the right thing for
/// the calling handler: ask the user to reconnect / re-enter the secret).
pub fn decrypt_with_domain(blob: &str, pepper: &[u8], domain: &[u8]) -> Result<String> {
    let raw = URL_SAFE_NO_PAD
        .decode(blob.as_bytes())
        .map_err(|_| Error::Unauthorized)?;
    if raw.len() <= NONCE_LEN {
        return Err(Error::Unauthorized);
    }
    let (nonce_bytes, ct) = raw.split_at(NONCE_LEN);
    let nonce = Nonce::from_slice(nonce_bytes);
    let key = derive_key(pepper, domain);
    let cipher = ChaCha20Poly1305::new(&key);
    let pt = cipher.decrypt(nonce, ct).map_err(|_| Error::Unauthorized)?;
    String::from_utf8(pt).map_err(|_| Error::Unauthorized)
}

/// YT-refresh-token convenience wrapper. Pins the domain to the legacy
/// `yt-refresh-token-v1` tag so existing ciphertexts keep decoding.
pub fn encrypt(plaintext: &str, pepper: &[u8]) -> Result<String> {
    encrypt_with_domain(plaintext, pepper, YT_REFRESH_DOMAIN)
}

/// YT-refresh-token convenience wrapper. See [`encrypt`].
pub fn decrypt(blob: &str, pepper: &[u8]) -> Result<String> {
    decrypt_with_domain(blob, pepper, YT_REFRESH_DOMAIN)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip() {
        let pepper = b"unit-test-pepper";
        let token = "1//0eABCDEFG.fakeRefreshToken_With-Various+Chars/=";
        let enc = encrypt(token, pepper).unwrap();
        assert_ne!(enc, token);
        assert_eq!(decrypt(&enc, pepper).unwrap(), token);
    }

    #[test]
    fn wrong_pepper_fails() {
        let enc = encrypt("hello", b"key-a").unwrap();
        assert!(decrypt(&enc, b"key-b").is_err());
    }

    #[test]
    fn truncated_fails() {
        let enc = encrypt("hello", b"k").unwrap();
        let truncated = &enc[..enc.len() / 2];
        assert!(decrypt(truncated, b"k").is_err());
    }
}
