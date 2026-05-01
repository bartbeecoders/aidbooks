import { useEffect, useState } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useSearchParams } from "react-router-dom";
import {
  ApiError,
  integrations,
  podcasts as podcastsApi,
  podcastImageUrl,
} from "../api";
import type { PodcastRow } from "../api";
import { useAuth } from "../store/auth";

export function Settings(): JSX.Element {
  const [params, setParams] = useSearchParams();
  const justConnected = params.get("connected") === "youtube";
  const oauthError = params.get("error");

  // Clear the `?connected=youtube` query after one render so a refresh
  // doesn't keep flashing the toast.
  useEffect(() => {
    if (!justConnected && !oauthError) return;
    const timer = window.setTimeout(() => {
      const next = new URLSearchParams(params);
      next.delete("connected");
      next.delete("error");
      setParams(next, { replace: true });
    }, 6000);
    return () => window.clearTimeout(timer);
  }, [justConnected, oauthError, params, setParams]);

  return (
    <section>
      <header className="mb-6">
        <h1 className="text-2xl font-semibold tracking-tight">Settings</h1>
        <p className="mt-1 text-sm text-slate-400">
          Connect external services AidBooks can publish or sync to.
        </p>
      </header>

      {justConnected && !oauthError && (
        <div className="mb-4 rounded-md border border-emerald-900/60 bg-emerald-950/40 p-3 text-sm text-emerald-200">
          YouTube connected.
        </div>
      )}
      {oauthError && (
        <div className="mb-4 rounded-md border border-rose-900/60 bg-rose-950/40 p-3 text-sm text-rose-200">
          Could not connect YouTube: {oauthError}
        </div>
      )}

      <div className="space-y-6">
        <YoutubeCard />
        <PodcastsCard />
      </div>
    </section>
  );
}

// =========================================================================
// YouTube
// =========================================================================

function YoutubeCard(): JSX.Element {
  const qc = useQueryClient();
  const status = useQuery({
    queryKey: ["integrations", "youtube", "account"],
    queryFn: () => integrations.youtube.account(),
  });

  const [busy, setBusy] = useState(false);
  const [err, setErr] = useState<string | null>(null);

  const disconnect = useMutation({
    mutationFn: () => integrations.youtube.disconnect(),
    onSuccess: () =>
      qc.invalidateQueries({ queryKey: ["integrations", "youtube", "account"] }),
  });

  const onConnect = async (): Promise<void> => {
    setBusy(true);
    setErr(null);
    try {
      const res = await integrations.youtube.oauthStart();
      // Hard-navigate so Google can take over the tab.
      window.location.href = res.url;
    } catch (e) {
      setBusy(false);
      setErr(
        e instanceof ApiError
          ? e.code === "config_error"
            ? "YouTube publishing is not configured on this server."
            : e.message
          : "Could not start YouTube connect",
      );
    }
  };

  const connected = status.data?.connected ?? false;

  return (
    <article className="rounded-xl border border-slate-800 bg-slate-900/40 p-5">
      <div className="flex items-start gap-4">
        <div className="grid h-10 w-10 shrink-0 place-items-center rounded-md border border-slate-700 bg-slate-950 text-lg">
          ▶︎
        </div>
        <div className="min-w-0 flex-1">
          <h2 className="text-base font-semibold text-slate-100">YouTube</h2>
          <p className="mt-1 text-sm text-slate-400">
            Publish finished audiobooks to your YouTube channel as videos
            (cover artwork + chaptered audio).
          </p>
          {status.isLoading ? (
            <p className="mt-3 text-xs text-slate-500">Loading status…</p>
          ) : connected ? (
            <p className="mt-3 text-xs text-slate-300">
              Connected to{" "}
              <span className="font-medium text-slate-100">
                {status.data?.channel_title ?? "(unknown channel)"}
              </span>
            </p>
          ) : (
            <p className="mt-3 text-xs text-slate-400">Not connected.</p>
          )}
          {err && <p className="mt-2 text-xs text-rose-400">{err}</p>}
          {disconnect.error && (
            <p className="mt-2 text-xs text-rose-400">
              {disconnect.error instanceof ApiError
                ? disconnect.error.message
                : "Could not disconnect"}
            </p>
          )}
        </div>
        <div className="flex shrink-0 flex-col gap-2">
          {connected ? (
            <button
              type="button"
              onClick={() => disconnect.mutate()}
              disabled={disconnect.isPending}
              className="rounded-md border border-slate-700 bg-slate-950 px-3 py-2 text-sm text-slate-200 hover:border-slate-600 hover:bg-slate-900 disabled:cursor-not-allowed disabled:opacity-40"
            >
              {disconnect.isPending ? "Disconnecting…" : "Disconnect"}
            </button>
          ) : (
            <button
              type="button"
              onClick={onConnect}
              disabled={busy}
              className="rounded-md bg-rose-600 px-3 py-2 text-sm font-medium text-white hover:bg-rose-500 disabled:cursor-not-allowed disabled:bg-rose-700/50"
            >
              {busy ? "Opening Google…" : "Connect YouTube"}
            </button>
          )}
        </div>
      </div>
    </article>
  );
}

// =========================================================================
// Podcasts
// =========================================================================

function PodcastsCard(): JSX.Element {
  const qc = useQueryClient();
  const [editing, setEditing] = useState<PodcastRow | null>(null);
  const [creating, setCreating] = useState(false);

  const list = useQuery({
    queryKey: ["podcasts"],
    queryFn: () => podcastsApi.list(),
  });
  // The YouTube card hydrates this query — read it here so we know
  // whether to surface the "Sync to YouTube" affordance.
  const youtube = useQuery({
    queryKey: ["integrations", "youtube", "account"],
    queryFn: () => integrations.youtube.account(),
  });
  const youtubeConnected = youtube.data?.connected ?? false;

  const remove = useMutation({
    mutationFn: (id: string) => podcastsApi.remove(id),
    onSuccess: () => {
      qc.invalidateQueries({ queryKey: ["podcasts"] });
      qc.invalidateQueries({ queryKey: ["audiobooks"] });
    },
  });

  const items = list.data?.items ?? [];

  return (
    <article className="rounded-xl border border-slate-800 bg-slate-900/40 p-5">
      <header className="flex items-start gap-4">
        <div className="grid h-10 w-10 shrink-0 place-items-center rounded-md border border-slate-700 bg-slate-950 text-lg">
          🎙
        </div>
        <div className="min-w-0 flex-1">
          <h2 className="text-base font-semibold text-slate-100">Podcasts</h2>
          <p className="mt-1 text-sm text-slate-400">
            Group audiobooks under a podcast show. Each podcast has its own
            title, description, and AI-generated cover art. The show's
            artwork can be used as the YouTube playlist cover when
            publishing.
          </p>
        </div>
        <div className="shrink-0">
          <button
            type="button"
            onClick={() => setCreating(true)}
            className="rounded-md bg-sky-600 px-3 py-2 text-sm font-medium text-white hover:bg-sky-500"
          >
            New podcast
          </button>
        </div>
      </header>

      <div className="mt-5">
        {list.isLoading ? (
          <p className="text-xs text-slate-500">Loading…</p>
        ) : items.length === 0 ? (
          <p className="rounded-lg border border-dashed border-slate-800 p-6 text-center text-sm text-slate-500">
            No podcasts yet. Create your first one to start grouping
            audiobooks.
          </p>
        ) : (
          <ul className="space-y-3">
            {items.map((p) => (
              <PodcastListItem
                key={p.id}
                podcast={p}
                youtubeConnected={youtubeConnected}
                onEdit={() => setEditing(p)}
                onDelete={() => {
                  const msg = p.audiobook_count
                    ? `Delete "${p.title}"? ${p.audiobook_count} audiobook(s) will be unassigned.`
                    : `Delete "${p.title}"?`;
                  if (window.confirm(msg)) remove.mutate(p.id);
                }}
                deleting={
                  remove.isPending && remove.variables === p.id
                }
              />
            ))}
          </ul>
        )}
        {remove.error && (
          <p className="mt-3 text-xs text-rose-400">
            {remove.error instanceof ApiError
              ? remove.error.message
              : "Delete failed"}
          </p>
        )}
      </div>

      {creating && (
        <PodcastEditor
          mode="create"
          onClose={() => setCreating(false)}
          onSaved={() => {
            setCreating(false);
            qc.invalidateQueries({ queryKey: ["podcasts"] });
          }}
        />
      )}
      {editing && (
        <PodcastEditor
          mode="edit"
          existing={editing}
          onClose={() => setEditing(null)}
          onSaved={() => {
            setEditing(null);
            qc.invalidateQueries({ queryKey: ["podcasts"] });
            qc.invalidateQueries({ queryKey: ["audiobooks"] });
          }}
        />
      )}
    </article>
  );
}

function PodcastListItem({
  podcast,
  youtubeConnected,
  onEdit,
  onDelete,
  deleting,
}: {
  podcast: PodcastRow;
  youtubeConnected: boolean;
  onEdit: () => void;
  onDelete: () => void;
  deleting: boolean;
}): JSX.Element {
  const qc = useQueryClient();
  const accessToken = useAuth((s) => s.accessToken) ?? "";
  // Bust the cache when the row's `updated_at` changes (e.g. a new image
  // landed) so the browser re-fetches instead of showing the old bytes.
  const imgSrc = podcast.has_image
    ? `${podcastImageUrl(podcast.id, accessToken)}&v=${encodeURIComponent(podcast.updated_at)}`
    : null;

  const sync = useMutation({
    mutationFn: () => podcastsApi.syncYoutube(podcast.id),
    onSuccess: () => qc.invalidateQueries({ queryKey: ["podcasts"] }),
  });

  return (
    <li className="flex items-start gap-4 rounded-lg border border-slate-800 bg-slate-950/40 p-3">
      <div className="grid h-16 w-16 shrink-0 place-items-center overflow-hidden rounded-md border border-slate-800 bg-slate-900 text-2xl text-slate-500">
        {imgSrc ? (
          <img
            src={imgSrc}
            alt=""
            className="h-full w-full object-cover"
          />
        ) : (
          "🎙"
        )}
      </div>
      <div className="min-w-0 flex-1">
        <h3 className="truncate text-sm font-medium text-slate-100">
          {podcast.title}
        </h3>
        {podcast.description && (
          <p className="mt-0.5 line-clamp-2 text-xs text-slate-400">
            {podcast.description}
          </p>
        )}
        <div className="mt-1 flex flex-wrap items-center gap-x-3 gap-y-1 text-[11px] text-slate-500 tabular-nums">
          <span>
            {podcast.audiobook_count} audiobook
            {podcast.audiobook_count === 1 ? "" : "s"}
          </span>
          {podcast.youtube_playlist_url ? (
            <a
              href={podcast.youtube_playlist_url}
              target="_blank"
              rel="noreferrer"
              className="inline-flex items-center gap-1 rounded-full border border-rose-900/60 bg-rose-950/40 px-2 py-0.5 text-rose-200 hover:border-rose-800"
            >
              ▶︎ YouTube playlist
            </a>
          ) : youtubeConnected ? (
            <span className="text-amber-300">Not synced to YouTube</span>
          ) : (
            <span className="text-slate-500">YouTube not connected</span>
          )}
          {sync.error && (
            <span className="text-rose-400">
              {sync.error instanceof ApiError
                ? sync.error.message
                : "Sync failed"}
            </span>
          )}
        </div>
      </div>
      <div className="flex shrink-0 gap-2">
        {youtubeConnected && (
          <button
            type="button"
            onClick={() => sync.mutate()}
            disabled={sync.isPending}
            title={
              podcast.youtube_playlist_id
                ? "Re-push title, description, and the podcast designation to YouTube"
                : "Mint the YouTube podcast playlist for this show"
            }
            className="rounded-md border border-slate-700 bg-slate-900 px-3 py-1.5 text-xs text-slate-200 hover:border-slate-600 disabled:opacity-40"
          >
            {sync.isPending
              ? "Syncing…"
              : podcast.youtube_playlist_id
                ? "Re-sync to YouTube"
                : "Sync to YouTube"}
          </button>
        )}
        <button
          type="button"
          onClick={onEdit}
          className="rounded-md border border-slate-700 bg-slate-900 px-3 py-1.5 text-xs text-slate-200 hover:border-slate-600"
        >
          Edit
        </button>
        <button
          type="button"
          onClick={onDelete}
          disabled={deleting}
          className="rounded-md border border-rose-900 bg-rose-950/40 px-3 py-1.5 text-xs text-rose-300 hover:border-rose-800 disabled:opacity-40"
        >
          {deleting ? "Deleting…" : "Delete"}
        </button>
      </div>
    </li>
  );
}

function PodcastEditor({
  mode,
  existing,
  onClose,
  onSaved,
}: {
  mode: "create" | "edit";
  existing?: PodcastRow;
  onClose: () => void;
  onSaved: () => void;
}): JSX.Element {
  const accessToken = useAuth((s) => s.accessToken) ?? "";
  const [title, setTitle] = useState(existing?.title ?? "");
  const [description, setDescription] = useState(existing?.description ?? "");
  // Holds the *new* image preview as a data URL while the user is
  // generating one. Empty = no new preview, fall back to the existing
  // cover (edit mode) or the placeholder (create mode).
  const [previewDataUrl, setPreviewDataUrl] = useState<string | null>(null);
  const [previewBase64, setPreviewBase64] = useState<string | null>(null);
  const [previewMime, setPreviewMime] = useState<string | null>(null);

  const preview = useMutation({
    mutationFn: () =>
      podcastsApi.previewImage({
        title: title.trim(),
        description: description.trim() || null,
      }),
    onSuccess: (res) => {
      setPreviewBase64(res.image_base64);
      setPreviewMime(res.mime_type);
      setPreviewDataUrl(`data:${res.mime_type};base64,${res.image_base64}`);
    },
  });

  const save = useMutation({
    mutationFn: async () => {
      const payload = {
        title: title.trim(),
        description: description.trim(),
        image_base64: previewBase64,
      };
      if (mode === "create") {
        return podcastsApi.create(payload);
      }
      return podcastsApi.patch(existing!.id, payload);
    },
    onSuccess: () => onSaved(),
  });

  const existingImageSrc =
    existing?.has_image && accessToken
      ? `${podcastImageUrl(existing.id, accessToken)}&v=${encodeURIComponent(existing.updated_at)}`
      : null;
  const displayImageSrc = previewDataUrl ?? existingImageSrc;

  const titleOk = title.trim().length >= 1 && title.trim().length <= 200;
  const canSave = titleOk && !save.isPending;

  return (
    <div
      role="dialog"
      aria-modal="true"
      onMouseDown={(e) => {
        if (e.target === e.currentTarget) onClose();
      }}
      className="fixed inset-0 z-50 flex items-center justify-center bg-black/60 p-4"
    >
      <div className="w-full max-w-2xl overflow-hidden rounded-xl border border-slate-700 bg-slate-900 shadow-2xl">
        <header className="flex items-center justify-between border-b border-slate-800 px-5 py-3">
          <h3 className="text-base font-semibold text-slate-100">
            {mode === "create" ? "New podcast" : `Edit "${existing?.title}"`}
          </h3>
          <button
            type="button"
            onClick={onClose}
            className="rounded-md border border-slate-700 bg-slate-950 px-2 py-1 text-xs text-slate-300 hover:border-slate-600"
          >
            Close
          </button>
        </header>

        <div className="grid grid-cols-1 gap-5 p-5 md:grid-cols-[200px,1fr]">
          {/* Image column */}
          <div>
            <div className="aspect-square w-full overflow-hidden rounded-lg border border-slate-800 bg-slate-950">
              {displayImageSrc ? (
                <img
                  src={displayImageSrc}
                  alt=""
                  className="h-full w-full object-cover"
                />
              ) : (
                <div className="grid h-full w-full place-items-center text-4xl text-slate-700">
                  🎙
                </div>
              )}
            </div>
            <button
              type="button"
              onClick={() => preview.mutate()}
              disabled={!titleOk || preview.isPending}
              className="mt-3 w-full rounded-md bg-sky-600 px-3 py-2 text-sm font-medium text-white hover:bg-sky-500 disabled:cursor-not-allowed disabled:opacity-40"
            >
              {preview.isPending
                ? "Generating…"
                : displayImageSrc
                  ? "Regenerate image"
                  : "Generate image"}
            </button>
            {preview.error && (
              <p className="mt-2 text-xs text-rose-400">
                {preview.error instanceof ApiError
                  ? preview.error.message
                  : "Image generation failed"}
              </p>
            )}
            {previewMime && (
              <p className="mt-2 text-[11px] text-slate-500">
                Preview ({previewMime}). Save to keep it.
              </p>
            )}
          </div>

          {/* Form column */}
          <div className="space-y-4">
            <label className="block">
              <span className="text-xs font-medium uppercase tracking-wide text-slate-400">
                Title
              </span>
              <input
                type="text"
                value={title}
                onChange={(e) => setTitle(e.target.value)}
                maxLength={200}
                placeholder="My Podcast Show"
                className="mt-1 w-full rounded-md border border-slate-700 bg-slate-950 px-3 py-2 text-sm text-slate-100 outline-none focus:border-sky-600"
              />
            </label>
            <label className="block">
              <span className="text-xs font-medium uppercase tracking-wide text-slate-400">
                Description
              </span>
              <textarea
                value={description}
                onChange={(e) => setDescription(e.target.value)}
                maxLength={4000}
                rows={6}
                placeholder="Tell listeners what this show is about. This is also used as the YouTube playlist description."
                className="mt-1 w-full resize-y rounded-md border border-slate-700 bg-slate-950 px-3 py-2 text-sm text-slate-100 outline-none focus:border-sky-600"
              />
              <span className="mt-1 block text-[11px] text-slate-500 tabular-nums">
                {description.length}/4000
              </span>
            </label>
            {save.error && (
              <p className="text-xs text-rose-400">
                {save.error instanceof ApiError
                  ? save.error.message
                  : "Save failed"}
              </p>
            )}
          </div>
        </div>

        <footer className="flex items-center justify-end gap-2 border-t border-slate-800 px-5 py-3">
          <button
            type="button"
            onClick={onClose}
            className="rounded-md border border-slate-700 bg-slate-950 px-3 py-1.5 text-xs text-slate-300 hover:border-slate-600"
          >
            Cancel
          </button>
          <button
            type="button"
            onClick={() => save.mutate()}
            disabled={!canSave}
            className="rounded-md bg-sky-600 px-3 py-1.5 text-xs font-medium text-white hover:bg-sky-500 disabled:cursor-not-allowed disabled:opacity-40"
          >
            {save.isPending ? "Saving…" : mode === "create" ? "Create" : "Save"}
          </button>
        </footer>
      </div>
    </div>
  );
}
