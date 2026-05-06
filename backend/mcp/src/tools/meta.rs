//! Hand-written "meta" tools: things that aren't a 1:1 endpoint mapping.
//!
//! These complement the OpenAPI-derived tools with conveniences: spec
//! introspection, building stream URLs, and listing the tools themselves.

use super::{ProgressSink, Registry, ToolHandler};
use crate::http_client::ApiClient;
use crate::proto::{CallToolResult, Tool};
use serde_json::{json, Value};
use std::sync::Arc;

pub fn register(reg: &mut Registry, client: Arc<ApiClient>) {
    reg.insert(SystemOpenapi {
        client: client.clone(),
        tool: Tool {
            name: "system_openapi".into(),
            description: "Fetch the live OpenAPI 3.1 specification of the listenai-api. Useful for agents that want to see request/response shapes for any endpoint.".into(),
            input_schema: json!({"type": "object", "properties": {}, "additionalProperties": false}),
        },
    });

    reg.insert(SystemBaseUrl {
        client: client.clone(),
        tool: Tool {
            name: "system_base_url".into(),
            description: "Return the base URL the MCP server uses to reach listenai-api, plus the bearer token mode (env vs per-call).".into(),
            input_schema: json!({"type": "object", "properties": {}, "additionalProperties": false}),
        },
    });

    reg.insert(StreamUrl {
        client,
        tool: Tool {
            name: "audiobook_stream_url".into(),
            description: "Build a streaming URL for an audiobook asset (chapter audio, chapter art, paragraph image, waveform, cover) plus the bearer token to use with it. The MCP protocol returns JSON, not raw bytes — hand the URL+token to a downloader/player. Available kinds: chapter_audio, chapter_art, paragraph_image, chapter_waveform, cover.".into(),
            input_schema: json!({
                "type": "object",
                "required": ["audiobook_id", "kind"],
                "properties": {
                    "audiobook_id": {"type": "string"},
                    "kind": {
                        "type": "string",
                        "enum": ["chapter_audio", "chapter_art", "paragraph_image", "chapter_waveform", "cover"]
                    },
                    "chapter": {"type": "integer", "minimum": 1, "description": "Required for chapter_*, paragraph_image."},
                    "paragraph": {"type": "integer", "minimum": 1, "description": "Required for paragraph_image."},
                    "image": {"type": "integer", "minimum": 1, "description": "Required for paragraph_image."},
                    "_token": {"type": "string", "description": "Bearer token override."}
                }
            }),
        },
    });
}

pub struct SystemOpenapi {
    client: Arc<ApiClient>,
    tool: Tool,
}

impl ToolHandler for SystemOpenapi {
    fn descriptor(&self) -> &Tool {
        &self.tool
    }
    async fn call(
        &self,
        _args: Value,
        _progress: Option<ProgressSink>,
    ) -> Result<CallToolResult, String> {
        let spec = self
            .client
            .fetch_openapi()
            .await
            .map_err(|e| e.to_string())?;
        Ok(CallToolResult::json(spec))
    }
}

pub struct SystemBaseUrl {
    client: Arc<ApiClient>,
    tool: Tool,
}

impl ToolHandler for SystemBaseUrl {
    fn descriptor(&self) -> &Tool {
        &self.tool
    }
    async fn call(
        &self,
        _args: Value,
        _progress: Option<ProgressSink>,
    ) -> Result<CallToolResult, String> {
        Ok(CallToolResult::json(json!({
            "base_url": self.client.base_url(),
            "default_token_set": self.client.resolve_token(None).is_some(),
        })))
    }
}

pub struct StreamUrl {
    client: Arc<ApiClient>,
    tool: Tool,
}

impl ToolHandler for StreamUrl {
    fn descriptor(&self) -> &Tool {
        &self.tool
    }
    async fn call(
        &self,
        args: Value,
        _progress: Option<ProgressSink>,
    ) -> Result<CallToolResult, String> {
        let id = args
            .get("audiobook_id")
            .and_then(|v| v.as_str())
            .ok_or("audiobook_id required")?;
        let kind = args
            .get("kind")
            .and_then(|v| v.as_str())
            .ok_or("kind required")?;
        let path = match kind {
            "cover" => format!("/audiobook/{id}/cover"),
            "chapter_audio" => {
                let n = args
                    .get("chapter")
                    .and_then(|v| v.as_u64())
                    .ok_or("chapter required")?;
                format!("/audiobook/{id}/chapter/{n}/audio")
            }
            "chapter_art" => {
                let n = args
                    .get("chapter")
                    .and_then(|v| v.as_u64())
                    .ok_or("chapter required")?;
                format!("/audiobook/{id}/chapter/{n}/art")
            }
            "chapter_waveform" => {
                let n = args
                    .get("chapter")
                    .and_then(|v| v.as_u64())
                    .ok_or("chapter required")?;
                format!("/audiobook/{id}/chapter/{n}/waveform")
            }
            "paragraph_image" => {
                let n = args
                    .get("chapter")
                    .and_then(|v| v.as_u64())
                    .ok_or("chapter required")?;
                let p = args
                    .get("paragraph")
                    .and_then(|v| v.as_u64())
                    .ok_or("paragraph required")?;
                let i = args
                    .get("image")
                    .and_then(|v| v.as_u64())
                    .ok_or("image required")?;
                format!("/audiobook/{id}/chapter/{n}/paragraph/{p}/image/{i}")
            }
            other => return Err(format!("unknown kind `{other}`")),
        };
        let token = args
            .get("_token")
            .and_then(|v| v.as_str())
            .map(str::to_string);
        let token = self.client.resolve_token(token.as_deref());
        let auth_header = token.as_ref().map(|t| format!("Bearer {t}"));
        Ok(CallToolResult::json(json!({
            "url": format!("{}{path}", self.client.base_url()),
            "method": "GET",
            "token": token,
            "auth_header": auth_header,
            "hint": "Hit the URL with the bearer token in the Authorization header to stream bytes."
        })))
    }
}
