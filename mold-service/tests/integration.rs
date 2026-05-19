//! End-to-end coverage that boots the real axum router but stubs the
//! upstream mold serve with a small in-process HTTP server. This lets
//! CI exercise the policy, auth, and OOM paths without needing a GPU.

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use mold_service::{router, AppState, Config};
use serde_json::Value;
use tokio::net::TcpListener;
use tokio::sync::oneshot;

/// Boot mold-service on `127.0.0.1:0` against an in-memory upstream
/// stub. Returns the service URL plus a handle so callers can drive
/// the stub (e.g. flip it from happy-path to OOM).
struct Harness {
    base: String,
    upstream_state: Arc<UpstreamState>,
    _shutdowns: Vec<oneshot::Sender<()>>,
}

#[derive(Default)]
struct UpstreamState {
    mode: std::sync::Mutex<UpstreamMode>,
    pull_count: std::sync::atomic::AtomicUsize,
    unload_count: std::sync::atomic::AtomicUsize,
    last_generate_body: std::sync::Mutex<Option<Value>>,
}

#[derive(Default, Clone, Copy)]
enum UpstreamMode {
    #[default]
    HappyPng,
    Oom,
    NotFound,
}

async fn spawn_upstream() -> (String, Arc<UpstreamState>, oneshot::Sender<()>) {
    use axum::extract::State;
    use axum::http::{HeaderMap, StatusCode};
    use axum::response::Response;
    use axum::routing::{delete, get, post};
    use axum::Router;

    async fn generate(
        State(state): State<Arc<UpstreamState>>,
        _headers: HeaderMap,
        body: axum::body::Bytes,
    ) -> Response {
        let body_json: Value = serde_json::from_slice(&body).unwrap_or(Value::Null);
        *state.last_generate_body.lock().unwrap() = Some(body_json);
        let mode = *state.mode.lock().unwrap();
        match mode {
            UpstreamMode::HappyPng => Response::builder()
                .status(StatusCode::OK)
                .header("content-type", "image/png")
                .header("x-mold-seed-used", "12345")
                .body(axum::body::Body::from(b"\x89PNG\r\n\x1a\nstub".to_vec()))
                .unwrap(),
            UpstreamMode::Oom => Response::builder()
                .status(StatusCode::INTERNAL_SERVER_ERROR)
                .header("content-type", "application/json")
                .body(axum::body::Body::from(
                    br#"{"error":"CUDA OUT_OF_MEMORY","code":"OUT_OF_MEMORY"}"#.to_vec(),
                ))
                .unwrap(),
            UpstreamMode::NotFound => Response::builder()
                .status(StatusCode::NOT_FOUND)
                .header("content-type", "application/json")
                .body(axum::body::Body::from(
                    br#"{"error":"model not pulled","code":"MODEL_NOT_FOUND"}"#.to_vec(),
                ))
                .unwrap(),
        }
    }

    async fn pull(State(state): State<Arc<UpstreamState>>, _body: axum::body::Bytes) -> &'static str {
        state.pull_count.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        "model 'stub:q8' pulled successfully\n"
    }

    async fn unload(State(state): State<Arc<UpstreamState>>) -> &'static str {
        state.unload_count.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        "unloaded model 'stub:q8'\n"
    }

    async fn healthz() -> &'static str {
        "ok"
    }

    let state = Arc::new(UpstreamState::default());
    let app = Router::new()
        .route("/api/generate", post(generate))
        .route("/api/models/pull", post(pull))
        .route("/api/models/unload", delete(unload))
        .route("/healthz", get(healthz))
        .with_state(state.clone());

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr: SocketAddr = listener.local_addr().unwrap();
    let (tx, rx) = oneshot::channel();
    tokio::spawn(async move {
        axum::serve(listener, app)
            .with_graceful_shutdown(async move {
                let _ = rx.await;
            })
            .await
            .unwrap();
    });
    (format!("http://{addr}"), state, tx)
}

async fn spawn_service(upstream_url: String, api_key: Option<&str>) -> (String, oneshot::Sender<()>) {
    let config = Config {
        bind: "127.0.0.1".into(),
        port: 0,
        api_key: api_key.map(str::to_string),
        upstream_url,
        upstream_api_key: None,
        max_concurrency: 2,
        timeout_secs: 30,
        pull_timeout_secs: 30,
        oom_cooldown_secs: 1, // keep tests fast
    };
    let state = AppState::new(config);
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr: SocketAddr = listener.local_addr().unwrap();
    let app = router(state);
    let (tx, rx) = oneshot::channel();
    tokio::spawn(async move {
        axum::serve(listener, app)
            .with_graceful_shutdown(async move {
                let _ = rx.await;
            })
            .await
            .unwrap();
    });
    (format!("http://{addr}"), tx)
}

async fn spawn_full(api_key: Option<&str>) -> Harness {
    let (upstream_url, upstream_state, upstream_tx) = spawn_upstream().await;
    let (base, service_tx) = spawn_service(upstream_url, api_key).await;
    Harness {
        base,
        upstream_state,
        _shutdowns: vec![upstream_tx, service_tx],
    }
}

fn client() -> reqwest::Client {
    reqwest::Client::builder()
        .timeout(Duration::from_secs(10))
        .build()
        .unwrap()
}

#[tokio::test]
async fn healthz_reports_upstream_reachable() {
    let h = spawn_full(None).await;
    let resp: Value = client()
        .get(format!("{}/healthz", h.base))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(resp["status"], "ok");
    assert_eq!(resp["upstream_reachable"], true);
}

#[tokio::test]
async fn defaults_reflect_policy() {
    let h = spawn_full(None).await;
    let resp: Value = client()
        .get(format!("{}/v1/defaults?is_short=true", h.base))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(resp["width"], 768);
    assert_eq!(resp["height"], 1360);
    assert_eq!(resp["model"], "flux2-klein:q8");
    assert_eq!(resp["steps"], 4);
    assert_eq!(resp["guidance"], 0.0);
}

#[tokio::test]
async fn generate_returns_base64_and_threads_defaults() {
    let h = spawn_full(None).await;
    let resp = client()
        .post(format!("{}/v1/generate", h.base))
        .json(&serde_json::json!({
            "prompt": "a cat",
            "is_short": false,
        }))
        .send()
        .await
        .unwrap();
    assert!(resp.status().is_success(), "status was {}", resp.status());
    let body: Value = resp.json().await.unwrap();
    assert_eq!(body["width"], 1024);
    assert_eq!(body["height"], 1024);
    assert_eq!(body["model"], "flux2-klein:q8");
    assert_eq!(body["steps"], 4);
    assert_eq!(body["seed_used"], 12345);
    assert_eq!(body["content_type"], "image/png");
    let b64 = body["image_base64"].as_str().unwrap();
    assert!(!b64.is_empty());

    // upstream saw the resolved fields, not the originals
    let sent = h.upstream_state.last_generate_body.lock().unwrap().clone().unwrap();
    assert_eq!(sent["width"], 1024);
    assert_eq!(sent["height"], 1024);
    assert_eq!(sent["steps"], 4);
    assert_eq!(sent["model"], "flux2-klein:q8");
}

#[tokio::test]
async fn generate_rejects_blank_prompt() {
    let h = spawn_full(None).await;
    let resp = client()
        .post(format!("{}/v1/generate", h.base))
        .json(&serde_json::json!({ "prompt": "   " }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), reqwest::StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn generate_rejects_non_16_aligned_dimensions() {
    let h = spawn_full(None).await;
    let resp = client()
        .post(format!("{}/v1/generate", h.base))
        .json(&serde_json::json!({
            "prompt": "a cat",
            "width": 100,
            "height": 100,
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), reqwest::StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn auth_required_when_api_key_set() {
    let h = spawn_full(Some("s3cret")).await;
    // healthz still public
    let r = client()
        .get(format!("{}/healthz", h.base))
        .send()
        .await
        .unwrap();
    assert!(r.status().is_success());

    // generate without header → 401
    let r = client()
        .post(format!("{}/v1/generate", h.base))
        .json(&serde_json::json!({ "prompt": "x" }))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), reqwest::StatusCode::UNAUTHORIZED);

    // generate with correct header → 200
    let r = client()
        .post(format!("{}/v1/generate", h.base))
        .header("X-Api-Key", "s3cret")
        .json(&serde_json::json!({ "prompt": "x" }))
        .send()
        .await
        .unwrap();
    assert!(r.status().is_success(), "status was {}", r.status());
}

#[tokio::test]
async fn upstream_oom_returns_502_with_cooldown_message() {
    let h = spawn_full(None).await;
    *h.upstream_state.mode.lock().unwrap() = UpstreamMode::Oom;

    let r = client()
        .post(format!("{}/v1/generate", h.base))
        .json(&serde_json::json!({ "prompt": "x" }))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), reqwest::StatusCode::BAD_GATEWAY);
    let body: Value = r.json().await.unwrap();
    assert!(body["error"]
        .as_str()
        .unwrap()
        .contains("OUT_OF_MEMORY"));
}

#[tokio::test]
async fn pull_and_unload_hit_upstream() {
    let h = spawn_full(None).await;
    let r = client()
        .post(format!("{}/v1/models/pull", h.base))
        .json(&serde_json::json!({ "model": "stub:q8" }))
        .send()
        .await
        .unwrap();
    assert!(r.status().is_success());
    let body: Value = r.json().await.unwrap();
    assert!(body["message"]
        .as_str()
        .unwrap()
        .contains("pulled successfully"));
    assert_eq!(h.upstream_state.pull_count.load(std::sync::atomic::Ordering::SeqCst), 1);

    let r = client()
        .delete(format!("{}/v1/models/unload", h.base))
        .send()
        .await
        .unwrap();
    assert!(r.status().is_success());
    assert_eq!(h.upstream_state.unload_count.load(std::sync::atomic::Ordering::SeqCst), 1);
}

#[tokio::test]
async fn upstream_404_is_surfaced_with_code() {
    let h = spawn_full(None).await;
    *h.upstream_state.mode.lock().unwrap() = UpstreamMode::NotFound;
    let r = client()
        .post(format!("{}/v1/generate", h.base))
        .json(&serde_json::json!({ "prompt": "x" }))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), reqwest::StatusCode::BAD_GATEWAY);
    let body: Value = r.json().await.unwrap();
    let err = body["error"].as_str().unwrap();
    assert!(err.contains("MODEL_NOT_FOUND"));
}
