import { useEffect, useState } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useSearchParams } from "react-router-dom";
import { ApiError, integrations } from "../api";

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

      <YoutubeCard />
    </section>
  );
}

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
