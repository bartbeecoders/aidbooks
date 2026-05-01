//! YouTube integration: OAuth dance, refresh-token storage, resumable upload.
//!
//! Layout:
//!   * [`encrypt`] — AEAD wrapper around the user's long-lived refresh token,
//!     keyed off `Config.password_pepper`. Never log the plaintext.
//!   * [`oauth`]   — consent-URL builder, code/refresh exchange, channel
//!     introspection, revocation.
//!   * [`upload`]  — resumable-upload protocol against `/upload/youtube/v3`.

pub mod account;
pub mod encrypt;
pub mod oauth;
pub mod playlist;
pub mod subtitles;
pub mod upload;
