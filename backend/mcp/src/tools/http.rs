//! HTTP-proxy tools generated from the api's OpenAPI spec.
//!
//! For each `(path, method)` pair we emit one MCP tool whose input schema
//! has a property per path/query parameter plus the request body fields
//! (flattened into the top level when the body is a JSON object). On call,
//! we substitute path params, attach query params, build the body, and
//! forward to the api with the caller's bearer token.

use super::{ProgressSink, Registry, ToolHandler};
use crate::http_client::ApiClient;
use crate::proto::{CallToolResult, Tool};
use serde_json::{json, Map, Value};
use std::collections::BTreeMap;
use std::sync::Arc;

const TOKEN_FIELD: &str = "_token";

/// Walk the OpenAPI spec and register one tool per operation. Returns the
/// number of tools added.
pub fn register_from_openapi(
    reg: &mut Registry,
    client: Arc<ApiClient>,
    spec: &Value,
) -> anyhow::Result<usize> {
    let paths = spec
        .get("paths")
        .and_then(|p| p.as_object())
        .ok_or_else(|| anyhow::anyhow!("openapi: no `paths` object"))?;

    let components = spec.get("components").cloned().unwrap_or(Value::Null);

    let mut count = 0usize;
    let mut used_names = std::collections::BTreeSet::<String>::new();

    for (raw_path, methods) in paths {
        let methods = match methods.as_object() {
            Some(m) => m,
            None => continue,
        };
        // Path-level common parameters apply to every operation under this
        // path. utoipa rarely emits them but we honour the spec.
        let path_params = methods
            .get("parameters")
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_default();

        for (method, op) in methods {
            if !is_http_method(method) {
                continue;
            }
            let op = match op.as_object() {
                Some(o) => o,
                None => continue,
            };
            let derived_name = derive_tool_name(op, method, raw_path);
            let name = unique(&mut used_names, derived_name);

            let summary = op
                .get("summary")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let description = op
                .get("description")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let tag_list = op
                .get("tags")
                .and_then(|v| v.as_array())
                .map(|arr| {
                    arr.iter()
                        .filter_map(|t| t.as_str().map(str::to_string))
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default();

            // Merge path-level and operation-level parameters.
            let mut params: Vec<Value> = path_params.clone();
            if let Some(more) = op.get("parameters").and_then(|v| v.as_array()) {
                params.extend(more.iter().cloned());
            }

            let mut path_param_names: Vec<String> = Vec::new();
            let mut query_param_names: Vec<String> = Vec::new();
            let mut props = Map::new();
            let mut required: Vec<Value> = Vec::new();

            for p in &params {
                let p_obj = match p.as_object() {
                    Some(o) => o,
                    None => continue,
                };
                let name = match p_obj.get("name").and_then(|v| v.as_str()) {
                    Some(n) => n.to_string(),
                    None => continue,
                };
                let location = p_obj
                    .get("in")
                    .and_then(|v| v.as_str())
                    .unwrap_or("query");
                if location != "path" && location != "query" {
                    continue;
                }
                let mut schema = p_obj
                    .get("schema")
                    .cloned()
                    .unwrap_or_else(|| json!({"type": "string"}));
                if let Some(desc) = p_obj.get("description").and_then(|v| v.as_str()) {
                    if let Some(obj) = schema.as_object_mut() {
                        obj.entry("description")
                            .or_insert_with(|| Value::String(desc.to_string()));
                    }
                }
                props.insert(name.clone(), schema);
                if p_obj
                    .get("required")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(location == "path")
                {
                    required.push(Value::String(name.clone()));
                }
                match location {
                    "path" => path_param_names.push(name),
                    "query" => query_param_names.push(name),
                    _ => {}
                }
            }

            // Request body: flatten if it's a JSON object schema; otherwise
            // expose the whole thing under a `body` key.
            let body_kind = parse_request_body(op.get("requestBody"), &components);
            match &body_kind {
                BodyKind::None => {}
                BodyKind::ObjectFlat { schema, required_fields } => {
                    if let Some(obj) = schema.get("properties").and_then(|v| v.as_object()) {
                        for (k, v) in obj {
                            // Don't let body fields shadow path/query names.
                            if !props.contains_key(k) {
                                props.insert(k.clone(), v.clone());
                            }
                        }
                    }
                    for r in required_fields {
                        if !required.iter().any(|x| x.as_str() == Some(r)) {
                            required.push(Value::String(r.clone()));
                        }
                    }
                }
                BodyKind::Opaque { schema } => {
                    props.insert("body".to_string(), schema.clone());
                }
            }

            // Auth field: optional override per-call.
            props.insert(
                TOKEN_FIELD.to_string(),
                json!({
                    "type": "string",
                    "description": "Bearer token for this call. Falls back to LISTENAI_TOKEN env."
                }),
            );

            let input_schema = json!({
                "type": "object",
                "properties": Value::Object(props),
                "required": Value::Array(required),
                "additionalProperties": true,
            });

            let mut full_desc = String::new();
            if !summary.is_empty() {
                full_desc.push_str(&summary);
            }
            if !description.is_empty() {
                if !full_desc.is_empty() {
                    full_desc.push_str("\n\n");
                }
                full_desc.push_str(&description);
            }
            if full_desc.is_empty() {
                full_desc = format!("{} {}", method.to_uppercase(), raw_path);
            }
            if !tag_list.is_empty() {
                full_desc.push_str(&format!("\n\nTags: {}", tag_list.join(", ")));
            }
            full_desc.push_str(&format!(
                "\n\nProxies: {} {}",
                method.to_uppercase(),
                raw_path
            ));

            let tool = Tool {
                name: name.clone(),
                description: full_desc,
                input_schema,
            };

            let handler = HttpProxyTool {
                tool,
                client: client.clone(),
                method: method.to_uppercase(),
                path_template: raw_path.to_string(),
                path_params: path_param_names,
                query_params: query_param_names,
                body_kind,
            };
            reg.insert(handler);
            count += 1;
        }
    }

    Ok(count)
}

#[derive(Debug, Clone)]
pub enum BodyKind {
    None,
    /// JSON object body whose top-level fields are merged into the tool's
    /// input schema.
    ObjectFlat {
        schema: Value,
        required_fields: Vec<String>,
    },
    /// Anything else (array, primitive, oneOf, etc) — accepted under a
    /// `body` field.
    Opaque {
        schema: Value,
    },
}

fn parse_request_body(rb: Option<&Value>, components: &Value) -> BodyKind {
    let rb = match rb.and_then(|v| v.as_object()) {
        Some(o) => o,
        None => return BodyKind::None,
    };
    let schema = rb
        .get("content")
        .and_then(|v| v.as_object())
        .and_then(|c| {
            c.get("application/json")
                .or_else(|| c.values().next())
        })
        .and_then(|c| c.get("schema"))
        .cloned();
    let schema = match schema {
        Some(s) => resolve_ref(&s, components),
        None => return BodyKind::None,
    };

    if schema
        .get("type")
        .and_then(|v| v.as_str())
        .map(|t| t == "object")
        .unwrap_or(false)
        || schema.get("properties").is_some()
    {
        let required_fields = schema
            .get("required")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str().map(str::to_string))
                    .collect()
            })
            .unwrap_or_default();
        return BodyKind::ObjectFlat {
            schema,
            required_fields,
        };
    }
    BodyKind::Opaque { schema }
}

/// Resolve `$ref` pointers (one hop deep — components.schemas.X) so the
/// MCP tool surface gets concrete shapes instead of refs that the agent
/// can't follow.
fn resolve_ref(schema: &Value, components: &Value) -> Value {
    if let Some(r) = schema.get("$ref").and_then(|v| v.as_str()) {
        // e.g. "#/components/schemas/CreateAudiobookRequest"
        if let Some(rest) = r.strip_prefix("#/components/schemas/") {
            if let Some(found) = components
                .pointer(&format!("/schemas/{rest}"))
                .cloned()
            {
                return found;
            }
        }
    }
    schema.clone()
}

fn is_http_method(s: &str) -> bool {
    matches!(
        s.to_ascii_lowercase().as_str(),
        "get" | "post" | "put" | "patch" | "delete" | "head" | "options"
    )
}

/// Derive a stable, agent-friendly tool name from method + path.
///
/// We deliberately ignore OpenAPI `operationId` because utoipa derives it
/// from Rust function names, which collide across handler modules
/// (e.g. `audiobook::create` and `topic_templates::create` both → `create`).
/// Method+path is always unique and self-describing.
///
/// Examples:
/// * `GET  /audiobook`                       → `audiobook_list`
/// * `POST /audiobook`                       → `audiobook_create`
/// * `GET  /audiobook/{id}`                  → `audiobook_get`
/// * `PATCH /audiobook/{id}`                 → `audiobook_update`
/// * `DELETE /audiobook/{id}`                → `audiobook_delete`
/// * `POST /audiobook/{id}/generate-chapters` → `audiobook_generate_chapters`
/// * `GET /admin/jobs`                       → `admin_jobs_list`
/// * `POST /admin/jobs/{id}/retry`           → `admin_jobs_retry`
/// * `GET /health`                           → `health_list` (singletons get the
///   same `_list` suffix as collections — the name is unambiguous and the
///   tool description disambiguates further).
fn derive_tool_name(_op: &Map<String, Value>, method: &str, path: &str) -> String {
    let segs: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();

    // Split into literal vs param segments; remember whether the LAST seg
    // is a param (= "item" endpoint) or literal (= "collection" / action).
    let mut literals: Vec<String> = Vec::new();
    let mut last_is_param = false;
    for s in &segs {
        let is_param = s.starts_with('{') || s.starts_with(':');
        if is_param {
            last_is_param = true;
        } else {
            literals.push(slug(s));
            last_is_param = false;
        }
    }

    let resource = literals.join("_");
    let m = method.to_ascii_lowercase();
    let has_any_param = segs
        .iter()
        .any(|s| s.starts_with('{') || s.starts_with(':'));

    let verb = if last_is_param {
        // Item endpoint (path ends in a param).
        match m.as_str() {
            "get" => "get",
            "patch" | "put" => "update",
            "delete" => "delete",
            "post" => "create",
            other => return format!("{resource}_{other}"),
        }
    } else if !has_any_param {
        // Collection-style path with no params anywhere (e.g. /admin/llm,
        // /audiobook, /admin/topic-templates). Trailing literal is the
        // resource — apply method-based verb so GET vs POST don't collide.
        match m.as_str() {
            "get" => "list",
            "post" => "create",
            "patch" | "put" => "update",
            "delete" => "delete",
            other => return format!("{resource}_{other}"),
        }
    } else {
        // Path has params somewhere AND ends in a literal — that trailing
        // literal is an action (e.g. /audiobook/{id}/generate-chapters).
        // Apply a verb suffix only for non-POST so we still distinguish
        // GET /audiobook/{id}/cover from POST /audiobook/{id}/cover.
        match m.as_str() {
            "post" => return resource,
            "get" => "get",
            "patch" | "put" => "update",
            "delete" => "delete",
            other => return format!("{resource}_{other}"),
        }
    };

    format!("{resource}_{verb}")
}

fn slug(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut prev_underscore = false;
    for c in s.chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c.to_ascii_lowercase());
            prev_underscore = false;
        } else if !prev_underscore && !out.is_empty() {
            out.push('_');
            prev_underscore = true;
        }
    }
    while out.ends_with('_') {
        out.pop();
    }
    if out.is_empty() {
        out = "tool".into();
    }
    out
}

fn unique(used: &mut std::collections::BTreeSet<String>, name: String) -> String {
    if used.insert(name.clone()) {
        return name;
    }
    let mut i = 2;
    loop {
        let candidate = format!("{name}_{i}");
        if used.insert(candidate.clone()) {
            return candidate;
        }
        i += 1;
    }
}

// ---- handler ----

pub struct HttpProxyTool {
    tool: Tool,
    client: Arc<ApiClient>,
    method: String,
    path_template: String,
    path_params: Vec<String>,
    query_params: Vec<String>,
    body_kind: BodyKind,
}

impl ToolHandler for HttpProxyTool {
    fn descriptor(&self) -> &Tool {
        &self.tool
    }

    async fn call(
        &self,
        args: Value,
        _progress: Option<ProgressSink>,
    ) -> Result<CallToolResult, String> {
        let args = args.as_object().cloned().unwrap_or_default();

        let token = args
            .get(TOKEN_FIELD)
            .and_then(|v| v.as_str())
            .map(str::to_string);
        let token = self.client.resolve_token(token.as_deref());

        let path = substitute_path(&self.path_template, &self.path_params, &args)?;
        let query = collect_query(&self.query_params, &args);

        let body = build_body(&self.body_kind, &self.path_params, &self.query_params, &args);

        let resp = self
            .client
            .call(
                &self.method,
                &path,
                &query,
                body,
                token.as_deref(),
            )
            .await
            .map_err(|e| e.to_string())?;

        let payload = json!({
            "status": resp.status,
            "ok": (200..300).contains(&resp.status),
            "body": resp.body,
        });
        if (200..300).contains(&resp.status) {
            Ok(CallToolResult::json(payload))
        } else {
            // Tool-level error: protocol-level success, isError = true so the
            // agent sees the failure but JSON-RPC stays clean.
            let mut r = CallToolResult::json(payload);
            r.is_error = Some(true);
            Ok(r)
        }
    }
}

fn substitute_path(
    template: &str,
    _path_params: &[String],
    args: &Map<String, Value>,
) -> Result<String, String> {
    // Split keeps empty leading segment so `/foo` -> ["", "foo"], which
    // re-joins back to `/foo`. We never collapse runs of slashes — the api's
    // routes don't have any.
    let mut parts: Vec<String> = Vec::new();
    for seg in template.split('/') {
        let resolved = if let Some(s) = seg.strip_prefix(':') {
            substitute_one(s, args)?
        } else if seg.starts_with('{') && seg.ends_with('}') && seg.len() >= 2 {
            substitute_one(&seg[1..seg.len() - 1], args)?
        } else {
            seg.to_string()
        };
        parts.push(resolved);
    }
    Ok(parts.join("/"))
}

fn substitute_one(name: &str, args: &Map<String, Value>) -> Result<String, String> {
    let v = args
        .get(name)
        .ok_or_else(|| format!("missing path param `{name}`"))?;
    let s = match v {
        Value::String(s) => s.clone(),
        Value::Number(n) => n.to_string(),
        Value::Bool(b) => b.to_string(),
        other => other.to_string(),
    };
    if s.is_empty() {
        return Err(format!("empty path param `{name}`"));
    }
    Ok(urlencode(&s))
}

fn urlencode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.as_bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(*b as char)
            }
            _ => out.push_str(&format!("%{:02X}", b)),
        }
    }
    out
}

fn collect_query(query_params: &[String], args: &Map<String, Value>) -> Vec<(String, String)> {
    let mut out = Vec::with_capacity(query_params.len());
    for name in query_params {
        if let Some(v) = args.get(name) {
            if v.is_null() {
                continue;
            }
            let s = match v {
                Value::String(s) => s.clone(),
                Value::Bool(b) => b.to_string(),
                Value::Number(n) => n.to_string(),
                other => other.to_string(),
            };
            out.push((name.clone(), s));
        }
    }
    out
}

fn build_body(
    body_kind: &BodyKind,
    path_params: &[String],
    query_params: &[String],
    args: &Map<String, Value>,
) -> Option<Value> {
    match body_kind {
        BodyKind::None => None,
        BodyKind::Opaque { .. } => args.get("body").cloned(),
        BodyKind::ObjectFlat { schema, .. } => {
            let mut obj = Map::new();
            // Only forward fields the schema knows about; this avoids
            // accidentally leaking the `_token` arg into request bodies.
            let known: Vec<String> = schema
                .get("properties")
                .and_then(|v| v.as_object())
                .map(|m| m.keys().cloned().collect())
                .unwrap_or_default();
            let skip: BTreeMap<&str, ()> = path_params
                .iter()
                .chain(query_params.iter())
                .map(|s| (s.as_str(), ()))
                .chain(std::iter::once((TOKEN_FIELD, ())))
                .collect();
            for k in known {
                if skip.contains_key(k.as_str()) {
                    continue;
                }
                if let Some(v) = args.get(&k) {
                    obj.insert(k, v.clone());
                }
            }
            // Empty body is still valid for some endpoints (e.g. POST that
            // takes no fields). Return an empty object rather than None so
            // axum's Json<T> extractor where T defaults works.
            Some(Value::Object(obj))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slug_handles_punctuation() {
        assert_eq!(slug("/audiobook/:id/generate-chapters"), "audiobook_id_generate_chapters");
        assert_eq!(slug("admin"), "admin");
        assert_eq!(slug("listAudiobookCategories"), "listaudiobookcategories");
    }

    #[test]
    fn derive_name_from_method_and_path() {
        let op = Map::new();
        assert_eq!(
            derive_tool_name(&op, "post", "/audiobook/{id}/generate-chapters"),
            "audiobook_generate_chapters"
        );
        assert_eq!(derive_tool_name(&op, "get", "/audiobook"), "audiobook_list");
        assert_eq!(derive_tool_name(&op, "post", "/audiobook"), "audiobook_create");
        assert_eq!(derive_tool_name(&op, "get", "/audiobook/{id}"), "audiobook_get");
        assert_eq!(derive_tool_name(&op, "patch", "/audiobook/{id}"), "audiobook_update");
        assert_eq!(derive_tool_name(&op, "delete", "/audiobook/{id}"), "audiobook_delete");
        assert_eq!(derive_tool_name(&op, "get", "/health"), "health_list");
        assert_eq!(
            derive_tool_name(&op, "post", "/admin/jobs/{id}/retry"),
            "admin_jobs_retry"
        );
        assert_eq!(
            derive_tool_name(&op, "patch", "/audiobook/{id}/chapter/{n}"),
            "audiobook_chapter_update"
        );
    }

    #[test]
    fn substitute_axum_style_params() {
        let mut args = Map::new();
        args.insert("id".into(), Value::String("abc".into()));
        let path = substitute_path(
            "/audiobook/:id/chapter/:n",
            &["id".into(), "n".into()],
            &{
                let mut a = args;
                a.insert("n".into(), Value::Number(7.into()));
                a
            },
        )
        .unwrap();
        assert_eq!(path, "/audiobook/abc/chapter/7");
    }
}
