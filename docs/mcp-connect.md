  You are connecting to the AidBooks (ListenAI) MCP server to create a new
  audiobook on a random topic and add it to the user's generation queue.

  == Server connection ==

  Transport: streamable HTTP MCP (spec rev 2025-06-18).
    Endpoint:  POST http://192.168.0.143:8788/mcp        
    Health:    GET  http://192.168.0.143:8788/mcp        (returns server-info JSON)
  Request body: standard JSON-RPC 2.0. Send `Accept: text/event-stream` if you
  want intermediate `notifications/progress` events streamed back as SSE;
  otherwise responses come as a single `application/json` payload.

  Run the standard MCP handshake first:
    1. `initialize` with your client info + protocolVersion "2025-06-18".
    2. `notifications/initialized`.
    3. `tools/list` to discover the live tool surface (the server
       auto-generates tools from the backend's OpenAPI spec, so always
       trust `tools/list` over any hardcoded names).

  == Authentication ==

  The MCP server proxies the backend ListenAI API, which requires a JWT
  bearer token. There are two ways to provide it:

    * Server-wide:  the MCP process was launched with `LISTENAI_TOKEN=<jwt>`
      in its environment — every tool call uses that token by default.
    * Per-call:     pass `_token: "<jwt>"` inside the `arguments` of any
      `tools/call`. This overrides the env token for that one call.

  If you don't already have a token, you can obtain one by calling the
  `auth_login_create` tool with `{ "email": "...", "password": "..." }`
  and reading `access_token` from the JSON `body`. Cache it and pass it as
  `_token` for subsequent calls.

  You can confirm the configured base URL + whether a default token is set
  by calling the `system_base_url` tool.

  == Tool result shape ==

  Every HTTP-proxy tool returns a `CallToolResult` whose first content item
  is a JSON blob of the shape:

    { "status": <http status>, "ok": <bool>, "body": <api response> }

  On non-2xx the tool sets `isError: true` but the JSON-RPC envelope is
  still a success — read `status` and `body` to diagnose.

  == Workflow: random-topic audiobook into the queue ==

  1. Pick a random topic.
     Call tool `topics_random_create` with arguments:
       { "language": "en" }            // or "nl", "de", etc.
     The response body has shape:
       { "topic": "...", "genre": "...", "length": "short|medium|long" }
     Keep all three values.

  2. Create the audiobook and append it to the queue in one shot.
     Call tool `audiobook_create` with arguments:
       {
         "topic":    "<topic from step 1>",
         "length":   "<length from step 1>",   // "short" | "medium" | "long"
         "genre":    "<genre from step 1>",    // optional but recommended
         "language": "en",
         "enqueue":  true                      // <-- this is the key flag
       }
     With `enqueue: true` the server creates the audiobook in `draft`
     state and appends it to the caller's generation queue instead of
     running the outline inline. The queue runner activates one item at a
     time per user. The response body contains the new audiobook's `id`.

  3. Verify the item landed.
     Call tool `queue_list` (no arguments). The response body shape:
       {
         "paused": <bool>,
         "items": [
           {
             "id":            "...",
             "position":      <u32>,
             "state":
  "queued|running|paused|completed|failed|cancelled",
             "audiobook_id":  "...",
             "title":         "...",
             "topic":         "...",
             "step":          "draft|outline|writing
  chapters|narrating|done|...",
             "progress_pct":  <0..100>,
             ...
           }
         ]
       }
     Find the entry whose `audiobook_id` matches the id you got in step 2
     and report back its `position` and `state`.

  (Optional) If you want to watch the book actually run later, call
  `audiobook_subscribe_progress` with
    { "audiobook_id": "<id>", "max_seconds": 1200 }
  `notifications/progress` events while the pipeline advances.

  == Hard rules ==
  
    * Always discover tool names via `tools/list` — do not invent them.
      The naming pattern is `<resource>_<verb>` (e.g. `audiobook_create`,
      `queue_list`, `topics_random_create`) but uniqueness suffixes may
      apply if two endpoints collide.
    * Do NOT call `audiobook_create` without `enqueue: true` for this
      task — without it the outline runs synchronously and you lose the
      queueing semantics the user asked for.
    * If `topics_random_create` fails (HTTP 502 / LLM error), retry up to
      twice, then fall back to a fixed seed: `{ "seed": "general knowledge",
      "language": "en" }`.
    * Never log or echo the bearer token in your final answer.

  Final output: return the new audiobook id, the topic/length/genre you
  chose, and the queue position + state.
