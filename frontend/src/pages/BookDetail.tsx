import { useEffect, useMemo, useState } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { Link, useNavigate, useParams } from "react-router-dom";
import {
  audiobooks,
  catalog,
  chapterArtUrl,
  coverImageUrl,
  integrations,
  jobs as jobsApi,
  publicationPreviewUrl,
  ApiError,
} from "../api";
import type {
  AudiobookCostSummary,
  ChapterSummary,
  JobSnapshot,
  Llm,
  PublicationRow,
  Voice,
} from "../api";
import { useAuth } from "../store/auth";
import { useProgressSocket } from "../hooks/useProgressSocket";
import { RenameAudiobookDialog } from "../components/RenameAudiobookDialog";
import { ArtStyleSelect } from "../components/ArtStylePicker";
import { ART_STYLES, styleIcon, styleLabel } from "../lib/art-styles";
import { imageCapableLlms } from "../lib/cover-llm";
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
  const [publishOpen, setPublishOpen] = useState(false);
  const [activeLang, setActiveLang] = useState<string | null>(null);
  const [preview, setPreview] = useState<{ src: string; alt: string } | null>(null);

  const { data, isLoading, error } = useQuery({
    queryKey: ["audiobook", id, activeLang ?? "primary"],
    queryFn: () => audiobooks.get(id!, activeLang ?? undefined),
    enabled: !!id,
  });
  const voicesQuery = useQuery({
    queryKey: ["voices"],
    queryFn: () => catalog.voices(),
  });
  const youtubeAccount = useQuery({
    queryKey: ["integrations", "youtube", "account"],
    queryFn: () => integrations.youtube.account(),
  });
  const publications = useQuery({
    queryKey: ["audiobook", id, "publications"],
    queryFn: () => integrations.youtube.listPublications(id!),
    enabled: !!id,
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
  const setArtStyle = useMutation({
    mutationFn: (next: string) => audiobooks.patch(id!, { art_style: next }),
    onSuccess: () => qc.invalidateQueries({ queryKey: ["audiobook", id] }),
  });
  const setCoverLlm = useMutation({
    mutationFn: (next: string) =>
      audiobooks.patch(id!, { cover_llm_id: next }),
    onSuccess: () => qc.invalidateQueries({ queryKey: ["audiobook", id] }),
  });
  const llmsQuery = useQuery({
    queryKey: ["llms"],
    queryFn: () => catalog.llms(),
  });
  const costsQuery = useQuery({
    queryKey: ["audiobook", id, "costs"],
    queryFn: () => audiobooks.costs(id!),
    enabled: !!id,
    // Refetch every 15s while jobs may still be running so the badge ticks
    // up live; React Query cache makes this nearly free when nothing changed.
    refetchInterval: 15_000,
  });
  const coverLlms = llmsQuery.data
    ? imageCapableLlms(llmsQuery.data.items)
    : [];

  // Refetch on terminal events.
  useEffect(() => {
    if (progress.terminalTick > 0) {
      qc.invalidateQueries({ queryKey: ["audiobook", id] });
      qc.invalidateQueries({ queryKey: ["audiobook", id, "publications"] });
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
  const cancelPipeline = useMutation({
    mutationFn: () => audiobooks.cancelPipeline(id!),
    onSuccess: () => {
      qc.invalidateQueries({ queryKey: ["audiobook", id] });
      burstSeed();
    },
  });

  const parentJobs = useMemo(
    () =>
      progress.jobs.filter(
        (j) =>
          j.kind === "chapters" ||
          j.kind === "tts" ||
          j.kind === "tts_chapter" ||
          j.kind === "translate" ||
          j.kind === "publish_youtube" ||
          j.kind === "cover",
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
  // Cancel applies whenever any pipeline job is non-terminal — covers the
  // gap between "queued by /audiobook" and "first WS event arrives" too.
  const pipelineInFlight =
    pendingKind !== null ||
    parentJobs.some(
      (j) =>
        j.status === "queued" ||
        j.status === "running" ||
        j.status === "throttled",
    );

  const viewLang = activeLang ?? data.language;

  return (
    <section>
      <div className="mb-6 flex flex-col gap-4 sm:flex-row sm:items-start">
        <CoverBlock
          audiobookId={data.id}
          hasCover={data.has_cover}
          accessToken={accessToken}
          updatedAt={data.updated_at}
          onRegenerate={() => regenCover.mutate()}
          regenerating={regenCover.isPending}
          regenError={
            regenCover.error
              ? regenCover.error instanceof ApiError
                ? regenCover.error.message
                : "Cover regen failed"
              : null
          }
          onPreview={setPreview}
          artStyle={data.art_style ?? null}
          onChangeArtStyle={(next) => setArtStyle.mutate(next)}
          changingStyle={setArtStyle.isPending}
          coverLlmId={data.cover_llm_id ?? ""}
          coverLlms={coverLlms}
          onChangeCoverLlm={(next) => setCoverLlm.mutate(next)}
          changingCoverLlm={setCoverLlm.isPending}
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
            <CostBadge data={costsQuery.data} />
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
      {publishOpen && (
        <PublishYoutubeDialog
          audiobookId={data.id}
          language={viewLang}
          languageLabel={langInfo(viewLang).label}
          accountConnected={youtubeAccount.data?.connected ?? false}
          onClose={() => setPublishOpen(false)}
          onQueued={() => {
            qc.invalidateQueries({ queryKey: ["audiobook", id, "publications"] });
            setPublishOpen(false);
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
        <button
          type="button"
          onClick={() => setPublishOpen(true)}
          disabled={!allAudioReady}
          title={
            !allAudioReady
              ? "Narrate every chapter in this language first"
              : !youtubeAccount.data?.connected
                ? "You'll be prompted to connect YouTube"
                : undefined
          }
          className="rounded-md border border-rose-900 bg-rose-950/40 px-3 py-2 text-sm text-rose-200 hover:border-rose-800 hover:bg-rose-950 disabled:cursor-not-allowed disabled:opacity-40"
        >
          ▶ Publish to YouTube
        </button>
        {pipelineInFlight && (
          <button
            type="button"
            onClick={() => {
              if (
                window.confirm(
                  "Cancel the pipeline? Queued steps stop immediately; running steps finish their current chunk and then stop.",
                )
              ) {
                cancelPipeline.mutate();
              }
            }}
            disabled={cancelPipeline.isPending}
            className="rounded-md border border-amber-900 bg-amber-950/40 px-3 py-2 text-sm text-amber-200 hover:border-amber-800 hover:bg-amber-950 disabled:cursor-not-allowed disabled:opacity-40"
          >
            {cancelPipeline.isPending ? "Cancelling…" : "✕ Cancel pipeline"}
          </button>
        )}
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
        {cancelPipeline.error && (
          <p className="w-full text-sm text-rose-400">
            {cancelPipeline.error instanceof ApiError
              ? cancelPipeline.error.message
              : "Could not cancel pipeline"}
          </p>
        )}
      </div>

      <ActivityLog
        audiobookId={data.id}
        accessToken={accessToken}
        parentJobs={parentJobs}
        pendingKind={pendingKind}
        publications={publications.data?.items ?? []}
      />


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
            <ChapterRow
              key={ch.id}
              audiobookId={data.id}
              ch={ch}
              job={perChapterJobs.get(ch.number)}
              accessToken={accessToken}
              updatedAt={data.updated_at}
              onChanged={() => qc.invalidateQueries({ queryKey: ["audiobook", id] })}
              onPreview={setPreview}
            />
          ))}
        </ol>
      )}

      {preview && (
        <ImagePreview
          src={preview.src}
          alt={preview.alt}
          onClose={() => setPreview(null)}
        />
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
  updatedAt,
  onRegenerate,
  regenerating,
  regenError,
  onPreview,
  artStyle,
  onChangeArtStyle,
  changingStyle,
  coverLlmId,
  coverLlms,
  onChangeCoverLlm,
  changingCoverLlm,
}: {
  audiobookId: string;
  hasCover: boolean;
  accessToken: string;
  updatedAt?: string | null;
  onRegenerate: () => void;
  regenerating: boolean;
  regenError: string | null;
  onPreview: (p: { src: string; alt: string }) => void;
  artStyle: string | null;
  onChangeArtStyle: (next: string) => void;
  changingStyle: boolean;
  coverLlmId: string;
  coverLlms: Llm[];
  onChangeCoverLlm: (next: string) => void;
  changingCoverLlm: boolean;
}): JSX.Element {
  // Cache-buster keyed on the audiobook's `updated_at`. The backend bumps
  // it on every cover regen; without this the browser keeps serving the
  // previous bytes from cache because the URL is otherwise identical.
  // Falls back to a per-render timestamp when `updated_at` is missing
  // (older books, mock fixtures).
  const cacheBust = encodeURIComponent(updatedAt ?? Date.now().toString());
  const src = `${coverImageUrl(audiobookId, accessToken)}&v=${cacheBust}&t=${regenerating ? "loading" : "ready"}`;
  // Default the picker to whatever is stored, falling back to the first
  // option so the `<select>` always has a concrete value.
  const styleValue =
    artStyle && ART_STYLES.some((s) => s.value === artStyle)
      ? artStyle
      : ART_STYLES[0]?.value ?? "cinematic";
  return (
    <div className="flex shrink-0 flex-col items-stretch gap-2">
      <div className="h-32 w-32 overflow-hidden rounded-lg border border-slate-800 bg-slate-950">
        {hasCover ? (
          <button
            type="button"
            onClick={() => onPreview({ src, alt: "Cover" })}
            title="Click to enlarge"
            className="block h-full w-full cursor-zoom-in p-0"
          >
            <img src={src} alt="Cover" className="h-full w-full object-cover" />
          </button>
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

      <div className="mt-1 space-y-1">
        <p className="flex items-center gap-1 text-[11px] uppercase tracking-wide text-slate-500">
          <span>{styleIcon(artStyle)}</span>
          <span>Style: {styleLabel(artStyle)}</span>
        </p>
        <ArtStyleSelect
          value={styleValue}
          onChange={onChangeArtStyle}
          className="w-full rounded-md border border-slate-800 bg-slate-900 px-2 py-1 text-[11px] text-slate-200 outline-none focus:border-sky-600 disabled:opacity-40"
        />
        <p className="text-[10px] leading-snug text-slate-500">
          {changingStyle
            ? "Saving…"
            : "Then click Regenerate cover (and chapter art) to apply."}
        </p>
      </div>

      {coverLlms.length > 1 && (
        <div className="mt-1 space-y-1">
          <p className="text-[11px] uppercase tracking-wide text-slate-500">
            Image model
          </p>
          <select
            value={coverLlmId}
            onChange={(e) => onChangeCoverLlm(e.target.value)}
            className="w-full rounded-md border border-slate-800 bg-slate-900 px-2 py-1 text-[11px] text-slate-200 outline-none focus:border-sky-600 disabled:opacity-40"
          >
            <option value="">Server default</option>
            {coverLlms.map((l) => (
              <option key={l.id} value={l.id}>
                {l.name}
              </option>
            ))}
          </select>
          {changingCoverLlm && (
            <p className="text-[10px] leading-snug text-slate-500">Saving…</p>
          )}
        </div>
      )}
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

function CostBadge({
  data,
}: {
  data: AudiobookCostSummary | undefined;
}): JSX.Element | null {
  const [open, setOpen] = useState(false);
  if (!data || data.event_count === 0) return null;
  const total = data.total_cost_usd;
  const label = total === 0 ? "free" : formatUsd(total);
  return (
    <>
      <button
        type="button"
        onClick={() => setOpen(true)}
        title="Click for breakdown"
        className="rounded-full border border-slate-800 bg-slate-900/40 px-2 py-0.5 text-[11px] text-slate-300 hover:border-slate-600 hover:text-slate-100"
      >
        💸 {label}
      </button>
      {open && <CostBreakdownDialog data={data} onClose={() => setOpen(false)} />}
    </>
  );
}

const COST_CATEGORIES: { key: string; label: string; icon: string; roles: string[] }[] = [
  {
    key: "text",
    label: "Text generation",
    icon: "📝",
    roles: ["outline", "chapter", "title", "random_topic", "moderation", "translate", "scene_extract"],
  },
  { key: "image", label: "Image generation", icon: "🎨", roles: ["cover", "paragraph_image"] },
  { key: "narrate", label: "Narration (TTS)", icon: "🎙", roles: ["tts"] },
];

function categoryFor(role: string): string {
  for (const c of COST_CATEGORIES) {
    if (c.roles.includes(role)) return c.key;
  }
  return "other";
}

function CostBreakdownDialog({
  data,
  onClose,
}: {
  data: AudiobookCostSummary;
  onClose: () => void;
}): JSX.Element {
  useEffect(() => {
    const onKey = (e: KeyboardEvent): void => {
      if (e.key === "Escape") onClose();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [onClose]);

  // Bucket each role into a category (text / image / narrate / other) so
  // the modal scans top-down rather than as one flat list.
  const buckets = new Map<string, { label: string; icon: string; rows: typeof data.by_role; cost: number }>();
  for (const cat of [...COST_CATEGORIES, { key: "other", label: "Other", icon: "•", roles: [] }]) {
    buckets.set(cat.key, { label: cat.label, icon: cat.icon, rows: [], cost: 0 });
  }
  for (const r of data.by_role) {
    const key = categoryFor(r.role);
    const b = buckets.get(key)!;
    b.rows.push(r);
    b.cost += r.cost_usd;
  }

  return (
    <div
      className="fixed inset-0 z-50 flex items-center justify-center bg-black/60 p-4"
      onClick={onClose}
    >
      <div
        onClick={(e) => e.stopPropagation()}
        className="w-full max-w-lg rounded-xl border border-slate-800 bg-slate-950 p-5 shadow-xl"
      >
        <div className="mb-4 flex items-baseline justify-between gap-3">
          <h2 className="text-base font-semibold text-slate-100">
            Generation cost
          </h2>
          <span className="text-xl font-semibold tabular-nums text-emerald-300">
            {formatUsd(data.total_cost_usd)}
          </span>
        </div>
        <p className="mb-4 text-xs text-slate-500">
          {data.event_count} call{data.event_count === 1 ? "" : "s"} ·{" "}
          {data.total_prompt_tokens.toLocaleString()} in /{" "}
          {data.total_completion_tokens.toLocaleString()} out
        </p>

        <div className="space-y-4">
          {[...buckets.entries()]
            // Hide empty buckets so an audio-only book doesn't show "Image: $0".
            .filter(([, b]) => b.rows.length > 0)
            .map(([key, bucket]) => (
              <CostCategorySection key={key} categoryKey={key} bucket={bucket} />
            ))}
        </div>

        <div className="mt-5 flex justify-end">
          <button
            type="button"
            onClick={onClose}
            className="rounded-md border border-slate-700 bg-slate-900 px-3 py-1.5 text-sm text-slate-200 hover:border-slate-600"
          >
            Close
          </button>
        </div>
      </div>
    </div>
  );
}

function CostCategorySection({
  categoryKey,
  bucket,
}: {
  categoryKey: string;
  bucket: { label: string; icon: string; rows: AudiobookCostSummary["by_role"]; cost: number };
}): JSX.Element {
  return (
    <section>
      <header className="mb-1 flex items-baseline justify-between gap-3 border-b border-slate-800 pb-1">
        <span className="text-sm font-medium text-slate-200">
          <span className="mr-1.5">{bucket.icon}</span>
          {bucket.label}
        </span>
        <span className="text-sm tabular-nums text-slate-300">
          {formatUsd(bucket.cost)}
        </span>
      </header>
      <ul className="space-y-1 text-xs text-slate-400">
        {bucket.rows.map((r) => (
          <li
            key={r.role}
            className="flex items-baseline justify-between gap-3 tabular-nums"
          >
            <span className="font-mono text-slate-300">{r.role}</span>
            <span className="text-right text-slate-500">
              {r.count} call{r.count === 1 ? "" : "s"}
              {categoryKey === "narrate" ? (
                // For TTS we repurpose prompt_tokens=chars and
                // completion_tokens=duration_ms (see audio.rs).
                <> · {r.prompt_tokens.toLocaleString()} chars</>
              ) : r.prompt_tokens > 0 || r.completion_tokens > 0 ? (
                <>
                  {" · "}
                  {r.prompt_tokens.toLocaleString()} in /{" "}
                  {r.completion_tokens.toLocaleString()} out
                </>
              ) : null}
            </span>
            <span className="w-16 text-right text-slate-300">
              {formatUsd(r.cost_usd)}
            </span>
          </li>
        ))}
      </ul>
    </section>
  );
}

/// Formats a USD float with adaptive precision: tiny costs (a few cents)
/// need 4 decimals to read meaningfully, larger ones round to 2.
function formatUsd(n: number): string {
  if (n === 0) return "$0.00";
  if (n < 0.01) return `$${n.toFixed(4)}`;
  if (n < 1) return `$${n.toFixed(3)}`;
  return `$${n.toFixed(2)}`;
}

function ParentJobRow({ job }: { job: JobSnapshot }): JSX.Element {
  const indeterminate = job.status === "queued" || job.status === "throttled";
  const title = activityJobTitle(job);
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
          {title}
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

function activityJobTitle(job: JobSnapshot): string {
  if (job.kind === "cover" && job.chapter_number != null) {
    return `Chapter ${job.chapter_number} cover art`;
  }
  if (job.kind === "cover") return "Main cover art";
  if (job.kind === "tts_chapter" && job.chapter_number != null) {
    return `Chapter ${job.chapter_number} narration`;
  }
  if (job.kind === "tts") return "Narration";
  if (job.kind === "publish_youtube") return "Publish YouTube";
  return job.kind.replace(/_/g, " ");
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

function ActivityLog({
  audiobookId,
  accessToken,
  parentJobs,
  pendingKind,
  publications,
}: {
  audiobookId: string;
  accessToken: string;
  parentJobs: JobSnapshot[];
  pendingKind: "chapters" | "tts" | null;
  publications: PublicationRow[];
}): JSX.Element | null {
  const hasJobs = parentJobs.length > 0 || pendingKind !== null;
  const hasPubs = publications.length > 0;
  if (!hasJobs && !hasPubs) return null;

  // Auto-open on first render when something needs the user's attention.
  // Once they toggle it themselves, the local state owns the panel.
  const [open, setOpen] = useState(() => {
    const running = parentJobs.some(
      (j) =>
        j.status === "running" ||
        j.status === "queued" ||
        j.status === "throttled",
    );
    const previewReady = publications.some(
      (p) => p.review && p.preview_ready_at !== null,
    );
    return running || previewReady;
  });

  const activeCount = parentJobs.filter(
    (j) =>
      j.status === "running" ||
      j.status === "queued" ||
      j.status === "throttled",
  ).length;
  const previewCount = publications.filter(
    (p) => p.review && p.preview_ready_at !== null,
  ).length;
  const failedCount = parentJobs.filter(
    (j) => j.status === "failed" || j.status === "dead",
  ).length;

  return (
    <details
      open={open}
      className="mb-6 overflow-hidden rounded-lg border border-slate-800 bg-slate-900/40"
    >
      <summary
        onClick={(e) => {
          e.preventDefault();
          setOpen((v) => !v);
        }}
        className="flex cursor-pointer select-none items-center gap-2 px-4 py-2.5 text-sm text-slate-200 hover:bg-slate-900/70"
      >
        <span aria-hidden="true" className="text-xs text-slate-500">
          {open ? "▾" : "▸"}
        </span>
        <span className="font-medium">Activity log</span>
        {activeCount > 0 && (
          <span className="rounded-full border border-sky-700 bg-sky-950/40 px-2 py-0.5 text-[11px] uppercase tracking-wide text-sky-200">
            {activeCount} running
          </span>
        )}
        {previewCount > 0 && (
          <span className="rounded-full border border-emerald-700 bg-emerald-950/40 px-2 py-0.5 text-[11px] uppercase tracking-wide text-emerald-200">
            {previewCount} preview ready
          </span>
        )}
        {failedCount > 0 && activeCount === 0 && (
          <span className="rounded-full border border-rose-800 bg-rose-950/40 px-2 py-0.5 text-[11px] uppercase tracking-wide text-rose-200">
            {failedCount} failed
          </span>
        )}
        {!activeCount && !previewCount && !failedCount && hasPubs && (
          <span className="rounded-full border border-slate-700 bg-slate-950 px-2 py-0.5 text-[11px] uppercase tracking-wide text-slate-400">
            {publications.length} publication{publications.length === 1 ? "" : "s"}
          </span>
        )}
      </summary>
      {open && (
        <div className="space-y-4 border-t border-slate-800 px-4 py-4">
          {hasJobs && (
            <div className="space-y-2">
              {parentJobs.length > 0
                ? parentJobs.map((j) => <ParentJobRow key={j.id} job={j} />)
                : pendingKind && <PendingJobRow kind={pendingKind} />}
            </div>
          )}
          {hasPubs && (
            <PublicationsPanel
              audiobookId={audiobookId}
              rows={publications}
              accessToken={accessToken}
            />
          )}
        </div>
      )}
    </details>
  );
}

function PublicationsPanel({
  audiobookId,
  rows,
  accessToken,
}: {
  audiobookId: string;
  rows: PublicationRow[];
  accessToken: string;
}): JSX.Element {
  return (
    <div className="mb-6 rounded-lg border border-slate-800 bg-slate-900/40 p-4">
      <h3 className="mb-2 text-sm font-semibold text-slate-200">
        Published to YouTube
      </h3>
      <ul className="space-y-2 text-sm">
        {rows.map((p) => {
          const isPlaylist = p.mode === "playlist";
          const link = isPlaylist ? p.playlist_url : p.video_url;
          const allChaptersDone =
            !isPlaylist ||
            (p.videos.length > 0 && p.videos.every((v) => v.video_id));
          const inReview = p.review;
          const previewReady = inReview && p.preview_ready_at !== null;
          const tone = inReview
            ? previewReady
              ? "border-sky-900/50 bg-sky-950/20"
              : "border-slate-800"
            : link
              ? allChaptersDone
                ? "border-emerald-900/50 bg-emerald-950/20"
                : "border-amber-900/50 bg-amber-950/20"
              : p.last_error
                ? "border-rose-900/50 bg-rose-950/20"
                : "border-slate-800";
          const doneCount = p.videos.filter((v) => v.video_id).length;
          return (
            <li
              key={p.id}
              className={`rounded-md border ${tone} px-3 py-2`}
            >
              <div className="flex flex-wrap items-center justify-between gap-2">
                <div className="min-w-0">
                  <span className="mr-2 inline-flex items-center gap-1 rounded-full border border-slate-700 bg-slate-950 px-2 py-0.5 text-[11px] uppercase tracking-wide text-slate-300">
                    {p.language}
                  </span>
                  <span className="mr-2 inline-flex items-center gap-1 rounded-full border border-slate-700 bg-slate-950 px-2 py-0.5 text-[11px] uppercase tracking-wide text-slate-300">
                    {isPlaylist ? "playlist" : "video"}
                  </span>
                  <span className="text-xs text-slate-400">{p.privacy_status}</span>
                  {inReview && (
                    <span className="ml-2 inline-flex items-center gap-1 rounded-full border border-sky-700 bg-sky-950/40 px-2 py-0.5 text-[11px] uppercase tracking-wide text-sky-200">
                      {previewReady ? "preview ready" : "encoding…"}
                    </span>
                  )}
                  {!inReview && isPlaylist && p.videos.length > 0 && (
                    <span className="ml-2 text-xs text-slate-400">
                      {doneCount}/{p.videos.length} chapters
                    </span>
                  )}
                  {p.last_error && (
                    <p className="mt-1 text-xs text-rose-300">{p.last_error}</p>
                  )}
                </div>
                <div className="flex items-center gap-3">
                  {!inReview && link && (
                    <a
                      href={link}
                      target="_blank"
                      rel="noreferrer"
                      className="text-xs text-sky-300 hover:text-sky-200"
                    >
                      Open ↗
                    </a>
                  )}
                  {!inReview && !link && (
                    <span className="text-xs text-slate-500">
                      {p.last_error ? "failed" : "in flight…"}
                    </span>
                  )}
                </div>
              </div>
              {inReview && (
                <PreviewPanel
                  audiobookId={audiobookId}
                  publication={p}
                  accessToken={accessToken}
                />
              )}
              {!inReview && isPlaylist && p.videos.length > 0 && (
                <details className="mt-2">
                  <summary className="cursor-pointer text-xs text-slate-400 hover:text-slate-200">
                    Chapter videos
                  </summary>
                  <ul className="mt-2 space-y-1 text-xs">
                    {p.videos.map((v) => (
                      <li
                        key={v.chapter_number}
                        className="flex items-center justify-between gap-2 rounded border border-slate-800 bg-slate-950 px-2 py-1"
                      >
                        <span className="min-w-0 truncate text-slate-300">
                          <span className="mr-2 text-slate-500">
                            #{v.chapter_number}
                          </span>
                          {v.title}
                        </span>
                        {v.video_url ? (
                          <a
                            href={v.video_url}
                            target="_blank"
                            rel="noreferrer"
                            className="shrink-0 text-sky-300 hover:text-sky-200"
                          >
                            Open ↗
                          </a>
                        ) : v.last_error ? (
                          <span
                            className="shrink-0 text-rose-300"
                            title={v.last_error}
                          >
                            failed
                          </span>
                        ) : (
                          <span className="shrink-0 text-slate-500">
                            queued…
                          </span>
                        )}
                      </li>
                    ))}
                  </ul>
                </details>
              )}
            </li>
          );
        })}
      </ul>
    </div>
  );
}

function PreviewPanel({
  audiobookId,
  publication,
  accessToken,
}: {
  audiobookId: string;
  publication: PublicationRow;
  accessToken: string;
}): JSX.Element {
  const qc = useQueryClient();
  const isPlaylist = publication.mode === "playlist";
  const previewReady = publication.preview_ready_at !== null;
  // Track which chapter (if any) the user is previewing. For single-mode
  // this stays null and the video tag streams the only file.
  const [activeChapter, setActiveChapter] = useState<number | null>(null);

  const approve = useMutation({
    mutationFn: () =>
      integrations.youtube.approve(audiobookId, publication.id),
    onSuccess: () =>
      qc.invalidateQueries({
        queryKey: ["audiobook", audiobookId, "publications"],
      }),
  });
  const cancel = useMutation({
    mutationFn: () =>
      integrations.youtube.cancel(audiobookId, publication.id),
    onSuccess: () =>
      qc.invalidateQueries({
        queryKey: ["audiobook", audiobookId, "publications"],
      }),
  });

  if (!previewReady) {
    return (
      <div className="mt-2 rounded-md border border-slate-800 bg-slate-950/60 p-3 text-xs text-slate-300">
        Encoding the preview…  This finishes when the publish job completes.
      </div>
    );
  }

  const chapterNumbers = isPlaylist
    ? publication.videos
        .map((v) => v.chapter_number)
        .sort((a, b) => a - b)
    : [];
  const chapter = isPlaylist
    ? activeChapter ?? chapterNumbers[0] ?? 1
    : undefined;
  const src = publicationPreviewUrl(
    audiobookId,
    publication.id,
    accessToken,
    chapter,
  );

  return (
    <div className="mt-2 space-y-2">
      <video
        key={src}
        src={src}
        controls
        preload="metadata"
        className="w-full rounded-md border border-slate-800 bg-black"
      />
      {isPlaylist && chapterNumbers.length > 1 && (
        <div className="flex flex-wrap gap-1">
          {chapterNumbers.map((n) => {
            const v = publication.videos.find((x) => x.chapter_number === n);
            const active = (activeChapter ?? chapterNumbers[0]) === n;
            return (
              <button
                key={n}
                type="button"
                onClick={() => setActiveChapter(n)}
                className={`rounded-md border px-2 py-1 text-[11px] ${
                  active
                    ? "border-sky-600 bg-sky-600/15 text-sky-200"
                    : "border-slate-800 bg-slate-950 text-slate-300 hover:border-slate-600"
                }`}
                title={v?.title ?? `Chapter ${n}`}
              >
                Ch. {n}
              </button>
            );
          })}
        </div>
      )}
      <div className="flex flex-wrap items-center gap-2">
        <button
          type="button"
          onClick={() => approve.mutate()}
          disabled={approve.isPending || cancel.isPending}
          className="rounded-md bg-emerald-600 px-3 py-1.5 text-xs font-medium text-white hover:bg-emerald-500 disabled:cursor-not-allowed disabled:bg-emerald-700/50"
        >
          {approve.isPending ? "Approving…" : "Approve & upload"}
        </button>
        <button
          type="button"
          onClick={() => {
            if (window.confirm("Discard this preview? The encoded files will be deleted.")) {
              cancel.mutate();
            }
          }}
          disabled={approve.isPending || cancel.isPending}
          className="rounded-md border border-slate-700 bg-slate-950 px-3 py-1.5 text-xs text-slate-200 hover:border-slate-600 hover:bg-slate-900 disabled:cursor-not-allowed disabled:opacity-40"
        >
          {cancel.isPending ? "Discarding…" : "Discard preview"}
        </button>
        {(approve.error || cancel.error) && (
          <span className="text-xs text-rose-400">
            {(approve.error || cancel.error) instanceof ApiError
              ? (approve.error || cancel.error)!.message
              : "Action failed"}
          </span>
        )}
      </div>
    </div>
  );
}

function PublishYoutubeDialog({
  audiobookId,
  language,
  languageLabel,
  accountConnected,
  onClose,
  onQueued,
}: {
  audiobookId: string;
  language: string;
  languageLabel: string;
  accountConnected: boolean;
  onClose: () => void;
  onQueued: () => void;
}): JSX.Element {
  const [privacy, setPrivacy] = useState<"private" | "unlisted" | "public">(
    "private",
  );
  const [mode, setMode] = useState<"single" | "playlist">("single");
  const [review, setReview] = useState(true);
  const [description, setDescription] = useState("");

  useEffect(() => {
    const onKey = (e: KeyboardEvent): void => {
      if (e.key === "Escape") onClose();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [onClose]);

  const publish = useMutation({
    mutationFn: () =>
      integrations.youtube.publish(audiobookId, {
        language,
        privacy_status: privacy,
        mode,
        review,
        description: description.trim() ? description : null,
      }),
    onSuccess: onQueued,
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
          if (!publish.isPending && accountConnected) publish.mutate();
        }}
        className="w-full max-w-md rounded-xl border border-slate-800 bg-slate-950 p-5 shadow-xl"
      >
        <h2 className="text-base font-semibold text-slate-100">
          Publish to YouTube
        </h2>
        <p className="mt-1 text-xs text-slate-400">
          {mode === "single" ? (
            <>
              Uploads the <strong>{languageLabel}</strong> version as a single
              video. The cover image is held for the full duration; chapter
              timestamps land in the description.
            </>
          ) : (
            <>
              Creates a playlist on your channel and uploads each chapter of
              the <strong>{languageLabel}</strong> version as its own video.
            </>
          )}
        </p>

        {!accountConnected && (
          <p className="mt-3 rounded-md border border-amber-900/60 bg-amber-950/30 p-2 text-xs text-amber-200">
            Connect a YouTube channel from{" "}
            <Link to="/app/settings" className="underline hover:text-amber-100">
              Settings
            </Link>{" "}
            first.
          </p>
        )}

        <fieldset className="mt-4">
          <legend className="text-xs font-medium text-slate-300">Format</legend>
          <div className="mt-2 grid grid-cols-2 gap-2">
            {(
              [
                {
                  value: "single",
                  title: "Single video",
                  blurb: "All chapters in one MP4.",
                },
                {
                  value: "playlist",
                  title: "Playlist",
                  blurb: "One video per chapter.",
                },
              ] as const
            ).map((opt) => (
              <label
                key={opt.value}
                className={`flex cursor-pointer flex-col items-start gap-0.5 rounded-md border px-3 py-2 text-left ${
                  mode === opt.value
                    ? "border-rose-600 bg-rose-600/10 text-rose-200"
                    : "border-slate-700 bg-slate-950 text-slate-200 hover:border-slate-600"
                }`}
              >
                <input
                  type="radio"
                  name="mode"
                  value={opt.value}
                  checked={mode === opt.value}
                  onChange={() => setMode(opt.value)}
                  className="sr-only"
                />
                <span className="text-sm font-medium">{opt.title}</span>
                <span className="text-[11px] text-slate-400">{opt.blurb}</span>
              </label>
            ))}
          </div>
        </fieldset>

        <label className="mt-4 flex cursor-pointer items-start gap-2 rounded-md border border-slate-800 bg-slate-900 p-3">
          <input
            type="checkbox"
            checked={review}
            onChange={(e) => setReview(e.target.checked)}
            className="mt-0.5 h-4 w-4 accent-rose-600"
          />
          <span className="min-w-0">
            <span className="block text-sm text-slate-100">
              Review before publishing
            </span>
            <span className="block text-[11px] text-slate-400">
              Encode the {mode === "playlist" ? "chapter videos" : "video"} so
              you can watch them locally first. Nothing is uploaded to YouTube
              until you approve.
            </span>
          </span>
        </label>

        <fieldset className="mt-4">
          <legend className="text-xs font-medium text-slate-300">
            Visibility
          </legend>
          <div className="mt-2 grid grid-cols-3 gap-2">
            {(["private", "unlisted", "public"] as const).map((v) => (
              <label
                key={v}
                className={`flex cursor-pointer items-center justify-center rounded-md border px-3 py-2 text-sm capitalize ${
                  privacy === v
                    ? "border-rose-600 bg-rose-600/10 text-rose-200"
                    : "border-slate-700 bg-slate-950 text-slate-200 hover:border-slate-600"
                }`}
              >
                <input
                  type="radio"
                  name="privacy"
                  value={v}
                  checked={privacy === v}
                  onChange={() => setPrivacy(v)}
                  className="sr-only"
                />
                {v}
              </label>
            ))}
          </div>
        </fieldset>

        <label className="mt-4 block text-xs font-medium text-slate-300">
          {mode === "playlist" ? "Playlist description (optional)" : "Description (optional)"}
          <textarea
            value={description}
            onChange={(e) => setDescription(e.target.value)}
            rows={3}
            placeholder={
              mode === "playlist"
                ? "Leave blank to auto-generate from topic. Per-chapter descriptions use the chapter synopsis."
                : "Leave blank to auto-generate from topic + chapter list."
            }
            className="mt-1 w-full resize-none rounded-md border border-slate-800 bg-slate-900 px-3 py-2 text-sm text-slate-100 outline-none focus:border-rose-600"
          />
        </label>

        {publish.error && (
          <p className="mt-3 text-xs text-rose-400">
            {publish.error instanceof ApiError
              ? publish.error.message
              : "Publish failed"}
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
            disabled={publish.isPending || (!accountConnected && !review)}
            className="rounded-md bg-rose-600 px-3 py-2 text-sm font-medium text-white hover:bg-rose-500 disabled:cursor-not-allowed disabled:bg-rose-700/50"
          >
            {publish.isPending
              ? "Queuing…"
              : review
                ? "Prepare preview"
                : "Publish"}
          </button>
        </div>
      </form>
    </div>
  );
}

function ImagePreview({
  src,
  alt,
  onClose,
}: {
  src: string;
  alt: string;
  onClose: () => void;
}): JSX.Element {
  useEffect(() => {
    const onKey = (e: KeyboardEvent): void => {
      if (e.key === "Escape") onClose();
    };
    window.addEventListener("keydown", onKey);
    // Prevent the page underneath from scrolling while the lightbox is open.
    const prev = document.body.style.overflow;
    document.body.style.overflow = "hidden";
    return () => {
      window.removeEventListener("keydown", onKey);
      document.body.style.overflow = prev;
    };
  }, [onClose]);

  return (
    <div
      className="fixed inset-0 z-50 flex items-center justify-center bg-black/85 p-4"
      onClick={onClose}
      role="dialog"
      aria-modal="true"
      aria-label={alt}
    >
      <button
        type="button"
        onClick={onClose}
        aria-label="Close preview"
        className="absolute right-4 top-4 rounded-full border border-slate-700 bg-slate-900/80 px-3 py-1 text-sm text-slate-200 hover:border-slate-500 hover:text-white"
      >
        Esc ✕
      </button>
      <img
        src={src}
        alt={alt}
        className="max-h-[90vh] max-w-[90vw] cursor-zoom-out rounded-lg shadow-2xl"
      />
    </div>
  );
}

function ChapterRow({
  audiobookId,
  ch,
  job,
  accessToken,
  updatedAt,
  onChanged,
  onPreview,
}: {
  audiobookId: string;
  ch: ChapterSummary;
  job: JobSnapshot | undefined;
  accessToken: string;
  updatedAt?: string | null;
  onChanged: () => void;
  onPreview: (p: { src: string; alt: string }) => void;
}): JSX.Element {
  const art = useMutation({
    mutationFn: () => audiobooks.regenerateChapterArt(audiobookId, ch.number),
    onSuccess: onChanged,
  });
  // Same cache-buster strategy as the cover: keyed on the audiobook's
  // `updated_at`, which the backend bumps on every chapter-art regen.
  const cacheBust = encodeURIComponent(updatedAt ?? Date.now().toString());
  const artSrc = `${chapterArtUrl(audiobookId, ch.number, accessToken, ch.language)}&v=${cacheBust}&t=${art.isPending ? "loading" : "ready"}`;
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
      <div className="flex items-start gap-3">
        <div className="h-20 w-20 shrink-0 overflow-hidden rounded-md border border-slate-800 bg-slate-950">
          {ch.has_art ? (
            <button
              type="button"
              onClick={() =>
                onPreview({ src: artSrc, alt: `Chapter ${ch.number}: ${ch.title}` })
              }
              title="Click to enlarge"
              className="block h-full w-full cursor-zoom-in p-0"
            >
              <img
                src={artSrc}
                alt=""
                className="h-full w-full object-cover"
                loading="lazy"
              />
            </button>
          ) : (
            <div className="flex h-full w-full items-center justify-center text-2xl text-slate-700">
              🎨
            </div>
          )}
        </div>
        <div className="min-w-0">
          <p className="text-xs uppercase tracking-wide text-slate-500">
            Chapter {ch.number}
          </p>
          <h3 className="truncate text-base font-medium text-slate-100">{ch.title}</h3>
          {ch.synopsis && (
            <p className="mt-1 line-clamp-2 text-sm text-slate-400">{ch.synopsis}</p>
          )}
        </div>
        <div className="ml-auto flex shrink-0 flex-col items-end gap-2">
          <span className="whitespace-nowrap text-xs text-slate-400">{statusLabel}</span>
          <button
            type="button"
            onClick={() => art.mutate()}
            disabled={art.isPending}
            className="rounded-md border border-slate-700 bg-slate-950 px-2 py-1 text-xs text-slate-300 hover:border-slate-600 hover:text-slate-100 disabled:cursor-not-allowed disabled:opacity-40"
          >
            {art.isPending ? "Generating…" : ch.has_art ? "Regenerate art" : "Generate art"}
          </button>
        </div>
      </div>
      {art.error && (
        <p className="mt-2 text-xs text-rose-400">
          {art.error instanceof ApiError ? art.error.message : "Could not generate chapter art"}
        </p>
      )}
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
