//! Prompt-template loading + `{{var}}` rendering.
//!
//! Templates live in the `prompt_template` DB table (seeded at startup).
//! The renderer is intentionally minimal — we keep full Tera/Handlebars out
//! of the dependency graph since the prompts only need flat variable
//! substitution.

use std::collections::HashMap;

use listenai_core::domain::PromptRole;
use listenai_core::{Error, Result};
use serde::Deserialize;

use crate::state::AppState;

#[derive(Debug, Clone, Deserialize)]
struct DbPrompt {
    body: String,
    variables: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct RenderedPrompt {
    pub body: String,
    /// The `variables` list declared by the admin on the template. Surfaced
    /// here so a future admin-editor can warn when a prompt uses a marker
    /// that isn't declared, or vice versa. Unused by the generation layer.
    #[allow(dead_code)]
    pub declared_variables: Vec<String>,
}

/// Fetch the currently-active template for `role` and render it against
/// `vars`. Returns an error if no active template exists.
pub async fn render(
    state: &AppState,
    role: PromptRole,
    vars: &HashMap<&str, String>,
) -> Result<RenderedPrompt> {
    let role_str = match role {
        PromptRole::Outline => "outline",
        PromptRole::Chapter => "chapter",
        PromptRole::RandomTopic => "random_topic",
        PromptRole::Moderation => "moderation",
        PromptRole::Title => "title",
        PromptRole::Cover => "cover",
        PromptRole::ParagraphImage => "paragraph_image",
        PromptRole::Translate => "translate",
        PromptRole::SceneExtract => "scene_extract",
    };

    let rows: Vec<DbPrompt> = state
        .db()
        .inner()
        .query(
            "SELECT body, variables, version FROM prompt_template \
             WHERE role = $role AND active = true ORDER BY version DESC LIMIT 1",
        )
        .bind(("role", role_str.to_string()))
        .await
        .map_err(|e| Error::Database(format!("load prompt: {e}")))?
        .take(0)
        .map_err(|e| Error::Database(format!("load prompt (decode): {e}")))?;

    let tpl = rows
        .into_iter()
        .next()
        .ok_or_else(|| Error::Database(format!("no active prompt template for role {role_str}")))?;

    Ok(RenderedPrompt {
        body: interpolate(&tpl.body, vars),
        declared_variables: tpl.variables,
    })
}

/// Replace every `{{name}}` occurrence with `vars[name]` if present, else
/// leave the marker in place (so template bugs are visible in output).
fn interpolate(body: &str, vars: &HashMap<&str, String>) -> String {
    let mut out = String::with_capacity(body.len());
    let bytes = body.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if i + 1 < bytes.len() && bytes[i] == b'{' && bytes[i + 1] == b'{' {
            if let Some(end) = find_close(body, i + 2) {
                let name = body[i + 2..end].trim();
                if let Some(val) = vars.get(name) {
                    out.push_str(val);
                } else {
                    out.push_str(&body[i..end + 2]);
                }
                i = end + 2;
                continue;
            }
        }
        out.push(bytes[i] as char);
        i += 1;
    }
    out
}

fn find_close(body: &str, start: usize) -> Option<usize> {
    let bytes = body.as_bytes();
    let mut i = start;
    while i + 1 < bytes.len() {
        if bytes[i] == b'}' && bytes[i + 1] == b'}' {
            return Some(i);
        }
        i += 1;
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn interpolates_simple_vars() {
        let mut vars = HashMap::new();
        vars.insert("topic", "space".to_string());
        vars.insert("count", "3".to_string());
        let out = interpolate("Write about {{topic}} across {{count}} chapters.", &vars);
        assert_eq!(out, "Write about space across 3 chapters.");
    }

    #[test]
    fn leaves_unknown_markers_alone() {
        let vars = HashMap::new();
        let out = interpolate("Hello {{missing}}", &vars);
        assert_eq!(out, "Hello {{missing}}");
    }

    #[test]
    fn passes_through_literal_braces_used_in_json_examples() {
        let mut vars = HashMap::new();
        vars.insert("x", "ok".to_string());
        // Single-brace JSON like `{"k": 1}` must not be touched.
        let out = interpolate("json: {\"k\": 1} and {{x}}", &vars);
        assert_eq!(out, "json: {\"k\": 1} and ok");
    }
}
