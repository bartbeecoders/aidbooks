//! Job handlers — thin wrappers that close over `AppState` and delegate to
//! the existing generation modules. Keeping the glue here (rather than in
//! `listenai-jobs`) is deliberate: only the API crate knows about the LLM /
//! TTS clients, config, and storage paths.

mod handlers;

pub use handlers::registry;
