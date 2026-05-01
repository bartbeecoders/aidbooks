//! Idempotent seed data.
//!
//! Always runs on startup:
//!   * 5 x.ai voices
//!   * 2 default OpenRouter LLM configs
//!
//! Runs only when `dev_seed` is true (set via `LISTENAI_DEV_SEED=true`):
//!   * demo admin user — email `demo@listenai.local`, password `demo`
//!
//! `UPSERT`/`MERGE` make every seed step safe to re-run.

use crate::Db;
use listenai_core::{crypto, Error, Result};
use serde_json::{json, Value};
use tracing::{info, warn};

pub async fn run(db: &Db, dev_seed: bool, password_pepper: &str) -> Result<()> {
    seed_voices(db).await?;
    seed_llms(db).await?;
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
    // Mirrors https://docs.x.ai/developers/model-capabilities/audio/voice-agent
    let voices: &[Value] = &[
        json!({ "id": "eve", "name": "Eve", "gender": "female", "accent": "energetic" }),
        json!({ "id": "ara", "name": "Ara", "gender": "female", "accent": "warm" }),
        json!({ "id": "rex", "name": "Rex", "gender": "male",   "accent": "confident" }),
        json!({ "id": "sal", "name": "Sal", "gender": "neutral","accent": "smooth" }),
        json!({ "id": "leo", "name": "Leo", "gender": "male",   "accent": "authoritative" }),
    ];

    for v in voices {
        let id = v["id"].as_str().expect("seed voice id");
        let sql = format!(
            r#"UPSERT voice:`{id}` CONTENT {{
                name: $name,
                provider: "xai",
                provider_voice_id: $voice_id,
                gender: $gender,
                accent: $accent,
                language: "en",
                enabled: true,
                premium_only: false
            }}"#
        );
        db.inner()
            .query(&sql)
            .bind(("name", v["name"].as_str().unwrap().to_string()))
            .bind(("voice_id", id.to_string()))
            .bind(("gender", v["gender"].as_str().unwrap().to_string()))
            .bind(("accent", v["accent"].as_str().unwrap().to_string()))
            .await
            .map_err(|e| Error::Database(format!("seed voice {id}: {e}")))?
            .check()
            .map_err(|e| Error::Database(format!("seed voice {id}: {e}")))?;
    }
    info!(count = voices.len(), "voices seeded");
    Ok(())
}

async fn seed_llms(db: &Db) -> Result<()> {
    let llms: &[Value] = &[
        json!({
            "id":            "claude_sonnet_4_6",
            "name":          "Claude Sonnet 4.6",
            "model_id":      "anthropic/claude-sonnet-4.6",
            "context":       200000,
            "cost_prompt":   3.0,
            "cost_completion": 15.0,
            "default_for":   ["outline", "chapter"],
            "function":      "text",
            "languages":     [],
            "priority":      10,
        }),
        json!({
            "id":            "claude_haiku_4_5",
            "name":          "Claude Haiku 4.5",
            "model_id":      "anthropic/claude-haiku-4.5",
            "context":       200000,
            "cost_prompt":   0.25,
            "cost_completion": 1.25,
            "default_for":   ["random_topic", "title"],
            "function":      "text",
            "languages":     [],
            "priority":      20,
        }),
    ];

    for l in llms {
        let id = l["id"].as_str().expect("seed llm id");
        let default_for: Vec<String> = l["default_for"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap().to_string())
            .collect();
        let languages: Vec<String> = l["languages"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap().to_string())
            .collect();
        // UPSERT MERGE so re-running doesn't clobber an admin's tweaks to
        // priority/cost/etc. on existing rows.
        let sql = format!(
            r#"UPSERT llm:`{id}` MERGE {{
                name: $name,
                provider: "open_router",
                model_id: $model_id,
                context_window: $context,
                cost_prompt_per_1k: $cost_p,
                cost_completion_per_1k: $cost_c,
                cost_per_megapixel: 0.0,
                enabled: true,
                default_for: $default_for,
                function: $function,
                languages: $languages,
                priority: $priority
            }}"#
        );
        db.inner()
            .query(&sql)
            .bind(("name", l["name"].as_str().unwrap().to_string()))
            .bind(("model_id", l["model_id"].as_str().unwrap().to_string()))
            .bind(("context", l["context"].as_i64().unwrap()))
            .bind(("cost_p", l["cost_prompt"].as_f64().unwrap()))
            .bind(("cost_c", l["cost_completion"].as_f64().unwrap()))
            .bind(("default_for", default_for))
            .bind(("function", l["function"].as_str().unwrap().to_string()))
            .bind(("languages", languages))
            .bind(("priority", l["priority"].as_i64().unwrap()))
            .await
            .map_err(|e| Error::Database(format!("seed llm {id}: {e}")))?
            .check()
            .map_err(|e| Error::Database(format!("seed llm {id}: {e}")))?;
    }
    info!(count = llms.len(), "llms seeded");
    Ok(())
}
