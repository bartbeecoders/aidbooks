//! Idempotent seed data.
//!
//! Always runs on startup:
//!   * the x.ai voice catalogue (snapshot of `GET /v1/tts/voices`)
//!   * the prompt library
//!
//! Runs only when `dev_seed` is true (set via `LISTENAI_DEV_SEED=true`):
//!   * demo admin user — email `demo@listenai.local`, password `demo`
//!
//! LLM rows are *not* seeded — admins manage them via the admin UI.
//! Re-seeding them on every boot would clobber any priority / cost /
//! `default_for` tweaks made there.
//!
//! `UPSERT`/`MERGE` make every seed step safe to re-run.

use crate::Db;
use listenai_core::{crypto, Error, Result};
use serde_json::{json, Value};
use tracing::{info, warn};

pub async fn run(db: &Db, dev_seed: bool, password_pepper: &str) -> Result<()> {
    seed_voices(db).await?;
    seed_prompts(db).await?;
    if dev_seed {
        seed_demo_admin(db, password_pepper).await?;
    }
    Ok(())
}

/// Upsert a well-known admin user `demo@listenai.local` / `demo`.
/// Loud warning on every startup so nobody leaves this on in prod.
async fn seed_demo_admin(db: &Db, pepper: &str) -> Result<()> {
    const EMAIL: &str = "demo@listenai.local";
    const PW: &str = "demo";
    const ID: &str = "demo_admin";

    warn!(
        email = EMAIL,
        "DEV SEED: upserting demo admin — DO NOT enable in production"
    );

    let password_hash = crypto::hash_password(PW, pepper.as_bytes())?;

    let sql = format!(
        r#"UPSERT user:`{ID}` MERGE {{
            email: $email,
            display_name: "Demo Admin",
            role: "admin",
            tier: "pro",
            password_hash: $hash,
            email_verified_at: time::now()
        }}"#
    );
    db.inner()
        .query(&sql)
        .bind(("email", EMAIL.to_string()))
        .bind(("hash", password_hash))
        .await
        .map_err(|e| Error::Database(format!("seed demo admin: {e}")))?
        .check()
        .map_err(|e| Error::Database(format!("seed demo admin: {e}")))?;
    Ok(())
}

async fn seed_prompts(db: &Db) -> Result<()> {
    let prompts: &[Value] = &[
        json!({
            "id": "outline_v1",
            "role": "outline",
            "variables": [
                "topic", "length", "genre", "chapter_count",
                "words_per_chapter", "language"
            ],
            "body": include_str!("prompts/outline_v1.md"),
        }),
        json!({
            "id": "chapter_v1",
            "role": "chapter",
            "variables": [
                "book_title", "chapter_number", "chapter_title",
                "chapter_synopsis", "target_words", "genre",
                "previous_ending", "language", "tags"
            ],
            "body": include_str!("prompts/chapter_v1.md"),
        }),
        json!({
            "id": "random_topic_v1",
            "role": "random_topic",
            "variables": ["seed", "language"],
            "body": include_str!("prompts/random_topic_v1.md"),
        }),
        json!({
            "id": "paragraph_visual_v1",
            "role": "paragraph_visual",
            "variables": [
                "book_title", "book_topic", "genre",
                "chapter_title", "paragraph_listing"
            ],
            "body": include_str!("prompts/paragraph_visual_v1.md"),
        }),
        json!({
            "id": "manim_code_v1",
            "role": "manim_code",
            "variables": [
                "book_title", "book_topic", "genre",
                "chapter_title", "theme", "run_seconds",
                "paragraph_text"
            ],
            "body": include_str!("prompts/manim_code_v1.md"),
        }),
        json!({
            "id": "voice_extract_v1",
            "role": "voice_extract",
            "variables": ["chapter_title", "chapter_body"],
            "body": include_str!("prompts/voice_extract_v1.md"),
        }),
    ];

    for p in prompts {
        let id = p["id"].as_str().expect("seed prompt id");
        let variables: Vec<String> = p["variables"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap().to_string())
            .collect();
        let sql = format!(
            r#"UPSERT prompt_template:`{id}` MERGE {{
                role: $role,
                body: $body,
                version: 1,
                active: true,
                variables: $variables
            }}"#
        );
        db.inner()
            .query(&sql)
            .bind(("role", p["role"].as_str().unwrap().to_string()))
            .bind(("body", p["body"].as_str().unwrap().to_string()))
            .bind(("variables", variables))
            .await
            .map_err(|e| Error::Database(format!("seed prompt {id}: {e}")))?
            .check()
            .map_err(|e| Error::Database(format!("seed prompt {id}: {e}")))?;
    }
    info!(count = prompts.len(), "prompts seeded");
    Ok(())
}

async fn seed_voices(db: &Db) -> Result<()> {
    // Snapshot of `GET https://api.x.ai/v1/tts/voices` — refresh by re-running
    // that endpoint and writing `.voices` to `xai_voices.json`. Each entry has
    // `voice_id`, `name`, `language` (BCP-47 or `multilingual`), `gender`, and
    // an optional `age` we map onto the `accent` column.
    const RAW: &str = include_str!("xai_voices.json");
    let voices: Vec<Value> = serde_json::from_str(RAW)
        .map_err(|e| Error::Database(format!("parse xai_voices.json: {e}")))?;

    for v in &voices {
        let id = v["voice_id"].as_str().expect("seed voice id");
        let name = v["name"].as_str().expect("seed voice name");
        let gender = v["gender"].as_str().expect("seed voice gender");
        let language = v["language"].as_str().expect("seed voice language");
        let accent = v.get("age").and_then(|a| a.as_str()).unwrap_or("");

        let sql = format!(
            r#"UPSERT voice:`{id}` CONTENT {{
                name: $name,
                provider: "xai",
                provider_voice_id: $voice_id,
                gender: $gender,
                accent: $accent,
                language: $language,
                enabled: true,
                premium_only: false
            }}"#
        );
        db.inner()
            .query(&sql)
            .bind(("name", name.to_string()))
            .bind(("voice_id", id.to_string()))
            .bind(("gender", gender.to_string()))
            .bind(("accent", accent.to_string()))
            .bind(("language", language.to_string()))
            .await
            .map_err(|e| Error::Database(format!("seed voice {id}: {e}")))?
            .check()
            .map_err(|e| Error::Database(format!("seed voice {id}: {e}")))?;
    }
    info!(count = voices.len(), "voices seeded");
    Ok(())
}

