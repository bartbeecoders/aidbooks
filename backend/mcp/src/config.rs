//! MCP server config — loaded from CLI args + env vars.

use std::env;

#[derive(Debug, Clone)]
pub struct Config {
    /// Base URL of the running listenai-api (e.g. `http://127.0.0.1:8787`).
    pub api_base_url: String,
    /// WebSocket base URL for progress streaming. Defaults to the api base
    /// URL with `http`→`ws` swapped.
    pub api_ws_url: String,
    /// Optional bearer token used for tool calls when the caller does not
    /// pass `_token` in the tool arguments. Mainly for stdio sessions where
    /// the user has logged in once at startup.
    pub default_token: Option<String>,
    /// Transport mode.
    pub transport: Transport,
    /// HTTP listener for `Transport::Http`.
    pub http_bind: String,
    /// Request timeout for the proxied API calls.
    pub request_timeout_secs: u64,
}

#[derive(Debug, Clone)]
pub enum Transport {
    Stdio,
    Http,
}

impl Config {
    pub fn from_args_and_env() -> anyhow::Result<Self> {
        let mut transport = Transport::Stdio;
        let mut api_base_url = env::var("LISTENAI_API_URL")
            .unwrap_or_else(|_| "http://127.0.0.1:8787".to_string());
        let mut http_bind = env::var("LISTENAI_MCP_BIND")
            .unwrap_or_else(|_| "127.0.0.1:8788".to_string());
        let default_token = env::var("LISTENAI_TOKEN").ok().filter(|s| !s.is_empty());
        let request_timeout_secs = env::var("LISTENAI_MCP_TIMEOUT_SECS")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(120);

        let mut args = env::args().skip(1);
        while let Some(arg) = args.next() {
            match arg.as_str() {
                "--stdio" => transport = Transport::Stdio,
                "--http" => transport = Transport::Http,
                "--api-url" => {
                    api_base_url = args
                        .next()
                        .ok_or_else(|| anyhow::anyhow!("--api-url requires a value"))?;
                }
                "--bind" => {
                    http_bind = args
                        .next()
                        .ok_or_else(|| anyhow::anyhow!("--bind requires a value"))?;
                }
                "-h" | "--help" => {
                    print!(
                        r#"listenai-mcp — MCP server for the ListenAI backend

USAGE:
  listenai-mcp [--stdio|--http] [--api-url URL] [--bind HOST:PORT]

TRANSPORTS:
  --stdio   newline-delimited JSON-RPC over stdin/stdout (default).
            Use this with Claude Code, Windsurf, Cursor.
  --http    streamable HTTP MCP server. POST /mcp with JSON-RPC.
            Use this with Hermes-Agent or any remote agent.

ENV:
  LISTENAI_API_URL       base URL of listenai-api (default http://127.0.0.1:8787)
  LISTENAI_TOKEN         bearer token for proxied calls (optional;
                         tool args may also pass `_token`)
  LISTENAI_MCP_BIND      bind address for --http (default 127.0.0.1:8788)
  LISTENAI_MCP_TIMEOUT_SECS  per-request timeout (default 120)
"#,
                    );
                    std::process::exit(0);
                }
                other => {
                    anyhow::bail!("unknown argument `{other}` (try --help)");
                }
            }
        }

        let api_ws_url = derive_ws(&api_base_url);

        Ok(Self {
            api_base_url,
            api_ws_url,
            default_token,
            transport,
            http_bind,
            request_timeout_secs,
        })
    }
}

fn derive_ws(http_url: &str) -> String {
    if let Some(rest) = http_url.strip_prefix("https://") {
        format!("wss://{rest}")
    } else if let Some(rest) = http_url.strip_prefix("http://") {
        format!("ws://{rest}")
    } else {
        http_url.to_string()
    }
}
