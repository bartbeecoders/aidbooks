import { useMemo, useState } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { admin, ApiError } from "../../api";
import type { AdminJobRow } from "../../api";
import { ErrorPane, Loading, PageHeader } from "./AdminLlms";

const STATUS_OPTIONS = [
  "queued",
  "running",
  "completed",
  "failed",
  "dead",
  "throttled",
] as const;
const KIND_OPTIONS = [
  "outline",
  "chapters",
  "tts",
  "tts_chapter",
  "post_process",
  "cover",
  "gc",
] as const;

export function AdminJobs(): JSX.Element {
  const qc = useQueryClient();
  const [status, setStatus] = useState<string>("");
  const [kind, setKind] = useState<string>("");

  const { data, isLoading, error, refetch, isFetching } = useQuery({
    queryKey: ["admin", "jobs", status, kind],
    queryFn: () =>
      admin.jobs.list({
        status: status || undefined,
        kind: kind || undefined,
      }),
    refetchInterval: 5000,
  });

  const retry = useMutation({
    mutationFn: (id: string) => admin.jobs.retry(id),
    onSuccess: () => qc.invalidateQueries({ queryKey: ["admin", "jobs"] }),
  });

  const totalsByStatus = useMemo(() => {
    const counts: Record<string, number> = {};
    for (const j of data?.items ?? []) counts[j.status] = (counts[j.status] ?? 0) + 1;
    return counts;
  }, [data]);

  return (
    <div>
      <PageHeader
        title="Jobs"
        description="Live view of every durable job. Filter by status / kind; dead jobs get a Retry button that clears attempts and re-queues."
      />

      <div className="mb-4 flex flex-wrap items-center gap-2 text-sm">
        <select
          value={status}
          onChange={(e) => setStatus(e.target.value)}
          className="rounded-md border border-slate-700 bg-slate-950 px-2 py-1 text-xs text-slate-100"
        >
          <option value="">All statuses</option>
          {STATUS_OPTIONS.map((s) => (
            <option key={s} value={s}>
              {s}
            </option>
          ))}
        </select>
        <select
          value={kind}
          onChange={(e) => setKind(e.target.value)}
          className="rounded-md border border-slate-700 bg-slate-950 px-2 py-1 text-xs text-slate-100"
        >
          <option value="">All kinds</option>
          {KIND_OPTIONS.map((k) => (
            <option key={k} value={k}>
              {k}
            </option>
          ))}
        </select>
        <button
          onClick={() => void refetch()}
          className="rounded-md border border-slate-700 bg-slate-900 px-2 py-1 text-xs text-slate-200 hover:border-slate-600"
        >
          {isFetching ? "Refreshing…" : "Refresh"}
        </button>
        <div className="ml-auto flex flex-wrap gap-2 text-xs text-slate-400">
          {Object.entries(totalsByStatus).map(([k, v]) => (
            <span
              key={k}
              className="rounded-full border border-slate-800 bg-slate-900/60 px-2 py-0.5"
            >
              {k}: {v}
            </span>
          ))}
        </div>
      </div>

      {isLoading && <Loading />}
      {error && <ErrorPane error={error} />}
      {data && (
        <table className="w-full text-sm">
          <thead className="text-left text-xs uppercase tracking-wide text-slate-500">
            <tr>
              <th className="py-2 pr-4">Kind</th>
              <th className="py-2 pr-4">Status</th>
              <th className="py-2 pr-4">Progress</th>
              <th className="py-2 pr-4">Attempts</th>
              <th className="py-2 pr-4">Book / chapter</th>
              <th className="py-2 pr-4">Queued</th>
              <th className="py-2 pr-4 text-right">Actions</th>
            </tr>
          </thead>
          <tbody>
            {data.items.map((j) => (
              <JobRow
                key={j.id}
                job={j}
                onRetry={() => retry.mutate(j.id)}
                retrying={retry.isPending && retry.variables === j.id}
              />
            ))}
            {data.items.length === 0 && (
              <tr>
                <td colSpan={7} className="py-6 text-center text-sm text-slate-500">
                  No jobs match the current filter.
                </td>
              </tr>
            )}
          </tbody>
        </table>
      )}
      {retry.error && (
        <p className="mt-3 text-sm text-rose-400">
          {retry.error instanceof ApiError ? retry.error.message : "Retry failed"}
        </p>
      )}
    </div>
  );
}

function JobRow({
  job,
  onRetry,
  retrying,
}: {
  job: AdminJobRow;
  onRetry: () => void;
  retrying: boolean;
}): JSX.Element {
  const canRetry = job.status === "dead" || job.status === "failed";
  return (
    <tr className="border-t border-slate-800 align-top">
      <td className="py-3 pr-4 font-mono text-xs text-slate-300">{job.kind}</td>
      <td className="py-3 pr-4">
        <span
          className={`rounded-full px-2 py-0.5 text-[11px] ${STATUS_CLASSES[job.status] ?? ""}`}
        >
          {job.status}
        </span>
      </td>
      <td className="py-3 pr-4 text-slate-300">
        {Math.round(job.progress_pct * 100)}%
      </td>
      <td className="py-3 pr-4 text-slate-300">
        {job.attempts}/{job.max_attempts}
      </td>
      <td className="py-3 pr-4 text-xs">
        <div className="font-mono text-slate-400">
          {job.audiobook_id ? job.audiobook_id.slice(0, 8) : "—"}
          {job.chapter_number ? ` · ch${job.chapter_number}` : ""}
        </div>
        {job.last_error && (
          <div className="mt-1 line-clamp-2 text-rose-400">{job.last_error}</div>
        )}
      </td>
      <td className="py-3 pr-4 text-xs text-slate-400">
        {new Date(job.queued_at).toLocaleTimeString()}
      </td>
      <td className="py-3 pr-4 text-right">
        {canRetry ? (
          <button
            onClick={onRetry}
            disabled={retrying}
            className="rounded-md border border-emerald-800 bg-emerald-950 px-2 py-1 text-xs text-emerald-200 hover:bg-emerald-900 disabled:opacity-50"
          >
            {retrying ? "…" : "Retry"}
          </button>
        ) : (
          <span className="text-xs text-slate-600">—</span>
        )}
      </td>
    </tr>
  );
}

const STATUS_CLASSES: Record<string, string> = {
  queued: "bg-amber-950 text-amber-300",
  running: "bg-sky-900 text-sky-200",
  completed: "bg-emerald-900 text-emerald-200",
  failed: "bg-rose-950 text-rose-300",
  dead: "bg-rose-900 text-rose-200",
  throttled: "bg-slate-800 text-slate-300",
};
