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

const MIGRATIONS: &[Migration] = &[
    Migration {
        name: "0001_init",
        sql: include_str!("../migrations/0001_init.surql"),
    },
    Migration {
        name: "0002_session",
        sql: include_str!("../migrations/0002_session.surql"),
    },
    Migration {
        name: "0003_content",
        sql: include_str!("../migrations/0003_content.surql"),
    },
    Migration {
        name: "0004_jobs",
        sql: include_str!("../migrations/0004_jobs.surql"),
    },
    Migration {
        name: "0005_cover",
        sql: include_str!("../migrations/0005_cover.surql"),
    },
    Migration {
        name: "0006_language",
        sql: include_str!("../migrations/0006_language.surql"),
    },
    Migration {
        name: "0007_chapter_lang",
        sql: include_str!("../migrations/0007_chapter_lang.surql"),
    },
    Migration {
        name: "0008_job_lang",
        sql: include_str!("../migrations/0008_job_lang.surql"),
    },
    Migration {
        name: "0009_translate_kind",
        sql: include_str!("../migrations/0009_translate_kind.surql"),
    },
    Migration {
        name: "0010_youtube",
        sql: include_str!("../migrations/0010_youtube.surql"),
    },
    Migration {
        name: "0011_chapter_art",
        sql: include_str!("../migrations/0011_chapter_art.surql"),
    },
    Migration {
        name: "0012_topic_template",
        sql: include_str!("../migrations/0012_topic_template.surql"),
    },
    Migration {
        name: "0013_art_style",
        sql: include_str!("../migrations/0013_art_style.surql"),
    },
    Migration {
        name: "0014_llm_meta",
        sql: include_str!("../migrations/0014_llm_meta.surql"),
    },
    Migration {
        name: "0015_cover_llm",
        sql: include_str!("../migrations/0015_cover_llm.surql"),
    },
    Migration {
        name: "0016_youtube_playlist",
        sql: include_str!("../migrations/0016_youtube_playlist.surql"),
    },
    Migration {
        name: "0017_youtube_review",
        sql: include_str!("../migrations/0017_youtube_review.surql"),
    },
    Migration {
        name: "0018_image_llm_pricing",
        sql: include_str!("../migrations/0018_image_llm_pricing.surql"),
    },
    Migration {
        name: "0019_image_llm_pricing_backfill",
        sql: include_str!("../migrations/0019_image_llm_pricing_backfill.surql"),
    },
    Migration {
        name: "0020_audiobook_auto_pipeline",
        sql: include_str!("../migrations/0020_audiobook_auto_pipeline.surql"),
    },
    Migration {
        name: "0021_audiobook_auto_pipeline_flexible",
        sql: include_str!("../migrations/0021_audiobook_auto_pipeline_flexible.surql"),
    },
    Migration {
        name: "0022_chapter_images",
        sql: include_str!("../migrations/0022_chapter_images.surql"),
    },
    Migration {
        name: "0023_paragraph_illustrations",
        sql: include_str!("../migrations/0023_paragraph_illustrations.surql"),
    },
    Migration {
        name: "0024_llm_provider_xai",
        sql: include_str!("../migrations/0024_llm_provider_xai.surql"),
    },
    Migration {
        name: "0025_youtube_description_footer",
        sql: include_str!("../migrations/0025_youtube_description_footer.surql"),
    },
    Migration {
        name: "0026_audiobook_category",
        sql: include_str!("../migrations/0026_audiobook_category.surql"),
    },
    Migration {
        name: "0027_audiobook_category_table",
        sql: include_str!("../migrations/0027_audiobook_category_table.surql"),
    },
    Migration {
        name: "0028_audiobook_tags",
        sql: include_str!("../migrations/0028_audiobook_tags.surql"),
    },
    Migration {
        name: "0029_podcast",
        sql: include_str!("../migrations/0029_podcast.surql"),
    },
    Migration {
        name: "0030_audiobook_tags_backfill",
        sql: include_str!("../migrations/0030_audiobook_tags_backfill.surql"),
    },
    Migration {
        name: "0031_audiobook_short",
        sql: include_str!("../migrations/0031_audiobook_short.surql"),
    },
    Migration {
        name: "0032_animate",
        sql: include_str!("../migrations/0032_animate.surql"),
    },
    Migration {
        name: "0033_audiobook_stem",
        sql: include_str!("../migrations/0033_audiobook_stem.surql"),
    },
    Migration {
        name: "0034_prompt_visual_role",
        sql: include_str!("../migrations/0034_prompt_visual_role.surql"),
    },
    Migration {
        name: "0035_manim_code_role",
        sql: include_str!("../migrations/0035_manim_code_role.surql"),
    },
    Migration {
        name: "0036_youtube_publish_settings",
        sql: include_str!("../migrations/0036_youtube_publish_settings.surql"),
    },
    Migration {
        name: "0037_idea",
        sql: include_str!("../migrations/0037_idea.surql"),
    },
    Migration {
        name: "0038_multi_voice",
        sql: include_str!("../migrations/0038_multi_voice.surql"),
    },
    Migration {
        name: "0039_youtube_like_subscribe_overlay",
        sql: include_str!("../migrations/0039_youtube_like_subscribe_overlay.surql"),
    },
    Migration {
        name: "0040_youtube_publication_overlay",
        sql: include_str!("../migrations/0040_youtube_publication_overlay.surql"),
    },
];

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
