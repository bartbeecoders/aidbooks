//! Durable job runner for the generation pipeline.
//!
//! Responsibilities split across the submodules:
//!   * [`repo`] — typed SurrealQL over the `job` table: enqueue, atomic
//!     pickup, complete, fail, dead-letter, child fan-out.
//!   * [`hub`] — in-memory [`tokio::sync::broadcast`] fan-out keyed by
//!     audiobook id, so WebSocket subscribers can watch jobs owned by that
//!     book without polling the DB.
//!   * [`runtime`] — the worker pool: bounded concurrency per `JobKind`,
//!     atomic pickup loop, retry-with-backoff, graceful shutdown,
//!     resume-on-boot.
//!
//! The handler type that actually *does* the work lives in the `api` crate
//! (it needs `AppState`); this crate only provides the trait + dispatcher.

pub mod handler;
pub mod hub;
pub mod repo;
pub mod runtime;

pub use handler::{JobContext, JobHandler, JobHandlerRegistry, JobOutcome};
pub use hub::{ProgressEvent, ProgressHub};
pub use repo::{EnqueueRequest, JobRepo, JobRow};
pub use runtime::{WorkerConfig, WorkerHandle};
