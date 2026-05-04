import { useCallback, useEffect, useRef, useState } from "react";
import { progressWebSocketUrl } from "../api";
import type { JobSnapshot, ProgressEvent } from "../api";
import { useAuth } from "../store/auth";

export type ProgressState = {
  /** Connection state for the badge in the header of the page. */
  readyState: "connecting" | "open" | "closed";
  /** Latest snapshot from the server (or the accumulated event state). */
  jobs: JobSnapshot[];
  /**
   * Latest `stage` string per job_id, populated only by live `progress`
   * events (snapshots/REST polls don't carry it because it isn't
   * persisted on JobSnapshot). Lets the UI show "rendering diagram
   * 3/12" without us widening the OpenAPI-derived JobSnapshot type
   * (which would require backfilling `stage` everywhere it's used).
   * Cleared back to "" on terminal events so a stale "rendering
   * paragraph 5/12" doesn't linger after Ready.
   */
  jobStages: Record<string, string>;
  /** Wall-clock of the most recent event; drives subtle UI shimmer. */
  lastEventAt: number | null;
  /** True right after any terminal event; screen can invalidate queries. */
  terminalTick: number;
};

export type ProgressApi = ProgressState & {
  /**
   * Inject jobs into the in-memory projection (e.g. from a REST poll right
   * after an enqueue mutation). Existing jobs are merged by id; the WS push
   * still wins once it arrives because it carries newer status/progress.
   */
  seedJobs: (jobs: JobSnapshot[]) => void;
};

/**
 * Opens a WebSocket to `/ws/audiobook/:id` and keeps an in-memory projection
 * of every job for that book up to date. The first frame is a `snapshot`; we
 * overlay incoming events onto it instead of round-tripping REST.
 *
 * Reconnects with a 1.5 s backoff if the socket drops — browsers close idle
 * sockets aggressively behind reverse proxies, so you want this by default.
 */
export function useProgressSocket(audiobookId: string | undefined): ProgressApi {
  const accessToken = useAuth((s) => s.accessToken);
  const [state, setState] = useState<ProgressState>({
    readyState: "connecting",
    jobs: [],
    jobStages: {},
    lastEventAt: null,
    terminalTick: 0,
  });
  const reconnectTimer = useRef<number | null>(null);

  useEffect(() => {
    if (!audiobookId || !accessToken) return;
    let disposed = false;
    let socket: WebSocket | null = null;

    function connect(): void {
      if (disposed || !audiobookId || !accessToken) return;
      setState((s) => ({ ...s, readyState: "connecting" }));
      socket = new WebSocket(progressWebSocketUrl(audiobookId, accessToken));

      socket.onopen = () => setState((s) => ({ ...s, readyState: "open" }));

      socket.onmessage = (ev) => {
        try {
          const event = JSON.parse(ev.data as string) as ProgressEvent;
          setState((prev) => applyEvent(prev, event));
        } catch {
          // Malformed frame — ignore, the next one is usually fine.
        }
      };

      socket.onerror = () => {
        // Paired with onclose which fires immediately after.
      };

      socket.onclose = () => {
        if (disposed) return;
        setState((s) => ({ ...s, readyState: "closed" }));
        // Reconnect unless we're unmounting.
        reconnectTimer.current = window.setTimeout(connect, 1500);
      };
    }

    connect();

    return () => {
      disposed = true;
      if (reconnectTimer.current) window.clearTimeout(reconnectTimer.current);
      socket?.close();
    };
  }, [audiobookId, accessToken]);

  const seedJobs = useCallback((incoming: JobSnapshot[]) => {
    setState((prev) => ({ ...prev, jobs: mergeJobs(prev.jobs, incoming) }));
  }, []);

  return { ...state, seedJobs };
}

function applyEvent(prev: ProgressState, event: ProgressEvent): ProgressState {
  const now = Date.now();
  switch (event.type) {
    case "snapshot":
      return { ...prev, jobs: mergeJobs(prev.jobs, event.jobs), lastEventAt: now };
    case "progress":
      return {
        ...prev,
        jobs: upsertJob(prev.jobs, event.job_id, (existing) => ({
          id: event.job_id,
          kind: event.kind,
          status: "running",
          progress_pct: event.pct,
          attempts: existing?.attempts ?? 0,
          chapter_number: existing?.chapter_number ?? event.chapter ?? null,
          last_error: existing?.last_error ?? null,
        })),
        jobStages: { ...prev.jobStages, [event.job_id]: event.stage ?? "" },
        lastEventAt: now,
      };
    case "completed":
      return {
        ...prev,
        jobs: upsertJob(prev.jobs, event.job_id, (existing) => ({
          id: event.job_id,
          kind: event.kind,
          status: "completed",
          progress_pct: 1,
          attempts: existing?.attempts ?? 0,
          chapter_number: existing?.chapter_number ?? null,
          last_error: existing?.last_error ?? null,
        })),
        jobStages: clearStage(prev.jobStages, event.job_id),
        lastEventAt: now,
        terminalTick: prev.terminalTick + 1,
      };
    case "failed":
      return {
        ...prev,
        jobs: upsertJob(prev.jobs, event.job_id, (existing) => ({
          id: event.job_id,
          kind: event.kind,
          status: "dead",
          progress_pct: existing?.progress_pct ?? 0,
          attempts: existing?.attempts ?? 0,
          chapter_number: existing?.chapter_number ?? null,
          last_error: event.error,
        })),
        jobStages: clearStage(prev.jobStages, event.job_id),
        lastEventAt: now,
        terminalTick: prev.terminalTick + 1,
      };
    default:
      return prev;
  }
}

function clearStage(stages: Record<string, string>, jobId: string): Record<string, string> {
  if (!(jobId in stages)) return stages;
  const next = { ...stages };
  delete next[jobId];
  return next;
}

function upsertJob(
  jobs: JobSnapshot[],
  id: string,
  build: (existing: JobSnapshot | undefined) => JobSnapshot,
): JobSnapshot[] {
  const idx = jobs.findIndex((j) => j.id === id);
  if (idx === -1) return [...jobs, build(undefined)];
  const next = jobs.slice();
  next[idx] = build(jobs[idx]);
  return next;
}

/**
 * Merge incoming jobs (from a snapshot or REST poll) with the in-memory
 * projection. New ids are appended; existing ids only refresh when the
 * incoming row is "newer" — i.e. has a more advanced status or higher
 * progress. This keeps an in-flight WS update from being clobbered by a
 * stale REST poll that arrives a few hundred ms later.
 */
function mergeJobs(prev: JobSnapshot[], incoming: JobSnapshot[]): JobSnapshot[] {
  const byId = new Map(prev.map((j) => [j.id, j] as const));
  for (const j of incoming) {
    const existing = byId.get(j.id);
    if (!existing || rank(j.status) > rank(existing.status)) {
      byId.set(j.id, j);
    } else if (rank(j.status) === rank(existing.status) && j.progress_pct > existing.progress_pct) {
      byId.set(j.id, j);
    }
  }
  return Array.from(byId.values());
}

function rank(status: string): number {
  switch (status) {
    case "queued":
    case "throttled":
      return 0;
    case "running":
      return 1;
    case "completed":
    case "failed":
    case "dead":
      return 2;
    default:
      return 0;
  }
}
