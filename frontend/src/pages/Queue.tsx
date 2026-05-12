import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { Link } from "react-router-dom";
import { queue } from "../api";
import type { QueueItem, QueueItemState } from "../api/types";

const STATE_BADGE: Record<QueueItemState, string> = {
  queued:
    "border-slate-700 bg-slate-900 text-slate-300",
  running:
    "border-sky-600 bg-sky-600/15 text-sky-200",
  paused:
    "border-amber-700 bg-amber-700/15 text-amber-200",
  completed:
    "border-emerald-700 bg-emerald-700/15 text-emerald-200",
  failed: "border-rose-700 bg-rose-700/15 text-rose-200",
  cancelled: "border-slate-700 bg-slate-900 text-slate-400",
};

export function Queue(): JSX.Element {
  const qc = useQueryClient();
  // Refetch every 3 s so progress + cost ticks forward without a websocket.
  // The list page doesn't need real-time precision, but stale data here
  // is what would make users think the queue is stuck.
  const q = useQuery({
    queryKey: ["queue"],
    queryFn: () => queue.list(),
    refetchInterval: 3000,
    refetchIntervalInBackground: false,
  });

  const invalidate = (): void => {
    qc.invalidateQueries({ queryKey: ["queue"] });
  };

  const pause = useMutation({
    mutationFn: () => queue.pause(),
    onSuccess: invalidate,
  });
  const resume = useMutation({
    mutationFn: () => queue.resume(),
    onSuccess: invalidate,
  });
  const clear = useMutation({
    mutationFn: () => queue.clear(),
    onSuccess: invalidate,
  });
  const cancel = useMutation({
    mutationFn: (id: string) => queue.cancel(id),
    onSuccess: invalidate,
  });

  const data = q.data;
  const items = data?.items ?? [];
  const paused = data?.paused ?? false;
  const running = items.find((it) => it.state === "running");
  const queued = items.filter((it) => it.state === "queued");
  const history = items.filter(
    (it) =>
      it.state === "completed" ||
      it.state === "failed" ||
      it.state === "cancelled",
  );

  return (
    <section className="mx-auto max-w-5xl space-y-6">
      <div className="flex items-baseline justify-between gap-4">
        <div>
          <h1 className="text-2xl font-semibold tracking-tight">Queue</h1>
          <p className="mt-1 text-sm text-slate-400">
            One audiobook is generated at a time. Add new books from{" "}
            <Link to="/app/new" className="underline hover:text-slate-300">
              New audiobook
            </Link>{" "}
            with <em>Add to queue</em>.
          </p>
        </div>
        <div className="flex flex-wrap items-center gap-2">
          {paused ? (
            <button
              onClick={() => resume.mutate()}
              disabled={resume.isPending}
              className="rounded-md border border-emerald-700 bg-emerald-700/10 px-3 py-1.5 text-sm font-medium text-emerald-200 hover:border-emerald-500 disabled:opacity-50"
            >
              {resume.isPending ? "Resuming…" : "Resume queue"}
            </button>
          ) : (
            <button
              onClick={() => pause.mutate()}
              disabled={pause.isPending || !running}
              className="rounded-md border border-amber-700 bg-amber-700/10 px-3 py-1.5 text-sm font-medium text-amber-200 hover:border-amber-500 disabled:opacity-50"
              title={
                running
                  ? "Pause — the currently running book keeps going, but the queue won't start the next one until you resume."
                  : "Nothing is running."
              }
            >
              {pause.isPending ? "Pausing…" : "Pause queue"}
            </button>
          )}
          <button
            onClick={() => {
              if (
                window.confirm(
                  "Drop every queued (not-yet-started) item? The currently running book is left alone.",
                )
              ) {
                clear.mutate();
              }
            }}
            disabled={clear.isPending || queued.length === 0}
            className="rounded-md border border-rose-700 bg-rose-700/10 px-3 py-1.5 text-sm font-medium text-rose-200 hover:border-rose-500 disabled:opacity-50"
          >
            {clear.isPending ? "Clearing…" : "Clear pending"}
          </button>
        </div>
      </div>

      {paused && (
        <p className="rounded-md border border-amber-900/60 bg-amber-950/30 p-3 text-sm text-amber-100">
          Queue is paused. The next audiobook won&apos;t start until you
          resume.
        </p>
      )}

      {q.isLoading && (
        <p className="text-sm text-slate-500">Loading queue…</p>
      )}

      {!q.isLoading && running && (
        <QueueSection title="Now generating">
          <QueueRow item={running} onCancel={() => cancel.mutate(running.id)} />
        </QueueSection>
      )}

      {!q.isLoading && queued.length > 0 && (
        <QueueSection title={`Up next (${queued.length})`}>
          {queued.map((it) => (
            <QueueRow
              key={it.id}
              item={it}
              onCancel={() => cancel.mutate(it.id)}
            />
          ))}
        </QueueSection>
      )}

      {!q.isLoading && history.length > 0 && (
        <QueueSection title="Recent">
          {history.slice(0, 12).map((it) => (
            <QueueRow key={it.id} item={it} onCancel={null} />
          ))}
        </QueueSection>
      )}

      {!q.isLoading && items.length === 0 && (
        <div className="rounded-lg border border-dashed border-slate-800 bg-slate-950/40 p-8 text-center">
          <p className="text-sm text-slate-400">
            The queue is empty. Add a book from{" "}
            <Link
              to="/app/new"
              className="underline decoration-dotted hover:text-slate-200"
            >
              New audiobook
            </Link>
            .
          </p>
        </div>
      )}
    </section>
  );
}

function QueueSection({
  title,
  children,
}: {
  title: string;
  children: React.ReactNode;
}): JSX.Element {
  return (
    <div>
      <h2 className="mb-2 text-xs font-semibold uppercase tracking-wide text-slate-500">
        {title}
      </h2>
      <ul className="space-y-2">{children}</ul>
    </div>
  );
}

function QueueRow({
  item,
  onCancel,
}: {
  item: QueueItem;
  onCancel: (() => void) | null;
}): JSX.Element {
  const isLive = item.state === "queued" || item.state === "running";
  const formatTitle = item.title?.trim() || item.topic || "(untitled)";
  return (
    <li className="rounded-lg border border-slate-800 bg-slate-900/40 p-4">
      <div className="flex items-start justify-between gap-3">
        <div className="min-w-0 flex-1">
          <div className="flex items-baseline gap-2">
            <span className="text-[11px] uppercase tracking-wide text-slate-500">
              #{item.position}
            </span>
            <p className="truncate text-base font-medium text-slate-100">
              {item.is_short && "🎬 "}
              {item.is_songbook && "🎵 "}
              {formatTitle}
            </p>
            <span
              className={`rounded-full border px-2 py-0.5 text-[11px] uppercase tracking-wide ${
                STATE_BADGE[item.state]
              }`}
            >
              {item.state}
            </span>
          </div>
          <p className="mt-0.5 truncate text-[11px] text-slate-500">
            {item.topic}
            {item.language ? ` · ${item.language}` : ""}
          </p>
        </div>
        <div className="flex shrink-0 items-center gap-2">
          <Link
            to={`/app/book/${item.audiobook_id}`}
            className="rounded-md border border-slate-700 bg-slate-900 px-2.5 py-1 text-xs text-slate-300 hover:border-slate-600 hover:text-slate-100"
            title="Open the book detail + logs"
          >
            Logs
          </Link>
          {onCancel && isLive && (
            <button
              onClick={() => {
                if (
                  window.confirm(
                    item.state === "running"
                      ? "Cancel the currently-running generation? Live jobs will be killed."
                      : "Remove this item from the queue?",
                  )
                ) {
                  onCancel();
                }
              }}
              className="rounded-md border border-rose-700 bg-rose-700/10 px-2.5 py-1 text-xs font-medium text-rose-200 hover:border-rose-500"
            >
              Cancel
            </button>
          )}
        </div>
      </div>

      <div className="mt-3 grid grid-cols-1 gap-3 sm:grid-cols-3">
        <Stat label="Step" value={item.step} />
        <Stat
          label="Progress"
          value={`${Math.round(Math.max(0, Math.min(100, item.progress_pct)))}%`}
        />
        <Stat
          label="Cost"
          value={`$${item.cost_usd.toFixed(3)}`}
          hint="Sum of LLM/TTS spend logged for this book"
        />
      </div>

      {(item.state === "running" || item.state === "queued") && (
        <ProgressBar pct={item.progress_pct} />
      )}

      {item.error && (
        <p className="mt-2 break-words text-xs text-rose-300">
          {item.error}
        </p>
      )}
    </li>
  );
}

function Stat({
  label,
  value,
  hint,
}: {
  label: string;
  value: string;
  hint?: string;
}): JSX.Element {
  return (
    <div className="rounded-md border border-slate-800 bg-slate-950/50 px-3 py-2">
      <p className="text-[10px] uppercase tracking-wide text-slate-500">
        {label}
      </p>
      <p
        className="mt-0.5 text-sm font-medium text-slate-100"
        title={hint}
      >
        {value}
      </p>
    </div>
  );
}

function ProgressBar({ pct }: { pct: number }): JSX.Element {
  const clamped = Math.max(0, Math.min(100, pct));
  return (
    <div className="mt-3 h-1.5 w-full overflow-hidden rounded-full bg-slate-800">
      <div
        className="h-full bg-sky-500 transition-[width]"
        style={{ width: `${clamped}%` }}
      />
    </div>
  );
}
