pub mod openrouter;

pub use openrouter::{ChatMessage, ChatRequest, ChatResponse, ChatUsage, LlmClient};

use listenai_core::domain::LlmRole;
use listenai_core::{Error, Result};
use serde::Deserialize;

use crate::state::AppState;

/// Pick a model where the **first matching role wins**. Useful for roles
/// that should sensibly degrade to a more general one (e.g. `Translate`
/// → `Chapter` when no row is explicitly tagged for translation). The
/// language preference applies independently within each role.
///
/// Selection order, applied in turn:
///   1. enabled rows where `default_for` contains the role
///   2. if `language` is given, prefer rows whose `languages` is empty
///      (means "any language") OR contains the language code
///   3. lowest `priority` first; fall back to alphabetical name as a stable
///      tiebreaker
///
/// Falls back to `Config.openrouter_default_model` when *no* role matches,
/// returning `_default_` as the LLM record id — generation_event rows from
/// the fallback path will reference a non-existent `llm:_default_` row,
/// which is fine since SurrealDB doesn't enforce FK existence.
pub async fn pick_llm_for_roles_lang(
    state: &AppState,
    roles: &[LlmRole],
    language: Option<&str>,
) -> Result<PickedLlm> {
    if roles.is_empty() {
        return Err(Error::Other(anyhow::anyhow!("pick_llm: empty roles")));
    }

    for role in roles {
        if let Some(picked) = try_pick_role_lang(state, *role, language).await? {
            return Ok(picked);
        }
    }

    let fallback = state.config().openrouter_default_model.trim().to_string();
    if fallback.is_empty() {
        let names: Vec<String> = roles
            .iter()
            .filter_map(|r| {
                serde_json::to_value(*r)
                    .ok()
                    .and_then(|v| v.as_str().map(str::to_string))
            })
            .collect();
        return Err(Error::Validation(format!(
            "no llm marked default_for any of [{}] and no fallback configured",
            names.join(", ")
        )));
    }
    Ok(PickedLlm {
        llm_id: "_default_".into(),
        provider: "open_router".into(),
        model_id: fallback,
    })
}

/// Convenience variant for callers that already have a single role.
pub async fn pick_llm_for_role(state: &AppState, role: LlmRole) -> Result<PickedLlm> {
    pick_llm_for_roles_lang(state, &[role], None).await
}

/// LLM record id + upstream model slug for the picked row.
#[derive(Debug, Clone)]
pub struct PickedLlm {
    /// SurrealDB record id (e.g. `gemini_flash_image`). Used to write the
    /// `llm:<id>` reference in `generation_event`.
    pub llm_id: String,
    /// Provider this row routes through (`open_router` | `xai`). Drives
    /// host + auth selection at the chat layer.
    pub provider: String,
    /// Upstream model slug (e.g. `google/gemini-2.5-flash-image`). Sent to
    /// the provider's `model` field.
    pub model_id: String,
}

/// Single-role lookup that returns `None` when nothing matches — caller
/// owns the fallback policy.
async fn try_pick_role_lang(
    state: &AppState,
    role: LlmRole,
    language: Option<&str>,
) -> Result<Option<PickedLlm>> {
    let role_str = serde_json::to_value(role)
        .ok()
        .and_then(|v| v.as_str().map(str::to_string))
        .ok_or_else(|| Error::Other(anyhow::anyhow!("encode role")))?;

    #[derive(Deserialize)]
    struct Row {
        id: surrealdb::sql::Thing,
        model_id: String,
        #[serde(default)]
        provider: Option<String>,
    }

    let lang = language
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string);

    // Two-step pick: first try language-matched rows, then any. Each step
    // is one query so SurrealDB can use the (status, kind) index path.
    // SurrealDB 2.6 insists every ORDER BY field appears in the projection,
    // so we select `priority, name` even though we only consume `model_id`.
    if let Some(lang) = &lang {
        let rows: Vec<Row> = state
            .db()
            .inner()
            .query(
                "SELECT id, model_id, provider, priority, name FROM llm \
                 WHERE enabled = true \
                   AND $r INSIDE default_for \
                   AND (array::len(languages) = 0 OR $lang INSIDE languages) \
                 ORDER BY priority ASC, name ASC LIMIT 1",
            )
            .bind(("r", role_str.clone()))
            .bind(("lang", lang.clone()))
            .await
            .map_err(|e| Error::Database(format!("pick_model_for_role: {e}")))?
            .take(0)
            .map_err(|e| Error::Database(format!("pick_model_for_role (decode): {e}")))?;
        if let Some(row) = rows.into_iter().next() {
            return Ok(Some(PickedLlm {
                llm_id: row.id.id.to_raw(),
                provider: row.provider.unwrap_or_else(|| "open_router".into()),
                model_id: row.model_id,
            }));
        }
    }

    let rows: Vec<Row> = state
        .db()
        .inner()
        .query(
            "SELECT id, model_id, provider, priority, name FROM llm \
             WHERE enabled = true AND $r INSIDE default_for \
             ORDER BY priority ASC, name ASC LIMIT 1",
        )
        .bind(("r", role_str.clone()))
        .await
        .map_err(|e| Error::Database(format!("pick_model_for_role: {e}")))?
        .take(0)
        .map_err(|e| Error::Database(format!("pick_model_for_role (decode): {e}")))?;
    Ok(rows.into_iter().next().map(|row| PickedLlm {
        llm_id: row.id.id.to_raw(),
        provider: row.provider.unwrap_or_else(|| "open_router".into()),
        model_id: row.model_id,
    }))
}
