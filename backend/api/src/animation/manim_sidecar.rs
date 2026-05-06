//! Long-lived Python (Manim) renderer pool — Phase G.6.
//!
//! Mirrors [`super::sidecar::RendererPool`] (the Revideo pool) but
//! drives the Python sidecar at `backend/manim/listenai_manim/server.py`.
//! Same lifecycle: persistent child per slot, NDJSON stdin/stdout,
//! recycle on N renders / M minutes / first transient failure.
//!
//! Request shape sent on stdin (one per line):
//!
//! ```json
//! {
//!     "version": 1,
//!     "template_id": "function_plot",
//!     "params": {"fn": "x**2", "domain": [-3, 3]},
//!     "duration_ms": 12000,
//!     "output_mp4": "/abs/path/seg.mp4"
//! }
//! ```
//!
//! Event shape on stdout — see `backend/manim/listenai_manim/server.py`.

use std::collections::VecDeque;
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde::Deserialize;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, Lines};
use tokio::process::{Child, ChildStdin, ChildStdout, Command};
use tokio::sync::{Mutex, Semaphore};
use tokio::task::JoinHandle;
use tracing::{debug, info, warn};

use super::sidecar::RenderFailure;

/// Python sidecar lifecycle bounds. Same justification as the
/// Revideo pool's: cap renders + age to bound any in-process Manim
/// memory drift without paying a fresh-process tax on every render.
const DEFAULT_MAX_RENDERS_PER_PROC: u32 = 30;
const DEFAULT_MAX_AGE_SECS: u64 = 30 * 60;

/// Bounded stderr tail per sidecar; helps diagnose Manim failures
/// without unbounded memory growth.
const STDERR_TAIL_BYTES: usize = 4_096;

/// Protocol version handshake. Mirrors the constant in
/// `listenai_manim/server.py`. Bump both at once on a breaking
/// change to the request / event shape.
const PROTOCOL_VERSION: u32 = 1;

/// One render request the pool dispatches to a sidecar.
///
/// Two shapes:
///   * [`ManimRequest::Template`] — pick a built-in Scene class out of
///     `listenai_manim/templates/` and pass `params` to it. The
///     classifier picks one of 8 well-known kinds (Phase G.4).
///   * [`ManimRequest::RawScene`] — pass an LLM-generated `Scene`
///     class body to the sidecar. The Python side AST-screens the
///     code, then `exec`s + renders it. Used for the `custom_manim`
///     visual_kind escape hatch (Phase H).
#[derive(Debug, Clone)]
pub enum ManimRequest {
    Template {
        template_id: String,
        params: serde_json::Value,
        duration_ms: u64,
        output_mp4: PathBuf,
    },
    RawScene {
        /// Python source of a single `class Scene(TemplateScene): ...`.
        /// The sidecar AST-screens this; failures come back as a
        /// `RawSceneError`-tagged error event.
        code: String,
        duration_ms: u64,
        output_mp4: PathBuf,
    },
}

/// NDJSON event shape from the sidecar. Mirrors
/// `listenai_manim/server.py::emit`.
#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum SidecarEvent {
    Ready,
    Started,
    Done {
        #[allow(dead_code)]
        mp4: PathBuf,
        #[allow(dead_code)]
        duration_ms: u64,
    },
    Error {
        message: String,
    },
    Bye,
}

#[derive(Debug, Clone)]
pub struct ManimSidecarCfg {
    /// Held for back-compat with `Config::animate_manim_python_bin`,
    /// but no longer used at spawn time — the sidecar script's
    /// `#!.venv/bin/python` shebang picks the right interpreter on
    /// its own. Forcing a `python_bin` here historically caused the
    /// host's `python` (often conda's) to load the script and fail
    /// to import `listenai_manim` because conda's site-packages
    /// isn't the same as the venv's. Kept as an unused field so
    /// existing `LISTENAI_ANIMATE_MANIM_PYTHON_BIN` env values
    /// don't crash config parsing.
    #[allow(dead_code)]
    pub python_bin: String,
    /// Path to the sidecar entry — typically the venv's
    /// `listenai-manim-server` console script. Must exist on disk;
    /// `spawn_sidecar` canonicalises before running.
    pub sidecar_cmd: PathBuf,
    /// LD_PRELOAD value applied to the sidecar's environment. On
    /// Arch, the wheel-bundled libfontconfig clashes with system
    /// pango; pre-loading `/usr/lib/libfontconfig.so.1` fixes it.
    /// Empty string = don't set LD_PRELOAD.
    pub ld_preload: String,
    pub max_renders_per_proc: u32,
    pub max_age_secs: u64,
}

impl ManimSidecarCfg {
    pub fn new(python_bin: String, sidecar_cmd: PathBuf, ld_preload: String) -> Self {
        Self {
            python_bin,
            sidecar_cmd,
            ld_preload,
            max_renders_per_proc: DEFAULT_MAX_RENDERS_PER_PROC,
            max_age_secs: DEFAULT_MAX_AGE_SECS,
        }
    }
}

/// Pool of long-lived Python (Manim) renderer processes.
pub struct ManimRendererPool {
    cfg: ManimSidecarCfg,
    permits: Semaphore,
    available: Mutex<VecDeque<Sidecar>>,
}

impl ManimRendererPool {
    pub fn new(cfg: ManimSidecarCfg, capacity: usize) -> Arc<Self> {
        let cap = capacity.max(1);
        Arc::new(Self {
            cfg,
            permits: Semaphore::new(cap),
            available: Mutex::new(VecDeque::with_capacity(cap)),
        })
    }

    /// Render one diagram segment. Acquires (or spawns) a sidecar,
    /// pipes the request, drains events through `_handle_event`
    /// until `Done` or `Error`, then returns the sidecar to the
    /// pool (or kills it if exhausted / the request failed
    /// transiently).
    pub async fn render(&self, req: &ManimRequest) -> Result<(), RenderFailure> {
        let _permit =
            self.permits.acquire().await.map_err(|e| {
                RenderFailure::Transient(format!("manim pool semaphore closed: {e}"))
            })?;

        let mut sidecar = self.acquire_sidecar().await?;
        let result = render_one(&mut sidecar, req).await;
        sidecar.renders_done += 1;

        let exhausted = sidecar.renders_done >= self.cfg.max_renders_per_proc
            || sidecar.spawned_at.elapsed().as_secs() >= self.cfg.max_age_secs;
        let dead = sidecar.is_dead || matches!(result, Err(RenderFailure::Transient(_)));

        if dead || exhausted {
            debug!(
                renders_done = sidecar.renders_done,
                age_secs = sidecar.spawned_at.elapsed().as_secs(),
                dead,
                "manim pool: recycling sidecar"
            );
            shutdown_sidecar(sidecar).await;
        } else {
            self.available.lock().await.push_back(sidecar);
        }

        result
    }

    async fn acquire_sidecar(&self) -> Result<Sidecar, RenderFailure> {
        if let Some(s) = self.available.lock().await.pop_front() {
            return Ok(s);
        }
        self.spawn_sidecar().await
    }

    async fn spawn_sidecar(&self) -> Result<Sidecar, RenderFailure> {
        let cmd = std::fs::canonicalize(&self.cfg.sidecar_cmd).map_err(|e| {
            RenderFailure::Fatal(format!(
                "manim sidecar cmd `{}` does not exist: {e}",
                self.cfg.sidecar_cmd.display()
            ))
        })?;

        // Invoke the script *directly* and let its shebang
        // (`#!.../.venv/bin/python`) pick the correct Python. We
        // used to do `Command::new(python_bin).arg(cmd)` but that
        // forces every spawn through `python_bin` (typically just
        // `python` from PATH), which resolves to the user's system
        // / conda Python — completely bypassing the venv where
        // `listenai_manim` is actually installed. The shebang is
        // baked in by `uv pip install` and points at the venv's
        // own interpreter.
        let mut command = Command::new(&cmd);
        command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        if !self.cfg.ld_preload.is_empty() {
            command.env("LD_PRELOAD", &self.cfg.ld_preload);
        }

        let mut child = command
            .spawn()
            .map_err(|e| RenderFailure::Transient(format!("spawn manim sidecar: {e}")))?;

        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| RenderFailure::Transient("manim sidecar stdin not piped".into()))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| RenderFailure::Transient("manim sidecar stdout not piped".into()))?;
        let stderr = child
            .stderr
            .take()
            .ok_or_else(|| RenderFailure::Transient("manim sidecar stderr not piped".into()))?;

        let stderr_buf = Arc::new(Mutex::new(String::new()));
        let drain_buf = stderr_buf.clone();
        let stderr_task = tokio::spawn(async move {
            let mut reader = BufReader::new(stderr).lines();
            while let Ok(Some(line)) = reader.next_line().await {
                debug!(target: "animate.manim", "{}", line);
                let mut buf = drain_buf.lock().await;
                if buf.len() + line.len() + 1 > STDERR_TAIL_BYTES {
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

        // Wait for the boot `ready`. Any other event = bad boot.
        match next_event(&mut sidecar).await {
            Ok(Some(SidecarEvent::Ready)) => {
                info!(cmd = %cmd.display(), "manim pool: sidecar booted");
                Ok(sidecar)
            }
            Ok(Some(SidecarEvent::Error { message })) => {
                let tail = sidecar.stderr_buf.lock().await.clone();
                Err(RenderFailure::Transient(format!(
                    "manim sidecar boot error: {message}\nstderr: {}",
                    tail.trim_end()
                )))
            }
            Ok(other) => {
                let tail = sidecar.stderr_buf.lock().await.clone();
                Err(RenderFailure::Transient(format!(
                    "manim sidecar boot: expected `ready`, got {other:?}\nstderr: {}",
                    tail.trim_end()
                )))
            }
            Err(e) => {
                let tail = sidecar.stderr_buf.lock().await.clone();
                Err(RenderFailure::Transient(format!(
                    "manim sidecar boot read failed: {e}\nstderr: {}",
                    tail.trim_end()
                )))
            }
        }
    }

    /// Best-effort shutdown of every pooled sidecar. Used at app
    /// teardown so SIGINT cleanly closes each Python process.
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

async fn render_one(sidecar: &mut Sidecar, req: &ManimRequest) -> Result<(), RenderFailure> {
    sidecar.stderr_buf.lock().await.clear();

    // Build the request JSON. Single line — server.py splits on
    // newline, so we never embed unescaped \n in the payload.
    // The `kind` discriminator picks the sidecar's branch; older
    // builds without it fall through to the template path on the
    // Python side, but we always set it explicitly to keep the
    // wire format unambiguous.
    let payload = match req {
        ManimRequest::Template {
            template_id,
            params,
            duration_ms,
            output_mp4,
        } => serde_json::json!({
            "version": PROTOCOL_VERSION,
            "kind": "template",
            "template_id": template_id,
            "params": params,
            "duration_ms": duration_ms,
            "output_mp4": output_mp4.to_string_lossy(),
        }),
        ManimRequest::RawScene {
            code,
            duration_ms,
            output_mp4,
        } => serde_json::json!({
            "version": PROTOCOL_VERSION,
            "kind": "raw_scene",
            "code": code,
            "duration_ms": duration_ms,
            "output_mp4": output_mp4.to_string_lossy(),
        }),
    };
    let line = serde_json::to_string(&payload)
        .map_err(|e| RenderFailure::Fatal(format!("encode manim request: {e}")))?;

    if let Err(e) = sidecar.stdin.write_all(line.as_bytes()).await {
        sidecar.is_dead = true;
        return Err(RenderFailure::Transient(format!(
            "manim sidecar stdin write failed: {e}"
        )));
    }
    if let Err(e) = sidecar.stdin.write_all(b"\n").await {
        sidecar.is_dead = true;
        return Err(RenderFailure::Transient(format!(
            "manim sidecar stdin newline failed: {e}"
        )));
    }
    if let Err(e) = sidecar.stdin.flush().await {
        sidecar.is_dead = true;
        return Err(RenderFailure::Transient(format!(
            "manim sidecar stdin flush failed: {e}"
        )));
    }

    let mut got_done = false;
    let mut error_msg: Option<String> = None;
    loop {
        let evt = match next_event(sidecar).await {
            Ok(Some(e)) => e,
            Ok(None) => {
                sidecar.is_dead = true;
                let tail = sidecar.stderr_buf.lock().await.clone();
                return Err(RenderFailure::Transient(format!(
                    "manim sidecar died mid-render: {}",
                    tail.trim_end()
                )));
            }
            Err(e) => {
                sidecar.is_dead = true;
                return Err(RenderFailure::Transient(format!(
                    "manim sidecar stdout read failed: {e}"
                )));
            }
        };

        match evt {
            SidecarEvent::Started => {
                // No-op; could be surfaced as progress 0.0 if the
                // caller wires events through. Phase G.6's caller
                // doesn't, so just acknowledge.
            }
            SidecarEvent::Done { .. } => {
                got_done = true;
            }
            SidecarEvent::Error { message } => {
                error_msg = Some(message);
            }
            SidecarEvent::Ready => break,
            SidecarEvent::Bye => {
                sidecar.is_dead = true;
                let tail = sidecar.stderr_buf.lock().await.clone();
                return Err(RenderFailure::Transient(format!(
                    "manim sidecar shut down mid-render: {}",
                    tail.trim_end()
                )));
            }
        }
    }

    if let Some(msg) = error_msg {
        return Err(RenderFailure::Fatal(format!("manim render error: {msg}")));
    }
    if !got_done {
        return Err(RenderFailure::Transient(
            "manim sidecar ready without done".into(),
        ));
    }
    Ok(())
}

async fn next_event(sidecar: &mut Sidecar) -> std::io::Result<Option<SidecarEvent>> {
    loop {
        match sidecar.stdout.next_line().await? {
            None => return Ok(None),
            Some(line) => {
                let trimmed = line.trim();
                if trimmed.is_empty() {
                    continue;
                }
                match serde_json::from_str::<SidecarEvent>(trimmed) {
                    Ok(evt) => return Ok(Some(evt)),
                    Err(e) => {
                        debug!(line = %trimmed, error = %e, "animate.manim: non-NDJSON line");
                    }
                }
            }
        }
    }
}

async fn shutdown_sidecar(mut sidecar: Sidecar) {
    drop(sidecar.stdin);
    let kill_deadline = Duration::from_secs(5);
    match tokio::time::timeout(kill_deadline, sidecar.child.wait()).await {
        Ok(Ok(status)) => debug!(?status, "manim sidecar exited cleanly"),
        Ok(Err(e)) => warn!(error = %e, "manim sidecar wait failed"),
        Err(_) => {
            warn!("manim sidecar didn't exit within 5s; killing");
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

    #[test]
    fn cfg_uses_defaults() {
        let cfg = ManimSidecarCfg::new("python".into(), PathBuf::from("/dev/null"), String::new());
        assert_eq!(cfg.max_renders_per_proc, DEFAULT_MAX_RENDERS_PER_PROC);
        assert_eq!(cfg.max_age_secs, DEFAULT_MAX_AGE_SECS);
    }

    #[test]
    fn pool_clamps_capacity_to_at_least_one() {
        let cfg = ManimSidecarCfg::new("python".into(), PathBuf::from("/dev/null"), String::new());
        let pool = ManimRendererPool::new(cfg, 0);
        assert_eq!(pool.permits.available_permits(), 1);
    }

    #[tokio::test]
    async fn spawn_with_missing_cmd_is_fatal() {
        let cfg = ManimSidecarCfg::new(
            "python".into(),
            PathBuf::from("/this/path/does/not/exist/server.py"),
            String::new(),
        );
        let pool = ManimRendererPool::new(cfg, 1);
        match pool.spawn_sidecar().await {
            Err(RenderFailure::Fatal(msg)) => {
                assert!(msg.contains("does not exist"), "got: {msg}");
            }
            Err(other) => panic!("expected fatal, got transient: {other:?}"),
            Ok(_) => panic!("expected fatal, got Ok(sidecar)"),
        }
    }
}
