//! Idempotent seed data: x.ai voice catalogue and two default OpenRouter
//! LLM entries. Runs on every startup; `UPSERT` keeps it safe.
//!
//! Admin-user seeding is deferred to Phase 2 (requires password hashing).

use crate::Db;
use listenai_core::{Error, Result};
use serde_json::{json, Value};
use tracing::info;

pub async fn run(db: &Db) -> Result<()> {
    seed_voices(db).await?;
    seed_llms(db).await?;
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
            "default_for":   ["outline", "chapter"]
        }),
        json!({
            "id":            "claude_haiku_4_5",
            "name":          "Claude Haiku 4.5",
            "model_id":      "anthropic/claude-haiku-4.5",
            "context":       200000,
            "cost_prompt":   0.25,
            "cost_completion": 1.25,
            "default_for":   ["random_topic", "title"]
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
        let sql = format!(
            r#"UPSERT llm:`{id}` CONTENT {{
                name: $name,
                provider: "open_router",
                model_id: $model_id,
                context_window: $context,
                cost_prompt_per_1k: $cost_p,
                cost_completion_per_1k: $cost_c,
                enabled: true,
                default_for: $default_for
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
            .await
            .map_err(|e| Error::Database(format!("seed llm {id}: {e}")))?
            .check()
            .map_err(|e| Error::Database(format!("seed llm {id}: {e}")))?;
    }
    info!(count = llms.len(), "llms seeded");
    Ok(())
}
