//! Long-lived Node renderer pool (Phase F.1a).
//!
//! Phase A–E spawned one `node dist/cli.js` per chapter render. Each
//! spawn paid a 10–30 s tax for Node startup, Vite, the Revideo import,
//! and the first JIT pass. Across a 12-chapter book that's 2–6 min of
//! pure overhead.
//!
//! This pool keeps `N` long-lived `node dist/server.js` sidecars alive
//! (where `N` matches `Config::animate_concurrency`). Each sidecar
//! processes specs over its lifetime using the `server.ts` protocol:
//!
//!   stdin  : one [`SceneSpec`] JSON per line
//!   stdout : the existing `started` / `frame` / `encoding` / `done` /
//!            `error` event stream, plus a `ready` event between
//!            renders and a `bye` on graceful shutdown.
//!
//! Lifecycle:
//!
//!   * **Spawn** — lazily on the first render that needs a slot. Each
//!     spawn waits for the initial `ready` event before being marked
//!     usable.
//!   * **Reuse** — after a successful render the sidecar goes back on
//!     the pool's queue and serves the next render.
//!   * **Respawn** — after [`SidecarCfg::max_renders_per_proc`] renders
//!     OR [`SidecarCfg::max_age_secs`] of wall clock since spawn. Bounds
//!     long-running memory leaks in Chromium / Revideo without a hard
//!     restart cost on every chapter.
//!   * **Die** — on a transient render failure (renderer crashed, EOF
//!     before `ready`, broken stdin pipe). Pool drops the sidecar; the
//!     next acquirer gets a fresh one.
//!
//! Per-render error events stay fatal at the call site (matches the
//! pre-pool behaviour) — the sidecar self-heals across them, but the
//! job sees the same `JobOutcome::Fatal` it always did.

use std::collections::VecDeque;
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::Arc;
use std::time::{Duration, Instant};

use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, Lines};
use tokio::process::{Child, ChildStdin, ChildStdout, Command};
use tokio::sync::{mpsc, Mutex, Semaphore};
use tokio::task::JoinHandle;
use tracing::{debug, info, warn};

use super::spec::{RenderEvent, SceneSpec};

/// How many renders a single sidecar serves before we recycle it.
/// Bound to keep Chromium / Vite memory drift contained.
const DEFAULT_MAX_RENDERS_PER_PROC: u32 = 50;

/// How long a sidecar may live before recycling, in seconds. Same idea
/// as the render-count bound, just keyed on time so an idle sidecar
/// doesn't accumulate weeks of state.
const DEFAULT_MAX_AGE_SECS: u64 = 30 * 60;

/// Bounded tail of stderr we keep per sidecar so a render failure can
/// surface useful diagnostics without unbounded memory growth.
const STDERR_TAIL_BYTES: usize = 4_096;

/// Failure shape mirrored from `publishers::animate::RenderFailure`.
/// Held standalone here so `sidecar` doesn't have to reach across the
/// jobs module hierarchy.
#[derive(Debug, Clone)]
pub enum RenderFailure {
    Transient(String),
    Fatal(String),
}

#[derive(Debug, Clone)]
pub struct SidecarCfg {
    pub node_bin: String,
    /// Path to the long-lived sidecar entry, normally
    /// `backend/render/dist/server.js`. The publisher resolves this
    /// against `Config::animate_renderer_cmd` (auto-deriving
    /// `server.js` from a legacy `cli.js` value).
    pub sidecar_cmd: PathBuf,
    pub max_renders_per_proc: u32,
    pub max_age_secs: u64,
}

impl SidecarCfg {
    pub fn new(node_bin: String, sidecar_cmd: PathBuf) -> Self {
        Self {
            node_bin,
            sidecar_cmd,
            max_renders_per_proc: DEFAULT_MAX_RENDERS_PER_PROC,
            max_age_secs: DEFAULT_MAX_AGE_SECS,
        }
    }
}

/// A pool of long-lived Node renderer processes. Cheap to clone via
/// the `Arc` it's wrapped in.
pub struct RendererPool {
    cfg: SidecarCfg,
    permits: Semaphore,
    available: Mutex<VecDeque<Sidecar>>,
}

impl RendererPool {
    pub fn new(cfg: SidecarCfg, capacity: usize) -> Arc<Self> {
        let cap = capacity.max(1);
        Arc::new(Self {
            cfg,
            permits: Semaphore::new(cap),
            available: Mutex::new(VecDeque::with_capacity(cap)),
        })
    }

    /// Render one chapter. Acquires a sidecar (spawning lazily if the
    /// pool is cold), pipes the spec in, forwards every progress event
    /// to `events_tx` until the sidecar reports `ready`, then returns
    /// the sidecar to the pool (or kills it if exhausted / the render
    /// failed transiently).
    ///
    /// The channel is the right shape for our caller: `ctx.progress`
    /// is itself `async`, and a sync callback would force ugly
    /// `block_on`-in-task gymnastics. The caller drains `events_tx`'s
    /// receiver in a parallel future via `tokio::join!`; when this
    /// function returns it drops the sender, terminating the drain
    /// loop naturally.
    pub async fn render(
        &self,
        spec: &SceneSpec,
        events_tx: mpsc::UnboundedSender<RenderEvent>,
    ) -> Result<(), RenderFailure> {
        let _permit = self
            .permits
            .acquire()
            .await
            .map_err(|e| RenderFailure::Transient(format!("pool semaphore closed: {e}")))?;

        let mut sidecar = self.acquire_sidecar().await?;
        let result = render_one(&mut sidecar, spec, &events_tx).await;
        sidecar.renders_done += 1;

        let exhausted = sidecar.renders_done >= self.cfg.max_renders_per_proc
            || sidecar.spawned_at.elapsed().as_secs() >= self.cfg.max_age_secs;
        let dead = sidecar.is_dead || matches!(result, Err(RenderFailure::Transient(_)));

        if dead || exhausted {
            debug!(
                renders_done = sidecar.renders_done,
                age_secs = sidecar.spawned_at.elapsed().as_secs(),
                dead,
                "renderer pool: recycling sidecar"
            );
            shutdown_sidecar(sidecar).await;
        } else {
            self.available.lock().await.push_back(sidecar);
        }

        result
    }

    /// Take a sidecar off the queue; spawn a new one if the queue is
    /// empty. Permits gate concurrent callers, so this never grows the
    /// pool past `capacity`.
    async fn acquire_sidecar(&self) -> Result<Sidecar, RenderFailure> {
        if let Some(s) = self.available.lock().await.pop_front() {
            return Ok(s);
        }
        self.spawn_sidecar().await
    }

    async fn spawn_sidecar(&self) -> Result<Sidecar, RenderFailure> {
        let cmd = std::fs::canonicalize(&self.cfg.sidecar_cmd).map_err(|e| {
            RenderFailure::Fatal(format!(
                "renderer cmd `{}` does not exist: {e} \
                 (set LISTENAI_ANIMATE_RENDERER_CMD to an absolute path \
                  ending in dist/server.js)",
                self.cfg.sidecar_cmd.display()
            ))
        })?;
        // dist/server.js → dist/ → backend/render/. The renderer's
        // `node_modules` lives at the latter; running with that cwd
        // lets Node resolve `@revideo/renderer` and friends.
        let render_dir = cmd
            .parent()
            .and_then(std::path::Path::parent)
            .ok_or_else(|| {
                RenderFailure::Fatal(format!(
                    "sidecar cmd `{}` has no grandparent dir; expected `<render>/dist/server.js`",
                    cmd.display()
                ))
            })?
            .to_path_buf();

        let mut command = Command::new(&self.cfg.node_bin);
        command
            .arg(&cmd)
            .current_dir(&render_dir)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);

        let mut child = command
            .spawn()
            .map_err(|e| RenderFailure::Transient(format!("spawn renderer sidecar: {e}")))?;

        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| RenderFailure::Transient("sidecar stdin not piped".into()))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| RenderFailure::Transient("sidecar stdout not piped".into()))?;
        let stderr = child
            .stderr
            .take()
            .ok_or_else(|| RenderFailure::Transient("sidecar stderr not piped".into()))?;

        let stderr_buf = Arc::new(Mutex::new(String::new()));
        let drain_buf = stderr_buf.clone();
        let stderr_task = tokio::spawn(async move {
            let mut reader = BufReader::new(stderr).lines();
            while let Ok(Some(line)) = reader.next_line().await {
                debug!(target: "animate.renderer", "{}", line);
                let mut buf = drain_buf.lock().await;
                if buf.len() + line.len() + 1 > STDERR_TAIL_BYTES {
                    // Drop the oldest half so we always keep recent
                    // context — a tail buffer, not a head buffer.
                    let drop_to = buf.len() / 2;
                    let split = buf
                        .char_indices()
                        .find(|(i, _)| *i >= drop_to)
                        .map(|(i, _)| i)
                        .unwrap_or(drop_to);
                    buf.replace_range(..split, "");
                }
                buf.push_str(&line);
                buf.push('\n');
            }
        });

        let mut sidecar = Sidecar {
            child,
            stdin,
            stdout: BufReader::new(stdout).lines(),
            stderr_buf,
            stderr_task: Some(stderr_task),
            renders_done: 0,
            spawned_at: Instant::now(),
            is_dead: false,
        };

        // Wait for the initial `ready` event before considering the
        // sidecar usable. Anything else (an error, EOF) is a hard fail
        // — we won't get a working sidecar from a process that didn't
        // boot cleanly.
        match next_event(&mut sidecar).await {
            Ok(Some(RenderEvent::Ready)) => {
                info!(
                    cmd = %cmd.display(),
                    "renderer pool: sidecar booted"
                );
                Ok(sidecar)
            }
            Ok(Some(RenderEvent::Error { message })) => {
                let tail = sidecar.stderr_buf.lock().await.clone();
                Err(RenderFailure::Transient(format!(
                    "sidecar boot error: {message}\nstderr: {}",
                    tail.trim_end()
                )))
            }
            Ok(other) => {
                let tail = sidecar.stderr_buf.lock().await.clone();
                Err(RenderFailure::Transient(format!(
                    "sidecar boot: expected `ready`, got {other:?}\nstderr: {}",
                    tail.trim_end()
                )))
            }
            Err(e) => {
                let tail = sidecar.stderr_buf.lock().await.clone();
                Err(RenderFailure::Transient(format!(
                    "sidecar boot read failed: {e}\nstderr: {}",
                    tail.trim_end()
                )))
            }
        }
    }

    /// Best-effort drain. Used at app shutdown to close stdin on every
    /// pooled sidecar so they exit cleanly via the `bye` path. Tests
    /// can use it to avoid leaking zombies.
    #[allow(dead_code)]
    pub async fn shutdown(&self) {
        let mut q = self.available.lock().await;
        while let Some(s) = q.pop_front() {
            shutdown_sidecar(s).await;
        }
    }
}

struct Sidecar {
    child: Child,
    stdin: ChildStdin,
    stdout: Lines<BufReader<ChildStdout>>,
    stderr_buf: Arc<Mutex<String>>,
    stderr_task: Option<JoinHandle<()>>,
    renders_done: u32,
    spawned_at: Instant,
    is_dead: bool,
}

/// Drive one render through an existing sidecar.
///
/// The sidecar's `server.ts` loop emits the standard event stream and
/// then a `ready` to signal it's available again. We forward each
/// non-control event over `events_tx`, stop at `ready`, and surface
/// any `error` event as `RenderFailure::Fatal` (the sidecar itself is
/// still alive — only this render failed). A receiver-dropped error
/// on send is non-fatal: the caller stopped listening, but the render
/// itself can still complete.
async fn render_one(
    sidecar: &mut Sidecar,
    spec: &SceneSpec,
    events_tx: &mpsc::UnboundedSender<RenderEvent>,
) -> Result<(), RenderFailure> {
    sidecar.stderr_buf.lock().await.clear();

    let json = serde_json::to_string(spec)
        .map_err(|e| RenderFailure::Fatal(format!("encode SceneSpec: {e}")))?;
    if let Err(e) = sidecar.stdin.write_all(json.as_bytes()).await {
        sidecar.is_dead = true;
        return Err(RenderFailure::Transient(format!(
            "sidecar stdin write failed: {e}"
        )));
    }
    if let Err(e) = sidecar.stdin.write_all(b"\n").await {
        sidecar.is_dead = true;
        return Err(RenderFailure::Transient(format!(
            "sidecar stdin newline failed: {e}"
        )));
    }
    if let Err(e) = sidecar.stdin.flush().await {
        sidecar.is_dead = true;
        return Err(RenderFailure::Transient(format!(
            "sidecar stdin flush failed: {e}"
        )));
    }

    let mut got_done = false;
    let mut error_msg: Option<String> = None;
    loop {
        let evt = match next_event(sidecar).await {
            Ok(Some(e)) => e,
            Ok(None) => {
                // EOF before `ready`. Sidecar crashed mid-render.
                sidecar.is_dead = true;
                let tail = sidecar.stderr_buf.lock().await.clone();
                return Err(RenderFailure::Transient(format!(
                    "sidecar died mid-render: {}",
                    tail.trim_end()
                )));
            }
            Err(e) => {
                sidecar.is_dead = true;
                return Err(RenderFailure::Transient(format!(
                    "sidecar stdout read failed: {e}"
                )));
            }
        };

        match evt {
            RenderEvent::Started | RenderEvent::Frame { .. } | RenderEvent::Encoding { .. } => {
                let _ = events_tx.send(evt);
            }
            RenderEvent::Done { .. } => {
                got_done = true;
                let _ = events_tx.send(evt);
            }
            RenderEvent::Error { message } => {
                // Stash and keep draining — the sidecar will still
                // emit a `ready` so we know it's available again.
                error_msg = Some(message);
            }
            RenderEvent::Ready => break,
            RenderEvent::Bye => {
                // Sidecar shut down mid-render. Treat as crash.
                sidecar.is_dead = true;
                let tail = sidecar.stderr_buf.lock().await.clone();
                return Err(RenderFailure::Transient(format!(
                    "sidecar shut down mid-render: {}",
                    tail.trim_end()
                )));
            }
        }
    }

    if let Some(msg) = error_msg {
        return Err(RenderFailure::Fatal(format!("renderer error: {msg}")));
    }
    if !got_done {
        // Ready without Done means the sidecar handled the spec but
        // never emitted Done. Shouldn't happen unless the protocol
        // breaks; surface as a transient so the job retries.
        return Err(RenderFailure::Transient(
            "sidecar emitted ready without done".into(),
        ));
    }
    Ok(())
}

/// Read one NDJSON event from the sidecar's stdout. Skips blank lines
/// and unparseable lines (logged at debug — they're either stray logs
/// or version drift, neither of which should fail a render).
async fn next_event(sidecar: &mut Sidecar) -> std::io::Result<Option<RenderEvent>> {
    loop {
        match sidecar.stdout.next_line().await? {
            None => return Ok(None),
            Some(line) => {
                let trimmed = line.trim();
                if trimmed.is_empty() {
                    continue;
                }
                match serde_json::from_str::<RenderEvent>(trimmed) {
                    Ok(evt) => return Ok(Some(evt)),
                    Err(e) => {
                        debug!(line = %trimmed, error = %e, "animate.renderer: non-NDJSON line");
                    }
                }
            }
        }
    }
}

/// Close stdin and wait briefly for the child to exit. Falls back to
/// SIGKILL via `kill_on_drop` if the child ignores the EOF — server.ts
/// exits 0 within milliseconds in the happy path.
async fn shutdown_sidecar(mut sidecar: Sidecar) {
    drop(sidecar.stdin);
    let kill_deadline = Duration::from_secs(5);
    match tokio::time::timeout(kill_deadline, sidecar.child.wait()).await {
        Ok(Ok(status)) => debug!(?status, "sidecar exited cleanly"),
        Ok(Err(e)) => warn!(error = %e, "sidecar wait failed"),
        Err(_) => {
            warn!("sidecar didn't exit within 5s; killing");
            let _ = sidecar.child.start_kill();
            let _ = sidecar.child.wait().await;
        }
    }
    if let Some(t) = sidecar.stderr_task.take() {
        let _ = t.await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    /// SidecarCfg has the bound fields we expect; defaults to the
    /// constants. Cheap sanity check that the constructor isn't
    /// quietly overriding values.
    #[test]
    fn cfg_uses_defaults() {
        let cfg = SidecarCfg::new("node".into(), PathBuf::from("/dev/null"));
        assert_eq!(cfg.max_renders_per_proc, DEFAULT_MAX_RENDERS_PER_PROC);
        assert_eq!(cfg.max_age_secs, DEFAULT_MAX_AGE_SECS);
    }

    #[test]
    fn pool_clamps_capacity_to_at_least_one() {
        // capacity 0 would deadlock the semaphore; the constructor
        // should bump it to 1 silently.
        let cfg = SidecarCfg::new("node".into(), PathBuf::from("/dev/null"));
        let pool = RendererPool::new(cfg, 0);
        assert_eq!(pool.permits.available_permits(), 1);
    }

    #[test]
    fn pool_respects_explicit_capacity() {
        let cfg = SidecarCfg::new("node".into(), PathBuf::from("/dev/null"));
        let pool = RendererPool::new(cfg, 3);
        assert_eq!(pool.permits.available_permits(), 3);
    }

    /// A cmd that doesn't exist surfaces a fatal error rather than
    /// hanging waiting for a sidecar that'll never boot.
    #[tokio::test]
    async fn spawn_with_missing_cmd_is_fatal() {
        let cfg = SidecarCfg::new(
            "node".into(),
            PathBuf::from("/this/path/does/not/exist/server.js"),
        );
        let pool = RendererPool::new(cfg, 1);
        let result = pool.spawn_sidecar().await;
        match result {
            Err(RenderFailure::Fatal(msg)) => {
                assert!(msg.contains("does not exist"), "got: {msg}");
            }
            Err(other) => panic!("expected fatal, got transient: {other:?}"),
            Ok(_) => panic!("expected fatal, got Ok(sidecar) — should be unreachable"),
        }
    }
}
