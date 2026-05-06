pub mod http;
pub mod meta;
pub mod ws;

use crate::http_client::ApiClient;
use crate::proto::{CallToolResult, Tool};
use serde_json::Value;
use std::collections::BTreeMap;
use std::sync::Arc;

/// A handler that executes a single MCP tool. `progress` is `Some` when the
/// caller passed a `_meta.progressToken` and is willing to receive
/// `notifications/progress`. Tools that aren't long-running can ignore it.
pub trait ToolHandler: Send + Sync {
    fn descriptor(&self) -> &Tool;
    fn call(
        &self,
        args: Value,
        progress: Option<ProgressSink>,
    ) -> impl std::future::Future<Output = Result<CallToolResult, String>> + Send;
}

/// Channel back to the caller for streaming progress notifications.
#[derive(Clone)]
pub struct ProgressSink {
    pub token: Value,
    pub tx: tokio::sync::mpsc::UnboundedSender<crate::proto::ProgressParams>,
}

impl ProgressSink {
    pub fn send(&self, progress: f64, total: Option<f64>, message: Option<String>) {
        let _ = self.tx.send(crate::proto::ProgressParams {
            progress_token: self.token.clone(),
            progress,
            total,
            message,
        });
    }
}

pub struct Registry {
    tools: BTreeMap<String, Arc<dyn DynToolHandler>>,
}

/// Object-safe wrapper for `ToolHandler` so we can store heterogeneous
/// implementations in the registry without boxing futures by hand at every
/// call site.
pub trait DynToolHandler: Send + Sync {
    fn descriptor(&self) -> &Tool;
    fn call<'a>(
        &'a self,
        args: Value,
        progress: Option<ProgressSink>,
    ) -> futures::future::BoxFuture<'a, Result<CallToolResult, String>>;
}

impl<T> DynToolHandler for T
where
    T: ToolHandler + 'static,
{
    fn descriptor(&self) -> &Tool {
        ToolHandler::descriptor(self)
    }
    fn call<'a>(
        &'a self,
        args: Value,
        progress: Option<ProgressSink>,
    ) -> futures::future::BoxFuture<'a, Result<CallToolResult, String>> {
        Box::pin(ToolHandler::call(self, args, progress))
    }
}

impl Registry {
    pub fn new() -> Self {
        Self {
            tools: BTreeMap::new(),
        }
    }

    pub fn insert<H: ToolHandler + 'static>(&mut self, handler: H) {
        let name = handler.descriptor().name.clone();
        self.tools.insert(name, Arc::new(handler));
    }

    pub fn list(&self) -> Vec<Tool> {
        self.tools
            .values()
            .map(|h| h.descriptor().clone())
            .collect()
    }

    pub fn get(&self, name: &str) -> Option<Arc<dyn DynToolHandler>> {
        self.tools.get(name).cloned()
    }

    pub fn len(&self) -> usize {
        self.tools.len()
    }
}

/// Construct the full tool registry: meta tools + WS subscribe + OpenAPI-derived
/// HTTP-proxy tools.
pub async fn build_registry(client: Arc<ApiClient>, ws_url: String) -> anyhow::Result<Registry> {
    let mut reg = Registry::new();

    meta::register(&mut reg, client.clone());
    ws::register(&mut reg, client.clone(), ws_url);

    let spec = client.fetch_openapi().await.map_err(|e| {
        anyhow::anyhow!("fetch /openapi.json failed: {e}. Is listenai-api running?")
    })?;

    let added = http::register_from_openapi(&mut reg, client, &spec)?;
    tracing::info!(
        total = reg.len(),
        from_openapi = added,
        "tool registry built"
    );
    Ok(reg)
}
