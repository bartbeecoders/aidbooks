# ListenAI MCP Server

`listenai-mcp` exposes the entire ListenAI backend as a Model Context Protocol
(MCP) server, so AI agents — Claude Code, Windsurf AI, Cursor, Hermes-Agent,
or any custom agent — can drive your audiobook pipeline directly.

It runs as a thin gateway in front of `listenai-api`: every MCP tool is a
proxy to a real HTTP endpoint, so the same auth, validation, idempotency,
and audit guarantees as the REST API apply automatically.

```
   AI agent  ──MCP──▶  listenai-mcp  ──HTTP──▶  listenai-api  ──▶  SurrealDB
                                                       │
                                                       └──▶  storage / x.ai / OpenRouter / YouTube
```

---

## 1. Quick start

### 1.1 Build

```bash
cd backend
cargo build --release -p mcp
# binary at backend/target/release/listenai-mcp
```

The api server (`listenai-api`) must be running for the MCP server to discover
tools at startup — it pulls the live OpenAPI spec from `/openapi.json` and
emits one MCP tool per operation.

### 1.2 Run (stdio mode — for Claude Code, Windsurf, Cursor)

```bash
# Defaults: connects to http://127.0.0.1:8787, no token.
listenai-mcp --stdio
```

### 1.3 Run (streamable HTTP — for Hermes-Agent or remote agents)

```bash
listenai-mcp --http --bind 127.0.0.1:8788
# Endpoint: POST http://127.0.0.1:8788/mcp
```

---

## 2. Configuration

Everything is configurable via CLI flags or environment variables. Flags
override env.

| Flag                        | Env                         | Default                    | Notes                             |
| --------------------------- | --------------------------- | -------------------------- | --------------------------------- |
| `--stdio` / `--http`        | —                           | `--stdio`                  | Transport mode.                   |
| `--api-url URL`             | `LISTENAI_API_URL`          | `http://127.0.0.1:8787`    | Base URL of `listenai-api`.       |
| `--bind HOST:PORT`          | `LISTENAI_MCP_BIND`         | `127.0.0.1:8788`           | HTTP transport listener.          |
| —                           | `LISTENAI_TOKEN`            | (unset)                    | Default JWT bearer token.         |
| —                           | `LISTENAI_MCP_TIMEOUT_SECS` | `120`                      | Per-call HTTP timeout.            |
| —                           | `LISTENAI_MCP_LOG`          | `listenai_mcp=info,info`   | `tracing` env-filter expression.  |

Stdio mode logs to **stderr** (stdout is the protocol channel); in HTTP mode
both go to stderr.

---

## 3. Authentication

Almost every endpoint behind the api requires a bearer JWT. The MCP server
forwards a token on every proxied call. Resolution order, per call:

1. `_token` argument passed in `tool/call.arguments` — wins.
2. `LISTENAI_TOKEN` env var set when the MCP server was launched.
3. No auth → endpoint will return `401 Unauthorized`.

### 3.1 Easiest path: log in once, set the env

```bash
# One-off: get a JWT.
curl -s http://127.0.0.1:8787/auth/login \
  -H 'content-type: application/json' \
  -d '{"email":"you@example.com","password":"…"}' \
  | jq -r .access_token > /tmp/listenai-token

# Then run the MCP server with it pre-populated.
LISTENAI_TOKEN=$(cat /tmp/listenai-token) listenai-mcp --stdio
```

### 3.2 Let the agent log itself in

The agent calls the `auth_login_create` tool, captures `access_token` from
the response, and passes it as `_token` on subsequent calls.

```jsonc
// Step 1 — agent logs in
{"name":"auth_login_create","arguments":{"email":"you@…","password":"…"}}
// Step 2 — agent calls anything else
{"name":"audiobook_list","arguments":{"_token":"<access_token>"}}
```

Refresh tokens are 30-day rolling; agents that run for hours should call
`auth_refresh_create` periodically (the api rotates the refresh token on
every call — reusing an old one revokes the entire session).

---

## 4. Client setup

### 4.1 Claude Code

Add the MCP server to your project once:

```bash
claude mcp add listenai \
  --env LISTENAI_TOKEN=$(cat /tmp/listenai-token) \
  -- /absolute/path/to/listenai-mcp --stdio
```

Or edit `~/.claude/settings.json` (or project-level `.claude/settings.json`):

```json
{
  "mcpServers": {
    "listenai": {
      "command": "/absolute/path/to/listenai-mcp",
      "args": ["--stdio"],
      "env": {
        "LISTENAI_API_URL": "http://127.0.0.1:8787",
        "LISTENAI_TOKEN": "eyJhbGciOiJIUzI1NiI…"
      }
    }
  }
}
```

Then restart Claude Code; tools appear under `mcp__listenai__*`.

### 4.2 Windsurf AI

Edit `~/.codeium/windsurf/mcp_config.json`:

```json
{
  "mcpServers": {
    "listenai": {
      "command": "/absolute/path/to/listenai-mcp",
      "args": ["--stdio"],
      "env": {
        "LISTENAI_API_URL": "http://127.0.0.1:8787",
        "LISTENAI_TOKEN": "eyJhbGciOiJIUzI1NiI…"
      }
    }
  }
}
```

Reload Cascade and the tools become available.

### 4.3 Cursor

`~/.cursor/mcp.json` — same shape as Windsurf:

```json
{
  "mcpServers": {
    "listenai": {
      "command": "/absolute/path/to/listenai-mcp",
      "args": ["--stdio"],
      "env": { "LISTENAI_TOKEN": "…" }
    }
  }
}
```

### 4.4 Hermes-Agent (or any remote / HTTP-based agent)

Run the server in HTTP mode:

```bash
listenai-mcp --http --bind 127.0.0.1:8788
```

Then point Hermes-Agent at:

```yaml
mcp_servers:
  listenai:
    transport: streamable_http
    url: http://192.168.0.143:8788/mcp
    headers:
      Authorization: Bearer <YOUR_LISTENAI_JWT>   # optional; tools also accept _token
```

Hermes-Agent (and any client that sets `Accept: text/event-stream`) will get
SSE responses for long-running tools, including intermediate progress
notifications. Clients that send `Accept: application/json` (the default for
most curl-style probes) will get a single JSON response per call.

### 4.5 Generic JSON-RPC over curl

```bash
curl -s -X POST http://127.0.0.1:8788/mcp \
  -H 'content-type: application/json' \
  -d '{"jsonrpc":"2.0","id":1,"method":"tools/list"}' | jq .
```

### 4.6 Remote access (LAN / another machine)

The HTTP-transport listener defaults to `127.0.0.1:8788`, which only accepts
connections from the same host. To reach the MCP server from another machine
on your subnet, bind on a routable interface:

```bash
listenai-mcp --http --bind 0.0.0.0:8788
# or
LISTENAI_MCP_BIND=0.0.0.0:8788 listenai-mcp --http
```

`scripts/dev.sh` already does this — it defaults `LISTENAI_MCP_BIND` to
`0.0.0.0:8788` so dev sessions are LAN-reachable. Set
`LISTENAI_MCP_BIND=127.0.0.1:8788` in `.env` to restrict to loopback.

From the remote client:

```bash
curl -s -X POST http://<server-lan-ip>:8788/mcp \
  -H 'content-type: application/json' \
  -H 'authorization: Bearer <jwt>' \
  -d '{"jsonrpc":"2.0","id":1,"method":"tools/list"}'
```

Checklist if it still doesn't connect:

1. **Verify the listener is on the LAN, not loopback** —
   `ss -tlnp | grep 8788` should show `0.0.0.0:8788` (or your LAN IP),
   not `127.0.0.1:8788`.
2. **Open the port in the host firewall.** On Arch with firewalld:
   `sudo firewall-cmd --add-port=8788/tcp` (add `--permanent` to keep it).
   With ufw: `sudo ufw allow 8788/tcp`.
3. **`listenai-api` can stay on `127.0.0.1`.** The MCP server proxies to it
   on its own host, so the api port (8787) does *not* need to be exposed
   for tool calls to work.
   Exception: tools that return URLs for binary streams (`audiobook_stream_url`,
   anything resolving to chapter audio / cover art / waveform) hand back a URL
   pointing at `LISTENAI_API_URL` — `http://127.0.0.1:8787/...` by default,
   which the remote client cannot fetch. If you need streaming from a remote
   client, set `LISTENAI_API_URL=http://<server-lan-ip>:8787` when starting
   the MCP server, bind the api on `0.0.0.0` (set `host` / `LISTENAI_HOST`
   in the api config), and open 8787 in the firewall too.
4. **Always require auth on a LAN-exposed server.** Anyone who can reach the
   port can drive the audiobook backend. Do *not* set `LISTENAI_TOKEN` as a
   default on the server — leave it unset so callers must present their own
   `Authorization: Bearer <jwt>` (or `_token` argument). For anything beyond
   trusted home networks, front the server with a reverse proxy (Caddy,
   nginx, Cloudflare Tunnel) that adds TLS and additional auth.

---

## 5. Tool catalogue

The server exposes **78 tools**:
* 74 are auto-derived from the OpenAPI spec at startup (one tool per HTTP
  operation), so the catalogue is always in sync with the api.
* 4 are hand-written conveniences (`system_*`, `audiobook_stream_url`,
  `audiobook_subscribe_progress`).

### 5.1 Naming convention

Tool names are derived from the route's `method + path`:

| HTTP                                       | Tool name                          |
| ------------------------------------------ | ---------------------------------- |
| `GET    /audiobook`                        | `audiobook_list`                   |
| `POST   /audiobook`                        | `audiobook_create`                 |
| `GET    /audiobook/{id}`                   | `audiobook_get`                    |
| `PATCH  /audiobook/{id}`                   | `audiobook_update`                 |
| `DELETE /audiobook/{id}`                   | `audiobook_delete`                 |
| `POST   /audiobook/{id}/generate-chapters` | `audiobook_generate_chapters`      |
| `GET    /audiobook/{id}/chapter/{n}/audio` | `audiobook_chapter_audio_get`      |
| `POST   /audiobook/{id}/chapter/{n}/art`   | `audiobook_chapter_art`            |
| `GET    /admin/jobs`                       | `admin_jobs_list`                  |

The full list is reported live by `tools/list`. Use the
`system_openapi` tool from inside an agent to dump the live spec and inspect
each tool's request/response shapes.

### 5.2 Tool input schema

Every OpenAPI-derived tool has the same shape:

```jsonc
{
  // path parameters (required)
  "id": "string",
  "n":  1,

  // query parameters (optional unless required: true)
  "limit": 50,

  // request body fields, flattened into the top level
  "topic":   "Story about a cat",
  "length":  "short",
  "genre":   "fairytale",
  "language": "en",

  // optional auth override
  "_token": "eyJhbGc…"
}
```

If the request body is something other than a JSON object (an array, a
primitive, etc.), it goes under the `body` key instead.

### 5.3 Return shape

All proxy tools return:

```json
{
  "content": [
    { "type": "text", "text": "{ \"status\": 200, \"ok\": true, \"body\": {...} }" }
  ],
  "structuredContent": { "status": 200, "ok": true, "body": { /* response */ } },
  "isError": false
}
```

When the api returns 4xx/5xx, the MCP layer still resolves the JSON-RPC
call successfully but sets `isError: true` so the agent sees the failure
without losing the structured payload.

### 5.4 Tool catalogue by domain

> Generate the live list at any time with `system_openapi` or:
>
> ```bash
> echo '{"jsonrpc":"2.0","id":1,"method":"tools/list"}' | listenai-mcp --stdio | jq -r '.result.tools[].name' | sort
> ```

#### Authentication & current user

| Tool                  | HTTP                  | Description                          |
| --------------------- | --------------------- | ------------------------------------ |
| `auth_register_create`| `POST /auth/register` | New account, returns access+refresh. |
| `auth_login_create`   | `POST /auth/login`    | Sign in with email + password.       |
| `auth_refresh_create` | `POST /auth/refresh`  | Rotate the refresh token.            |
| `auth_logout_create`  | `POST /auth/logout`   | Revoke the current session.          |
| `me_list`             | `GET  /me`            | Current user profile.                |
| `me_update`           | `PATCH /me`           | Update display name, etc.            |

#### Audiobook lifecycle

| Tool                                | HTTP                                                  | Description                       |
| ----------------------------------- | ----------------------------------------------------- | --------------------------------- |
| `audiobook_create`                  | `POST  /audiobook`                                    | Start a new audiobook (optionally with auto-pipeline). |
| `audiobook_list`                    | `GET   /audiobook`                                    | Library listing.                  |
| `audiobook_get`                     | `GET   /audiobook/{id}`                               | Detail view.                      |
| `audiobook_update`                  | `PATCH /audiobook/{id}`                               | Title, voice, art style, etc.     |
| `audiobook_delete`                  | `DELETE /audiobook/{id}`                              | Hard delete + asset GC.           |
| `audiobook_generate_chapters`       | `POST  /audiobook/{id}/generate-chapters`             | Enqueue chapter writing.          |
| `audiobook_generate_audio`          | `POST  /audiobook/{id}/generate-audio`                | Enqueue narration.                |
| `audiobook_translate`               | `POST  /audiobook/{id}/translate`                     | Translate text + re-narrate.      |
| `audiobook_cancel_pipeline`         | `POST  /audiobook/{id}/cancel-pipeline`               | Cancel every active job.          |
| `audiobook_chapter_update`          | `PATCH /audiobook/{id}/chapter/{n}`                   | Edit a single chapter's text.     |
| `audiobook_chapter_regenerate`      | `POST  /audiobook/{id}/chapter/{n}/regenerate`        | Re-write one chapter.             |
| `audiobook_chapter_regenerate_audio`| `POST  /audiobook/{id}/chapter/{n}/regenerate-audio`  | Re-narrate one chapter.           |
| `audiobook_chapter_art`             | `POST  /audiobook/{id}/chapter/{n}/art`               | Re-generate chapter cover art.    |
| `audiobook_cover`                   | `POST  /audiobook/{id}/cover`                         | Re-generate the main cover.       |
| `audiobook_costs_get`               | `GET   /audiobook/{id}/costs`                         | Per-role token+TTS+image costs.   |
| `audiobook_jobs_get`                | `GET   /audiobook/{id}/jobs`                          | Snapshot of jobs for one book.    |

#### File / asset access (binary streams)

These endpoints stream bytes (audio, images, waveform JSON). MCP returns
JSON, not raw bytes — so the proxy tools (`*_get` for these paths) return
a small JSON envelope containing the URL and bearer token; the agent (or a
sub-process) downloads from that URL directly.

For convenience, **`audiobook_stream_url`** does the URL building for you:

```jsonc
{"name":"audiobook_stream_url",
 "arguments":{"audiobook_id":"abc","kind":"chapter_audio","chapter":1}}
```

→
```json
{
  "url": "http://127.0.0.1:8787/audiobook/abc/chapter/1/audio",
  "method": "GET",
  "auth_header": "Bearer eyJhbGc…",
  "hint": "…"
}
```

`kind` ∈ `chapter_audio | chapter_art | paragraph_image | chapter_waveform | cover`.

#### Catalogues (open to all logged-in users)

| Tool                        | HTTP                            |
| --------------------------- | ------------------------------- |
| `voices_list`               | `GET /voices`                   |
| `llms_list`                 | `GET /llms`                     |
| `audiobook_categories_list` | `GET /audiobook-categories`     |
| `topic_templates_list`      | `GET /topic-templates`          |
| `topics_random_create`      | `POST /topics/random`           |
| `cover_art_preview_create`  | `POST /cover-art/preview`       |

#### Job inspection & live progress

| Tool                            | What it does                                                                                                |
| ------------------------------- | ----------------------------------------------------------------------------------------------------------- |
| `audiobook_jobs_get`            | One-shot job snapshot for an audiobook.                                                                     |
| `audiobook_subscribe_progress`  | **Long-running** — connects to the WebSocket progress stream and emits MCP `notifications/progress` events. |
| `admin_jobs_list`               | Admin: every job in the system, filterable.                                                                 |
| `admin_jobs_retry`              | Admin: retry a failed job.                                                                                  |
| `admin_jobs_cancel`             | Admin: cancel a non-terminal job.                                                                           |
| `admin_jobs_delete`             | Admin: hard-delete a job row.                                                                               |

`audiobook_subscribe_progress` is the centerpiece for agentic workflows. It
blocks until either:
* every job for the book reaches a terminal state,
* `max_seconds` (default 600) elapses, or
* the agent cancels.

To receive intermediate progress, set `_meta.progressToken` on the
`tools/call` request:

```jsonc
{
  "jsonrpc":"2.0", "id": 7, "method":"tools/call",
  "params": {
    "name": "audiobook_subscribe_progress",
    "arguments": { "audiobook_id": "abc" },
    "_meta": { "progressToken": "abc-watch" }
  }
}
```

The server then emits zero or more:
```json
{"jsonrpc":"2.0","method":"notifications/progress","params":{
  "progressToken":"abc-watch","progress":42.5,"total":100.0,"message":"3 job(s)"
}}
```
…and finally a regular `tools/call` response with the last snapshot.

#### Admin (admin-tier JWT required)

Domain-by-domain CRUD, plus a few service tools:

* **System**: `admin_system_list` (system overview).
* **LLMs**: `admin_llm_list`, `admin_llm_create`, `admin_llm_update`,
  `admin_llm_delete`, `admin_test_llm_create`.
* **Voices**: `admin_voice_list`, `admin_voice_update`, `admin_test_voice_create`.
* **Users**: `admin_users_list`, `admin_users_update`, `admin_users_revoke_sessions`.
* **Jobs**: `admin_jobs_list`, `admin_jobs_retry`, `admin_jobs_cancel`,
  `admin_jobs_delete`.
* **Audiobook categories**: `admin_audiobook_categories_{list,create,update,delete}`.
* **YouTube footers (per language)**: `admin_youtube_settings_{list,update,delete}`.
* **Topic templates**: `admin_topic_templates_{list,create,update,delete}`.
* **Provider catalogues**: `admin_openrouter_models_list`,
  `admin_xai_models_list`, `admin_xai_image_models_list`.

#### YouTube publishing

| Tool                                    | HTTP                                              |
| --------------------------------------- | ------------------------------------------------- |
| `integrations_youtube_oauth_start_list` | `GET    /integrations/youtube/oauth/start`        |
| `integrations_youtube_oauth_callback_list` | `GET /integrations/youtube/oauth/callback`     |
| `integrations_youtube_account_list`     | `GET    /integrations/youtube/account`            |
| `integrations_youtube_account_delete`   | `DELETE /integrations/youtube/account`            |
| `audiobook_publish_youtube`             | `POST   /audiobook/{id}/publish/youtube`          |
| `audiobook_publications_get`            | `GET    /audiobook/{id}/publications`             |
| `audiobook_publications_approve`        | `POST   /audiobook/{id}/publications/{pid}/approve` |
| `audiobook_publications_cancel`         | `POST   /audiobook/{id}/publications/{pid}/cancel`  |
| `audiobook_publications_preview_get`    | `GET    /audiobook/{id}/publications/{pid}/preview` |

#### System / introspection

| Tool                | What it does                                                            |
| ------------------- | ----------------------------------------------------------------------- |
| `system_openapi`    | Dump the live OpenAPI 3.1 spec — useful for shape lookup.               |
| `system_base_url`   | Show the api base URL the MCP server is using and whether a default token is set. |
| `health_list`       | Liveness probe. Cheap, no I/O.                                          |
| `ready_list`        | Readiness probe. Checks the DB.                                         |

---

## 6. Coverage of the original brief

The brief asked for the whole backend to be exposed. Here's how each
category maps to MCP tools:

### 6.1 "Expose all endpoints as tools" ✓
74 OpenAPI operations → 74 auto-generated proxy tools, kept in sync with
the api at every server start.

### 6.2 "Expose all database operations as tools" ✓
SurrealDB is internal — exposing raw `SELECT`/`UPDATE`/`CREATE` queries
would bypass row-level auth and validation. Instead, every database write
or read has a typed endpoint behind `listenai-api`, and every endpoint is
an MCP tool. The full data model — users, audiobooks, chapters, jobs,
sessions, LLMs, voices, categories, topic templates, footers, publications
— is reachable via the `*_list / *_get / *_create / *_update / *_delete`
tool families documented above.

### 6.3 "Expose all job operations as tools" ✓
* `audiobook_jobs_get` — per-book job list.
* `admin_jobs_*` — system-wide list, retry, cancel, delete.
* `audiobook_subscribe_progress` — live WebSocket-backed progress stream.
* `audiobook_cancel_pipeline` — cancel everything for one book.

### 6.4 "Expose all file operations as tools" ✓
Audiobook assets (audio, images, waveforms) are streamed by the api over
HTTP. Because MCP carries JSON, the corresponding tools (and the helper
`audiobook_stream_url`) return URL + bearer token rather than the bytes
themselves. The agent then either:
1. Hands the URL to a downloader, browser, or audio player, or
2. Spawns its own HTTP fetch with the supplied auth header.

This avoids inflating MCP messages with multi-MB base64 blobs and is how
real frontends (web + iOS) talk to the same endpoints.

### 6.5 "Expose all system operations as tools" ✓
* `system_openapi`, `system_base_url`
* `health_list`, `ready_list`
* `admin_system_list` (system overview: DB stats, queue depth, etc.)

---

## 7. Wire protocol

The server speaks MCP **`2025-06-18`** (the current revision) and
JSON-RPC 2.0. Supported methods:

| Method                          | Notes                                                      |
| ------------------------------- | ---------------------------------------------------------- |
| `initialize`                    | Returns `serverInfo`, `protocolVersion`, capabilities.     |
| `notifications/initialized`     | Accepted (no-op).                                          |
| `ping`                          | Returns `{}`.                                              |
| `tools/list`                    | Lists all tools and their JSON Schemas.                    |
| `tools/call`                    | Calls a tool, optionally streaming progress notifications. |
| `notifications/cancelled`       | Accepted (no-op — we do not yet abort in-flight calls).    |
| `logging/setLevel`              | Accepted (no-op).                                          |
| `resources/list`, `prompts/list`| Return empty arrays (we do not expose resources/prompts).  |

Capabilities advertised:
```json
{
  "tools":   { "listChanged": false },
  "logging": {}
}
```

---

## 8. Local end-to-end smoke test

```bash
# 1. Run the api in one shell.
just dev-backend

# 2. In another, talk to MCP over stdio without a real client.
cd backend
cat <<'EOF' | cargo run -q -p mcp -- --stdio
{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18"}}
{"jsonrpc":"2.0","id":2,"method":"tools/list"}
{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"health_list","arguments":{}}}
EOF
```

Expected: an `initialize` result, a `tools/list` with ~78 entries, and a
`tools/call` result returning `{ status: 200, ok: true, body: { service:
"listenai-api", … } }`.

For the HTTP transport:

```bash
listenai-mcp --http --bind 127.0.0.1:8788 &
curl -s -X POST http://127.0.0.1:8788/mcp \
  -H 'content-type: application/json' \
  -d '{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"health_list","arguments":{}}}' \
  | jq .result.structuredContent
```

---

## 9. Troubleshooting

* **`fetch /openapi.json failed: …`** at startup — the api isn't running, or
  `LISTENAI_API_URL` points somewhere unexpected. Bring up the api first.
* **Every tool returns `401 Unauthorized`** — no bearer token. Set
  `LISTENAI_TOKEN` or pass `_token` in tool arguments.
* **Tools return the right data but stdout has weird text mixed in** —
  something is logging to stdout in stdio mode. The MCP server only logs to
  stderr; check the api or shell wrappers if you see this.
* **`audiobook_subscribe_progress` returns immediately** — the api had no
  active jobs for that audiobook, so the snapshot was already terminal.
  Trigger a `audiobook_generate_*` first.
* **HTTP-transport SSE response is buffered until the call completes** —
  some HTTP clients don't honour `text/event-stream` until they see a
  `Content-Length` (which streams don't have). Use a real MCP client, or
  `curl -N`.
* **Connection refused / timeout from another machine on the LAN** —
  the listener is bound to `127.0.0.1` (loopback only), or the host
  firewall is blocking 8788. See §4.6.

---

## 10. Source layout

```
backend/mcp/
├── Cargo.toml
└── src/
    ├── main.rs               # entry point
    ├── config.rs             # CLI/env parsing
    ├── http_client.rs        # reqwest wrapper around listenai-api
    ├── proto.rs              # JSON-RPC 2.0 + MCP wire types
    ├── server.rs             # tools/list, tools/call dispatcher
    ├── tools/
    │   ├── mod.rs            # ToolHandler trait + Registry
    │   ├── http.rs           # OpenAPI-derived proxy tools
    │   ├── meta.rs           # system_openapi, system_base_url, audiobook_stream_url
    │   └── ws.rs             # audiobook_subscribe_progress
    └── transport/
        ├── stdio.rs          # newline-delimited JSON-RPC
        └── http.rs           # streamable-HTTP (POST /mcp, optional SSE)
```

About 1100 lines of Rust, no rmcp dependency — the MCP protocol over
stdio is just JSON-RPC 2.0 which is small enough to implement directly.
