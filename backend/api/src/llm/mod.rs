pub mod openrouter;

pub use openrouter::{ChatMessage, ChatRequest, ChatResponse, LlmClient};

use listenai_core::domain::LlmRole;
use listenai_core::{Error, Result};
use serde::Deserialize;

use crate::state::AppState;

/// Pick the upstream `model_id` for a given role.
///
/// Strategy: first enabled `llm` row whose `default_for` contains the role.
/// Falls back to `Config.openrouter_default_model` so existing flows that
/// don't set a per-role default keep working. Returns `Err(Validation)`
/// only when both lookups produce nothing usable.
pub async fn pick_model_for_role(state: &AppState, role: LlmRole) -> Result<String> {
    let role_str = serde_json::to_value(role)
        .ok()
        .and_then(|v| v.as_str().map(str::to_string))
        .ok_or_else(|| Error::Other(anyhow::anyhow!("encode role")))?;

    #[derive(Deserialize)]
    struct Row {
        model_id: String,
    }
    let rows: Vec<Row> = state
        .db()
        .inner()
        .query(
            "SELECT model_id, name FROM llm \
             WHERE enabled = true AND $r INSIDE default_for \
             ORDER BY name ASC LIMIT 1",
        )
        .bind(("r", role_str.clone()))
        .await
        .map_err(|e| Error::Database(format!("pick_model_for_role: {e}")))?
        .take(0)
        .map_err(|e| Error::Database(format!("pick_model_for_role (decode): {e}")))?;

    if let Some(row) = rows.into_iter().next() {
        return Ok(row.model_id);
    }

    let fallback = state.config().openrouter_default_model.trim().to_string();
    if fallback.is_empty() {
        return Err(Error::Validation(format!(
            "no llm marked default_for `{role_str}` and no fallback configured"
        )));
    }
    Ok(fallback)
}
