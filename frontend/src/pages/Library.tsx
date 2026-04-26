import { useEffect, useRef, useState } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { Link, useNavigate } from "react-router-dom";
import { audiobooks, ApiError, coverImageUrl } from "../api";
import type { AudiobookStatus, AudiobookSummary } from "../api";
import { useAuth } from "../store/auth";
import { RenameAudiobookDialog } from "../components/RenameAudiobookDialog";

export function Library(): JSX.Element {
  const { data, isLoading, error } = useQuery({
    queryKey: ["audiobooks"],
    queryFn: () => audiobooks.list(),
  });
  const [renaming, setRenaming] = useState<AudiobookSummary | null>(null);

  return (
    <section>
      <div className="mb-6 flex items-end justify-between">
        <div>
          <h1 className="text-2xl font-semibold tracking-tight">Library</h1>
          <p className="text-sm text-slate-400">
            Your generated audiobooks. Click one to continue, or create a new one.
          </p>
        </div>
        <Link
          to="/app/new"
          className="rounded-md bg-sky-600 px-3 py-2 text-sm font-medium text-white hover:bg-sky-500"
        >
          New audiobook
        </Link>
      </div>

      {isLoading && <p className="text-sm text-slate-400">Loading…</p>}
      {error && (
        <p className="text-sm text-rose-400">
          Couldn&apos;t load your library: {(error as Error).message}
        </p>
      )}

      {data && data.items.length === 0 && (
        <div className="rounded-xl border border-dashed border-slate-800 p-10 text-center">
          <p className="text-slate-300">No audiobooks yet.</p>
          <Link
            to="/app/new"
            className="mt-4 inline-block text-sm text-sky-400 hover:text-sky-300"
          >
            Create your first one →
          </Link>
        </div>
      )}

      {data && data.items.length > 0 && (
        <ul className="grid gap-3 sm:grid-cols-2 lg:grid-cols-3">
          {data.items.map((b) => (
            <Card key={b.id} book={b} onRename={() => setRenaming(b)} />
          ))}
        </ul>
      )}

      {renaming && (
        <RenameAudiobookDialog book={renaming} onClose={() => setRenaming(null)} />
      )}
    </section>
  );
}

function Card({
  book,
  onRename,
}: {
  book: AudiobookSummary;
  onRename: () => void;
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
    onSuccess: () => qc.invalidateQueries({ queryKey: ["audiobooks"] }),
  });

  const playable = book.status === "audio_ready";
  const target = playable ? `/app/play/${book.id}` : `/app/book/${book.id}`;

  const goTo = (): void => {
    if (remove.isPending) return;
    navigate(target);
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
        onClick={goTo}
        onKeyDown={(e) => {
          if (e.key === "Enter" || e.key === " ") {
            e.preventDefault();
            goTo();
          }
        }}
        className={`relative block cursor-pointer rounded-xl border border-slate-800 bg-slate-900/50 p-4 hover:border-slate-700 hover:bg-slate-900 ${
          remove.isPending ? "opacity-50" : ""
        }`}
      >
        <div className="flex items-start gap-3">
          {book.has_cover ? (
            <img
              src={coverImageUrl(book.id, accessToken)}
              alt=""
              className="h-12 w-12 shrink-0 rounded-md object-cover"
              loading="lazy"
            />
          ) : (
            <div className="flex h-12 w-12 shrink-0 items-center justify-center rounded-md border border-slate-800 bg-slate-950 text-base text-slate-600">
              📖
            </div>
          )}
          <h3 className="min-w-0 flex-1 truncate text-base font-medium text-slate-100">
            {book.title}
          </h3>
          <div ref={menuRef} className="relative">
            <button
              type="button"
              aria-label="Audiobook actions"
              onClick={(e) => {
                e.stopPropagation();
                setMenuOpen((v) => !v);
              }}
              className="rounded-md p-1 text-slate-400 hover:bg-slate-800 hover:text-slate-200"
            >
              <svg width="16" height="16" viewBox="0 0 16 16" fill="currentColor">
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
        <p className="mt-1 line-clamp-2 text-xs text-slate-400">{book.topic}</p>
        <div className="mt-3 flex items-center justify-between text-xs">
          <StatusPill status={book.status} />
          <span className="text-slate-500">{book.length}</span>
        </div>
        {remove.error && (
          <p className="mt-2 text-xs text-rose-400">
            {remove.error instanceof ApiError
              ? remove.error.message
              : "Could not delete"}
          </p>
        )}
      </div>
    </li>
  );
}

export function StatusPill({ status }: { status: AudiobookStatus }): JSX.Element {
  const cls = STATUS_STYLES[status] ?? "bg-slate-800 text-slate-300";
  return (
    <span className={`rounded-full px-2 py-0.5 text-[11px] font-medium ${cls}`}>
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
