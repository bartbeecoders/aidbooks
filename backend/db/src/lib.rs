//! Embedded SurrealDB (RocksDB backend) wrapper, migration runner, and
//! seed data for ListenAI.

pub mod migrate;
pub mod seed;

use std::path::{Path, PathBuf};

use listenai_core::{Error, Result};
use surrealdb::{engine::local::Db as Engine, engine::local::RocksDb, Surreal};
use tracing::info;

/// Shared database handle. Cheap to clone.
#[derive(Clone)]
pub struct Db {
    inner: Surreal<Engine>,
    /// Absolute directory path of the RocksDB files, for logging.
    path: PathBuf,
}

impl Db {
    /// Open (or create) an embedded SurrealDB instance rooted at `path`,
    /// use namespace `listenai` and database `main`, and return a clonable
    /// handle. The caller is responsible for running migrations afterwards.
    pub async fn open(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref().to_path_buf();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| Error::Database(format!("create {parent:?}: {e}")))?;
        }
        std::fs::create_dir_all(&path)
            .map_err(|e| Error::Database(format!("create {path:?}: {e}")))?;

        info!(path = %path.display(), "opening surrealdb (rocksdb)");

        let inner: Surreal<Engine> = Surreal::new::<RocksDb>(path.to_string_lossy().as_ref())
            .await
            .map_err(|e| Error::Database(format!("open surrealdb: {e}")))?;

        inner
            .use_ns("listenai")
            .use_db("main")
            .await
            .map_err(|e| Error::Database(format!("select ns/db: {e}")))?;

        Ok(Self { inner, path })
    }

    pub fn inner(&self) -> &Surreal<Engine> {
        &self.inner
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
}
