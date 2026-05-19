pub mod fal;
pub mod mold;
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
        base_url: None,
        api_key: None,
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
    /// Provider this row routes through (`open_router` | `xai` | `openai` |
    /// `mold` | `fal`). Drives host + auth selection at the chat layer.
    pub provider: String,
    /// Upstream model slug (e.g. `google/gemini-2.5-flash-image`). Sent to
    /// the provider's `model` field.
    pub model_id: String,
    /// OpenAI-compat base URL (only set when `provider = "openai"`).
    pub base_url: Option<String>,
    /// Decrypted API key for openai-compat rows. `None` when the row has
    /// no key (LMStudio's default) or the provider isn't `openai`.
    pub api_key: Option<String>,
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

    let lang = language
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string);

    // Image-output roles must come from image-function rows. Without this
    // filter, a text row that happens to have `cover_art` ticked in its
    // `default_for` would be picked and then routed to the provider's
    // image endpoint (xAI grok-4.3 → /images/generations 400).
    let func_filter = matches!(role, LlmRole::CoverArt);

    // Two-step pick: first try language-matched rows, then any. Each step
    // is one query so SurrealDB can use the (status, kind) index path.
    // SurrealDB 2.6 insists every ORDER BY field appears in the projection,
    // so we select `priority, name` even though we only consume `model_id`.
    if let Some(lang) = &lang {
        let rows: Vec<PickedRow> = state
            .db()
            .inner()
            .query(
                "SELECT id, model_id, provider, base_url, api_key_enc, priority, name FROM llm \
                 WHERE enabled = true \
                   AND $r INSIDE default_for \
                   AND (!$image_only OR `function` = 'image') \
                   AND (array::len(languages) = 0 OR $lang INSIDE languages) \
                 ORDER BY priority ASC, name ASC LIMIT 1",
            )
            .bind(("r", role_str.clone()))
            .bind(("image_only", func_filter))
            .bind(("lang", lang.clone()))
            .await
            .map_err(|e| Error::Database(format!("pick_model_for_role: {e}")))?
            .take(0)
            .map_err(|e| Error::Database(format!("pick_model_for_role (decode): {e}")))?;
        if let Some(row) = rows.into_iter().next() {
            return Ok(Some(decode_picked(state, row)));
        }
    }

    let rows: Vec<PickedRow> = state
        .db()
        .inner()
        .query(
            "SELECT id, model_id, provider, base_url, api_key_enc, priority, name FROM llm \
             WHERE enabled = true \
               AND $r INSIDE default_for \
               AND (!$image_only OR `function` = 'image') \
             ORDER BY priority ASC, name ASC LIMIT 1",
        )
        .bind(("r", role_str.clone()))
        .bind(("image_only", func_filter))
        .await
        .map_err(|e| Error::Database(format!("pick_model_for_role: {e}")))?
        .take(0)
        .map_err(|e| Error::Database(format!("pick_model_for_role (decode): {e}")))?;
    Ok(rows.into_iter().next().map(|row| decode_picked(state, row)))
}

/// Decode a picker row into a `PickedLlm`, decrypting the openai-compat
/// API key when present. A decrypt failure is treated as "no key" rather
/// than poisoning the whole pick — the row still has a `base_url`, and
/// the upstream call surfaces a clearer 401 than a generic decode error.
fn decode_picked(state: &AppState, row: PickedRow) -> PickedLlm {
    let provider = row.provider.unwrap_or_else(|| "open_router".into());
    let base_url = row.base_url.filter(|s| !s.trim().is_empty());
    let api_key = row.api_key_enc.as_deref().and_then(|enc| {
        match crate::youtube::encrypt::decrypt_with_domain(
            enc,
            state.config().password_pepper.as_bytes(),
            crate::youtube::encrypt::LLM_API_KEY_DOMAIN,
        ) {
            Ok(key) => Some(key),
            Err(e) => {
                tracing::warn!(
                    llm_id = %row.id.id.to_raw(),
                    error = %e,
                    "decrypt llm api_key failed; using row without key"
                );
                None
            }
        }
    });
    PickedLlm {
        llm_id: row.id.id.to_raw(),
        provider,
        model_id: row.model_id,
        base_url,
        api_key,
    }
}

/// Decoded subset of an `llm` row used by the picker. Module-scoped so
/// `try_pick_role_lang` and `decode_picked` share the same shape.
#[derive(Deserialize)]
struct PickedRow {
    id: surrealdb::sql::Thing,
    model_id: String,
    #[serde(default)]
    provider: Option<String>,
    #[serde(default)]
    base_url: Option<String>,
    #[serde(default)]
    api_key_enc: Option<String>,
}
