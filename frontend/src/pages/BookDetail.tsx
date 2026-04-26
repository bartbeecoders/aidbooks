import { useEffect, useMemo, useState } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { Link, useNavigate, useParams } from "react-router-dom";
import {
  audiobooks,
  catalog,
  coverImageUrl,
  jobs as jobsApi,
  ApiError,
} from "../api";
import type { ChapterSummary, JobSnapshot, Voice } from "../api";
import { useAuth } from "../store/auth";
import { useProgressSocket } from "../hooks/useProgressSocket";
import { RenameAudiobookDialog } from "../components/RenameAudiobookDialog";
import { StatusPill } from "./Library";

const ALL_LANGUAGES: { code: string; label: string; flag: string }[] = [
  { code: "en", label: "English", flag: "🇬🇧" },
  { code: "nl", label: "Dutch", flag: "🇳🇱" },
  { code: "fr", label: "French", flag: "🇫🇷" },
  { code: "de", label: "German", flag: "🇩🇪" },
  { code: "es", label: "Spanish", flag: "🇪🇸" },
  { code: "it", label: "Italian", flag: "🇮🇹" },
  { code: "pt", label: "Portuguese", flag: "🇵🇹" },
  { code: "ru", label: "Russian", flag: "🇷🇺" },
  { code: "zh", label: "Chinese", flag: "🇨🇳" },
  { code: "ja", label: "Japanese", flag: "🇯🇵" },
  { code: "ko", label: "Korean", flag: "🇰🇷" },
];

function langInfo(code: string): { label: string; flag: string } {
  const m = ALL_LANGUAGES.find((l) => l.code === code);
  return m ? { label: m.label, flag: m.flag } : { label: code, flag: "🏳️" };
}

export function BookDetail(): JSX.Element {
  const { id } = useParams<{ id: string }>();
  const navigate = useNavigate();
  const qc = useQueryClient();
  const accessToken = useAuth((s) => s.accessToken) ?? "";

  const [renameOpen, setRenameOpen] = useState(false);
  const [voiceOpen, setVoiceOpen] = useState(false);
  const [translateOpen, setTranslateOpen] = useState(false);
  const [activeLang, setActiveLang] = useState<string | null>(null);

  const { data, isLoading, error } = useQuery({
    queryKey: ["audiobook", id, activeLang ?? "primary"],
    queryFn: () => audiobooks.get(id!, activeLang ?? undefined),
    enabled: !!id,
  });
  const voicesQuery = useQuery({
    queryKey: ["voices"],
    queryFn: () => catalog.voices(),
  });
  const progress = useProgressSocket(id);

  const remove = useMutation({
    mutationFn: () => audiobooks.remove(id!),
    onSuccess: () => {
      qc.invalidateQueries({ queryKey: ["audiobooks"] });
      qc.removeQueries({ queryKey: ["audiobook", id] });
      navigate("/app", { replace: true });
    },
  });

  const regenCover = useMutation({
    mutationFn: () => audiobooks.regenerateCover(id!),
    onSuccess: () => qc.invalidateQueries({ queryKey: ["audiobook", id] }),
  });

  // Refetch on terminal events.
  useEffect(() => {
    if (progress.terminalTick > 0) {
      qc.invalidateQueries({ queryKey: ["audiobook", id] });
    }
  }, [progress.terminalTick, qc, id]);

  // While a translate job is running there's no per-chapter event, but the
  // backend creates rows one at a time. Poll the detail every 3 s so the
  // chapter list grows live instead of jumping at the end.
  const translateRunning = progress.jobs.some(
    (j) => j.kind === "translate" && (j.status === "queued" || j.status === "running"),
  );
  useEffect(() => {
    if (!translateRunning || !id) return;
    const t = window.setInterval(() => {
      qc.invalidateQueries({ queryKey: ["audiobook", id] });
    }, 3000);
    return () => window.clearInterval(t);
  }, [translateRunning, qc, id]);

  const burstSeed = (): void => {
    if (!id) return;
    const tick = async (): Promise<void> => {
      try {
        const list = await jobsApi.listForAudiobook(id);
        progress.seedJobs(list.jobs);
      } catch {
        /* WS will catch up */
      }
    };
    void tick();
    [400, 1200, 2500, 5000].forEach((ms) =>
      window.setTimeout(() => void tick(), ms),
    );
  };

  const generateChapters = useMutation({
    mutationFn: () => audiobooks.generateChapters(id!),
    onSuccess: () => {
      qc.invalidateQueries({ queryKey: ["audiobook", id] });
      burstSeed();
    },
  });
  const generateAudio = useMutation({
    mutationFn: () => audiobooks.generateAudio(id!, activeLang ?? undefined),
    onSuccess: () => {
      qc.invalidateQueries({ queryKey: ["audiobook", id] });
      burstSeed();
    },
  });

  const parentJobs = useMemo(
    () =>
      progress.jobs.filter(
        (j) => j.kind === "chapters" || j.kind === "tts" || j.kind === "translate",
      ),
    [progress.jobs],
  );
  const perChapterJobs = useMemo(() => {
    const map = new Map<number, JobSnapshot>();
    for (const j of progress.jobs) {
      if (j.kind === "tts_chapter" && j.chapter_number != null) {
        map.set(j.chapter_number, j);
      }
    }
    return map;
  }, [progress.jobs]);

  if (!id) return <p>Missing id.</p>;
  if (isLoading) return <p className="text-sm text-slate-400">Loading…</p>;
  if (error) return <p className="text-sm text-rose-400">{(error as Error).message}</p>;
  if (!data) return <p className="text-sm text-slate-400">Not found.</p>;

  const isPrimaryView =
    !activeLang || activeLang === data.language;
  const canWriteChapters =
    isPrimaryView &&
    (data.status === "outline_ready" ||
      data.status === "failed" ||
      data.status === "text_ready");
  // Audio can be generated for any language that has chapters with text.
  const haveText = data.chapters.length > 0;
  const allAudioReady =
    haveText && data.chapters.every((c) => c.status === "audio_ready");
  const canNarrate = haveText && !generateAudio.isPending;
  const ready = isPrimaryView && data.status === "audio_ready";

  const voices = voicesQuery.data?.items ?? [];
  const voiceLabel = voiceFor(voices, data.voice_id ?? null);

  const pendingKind: "chapters" | "tts" | null =
    parentJobs.length > 0
      ? null
      : data.status === "chapters_running" || generateChapters.isPending
        ? "chapters"
        : generateAudio.isPending ||
            data.chapters.some((c) => c.status === "running")
          ? "tts"
          : null;

  const viewLang = activeLang ?? data.language;

  return (
    <section>
      <div className="mb-6 flex flex-col gap-4 sm:flex-row sm:items-start">
        <CoverBlock
          audiobookId={data.id}
          hasCover={data.has_cover}
          accessToken={accessToken}
          onRegenerate={() => regenCover.mutate()}
          regenerating={regenCover.isPending}
          regenError={
            regenCover.error
              ? regenCover.error instanceof ApiError
                ? regenCover.error.message
                : "Cover regen failed"
              : null
          }
        />
        <div className="min-w-0 flex-1">
          <h1 className="truncate text-2xl font-semibold tracking-tight">
            {data.title}
          </h1>
          <p className="mt-1 max-w-xl text-sm text-slate-400">{data.topic}</p>
          <div className="mt-3 flex flex-wrap items-center gap-2 text-xs text-slate-400">
            <StatusPill status={data.status} />
            <span>{data.length}</span>
            {data.genre && <span>· {data.genre}</span>}
            <span>·</span>
            <Pill icon={langInfo(viewLang).flag} text={langInfo(viewLang).label} />
            <button
              type="button"
              onClick={() => setVoiceOpen(true)}
              title="Change voice"
              className="rounded-full border border-slate-800 bg-slate-900/40 px-2 py-0.5 text-[11px] hover:border-slate-700"
            >
              🎙 {voiceLabel}
            </button>
            <ConnectionBadge readyState={progress.readyState} />
          </div>
        </div>
        <div className="flex flex-wrap items-center justify-end gap-2">
          {ready && (
            <Link
              to={`/app/play/${data.id}`}
              className="whitespace-nowrap rounded-md bg-emerald-600 px-3 py-2 text-sm font-medium text-white hover:bg-emerald-500"
            >
              Open player
            </Link>
          )}
          <button
            type="button"
            onClick={() => setRenameOpen(true)}
            className="rounded-md border border-slate-700 bg-slate-900 px-3 py-2 text-sm text-slate-200 hover:border-slate-600 hover:bg-slate-800"
          >
            Rename
          </button>
          <button
            type="button"
            onClick={() => {
              if (
                window.confirm(
                  `Delete "${data.title}"? This removes its chapters and audio.`,
                )
              ) {
                remove.mutate();
              }
            }}
            disabled={remove.isPending}
            className="rounded-md border border-rose-900 bg-rose-950/40 px-3 py-2 text-sm text-rose-300 hover:border-rose-800 hover:bg-rose-950 disabled:cursor-not-allowed disabled:opacity-40"
          >
            {remove.isPending ? "Deleting…" : "Delete"}
          </button>
        </div>
      </div>

      {remove.error && (
        <p className="mb-4 text-sm text-rose-400">
          {remove.error instanceof ApiError
            ? remove.error.message
            : "Could not delete audiobook"}
        </p>
      )}

      <LanguageTabs
        languages={data.available_languages}
        primary={data.language}
        active={viewLang}
        onSelect={(lang) =>
          setActiveLang(lang === data.language ? null : lang)
        }
        onAdd={() => setTranslateOpen(true)}
      />

      {renameOpen && (
        <RenameAudiobookDialog book={data} onClose={() => setRenameOpen(false)} />
      )}
      {voiceOpen && (
        <ChangeVoiceDialog
          audiobookId={data.id}
          currentVoiceId={data.voice_id ?? null}
          voices={voices}
          onClose={() => setVoiceOpen(false)}
          onSaved={() => {
            qc.invalidateQueries({ queryKey: ["audiobook", id] });
            setVoiceOpen(false);
          }}
        />
      )}
      {translateOpen && (
        <TranslateDialog
          audiobookId={data.id}
          primary={data.language}
          existing={data.available_languages}
          onClose={() => setTranslateOpen(false)}
          onQueued={(target) => {
            // Switch the view to the target so chapters appear as the
            // background job lands them; the parent job row at the top
            // shows progress.
            setActiveLang(target);
            setTranslateOpen(false);
            burstSeed();
          }}
        />
      )}

      <div className="mb-6 flex flex-wrap gap-2">
        <button
          onClick={() => generateChapters.mutate()}
          disabled={!canWriteChapters || generateChapters.isPending}
          title={
            isPrimaryView
              ? undefined
              : "Re-write chapters only applies to the primary language"
          }
          className="rounded-md bg-sky-600 px-3 py-2 text-sm font-medium text-white hover:bg-sky-500 disabled:cursor-not-allowed disabled:bg-sky-700/50"
        >
          {generateChapters.isPending
            ? "Queuing…"
            : data.status === "outline_ready"
              ? "Write chapters"
              : "Re-write chapters"}
        </button>
        <button
          onClick={() => generateAudio.mutate()}
          disabled={!canNarrate}
          className="rounded-md border border-slate-700 bg-slate-900 px-3 py-2 text-sm text-slate-200 hover:border-slate-600 hover:bg-slate-800 disabled:cursor-not-allowed disabled:opacity-40"
        >
          {generateAudio.isPending
            ? "Queuing…"
            : allAudioReady
              ? `Re-narrate (${langInfo(viewLang).label})`
              : `Narrate (${langInfo(viewLang).label})`}
        </button>
        {generateChapters.error && (
          <p className="w-full text-sm text-rose-400">
            {generateChapters.error instanceof ApiError
              ? generateChapters.error.message
              : "Could not queue chapter generation"}
          </p>
        )}
        {generateAudio.error && (
          <p className="w-full text-sm text-rose-400">
            {generateAudio.error instanceof ApiError
              ? generateAudio.error.message
              : "Could not queue audio generation"}
          </p>
        )}
      </div>

      {(parentJobs.length > 0 || pendingKind !== null) && (
        <div className="mb-6 space-y-2">
          {parentJobs.length > 0
            ? parentJobs.map((j) => <ParentJobRow key={j.id} job={j} />)
            : pendingKind && <PendingJobRow kind={pendingKind} />}
        </div>
      )}

      {data.chapters.length === 0 ? (
        <p className="rounded-lg border border-dashed border-slate-800 p-6 text-center text-sm text-slate-500">
          No chapters in {langInfo(viewLang).label} yet.
          {viewLang !== data.language && (
            <span className="ml-1">Translate the book to populate this language.</span>
          )}
        </p>
      ) : (
        <ol className="space-y-2">
          {data.chapters.map((ch) => (
            <ChapterRow key={ch.id} ch={ch} job={perChapterJobs.get(ch.number)} />
          ))}
        </ol>
      )}
    </section>
  );
}

function voiceFor(voices: Voice[], voiceId: string | null): string {
  if (!voiceId) return "Default";
  const v = voices.find((x) => x.id === voiceId);
  return v?.name ?? voiceId;
}

function Pill({ icon, text }: { icon: string; text: string }): JSX.Element {
  return (
    <span className="inline-flex items-center gap-1 rounded-full border border-slate-800 bg-slate-900/40 px-2 py-0.5 text-[11px]">
      <span>{icon}</span>
      <span>{text}</span>
    </span>
  );
}

function CoverBlock({
  audiobookId,
  hasCover,
  accessToken,
  onRegenerate,
  regenerating,
  regenError,
}: {
  audiobookId: string;
  hasCover: boolean;
  accessToken: string;
  onRegenerate: () => void;
  regenerating: boolean;
  regenError: string | null;
}): JSX.Element {
  return (
    <div className="flex shrink-0 flex-col items-stretch gap-2">
      <div className="h-32 w-32 overflow-hidden rounded-lg border border-slate-800 bg-slate-950">
        {hasCover ? (
          <img
            src={`${coverImageUrl(audiobookId, accessToken)}&t=${regenerating ? "loading" : "ready"}`}
            alt="Cover"
            className="h-full w-full object-cover"
          />
        ) : (
          <div className="flex h-full w-full items-center justify-center text-3xl text-slate-700">
            📖
          </div>
        )}
      </div>
      <button
        type="button"
        onClick={onRegenerate}
        disabled={regenerating}
        className="rounded-md border border-slate-700 bg-slate-900 px-2 py-1 text-xs text-slate-300 hover:border-slate-600 hover:text-slate-100 disabled:cursor-not-allowed disabled:opacity-40"
      >
        {regenerating ? "Generating…" : hasCover ? "Regenerate cover" : "Generate cover"}
      </button>
      {regenError && <p className="text-[11px] text-rose-400">{regenError}</p>}
    </div>
  );
}

function LanguageTabs({
  languages,
  primary,
  active,
  onSelect,
  onAdd,
}: {
  languages: string[];
  primary: string;
  active: string;
  onSelect: (code: string) => void;
  onAdd: () => void;
}): JSX.Element {
  // Always show the primary first; dedupe in case the server didn't.
  const ordered = [primary, ...languages.filter((l) => l !== primary)];
  return (
    <div className="mb-6 flex flex-wrap items-center gap-2 border-b border-slate-800 pb-2">
      {ordered.map((code) => {
        const isActive = code === active;
        const info = langInfo(code);
        return (
          <button
            key={code}
            type="button"
            onClick={() => onSelect(code)}
            className={`rounded-md px-3 py-1.5 text-xs ${
              isActive
                ? "bg-sky-600/15 text-sky-200"
                : "text-slate-400 hover:bg-slate-900 hover:text-slate-200"
            }`}
          >
            <span className="mr-1">{info.flag}</span>
            {info.label}
            {code === primary && (
              <span className="ml-1.5 text-[10px] uppercase tracking-wide text-slate-500">
                primary
              </span>
            )}
          </button>
        );
      })}
      <button
        type="button"
        onClick={onAdd}
        className="ml-auto rounded-md border border-dashed border-slate-700 px-3 py-1.5 text-xs text-slate-400 hover:border-slate-500 hover:text-slate-200"
      >
        + Add language
      </button>
    </div>
  );
}

function ChangeVoiceDialog({
  audiobookId,
  currentVoiceId,
  voices,
  onClose,
  onSaved,
}: {
  audiobookId: string;
  currentVoiceId: string | null;
  voices: Voice[];
  onClose: () => void;
  onSaved: () => void;
}): JSX.Element {
  const [picked, setPicked] = useState<string | null>(currentVoiceId);

  useEffect(() => {
    const onKey = (e: KeyboardEvent): void => {
      if (e.key === "Escape") onClose();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [onClose]);

  const save = useMutation({
    mutationFn: () =>
      audiobooks.patch(audiobookId, {
        voice_id: picked ?? "",
      }),
    onSuccess: onSaved,
  });

  return (
    <div
      className="fixed inset-0 z-50 flex items-center justify-center bg-black/60 p-4"
      onClick={onClose}
    >
      <form
        onClick={(e) => e.stopPropagation()}
        onSubmit={(e) => {
          e.preventDefault();
          if (!save.isPending) save.mutate();
        }}
        className="w-full max-w-lg rounded-xl border border-slate-800 bg-slate-950 p-5 shadow-xl"
      >
        <h2 className="text-base font-semibold text-slate-100">Change voice</h2>
        <p className="mt-1 text-xs text-slate-400">
          Re-narrate the audiobook after saving for the new voice to take effect.
        </p>
        <div className="mt-4 grid grid-cols-2 gap-2 sm:grid-cols-3">
          <SelectableVoice
            active={picked === null}
            onSelect={() => setPicked(null)}
            title="Default"
            subtitle="Server pick"
          />
          {voices.map((v) => (
            <SelectableVoice
              key={v.id}
              active={picked === v.id}
              onSelect={() => setPicked(v.id)}
              title={v.name}
              subtitle={v.accent}
            />
          ))}
        </div>
        {save.error && (
          <p className="mt-3 text-xs text-rose-400">
            {save.error instanceof ApiError
              ? save.error.message
              : "Could not save"}
          </p>
        )}
        <div className="mt-5 flex justify-end gap-2">
          <button
            type="button"
            onClick={onClose}
            className="rounded-md border border-slate-800 bg-slate-900 px-3 py-2 text-sm text-slate-200 hover:border-slate-700"
          >
            Cancel
          </button>
          <button
            type="submit"
            disabled={save.isPending || picked === currentVoiceId}
            className="rounded-md bg-sky-600 px-3 py-2 text-sm font-medium text-white hover:bg-sky-500 disabled:cursor-not-allowed disabled:bg-sky-700/50"
          >
            {save.isPending ? "Saving…" : "Save"}
          </button>
        </div>
      </form>
    </div>
  );
}

function SelectableVoice({
  active,
  onSelect,
  title,
  subtitle,
}: {
  active: boolean;
  onSelect: () => void;
  title: string;
  subtitle: string;
}): JSX.Element {
  return (
    <button
      type="button"
      onClick={onSelect}
      className={`flex flex-col items-start gap-0.5 rounded-md border px-3 py-2 text-left ${
        active
          ? "border-sky-600 bg-sky-600/10"
          : "border-slate-700 bg-slate-950 hover:border-slate-600"
      }`}
    >
      <span
        className={`text-sm font-medium ${
          active ? "text-sky-200" : "text-slate-100"
        }`}
      >
        {title}
      </span>
      <span className="text-[11px] capitalize text-slate-400">{subtitle}</span>
    </button>
  );
}

function TranslateDialog({
  audiobookId,
  primary,
  existing,
  onClose,
  onQueued,
}: {
  audiobookId: string;
  primary: string;
  existing: string[];
  onClose: () => void;
  onQueued: (target: string) => void;
}): JSX.Element {
  const available = ALL_LANGUAGES.filter((l) => !existing.includes(l.code));
  const [target, setTarget] = useState<string>(available[0]?.code ?? "");

  useEffect(() => {
    const onKey = (e: KeyboardEvent): void => {
      if (e.key === "Escape") onClose();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [onClose]);

  const translate = useMutation({
    mutationFn: () =>
      audiobooks.translate(audiobookId, {
        target_language: target,
        source_language: primary,
      }),
    onSuccess: () => onQueued(target),
  });

  return (
    <div
      className="fixed inset-0 z-50 flex items-center justify-center bg-black/60 p-4"
      onClick={onClose}
    >
      <form
        onClick={(e) => e.stopPropagation()}
        onSubmit={(e) => {
          e.preventDefault();
          if (!translate.isPending && target) translate.mutate();
        }}
        className="w-full max-w-md rounded-xl border border-slate-800 bg-slate-950 p-5 shadow-xl"
      >
        <h2 className="text-base font-semibold text-slate-100">Add language</h2>
        <p className="mt-1 text-xs text-slate-400">
          Translates every chapter from <strong>{langInfo(primary).label}</strong> to
          the chosen language. Audio is generated separately.
        </p>

        {available.length === 0 ? (
          <p className="mt-4 text-sm text-slate-500">
            All supported languages are already available.
          </p>
        ) : (
          <label className="mt-4 block text-xs font-medium text-slate-300">
            Target
            <select
              value={target}
              onChange={(e) => setTarget(e.target.value)}
              className="mt-1 w-full rounded-md border border-slate-800 bg-slate-900 px-3 py-2 text-sm text-slate-100 outline-none focus:border-sky-600"
            >
              {available.map((l) => (
                <option key={l.code} value={l.code}>
                  {l.flag} {l.label}
                </option>
              ))}
            </select>
          </label>
        )}

        {translate.error && (
          <p className="mt-3 text-xs text-rose-400">
            {translate.error instanceof ApiError
              ? translate.error.message
              : "Translation failed"}
          </p>
        )}

        <div className="mt-5 flex justify-end gap-2">
          <button
            type="button"
            onClick={onClose}
            className="rounded-md border border-slate-800 bg-slate-900 px-3 py-2 text-sm text-slate-200 hover:border-slate-700"
          >
            Cancel
          </button>
          <button
            type="submit"
            disabled={translate.isPending || !target || available.length === 0}
            className="rounded-md bg-sky-600 px-3 py-2 text-sm font-medium text-white hover:bg-sky-500 disabled:cursor-not-allowed disabled:bg-sky-700/50"
          >
            {translate.isPending ? "Queuing…" : "Translate"}
          </button>
        </div>
      </form>
    </div>
  );
}

function ConnectionBadge({
  readyState,
}: {
  readyState: "connecting" | "open" | "closed";
}): JSX.Element {
  const cls =
    readyState === "open"
      ? "bg-emerald-900 text-emerald-200"
      : readyState === "connecting"
        ? "bg-amber-950 text-amber-300"
        : "bg-rose-950 text-rose-300";
  return (
    <span className={`ml-2 rounded-full px-2 py-0.5 text-[11px] ${cls}`}>
      ws: {readyState}
    </span>
  );
}

function ParentJobRow({ job }: { job: JobSnapshot }): JSX.Element {
  const indeterminate = job.status === "queued" || job.status === "throttled";
  const label =
    job.status === "running"
      ? `${Math.round(job.progress_pct * 100)}%`
      : job.status === "queued"
        ? "queued — waiting for worker"
        : job.status === "throttled"
          ? "throttled — retrying soon"
          : job.status;
  return (
    <div className="rounded-lg border border-slate-800 bg-slate-900/40 p-3">
      <div className="flex items-center justify-between text-sm">
        <span className="font-medium capitalize text-slate-200">
          {job.kind.replace(/_/g, " ")}
        </span>
        <span className="text-xs text-slate-400">{label}</span>
      </div>
      <div className="mt-2 h-1.5 overflow-hidden rounded-full bg-slate-800">
        {indeterminate ? (
          <div className="indeterminate-bar h-full w-1/3 rounded-full bg-sky-500" />
        ) : (
          <div
            className={`h-full transition-all ${
              job.status === "dead"
                ? "bg-rose-500"
                : job.status === "completed"
                  ? "bg-emerald-500"
                  : "bg-sky-500"
            }`}
            style={{ width: `${Math.max(2, Math.round(job.progress_pct * 100))}%` }}
          />
        )}
      </div>
      {job.last_error && (
        <p className="mt-2 text-xs text-rose-400">{job.last_error}</p>
      )}
    </div>
  );
}

function PendingJobRow({ kind }: { kind: "chapters" | "tts" }): JSX.Element {
  const label = kind === "chapters" ? "chapters" : "tts";
  return (
    <div className="rounded-lg border border-slate-800 bg-slate-900/40 p-3">
      <div className="flex items-center justify-between text-sm">
        <span className="font-medium capitalize text-slate-200">{label}</span>
        <span className="text-xs text-slate-400">queued — waiting for worker</span>
      </div>
      <div className="mt-2 h-1.5 overflow-hidden rounded-full bg-slate-800">
        <div className="indeterminate-bar h-full w-1/3 rounded-full bg-sky-500" />
      </div>
    </div>
  );
}

function ChapterRow({
  ch,
  job,
}: {
  ch: ChapterSummary;
  job: JobSnapshot | undefined;
}): JSX.Element {
  const statusLabel = job
    ? job.status === "running"
      ? `narrating ${Math.round(job.progress_pct * 100)}%`
      : job.status
    : ch.status.replace(/_/g, " ");
  const tone =
    ch.status === "audio_ready"
      ? "border-emerald-900/60"
      : ch.status === "failed"
        ? "border-rose-900/60"
        : "border-slate-800";
  return (
    <li className={`rounded-lg border ${tone} bg-slate-900/40 p-4`}>
      <div className="flex items-start justify-between gap-3">
        <div className="min-w-0">
          <p className="text-xs uppercase tracking-wide text-slate-500">
            Chapter {ch.number}
          </p>
          <h3 className="truncate text-base font-medium text-slate-100">{ch.title}</h3>
          {ch.synopsis && (
            <p className="mt-1 line-clamp-2 text-sm text-slate-400">{ch.synopsis}</p>
          )}
        </div>
        <span className="whitespace-nowrap text-xs text-slate-400">{statusLabel}</span>
      </div>
      {ch.body_md && (
        <details className="mt-3">
          <summary className="cursor-pointer text-xs text-sky-400 hover:text-sky-300">
            Read prose ({Math.round(ch.body_md.length / 1024)} KB)
          </summary>
          <pre className="mt-2 max-h-80 overflow-auto whitespace-pre-wrap rounded-md border border-slate-800 bg-slate-950 p-3 text-xs leading-relaxed text-slate-200">
            {ch.body_md}
          </pre>
        </details>
      )}
    </li>
  );
}
