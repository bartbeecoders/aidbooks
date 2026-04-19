//! Tiny, hand-rolled forward-only migration runner.
//!
//! Migrations are `.surql` files embedded at compile time via `include_str!`.
//! We record each applied migration's name in a `_migrations` table and skip
//! those that have already been applied. The runner is intentionally simple:
//! migrations are stored in a fixed ordered list rather than discovered
//! dynamically at runtime, so `cargo build` fails early if a file is renamed.

use crate::Db;
use listenai_core::{Error, Result};
use serde::Deserialize;
use tracing::{info, warn};

struct Migration {
    name: &'static str,
    sql: &'static str,
}

const MIGRATIONS: &[Migration] = &[Migration {
    name: "0001_init",
    sql: include_str!("../migrations/0001_init.surql"),
}];

/// Run any pending migrations. Safe to call on every startup.
pub async fn run(db: &Db) -> Result<()> {
    ensure_migrations_table(db).await?;
    let applied = applied_names(db).await?;

    let mut ran = 0usize;
    for m in MIGRATIONS {
        if applied.iter().any(|n| n == m.name) {
            continue;
        }
        info!(name = m.name, "applying migration");
        db.inner()
            .query(m.sql)
            .await
            .map_err(|e| Error::Database(format!("run {}: {e}", m.name)))?
            .check()
            .map_err(|e| Error::Database(format!("run {}: {e}", m.name)))?;

        // Record success. We escape the name into an explicit id so retries
        // are idempotent even if the process crashes between query and record.
        let record_sql = format!(
            "CREATE _migrations:`{name}` CONTENT {{ name: '{name}', applied_at: time::now() }}",
            name = m.name
        );
        db.inner()
            .query(record_sql)
            .await
            .map_err(|e| Error::Database(format!("record {}: {e}", m.name)))?
            .check()
            .map_err(|e| Error::Database(format!("record {}: {e}", m.name)))?;
        ran += 1;
    }

    if ran == 0 {
        info!("schema up to date");
    } else {
        info!(count = ran, "migrations applied");
    }
    Ok(())
}

async fn ensure_migrations_table(db: &Db) -> Result<()> {
    let sql = r#"
        DEFINE TABLE IF NOT EXISTS _migrations SCHEMAFULL;
        DEFINE FIELD IF NOT EXISTS name ON _migrations TYPE string;
        DEFINE FIELD IF NOT EXISTS applied_at ON _migrations TYPE datetime
            VALUE $before OR time::now() DEFAULT time::now();
    "#;
    db.inner()
        .query(sql)
        .await
        .map_err(|e| Error::Database(format!("create _migrations: {e}")))?
        .check()
        .map_err(|e| Error::Database(format!("create _migrations: {e}")))?;
    Ok(())
}

async fn applied_names(db: &Db) -> Result<Vec<String>> {
    #[derive(Deserialize)]
    struct Row {
        name: String,
    }
    let rows: Vec<Row> = db
        .inner()
        .query("SELECT name FROM _migrations")
        .await
        .map_err(|e| Error::Database(format!("list _migrations: {e}")))?
        .take(0)
        .map_err(|e| {
            warn!(error = %e, "could not read _migrations rows");
            Error::Database(format!("read _migrations: {e}"))
        })?;
    Ok(rows.into_iter().map(|r| r.name).collect())
}
