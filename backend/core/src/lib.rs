//! Shared domain types, config, and error enum for the ListenAI backend.

pub mod config;
pub mod domain;
pub mod error;
pub mod id;

pub use error::{Error, Result};
