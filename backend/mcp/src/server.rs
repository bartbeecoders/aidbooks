//! Core MCP server: dispatches JSON-RPC methods to the tool registry. The
//! server is transport-agnostic — the stdio and HTTP transports both use
//! `Server::handle_message` and `Server::stream_call`.

use crate::proto::{
    codes, CallToolParams, CallToolResult, Id, InitializeParams, InitializeResult,
    ListToolsResult, Notification, ProgressParams, Response, ServerCapabilities, ServerInfo,
    ToolsCapability, MCP_PROTOCOL_VERSION,
};
use crate::tools::{ProgressSink, Registry};
use serde_json::{json, Value};
use std::sync::Arc;
use tokio::sync::mpsc::UnboundedSender;

pub struct Server {
    registry: Arc<Registry>,
}

/// Outbound message a transport can deliver to the client.
#[derive(Debug)]
pub enum Outbound {
    /// JSON-RPC response to a numbered request.
    Response(Response),
    /// JSON-RPC notification (no id, no response expected).
    Notification(serde_json::Value),
}

impl Server {
    pub fn new(registry: Arc<Registry>) -> Self {
        Self { registry }
    }

    #[allow(dead_code)]
    pub fn registry(&self) -> &Registry {
        &self.registry
    }

    /// Handle a single inbound JSON-RPC message. `outbound` is the sink the
    /// transport drains and writes to the client. Long-running tools
    /// (`tools/call` with a progress token) push intermediate
    /// `notifications/progress` notifications onto `outbound` while the
    /// final response is also routed through it.
    pub async fn dispatch(
        self: Arc<Self>,
        raw: Value,
        outbound: UnboundedSender<Outbound>,
    ) {
        // We accept both single objects and arrays (batch). Batches are rare
        // in MCP clients but cheap to support.
        let msgs = match raw {
            Value::Array(arr) => arr,
            other => vec![other],
        };
        for msg in msgs {
            let server = self.clone();
            let out = outbound.clone();
            tokio::spawn(async move {
                server.handle_one(msg, out).await;
            });
        }
    }

    async fn handle_one(self: Arc<Self>, msg: Value, outbound: UnboundedSender<Outbound>) {
        // Notifications have no `id` and expect no response.
        let id = msg.get("id").cloned();
        let method = match msg.get("method").and_then(|v| v.as_str()) {
            Some(m) => m.to_string(),
            None => return,
        };
        let params = msg.get("params").cloned().unwrap_or(Value::Null);

        // Notifications first — these never produce a response.
        if id.is_none() {
            match method.as_str() {
                "notifications/initialized" | "initialized" => {}
                "notifications/cancelled" => {}
                "ping" => {}
                _ => {
                    tracing::debug!(%method, "ignoring unknown notification");
                }
            }
            return;
        }

        let id = parse_id(id);

        let resp = match method.as_str() {
            "initialize" => self.handle_initialize(id.clone(), params).await,
            "ping" => Response::ok(id.clone(), json!({})),
            "tools/list" => self.handle_list_tools(id.clone()).await,
            "tools/call" => {
                self.handle_call_tool(id.clone(), params, outbound.clone())
                    .await
            }
            // logging/setLevel — accept silently so we don't error out.
            "logging/setLevel" => Response::ok(id.clone(), json!({})),
            // resources/list, prompts/list — we don't expose any. Return an
            // empty list rather than method-not-found so older clients that
            // probe for these don't show errors.
            "resources/list" => Response::ok(id.clone(), json!({"resources": []})),
            "resources/templates/list" => Response::ok(id.clone(), json!({"resourceTemplates": []})),
            "prompts/list" => Response::ok(id.clone(), json!({"prompts": []})),
            other => Response::err(
                id.clone(),
                codes::METHOD_NOT_FOUND,
                format!("unsupported method `{other}`"),
            ),
        };
        let _ = outbound.send(Outbound::Response(resp));
    }

    async fn handle_initialize(&self, id: Id, params: Value) -> Response {
        let parsed: InitializeParams = match serde_json::from_value(params) {
            Ok(p) => p,
            Err(e) => {
                // Tolerate malformed init — log and proceed with defaults.
                tracing::warn!(error = %e, "initialize params decode");
                InitializeParams {
                    protocol_version: None,
                    capabilities: Value::Null,
                    client_info: None,
                }
            }
        };
        if let Some(client) = parsed.client_info {
            tracing::info!(
                name = %client.name.unwrap_or_default(),
                version = %client.version.unwrap_or_default(),
                requested_proto = %parsed.protocol_version.unwrap_or_default(),
                "client connected"
            );
        }
        let result = InitializeResult {
            protocol_version: MCP_PROTOCOL_VERSION,
            capabilities: ServerCapabilities {
                tools: ToolsCapability {
                    list_changed: false,
                },
                logging: json!({}),
            },
            server_info: ServerInfo {
                name: "listenai-mcp",
                version: env!("CARGO_PKG_VERSION"),
            },
        };
        Response::ok(
            id,
            serde_json::to_value(result).unwrap_or(Value::Null),
        )
    }

    async fn handle_list_tools(&self, id: Id) -> Response {
        let tools = self.registry.list();
        let result = ListToolsResult { tools };
        Response::ok(
            id,
            serde_json::to_value(result).unwrap_or(Value::Null),
        )
    }

    async fn handle_call_tool(
        &self,
        id: Id,
        params: Value,
        outbound: UnboundedSender<Outbound>,
    ) -> Response {
        let parsed: CallToolParams = match serde_json::from_value(params) {
            Ok(p) => p,
            Err(e) => {
                return Response::err(
                    id,
                    codes::INVALID_PARAMS,
                    format!("tools/call params: {e}"),
                );
            }
        };
        let handler = match self.registry.get(&parsed.name) {
            Some(h) => h,
            None => {
                return Response::err(
                    id,
                    codes::METHOD_NOT_FOUND,
                    format!("no such tool `{}`", parsed.name),
                );
            }
        };

        // Wire up a progress sink if the caller passed a token.
        let progress_token = parsed
            .meta
            .get("progressToken")
            .cloned()
            .filter(|v| !v.is_null());
        let progress_sink = if let Some(token) = progress_token {
            let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<ProgressParams>();
            let outbound2 = outbound.clone();
            tokio::spawn(async move {
                while let Some(p) = rx.recv().await {
                    let n = Notification::new(
                        "notifications/progress",
                        serde_json::to_value(&p).unwrap_or(Value::Null),
                    );
                    let _ = outbound2.send(Outbound::Notification(
                        serde_json::to_value(n).unwrap_or(Value::Null),
                    ));
                }
            });
            Some(ProgressSink { token, tx })
        } else {
            None
        };

        let result: CallToolResult = match handler.call(parsed.arguments, progress_sink).await {
            Ok(r) => r,
            Err(msg) => CallToolResult::error(msg),
        };
        Response::ok(
            id,
            serde_json::to_value(result).unwrap_or(Value::Null),
        )
    }
}

fn parse_id(v: Option<Value>) -> Id {
    match v {
        Some(Value::Number(n)) => n.as_i64().map(Id::Number).unwrap_or(Id::Null),
        Some(Value::String(s)) => Id::String(s),
        Some(Value::Null) | None => Id::Null,
        _ => Id::Null,
    }
}
