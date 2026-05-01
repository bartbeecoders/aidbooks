import { useEffect, useMemo, useRef, useState } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { Link, useNavigate, useParams } from "react-router-dom";
import { audiobooks, ApiError, coverImageUrl } from "../api";
import type { AudiobookStatus, AudiobookSummary } from "../api";
import { useAuth } from "../store/auth";
import { RenameAudiobookDialog } from "../components/RenameAudiobookDialog";
import { BookDetail } from "./BookDetail";

type SortKey = "title" | "language" | "status" | "updated" | "duration";
type StatusFilter = "all" | AudiobookStatus;
type DurationFilter = "all" | "unknown" | "lt30" | "30to60" | "1to3h" | "gt3h";

const SORT_OPTIONS: { key: SortKey; label: string }[] = [
  { key: "updated", label: "Updated" },
  { key: "title", label: "Title" },
  { key: "language", label: "Language" },
  { key: "status", label: "Status" },
  { key: "duration", label: "Duration" },
];

// Bucket bounds in milliseconds. Inclusive of lower, exclusive of upper.
const DURATION_BUCKETS: { key: DurationFilter; label: string; min: number; max: number }[] = [
  { key: "lt30", label: "Under 30 min", min: 0, max: 30 * 60_000 },
  { key: "30to60", label: "30 min – 1 h", min: 30 * 60_000, max: 60 * 60_000 },
  { key: "1to3h", label: "1 – 3 h", min: 60 * 60_000, max: 3 * 60 * 60_000 },
  { key: "gt3h", label: "Over 3 h", min: 3 * 60 * 60_000, max: Number.POSITIVE_INFINITY },
];

// Order in which status filter chips appear; "all" first, then a useful
// reading order through the pipeline.
const STATUS_ORDER: AudiobookStatus[] = [
  "draft",
  "outline_pending",
  "outline_ready",
  "chapters_running",
  "text_ready",
  "audio_ready",
  "failed",
];

const UNCATEGORIZED = "Uncategorized";

export function Library(): JSX.Element {
  const { id: selectedId } = useParams<{ id?: string }>();
  const { data, isLoading, error } = useQuery({
    queryKey: ["audiobooks"],
    queryFn: () => audiobooks.list(),
  });
  const [renaming, setRenaming] = useState<AudiobookSummary | null>(null);
  const [statusFilter, setStatusFilter] = useState<StatusFilter>("all");
  const [durationFilter, setDurationFilter] = useState<DurationFilter>("all");
  const [sortKey, setSortKey] = useState<SortKey>("updated");
  const [groupByCategory, setGroupByCategory] = useState(true);
  const [search, setSearch] = useState("");
  // Drag-and-drop state for category reassignment. Only meaningful when
  // grouping is on; otherwise the rows aren't sorted into category
  // sections so dropping wouldn't have a target.
  const [draggingId, setDraggingId] = useState<string | null>(null);
  // `null` here = "Uncategorized" target; `undefined` = no group hovered.
  const [dragOverCategory, setDragOverCategory] = useState<
    string | null | undefined
  >(undefined);

  const qc = useQueryClient();
  const setCategory = useMutation({
    mutationFn: ({ id, category }: { id: string; category: string }) =>
      audiobooks.patch(id, { category }),
    onMutate: async ({ id, category }) => {
      // Optimistic: snap the row into its new group immediately, then
      // reconcile from the server response. Failure rolls back.
      await qc.cancelQueries({ queryKey: ["audiobooks"] });
      const prev = qc.getQueryData<{ items: AudiobookSummary[] }>([
        "audiobooks",
      ]);
      qc.setQueryData<{ items: AudiobookSummary[] } | undefined>(
        ["audiobooks"],
        (old) => {
          if (!old) return old;
          return {
            ...old,
            items: old.items.map((b) =>
              b.id === id ? { ...b, category: category || null } : b,
            ),
          };
        },
      );
      return { prev };
    },
    onError: (_e, _v, ctx) => {
      if (ctx?.prev) qc.setQueryData(["audiobooks"], ctx.prev);
    },
    onSettled: () => qc.invalidateQueries({ queryKey: ["audiobooks"] }),
  });

  // Per-status counts drive the badge on each filter chip and let us
  // hide chips that would match nothing.
  const statusCounts = useMemo(() => {
    const map: Partial<Record<AudiobookStatus, number>> = {};
    for (const b of data?.items ?? []) {
      map[b.status] = (map[b.status] ?? 0) + 1;
    }
    return map;
  }, [data]);

  const filteredSorted = useMemo(() => {
    const items = data?.items ?? [];
    const ql = search.trim().toLowerCase();
    const matched = items.filter((b) => {
      if (statusFilter !== "all" && b.status !== statusFilter) return false;
      if (durationFilter !== "all" && !matchesDuration(b.duration_ms, durationFilter)) {
        return false;
      }
      if (ql) {
        const hay = `${b.title} ${b.topic} ${b.category ?? ""}`.toLowerCase();
        if (!hay.includes(ql)) return false;
      }
      return true;
    });
    matched.sort((a, b) => sortBooks(a, b, sortKey));
    return matched;
  }, [data, statusFilter, durationFilter, search, sortKey]);

  // When grouping is on, bucket the (already-sorted) list by category.
  // Each group keeps the surrounding sort order. "Uncategorized" rows
  // collect into their own bucket and sort to the end.
  const grouped = useMemo(() => {
    if (!groupByCategory) {
      return [{ category: null, items: filteredSorted }];
    }
    const buckets = new Map<string, AudiobookSummary[]>();
    for (const b of filteredSorted) {
      const key = b.category?.trim() || UNCATEGORIZED;
      const list = buckets.get(key) ?? [];
      list.push(b);
      buckets.set(key, list);
    }
    const keys = [...buckets.keys()].sort((a, b) => {
      // Uncategorized sinks to the bottom; otherwise alphabetical.
      if (a === UNCATEGORIZED) return 1;
      if (b === UNCATEGORIZED) return -1;
      return a.localeCompare(b);
    });
    return keys.map((category) => ({
      category: category === UNCATEGORIZED ? null : category,
      items: buckets.get(category)!,
    }));
  }, [filteredSorted, groupByCategory]);

  return (
    <section className="grid grid-cols-1 gap-4 lg:grid-cols-[320px,1fr]">
      <aside className="lg:sticky lg:top-4 lg:flex lg:max-h-[calc(100vh-6rem)] lg:flex-col">
        {/* Header (search/filter/sort) — non-scrolling. Stays put while
            the list scrolls underneath. */}
        <div className="lg:shrink-0">
          <div className="mb-3 flex items-baseline justify-between">
            <h1 className="text-xl font-semibold tracking-tight">Library</h1>
            <Link
              to="/app/new"
              className="rounded-md bg-sky-600 px-2.5 py-1 text-xs font-medium text-white hover:bg-sky-500"
            >
              + New
            </Link>
          </div>

          <input
            type="search"
            value={search}
            onChange={(e) => setSearch(e.target.value)}
            placeholder="Search title, topic, category…"
            className="mb-3 w-full rounded-md border border-slate-800 bg-slate-950 px-3 py-1.5 text-sm text-slate-100 outline-none focus:border-sky-600"
          />

          <FilterRow
            statusFilter={statusFilter}
            onStatusFilter={setStatusFilter}
            counts={statusCounts}
            totalCount={data?.items.length ?? 0}
          />

          <div className="mb-3 flex flex-wrap items-center gap-2 text-xs text-slate-400">
            <label className="text-slate-500">Sort:</label>
            <select
              value={sortKey}
              onChange={(e) => setSortKey(e.target.value as SortKey)}
              className="rounded-md border border-slate-800 bg-slate-950 px-2 py-1 text-xs text-slate-100"
            >
              {SORT_OPTIONS.map((o) => (
                <option key={o.key} value={o.key}>
                  {o.label}
                </option>
              ))}
            </select>
            <label className="text-slate-500">Duration:</label>
            <select
              value={durationFilter}
              onChange={(e) => setDurationFilter(e.target.value as DurationFilter)}
              className="rounded-md border border-slate-800 bg-slate-950 px-2 py-1 text-xs text-slate-100"
            >
              <option value="all">Any</option>
              {DURATION_BUCKETS.map((b) => (
                <option key={b.key} value={b.key}>
                  {b.label}
                </option>
              ))}
              <option value="unknown">Not narrated</option>
            </select>
            <label className="ml-auto inline-flex cursor-pointer items-center gap-1.5">
              <input
                type="checkbox"
                checked={groupByCategory}
                onChange={(e) => setGroupByCategory(e.target.checked)}
                className="h-3.5 w-3.5 accent-sky-500"
              />
              Group by category
            </label>
          </div>
        </div>

        {/* Scrolling list. The thin scrollbar only renders when this
            inner column overflows; the controls above stay put. */}
        <div
          className="lg:min-h-0 lg:flex-1 lg:overflow-y-auto lg:pr-2
            [scrollbar-width:thin] [scrollbar-color:rgb(51_65_85)_transparent]
            [&::-webkit-scrollbar]:w-1.5
            [&::-webkit-scrollbar-track]:bg-transparent
            [&::-webkit-scrollbar-thumb]:rounded-full
            [&::-webkit-scrollbar-thumb]:bg-slate-700
            hover:[&::-webkit-scrollbar-thumb]:bg-slate-600"
        >
          {isLoading && <p className="text-xs text-slate-500">Loading…</p>}
          {error && (
            <p className="text-xs text-rose-400">
              {(error as Error).message}
            </p>
          )}
          {data && data.items.length === 0 && (
            <div className="rounded-lg border border-dashed border-slate-800 p-6 text-center">
              <p className="text-sm text-slate-300">No audiobooks yet.</p>
              <Link
                to="/app/new"
                className="mt-2 inline-block text-xs text-sky-400 hover:text-sky-300"
              >
                Create your first one →
              </Link>
            </div>
          )}
          {data && data.items.length > 0 && filteredSorted.length === 0 && (
            <p className="rounded-lg border border-dashed border-slate-800 p-4 text-center text-xs text-slate-500">
              No matches for the current filter.
            </p>
          )}

          <ul className="space-y-4">
            {grouped.map((group) => {
              // Use `null` for the Uncategorized bucket so the drop
              // handler can distinguish "(uncategorized)" from
              // "(no group hovered)" via the tri-state.
              const groupKey = group.category ?? null;
              const isOver =
                groupByCategory &&
                draggingId !== null &&
                dragOverCategory === groupKey;
              return (
                <li
                  key={group.category ?? UNCATEGORIZED}
                  // Drop targets only meaningful when grouping is on. The
                  // entire group column accepts drops, not just the
                  // header — easier to land a drag in.
                  onDragOver={
                    groupByCategory && draggingId
                      ? (e) => {
                          e.preventDefault();
                          e.dataTransfer.dropEffect = "move";
                          if (dragOverCategory !== groupKey) {
                            setDragOverCategory(groupKey);
                          }
                        }
                      : undefined
                  }
                  onDrop={
                    groupByCategory && draggingId
                      ? (e) => {
                          e.preventDefault();
                          const id = draggingId;
                          setDraggingId(null);
                          setDragOverCategory(undefined);
                          if (!id) return;
                          const dragged = data?.items.find((b) => b.id === id);
                          if (!dragged) return;
                          const current = dragged.category ?? null;
                          if (current === groupKey) return;
                          setCategory.mutate({
                            id,
                            category: groupKey ?? "",
                          });
                        }
                      : undefined
                  }
                  className={
                    isOver
                      ? "rounded-md bg-sky-900/15 outline outline-1 outline-sky-700"
                      : undefined
                  }
                >
                  {groupByCategory && (
                    <p className="mb-1 px-1 text-[10px] font-semibold uppercase tracking-wide text-slate-500">
                      {group.category ?? UNCATEGORIZED}
                      <span className="ml-1.5 text-slate-600">
                        {group.items.length}
                      </span>
                      {isOver && (
                        <span className="ml-2 text-sky-300">
                          drop to move here
                        </span>
                      )}
                    </p>
                  )}
                  <ul className="space-y-1">
                    {group.items.map((b) => (
                      <Row
                        key={b.id}
                        book={b}
                        selected={b.id === selectedId}
                        onRename={() => setRenaming(b)}
                        draggable={groupByCategory}
                        dragging={draggingId === b.id}
                        onDragStart={() => setDraggingId(b.id)}
                        onDragEnd={() => {
                          setDraggingId(null);
                          setDragOverCategory(undefined);
                        }}
                      />
                    ))}
                  </ul>
                </li>
              );
            })}
          </ul>
        </div>
      </aside>

      <div>
        {selectedId ? (
          // `key` forces a remount on id change so internal mutation state,
          // dialogs, etc. don't leak between books.
          <BookDetail key={selectedId} />
        ) : (
          <EmptyDetailPane />
        )}
      </div>

      {renaming && (
        <RenameAudiobookDialog book={renaming} onClose={() => setRenaming(null)} />
      )}
    </section>
  );
}

function sortBooks(
  a: AudiobookSummary,
  b: AudiobookSummary,
  key: SortKey,
): number {
  switch (key) {
    case "title":
      return a.title.localeCompare(b.title);
    case "language":
      return (a.language || "").localeCompare(b.language || "")
        || a.title.localeCompare(b.title);
    case "status":
      return STATUS_ORDER.indexOf(a.status) - STATUS_ORDER.indexOf(b.status)
        || a.title.localeCompare(b.title);
    case "duration": {
      // Longest first; un-narrated rows sink to the bottom.
      const da = a.duration_ms ?? -1;
      const db = b.duration_ms ?? -1;
      return db - da || a.title.localeCompare(b.title);
    }
    case "updated":
    default:
      // Newest first. ISO-8601 timestamps sort lexicographically.
      return (b.updated_at ?? "").localeCompare(a.updated_at ?? "");
  }
}

function matchesDuration(
  ms: number | null | undefined,
  filter: DurationFilter,
): boolean {
  if (filter === "all") return true;
  if (filter === "unknown") return ms == null || ms <= 0;
  if (ms == null || ms <= 0) return false;
  const bucket = DURATION_BUCKETS.find((b) => b.key === filter);
  if (!bucket) return true;
  return ms >= bucket.min && ms < bucket.max;
}

function formatDuration(ms: number): string {
  const totalSec = Math.round(ms / 1000);
  const h = Math.floor(totalSec / 3600);
  const m = Math.floor((totalSec % 3600) / 60);
  if (h > 0) return `${h}h ${m.toString().padStart(2, "0")}m`;
  if (m > 0) return `${m}m`;
  return `${totalSec}s`;
}

function FilterRow({
  statusFilter,
  onStatusFilter,
  counts,
  totalCount,
}: {
  statusFilter: StatusFilter;
  onStatusFilter: (s: StatusFilter) => void;
  counts: Partial<Record<AudiobookStatus, number>>;
  totalCount: number;
}): JSX.Element {
  return (
    <div className="mb-3 flex flex-wrap gap-1">
      <FilterChip
        label="All"
        count={totalCount}
        active={statusFilter === "all"}
        onClick={() => onStatusFilter("all")}
      />
      {STATUS_ORDER.map((s) => {
        const c = counts[s] ?? 0;
        if (c === 0) return null;
        return (
          <FilterChip
            key={s}
            label={s.replace(/_/g, " ")}
            count={c}
            active={statusFilter === s}
            onClick={() => onStatusFilter(s)}
          />
        );
      })}
    </div>
  );
}

function FilterChip({
  label,
  count,
  active,
  onClick,
}: {
  label: string;
  count: number;
  active: boolean;
  onClick: () => void;
}): JSX.Element {
  return (
    <button
      type="button"
      onClick={onClick}
      className={`rounded-full border px-2 py-0.5 text-[11px] capitalize ${
        active
          ? "border-sky-600 bg-sky-900/30 text-sky-200"
          : "border-slate-800 bg-slate-900/40 text-slate-400 hover:border-slate-700 hover:text-slate-200"
      }`}
    >
      {label}
      <span className="ml-1 text-slate-500">{count}</span>
    </button>
  );
}

function EmptyDetailPane(): JSX.Element {
  return (
    <div className="flex h-full min-h-[60vh] items-center justify-center rounded-xl border border-dashed border-slate-800 p-10 text-center">
      <div>
        <p className="text-2xl">📚</p>
        <p className="mt-2 text-sm text-slate-300">
          Pick an audiobook from the list,
          <br />
          or create a new one.
        </p>
        <Link
          to="/app/new"
          className="mt-4 inline-block text-sm text-sky-400 hover:text-sky-300"
        >
          New audiobook →
        </Link>
      </div>
    </div>
  );
}

function Row({
  book,
  selected,
  onRename,
  draggable,
  dragging,
  onDragStart,
  onDragEnd,
}: {
  book: AudiobookSummary;
  selected: boolean;
  onRename: () => void;
  draggable: boolean;
  dragging: boolean;
  onDragStart: () => void;
  onDragEnd: () => void;
}): JSX.Element {
  const navigate = useNavigate();
  const qc = useQueryClient();
  const accessToken = useAuth((s) => s.accessToken) ?? "";
  const [menuOpen, setMenuOpen] = useState(false);
  const menuRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    if (!menuOpen) return;
    const onDoc = (e: MouseEvent): void => {
      if (menuRef.current && !menuRef.current.contains(e.target as Node)) {
        setMenuOpen(false);
      }
    };
    document.addEventListener("mousedown", onDoc);
    return () => document.removeEventListener("mousedown", onDoc);
  }, [menuOpen]);

  const remove = useMutation({
    mutationFn: () => audiobooks.remove(book.id),
    onSuccess: () => {
      qc.invalidateQueries({ queryKey: ["audiobooks"] });
      // If the removed book was the selected one, fall back to the bare
      // library URL so the right pane shows the empty state.
      if (selected) navigate("/app", { replace: true });
    },
  });

  const goTo = (): void => {
    if (remove.isPending) return;
    navigate(`/app/book/${book.id}`);
  };

  const onDelete = (): void => {
    setMenuOpen(false);
    if (window.confirm(`Delete "${book.title}"? This removes its chapters and audio.`)) {
      remove.mutate();
    }
  };

  const onRenameClick = (): void => {
    setMenuOpen(false);
    onRename();
  };

  return (
    <li>
      <div
        role="button"
        tabIndex={0}
        draggable={draggable}
        onDragStart={(e) => {
          // Firefox needs *some* dataTransfer payload or the drag won't
          // initiate. The id is also useful as a fallback when reading
          // back the source from the drop event.
          e.dataTransfer.setData("text/plain", book.id);
          e.dataTransfer.effectAllowed = "move";
          onDragStart();
        }}
        onDragEnd={onDragEnd}
        onClick={goTo}
        onKeyDown={(e) => {
          if (e.key === "Enter" || e.key === " ") {
            e.preventDefault();
            goTo();
          }
        }}
        className={`relative flex items-center gap-2 rounded-md border px-2 py-1.5 ${
          selected
            ? "border-sky-700 bg-sky-900/20"
            : "border-slate-800 bg-slate-900/40 hover:border-slate-700 hover:bg-slate-900"
        } ${remove.isPending ? "opacity-50" : ""} ${
          dragging ? "opacity-40" : ""
        } ${draggable ? "cursor-grab active:cursor-grabbing" : "cursor-pointer"}`}
      >
        {book.has_cover ? (
          <img
            src={coverImageUrl(book.id, accessToken)}
            alt=""
            className="h-9 w-9 shrink-0 rounded object-cover"
            loading="lazy"
          />
        ) : (
          <div className="flex h-9 w-9 shrink-0 items-center justify-center rounded border border-slate-800 bg-slate-950 text-sm text-slate-600">
            📖
          </div>
        )}
        <div className="min-w-0 flex-1">
          <p
            className={`truncate text-sm ${
              selected ? "font-semibold text-slate-100" : "text-slate-200"
            }`}
          >
            {book.title}
          </p>
          <div className="flex items-center gap-1.5 text-[10px] text-slate-500">
            <StatusPill status={book.status} compact />
            <span>·</span>
            <span className="uppercase">{book.language}</span>
            {book.duration_ms != null && book.duration_ms > 0 && (
              <>
                <span>·</span>
                <span className="tabular-nums">{formatDuration(book.duration_ms)}</span>
              </>
            )}
          </div>
        </div>
        <div ref={menuRef} className="relative">
          <button
            type="button"
            aria-label="Audiobook actions"
            onClick={(e) => {
              e.stopPropagation();
              setMenuOpen((v) => !v);
            }}
            className="rounded-md p-1 text-slate-500 hover:bg-slate-800 hover:text-slate-200"
          >
            <svg width="14" height="14" viewBox="0 0 16 16" fill="currentColor">
              <circle cx="8" cy="3" r="1.4" />
              <circle cx="8" cy="8" r="1.4" />
              <circle cx="8" cy="13" r="1.4" />
            </svg>
          </button>
          {menuOpen && (
            <div
              onClick={(e) => e.stopPropagation()}
              className="absolute right-0 top-full z-20 mt-1 w-36 overflow-hidden rounded-md border border-slate-800 bg-slate-950 text-sm shadow-lg"
            >
              <button
                type="button"
                onClick={onRenameClick}
                className="block w-full px-3 py-2 text-left text-slate-200 hover:bg-slate-800"
              >
                Rename…
              </button>
              <button
                type="button"
                onClick={onDelete}
                className="block w-full px-3 py-2 text-left text-rose-400 hover:bg-rose-950/50"
              >
                Delete
              </button>
            </div>
          )}
        </div>
      </div>
      {remove.error && (
        <p className="mt-1 px-2 text-[10px] text-rose-400">
          {remove.error instanceof ApiError
            ? remove.error.message
            : "Could not delete"}
        </p>
      )}
    </li>
  );
}

export function StatusPill({
  status,
  compact = false,
}: {
  status: AudiobookStatus;
  compact?: boolean;
}): JSX.Element {
  const cls = STATUS_STYLES[status] ?? "bg-slate-800 text-slate-300";
  const padding = compact ? "px-1.5 py-0" : "px-2 py-0.5";
  const size = compact ? "text-[10px]" : "text-[11px]";
  return (
    <span className={`rounded-full ${padding} ${size} font-medium ${cls}`}>
      {status.replace(/_/g, " ")}
    </span>
  );
}

const STATUS_STYLES: Record<AudiobookStatus, string> = {
  draft: "bg-slate-800 text-slate-300",
  outline_pending: "bg-indigo-950 text-indigo-300",
  outline_ready: "bg-indigo-900 text-indigo-200",
  chapters_running: "bg-amber-950 text-amber-300",
  text_ready: "bg-sky-900 text-sky-200",
  audio_ready: "bg-emerald-900 text-emerald-200",
  failed: "bg-rose-900 text-rose-200",
};
