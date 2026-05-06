import { useEffect, useMemo, useState } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { Link, useNavigate, useParams } from "react-router-dom";
import {
  audiobooks,
  audiobookTestManimVideoUrl,
  catalog,
  chapterArtUrl,
  chapterVideoUrl,
  coverImageUrl,
  integrations,
  jobs as jobsApi,
  podcasts as podcastsApi,
  publicationPreviewUrl,
  ApiError,
} from "../api";
import type {
  AudiobookCostSummary,
  ChapterSummary,
  JobSnapshot,
  Llm,
  PodcastRow,
  PublicationRow,
  Voice,
} from "../api";
import { useAuth } from "../store/auth";
import { useProgressSocket } from "../hooks/useProgressSocket";
import { RenameAudiobookDialog } from "../components/RenameAudiobookDialog";
import { ArtStyleSelect } from "../components/ArtStylePicker";
import { CopyButton } from "../components/CopyButton";
import { ART_STYLES, styleIcon, styleLabel } from "../lib/art-styles";
import { imageCapableLlms } from "../lib/cover-llm";
import { voicesForLanguage } from "../lib/voices";
import {
  useVoicePreview,
  type VoicePreviewState,
} from "../lib/useVoicePreview";
import { VoicePreviewButton } from "../components/VoicePreview";
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
  const setCategory = useMutation({
    mutationFn: (next: string) => audiobooks.patch(id!, { category: next }),
    onSuccess: () => {
      qc.invalidateQueries({ queryKey: ["audiobook", id] });
      // The library list groups by category, so refresh it too.
      qc.invalidateQueries({ queryKey: ["audiobooks"] });
    },
  });
  const setPodcast = useMutation({
    mutationFn: (next: string) => audiobooks.patch(id!, { podcast_id: next }),
    onSuccess: () => {
      qc.invalidateQueries({ queryKey: ["audiobook", id] });
      qc.invalidateQueries({ queryKey: ["audiobooks"] });
      qc.invalidateQueries({ queryKey: ["podcasts"] });
    },
  });
  // Three-state STEM override. Sending `null` clears (returns to LLM
  // verdict); `true`/`false` forces. Phase G's Manim diagram path
  // reads this to decide whether to attempt diagrammatic visuals.
  const setStemOverride = useMutation({
    mutationFn: (next: boolean | null) =>
      audiobooks.patch(id!, { stem_override: next }),
    onSuccess: () => qc.invalidateQueries({ queryKey: ["audiobook", id] }),
  });
  const categoriesQuery = useQuery({
    queryKey: ["audiobook-categories"],
    queryFn: () => catalog.audiobookCategories(),
  });
  const podcastsQuery = useQuery({
    queryKey: ["podcasts"],
    queryFn: () => podcastsApi.list(),
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
  const [animateTheme, setAnimateTheme] =
    useState<import("../api").AnimationTheme>("library");
  const animate = useMutation({
    mutationFn: () =>
      audiobooks.animate(id!, {
        language: activeLang ?? undefined,
        theme: animateTheme,
      }),
    onSuccess: () => {
      qc.invalidateQueries({ queryKey: ["audiobook", id] });
      burstSeed();
    },
  });
  // Per-chapter regenerate. Backend deletes the existing MP4 + busts
  // the F.1e cache before enqueueing, so the inline preview will
  // 404 until the new render completes — that's the intended UX
  // signal that the chapter is being rebuilt.
  const animateChapter = useMutation({
    mutationFn: (n: number) =>
      audiobooks.animateChapter(id!, n, {
        language: activeLang ?? undefined,
        theme: animateTheme,
      }),
    onSuccess: () => {
      qc.invalidateQueries({ queryKey: ["audiobook", id] });
      burstSeed();
    },
  });
  // Phase G — backfill the per-paragraph visual classifier on a
  // chapter that was generated before STEM was enabled. Returns the
  // updated chapter summary; query invalidation refreshes the
  // diagram badge so the user sees the count change in place.
  const classifyChapterVisuals = useMutation({
    mutationFn: (n: number) => audiobooks.classifyChapterVisuals(id!, n),
    onSuccess: () => {
      qc.invalidateQueries({ queryKey: ["audiobook", id] });
    },
  });
  // Phase H — re-run the bespoke Manim code-gen LLM against this
  // chapter's `custom_manim` paragraphs. Used as a backfill *and*
  // as a "regenerate with the model I just assigned to ManimCode"
  // knob; invalidating the audiobook query refreshes the diagram
  // badge tooltip so the user sees codes appear inline.
  const regenerateChapterManimCode = useMutation({
    mutationFn: (n: number) => audiobooks.regenerateChapterManimCode(id!, n),
    onSuccess: () => {
      qc.invalidateQueries({ queryKey: ["audiobook", id] });
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
          j.kind === "cover" ||
          j.kind === "animate" ||
          j.kind === "animate_chapter",
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
  // Per-chapter animation render jobs. Indexed by chapter_number; the
  // AnimationSection renders one row per chapter and looks up its
  // status here.
  const animationJobsByChapter = useMemo(() => {
    const map = new Map<number, JobSnapshot>();
    for (const j of progress.jobs) {
      if (j.kind === "animate_chapter" && j.chapter_number != null) {
        map.set(j.chapter_number, j);
      }
    }
    return map;
  }, [progress.jobs]);
  const animateParentRunning = useMemo(
    () =>
      progress.jobs.some(
        (j) =>
          j.kind === "animate" &&
          (j.status === "queued" || j.status === "running"),
      ),
    [progress.jobs],
  );
  // Pipeline-step failures surfaced at the top of the page so the user
  // doesn't have to expand the activity log to discover what broke.
  // Includes both `dead` (terminal — exhausted retries) and `failed`
  // (snapshot-side raw status) states.
  const failedParentJobs = useMemo(
    () =>
      parentJobs.filter(
        (j) =>
          (j.status === "dead" || j.status === "failed") &&
          !!j.last_error?.trim(),
      ),
    [parentJobs],
  );

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

  const allVoices = voicesQuery.data?.items ?? [];
  const voices = voicesForLanguage(allVoices, data.language);
  const voiceLabel = voiceFor(allVoices, data.voice_id ?? null);

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
            <CategoryPicker
              current={data.category ?? null}
              options={categoriesQuery.data?.items.map((c) => c.name) ?? []}
              onChange={(next) => setCategory.mutate(next)}
              saving={setCategory.isPending}
            />
            <PodcastPicker
              current={data.podcast_id ?? null}
              options={podcastsQuery.data?.items ?? []}
              onChange={(next) => setPodcast.mutate(next)}
              saving={setPodcast.isPending}
            />
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

      {failedParentJobs.length > 0 && (
        <div className="mb-4 space-y-2">
          {failedParentJobs.map((job) => (
            <div
              key={job.id}
              className="rounded-md border border-rose-900 bg-rose-950/30 p-3"
            >
              <div className="flex items-start justify-between gap-2">
                <div className="min-w-0 flex-1">
                  <p className="text-sm font-medium text-rose-200">
                    {activityJobTitle(job)} failed
                  </p>
                  <p className="mt-1 break-all text-xs text-rose-300">
                    {job.last_error}
                  </p>
                </div>
                <CopyButton
                  text={job.last_error ?? ""}
                  title="Copy error message"
                  className="shrink-0"
                />
              </div>
            </div>
          ))}
        </div>
      )}

      <SpeechTagsRow tags={data.tags ?? []} />

      <MultiVoicePanel
        audiobookId={data.id}
        enabled={data.multi_voice_enabled ?? false}
        roles={(data.voice_roles ?? {}) as Record<string, string>}
        voices={voices}
        onUpdated={() => qc.invalidateQueries({ queryKey: ["audiobook", id] })}
      />

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
          isShort={data.is_short ?? false}
          animationsReady={
            data.chapters.length > 0 &&
            data.chapters.every(
              (c) =>
                animationJobsByChapter.get(c.number)?.status === "completed",
            )
          }
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
          onClick={() => animate.mutate()}
          disabled={!allAudioReady || animate.isPending || animateParentRunning}
          title={
            !allAudioReady
              ? "Narrate every chapter in this language first"
              : "Render the animated companion videos for every chapter"
          }
          className="rounded-md border border-violet-900 bg-violet-950/40 px-3 py-2 text-sm text-violet-200 hover:border-violet-800 hover:bg-violet-950 disabled:cursor-not-allowed disabled:opacity-40"
        >
          {animate.isPending || animateParentRunning
            ? "Animating…"
            : `🎬 Animate (${langInfo(viewLang).label})`}
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
        {animate.error && (
          <p className="w-full text-sm text-rose-400">
            {animate.error instanceof ApiError
              ? animate.error.message
              : "Could not queue animation"}
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
        jobStages={progress.jobStages}
        pendingKind={pendingKind}
        publications={publications.data?.items ?? []}
      />

      {allAudioReady && data.chapters.length > 0 && (
        <AnimationSection
          audiobookId={data.id}
          chapters={data.chapters}
          jobsByChapter={animationJobsByChapter}
          jobStages={progress.jobStages}
          parentRunning={animateParentRunning}
          theme={animateTheme}
          onThemeChange={setAnimateTheme}
          accessToken={accessToken}
          language={viewLang}
          onRegenerateChapter={(n) => animateChapter.mutate(n)}
          regeneratingChapter={
            animateChapter.isPending ? animateChapter.variables ?? null : null
          }
          stemDetected={data.stem_detected ?? null}
          stemOverride={data.stem_override ?? null}
          isStem={data.is_stem ?? false}
          onSetStemOverride={(v) => setStemOverride.mutate(v)}
          stemPending={setStemOverride.isPending}
          onClassifyChapterVisuals={(n) => classifyChapterVisuals.mutate(n)}
          classifyingChapter={
            classifyChapterVisuals.isPending
              ? classifyChapterVisuals.variables ?? null
              : null
          }
          classifyError={
            classifyChapterVisuals.isError
              ? {
                  chapter: classifyChapterVisuals.variables ?? null,
                  message:
                    classifyChapterVisuals.error instanceof Error
                      ? classifyChapterVisuals.error.message
                      : "Classify request failed",
                }
              : null
          }
          onRegenerateManimCode={(n) => regenerateChapterManimCode.mutate(n)}
          regeneratingManimCodeChapter={
            regenerateChapterManimCode.isPending
              ? regenerateChapterManimCode.variables ?? null
              : null
          }
          regenerateManimCodeError={
            regenerateChapterManimCode.isError
              ? {
                  chapter: regenerateChapterManimCode.variables ?? null,
                  message:
                    regenerateChapterManimCode.error instanceof Error
                      ? regenerateChapterManimCode.error.message
                      : "Regenerate Manim code request failed",
                }
              : null
          }
        />
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

function SpeechTagsRow({ tags }: { tags: string[] }): JSX.Element | null {
  if (tags.length === 0) return null;
  return (
    <div
      className="mb-4 flex flex-wrap items-center gap-2"
      title="X.ai TTS speech tags embedded inline in the chapter prose to shape narration."
    >
      <span className="text-[11px] uppercase tracking-wide text-slate-500">
        Speech tags
      </span>
      {tags.map((t) => (
        <span
          key={t}
          className="rounded-full border border-violet-900/60 bg-violet-950/40 px-2 py-0.5 font-mono text-[11px] text-violet-300"
        >
          {t}
        </span>
      ))}
    </div>
  );
}

const MULTI_VOICE_ROLES: { id: string; label: string; hint: string }[] = [
  { id: "narrator", label: "Narrator", hint: "Descriptive prose & action" },
  { id: "dialogue_male", label: "Male dialogue", hint: "Speech by male characters" },
  { id: "dialogue_female", label: "Female dialogue", hint: "Speech by female characters" },
];

/**
 * Multi-voice settings card — collapsible panel sitting above the
 * language tabs. Toggle plus three role pickers (narrator / male /
 * female). The backend extracts segments lazily on the next narration,
 * so changes here don't trigger work until the user re-narrates.
 */
function MultiVoicePanel({
  audiobookId,
  enabled,
  roles,
  voices,
  onUpdated,
}: {
  audiobookId: string;
  enabled: boolean;
  roles: Record<string, string>;
  voices: Voice[];
  onUpdated: () => void;
}): JSX.Element {
  const [open, setOpen] = useState(enabled);

  const toggle = useMutation({
    mutationFn: (next: boolean) =>
      audiobooks.patch(audiobookId, { multi_voice_enabled: next }),
    onSuccess: onUpdated,
  });

  const setRole = useMutation({
    mutationFn: (next: Record<string, string>) =>
      audiobooks.patch(audiobookId, { voice_roles: next }),
    onSuccess: onUpdated,
  });

  const onPickRole = (roleId: string, voiceId: string | null): void => {
    const next = { ...roles };
    if (voiceId) {
      next[roleId] = voiceId;
    } else {
      delete next[roleId];
    }
    setRole.mutate(next);
  };

  return (
    <details
      open={open}
      onToggle={(e) => setOpen((e.currentTarget as HTMLDetailsElement).open)}
      className="mb-4 overflow-hidden rounded-lg border border-slate-800 bg-slate-900/40"
    >
      <summary className="flex cursor-pointer select-none items-center gap-2 px-4 py-2.5 text-sm text-slate-200 hover:bg-slate-900/70">
        <span aria-hidden="true" className="text-xs text-slate-500">
          {open ? "▾" : "▸"}
        </span>
        <span className="font-medium">Multi-voice narration</span>
        {enabled ? (
          <span className="rounded-full border border-emerald-700 bg-emerald-950/40 px-2 py-0.5 text-[11px] uppercase tracking-wide text-emerald-200">
            on
          </span>
        ) : (
          <span className="rounded-full border border-slate-700 bg-slate-950 px-2 py-0.5 text-[11px] uppercase tracking-wide text-slate-400">
            off
          </span>
        )}
      </summary>
      <div className="space-y-4 border-t border-slate-800 px-4 py-4">
        <p className="text-xs text-slate-400">
          When on, the next narration runs an extra LLM pass to split prose
          by speaker, then renders each segment with the role's mapped
          voice. Re-narrate the audiobook after toggling or changing voices
          for the new mapping to take effect.
        </p>
        <label className="inline-flex cursor-pointer items-center gap-2 text-sm text-slate-200">
          <input
            type="checkbox"
            checked={enabled}
            onChange={(e) => toggle.mutate(e.target.checked)}
            disabled={toggle.isPending}
            className="h-4 w-4 accent-sky-500"
          />
          Enable multi-voice narration
        </label>
        {enabled && (
          <div className="space-y-3">
            {MULTI_VOICE_ROLES.map((r) => (
              <RoleVoicePicker
                key={r.id}
                role={r}
                value={roles[r.id] ?? null}
                voices={voices}
                onChange={(v) => onPickRole(r.id, v)}
                saving={setRole.isPending}
              />
            ))}
            {(toggle.error || setRole.error) && (
              <p className="text-xs text-rose-400">
                {(toggle.error ?? setRole.error) instanceof ApiError
                  ? ((toggle.error ?? setRole.error) as ApiError).message
                  : "Could not save"}
              </p>
            )}
          </div>
        )}
      </div>
    </details>
  );
}

function RoleVoicePicker({
  role,
  value,
  voices,
  onChange,
  saving,
}: {
  role: { id: string; label: string; hint: string };
  value: string | null;
  voices: Voice[];
  onChange: (voiceId: string | null) => void;
  saving: boolean;
}): JSX.Element {
  const genderHint =
    role.id === "dialogue_male"
      ? "male"
      : role.id === "dialogue_female"
        ? "female"
        : null;
  return (
    <div className="rounded-md border border-slate-800 bg-slate-950/40 p-3">
      <div className="flex items-baseline justify-between gap-2">
        <div>
          <p className="text-sm font-medium text-slate-100">{role.label}</p>
          <p className="text-[11px] text-slate-500">{role.hint}</p>
        </div>
        <select
          value={value ?? ""}
          onChange={(e) => onChange(e.target.value || null)}
          disabled={saving}
          className="rounded-md border border-slate-700 bg-slate-950 px-2 py-1.5 text-sm text-slate-100 outline-none focus:border-sky-600 disabled:opacity-50"
        >
          <option value="">— Use narrator —</option>
          {voices.map((v) => {
            const match = genderHint && v.gender === genderHint;
            return (
              <option key={v.id} value={v.id}>
                {v.name}
                {v.gender ? ` (${v.gender})` : ""}
                {match ? " ✓" : ""}
              </option>
            );
          })}
        </select>
      </div>
    </div>
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
  const preview = useVoicePreview();

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
              previewState={preview.stateFor(v.id)}
              onPreview={() => preview.toggle(v.id)}
            />
          ))}
        </div>
        {preview.error && (
          <p className="mt-3 text-xs text-rose-400">{preview.error}</p>
        )}
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
  previewState,
  onPreview,
}: {
  active: boolean;
  onSelect: () => void;
  title: string;
  subtitle: string;
  previewState?: VoicePreviewState;
  onPreview?: () => void;
}): JSX.Element {
  return (
    <div
      className={`relative flex flex-col items-start gap-0.5 rounded-md border px-3 py-2 pr-9 text-left ${
        active
          ? "border-sky-600 bg-sky-600/10"
          : "border-slate-700 bg-slate-950 hover:border-slate-600"
      }`}
    >
      <button
        type="button"
        onClick={onSelect}
        className="absolute inset-0 rounded-md focus:outline-none focus:ring-1 focus:ring-sky-500"
        aria-label={`Select ${title}`}
      />
      <span
        className={`relative text-sm font-medium ${
          active ? "text-sky-200" : "text-slate-100"
        }`}
      >
        {title}
      </span>
      <span className="relative text-[11px] capitalize text-slate-400">
        {subtitle}
      </span>
      {onPreview && previewState && (
        <VoicePreviewButton state={previewState} onToggle={onPreview} />
      )}
    </div>
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

// Native <select> popups render via OS chrome — Tailwind on the
// <select> doesn't reach the dropdown items. Set the colours on each
// <option> so the popup reads against the dark theme on every platform.
const OPTION_CLS = "bg-slate-900 text-slate-100";

function CategoryPicker({
  current,
  options,
  onChange,
  saving,
}: {
  current: string | null;
  options: string[];
  onChange: (next: string) => void;
  saving: boolean;
}): JSX.Element {
  // Inline native <select> rendered as a pill so it sits next to the
  // other metadata badges. Empty value means "Uncategorized" (clears
  // the field server-side via empty-string semantics on patch).
  return (
    <label
      title="Change category"
      className="inline-flex items-center gap-1 rounded-full border border-slate-800 bg-slate-900/40 px-2 py-0.5 text-[11px] hover:border-slate-700"
    >
      <span aria-hidden>🏷</span>
      <select
        value={current ?? ""}
        onChange={(e) => onChange(e.target.value)}
        disabled={saving}
        className="bg-transparent text-slate-200 outline-none [appearance:none]"
      >
        <option value="" className={OPTION_CLS}>
          Uncategorized
        </option>
        {/* If the current category isn't in the curated list anymore
            (e.g. it was just deleted), still show it so the user knows
            what they're sitting on. */}
        {current && !options.includes(current) && (
          <option value={current} className={OPTION_CLS}>
            {current} (legacy)
          </option>
        )}
        {options.map((c) => (
          <option key={c} value={c} className={OPTION_CLS}>
            {c}
          </option>
        ))}
      </select>
    </label>
  );
}

function PodcastPicker({
  current,
  options,
  onChange,
  saving,
}: {
  current: string | null;
  options: PodcastRow[];
  onChange: (next: string) => void;
  saving: boolean;
}): JSX.Element {
  // Mirror CategoryPicker — empty value means "no podcast" and the
  // patch endpoint maps an empty string to a NONE update server-side.
  const knownIds = new Set(options.map((p) => p.id));
  return (
    <label
      title="Assign to podcast"
      className="inline-flex items-center gap-1 rounded-full border border-slate-800 bg-slate-900/40 px-2 py-0.5 text-[11px] hover:border-slate-700"
    >
      <span aria-hidden>🎙</span>
      <select
        value={current ?? ""}
        onChange={(e) => onChange(e.target.value)}
        disabled={saving}
        className="bg-transparent text-slate-200 outline-none [appearance:none]"
      >
        <option value="" className={OPTION_CLS}>
          No podcast
        </option>
        {/* Surface a dangling reference (e.g. the podcast was deleted)
            so the user can still see + clear it. */}
        {current && !knownIds.has(current) && (
          <option value={current} className={OPTION_CLS}>
            (deleted podcast)
          </option>
        )}
        {options.map((p) => (
          <option key={p.id} value={p.id} className={OPTION_CLS}>
            {p.title}
          </option>
        ))}
      </select>
    </label>
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
  // Animation: the per-paragraph visual classifier (`paragraph_visual`,
  // decides whether a paragraph gets a Manim render) and the per-paragraph
  // Manim Python codegen (`manim_code`). Both are LLM calls that show up
  // in `generation_event` and bucket separately from text/image/narration.
  { key: "animation", label: "Animation", icon: "🎬", roles: ["paragraph_visual", "manim_code"] },
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
      <ul className="space-y-2 text-xs text-slate-400">
        {bucket.rows.map((r) => {
          // The backend now groups by (role, llm), so the same role can
          // appear twice when an admin swapped models mid-build. Make the
          // key composite to keep React happy in that case.
          const key = `${r.role}::${r.llm_id ?? ""}`;
          // Pick a sensible display label: real llm_name when we have it,
          // raw `llm_id` when the row was deleted, "fallback" when the
          // event used the env-configured default. TTS rows carry the
          // voice id in `llm_name` (see CostByRole comment).
          let modelLabel: string | null;
          if (r.llm_name) {
            modelLabel = r.llm_name;
          } else if (r.llm_id === "_default_") {
            modelLabel = "fallback model";
          } else if (r.llm_id) {
            modelLabel = r.llm_id;
          } else {
            modelLabel = null;
          }
          return (
            <li key={key} className="space-y-0.5">
              <div className="flex items-baseline justify-between gap-3 tabular-nums">
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
              </div>
              {modelLabel && (
                <div
                  className="pl-3 text-[11px] text-slate-500"
                  title={r.model_id ?? undefined}
                >
                  ↳ {modelLabel}
                  {r.model_id && r.model_id !== modelLabel && (
                    <span className="ml-1 font-mono text-slate-600">
                      ({r.model_id})
                    </span>
                  )}
                </div>
              )}
            </li>
          );
        })}
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

function ParentJobRow({
  job,
  stage,
}: {
  job: JobSnapshot;
  /**
   * Live `stage` string from the WebSocket for this job's id, e.g.
   * `"narrating with Eve"`. Empty/null when no progress event has
   * arrived yet (or after a terminal event clears it).
   */
  stage: string | null;
}): JSX.Element {
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
  // Only surface the stage line while the job is actively running —
  // queued/throttled jobs haven't picked up a worker yet, and finished
  // jobs already say "completed" in the right-aligned label.
  const showStage = job.status === "running" && stage && stage.length > 0;
  return (
    <div className="rounded-lg border border-slate-800 bg-slate-900/40 p-3">
      <div className="flex items-center justify-between text-sm">
        <span className="font-medium capitalize text-slate-200">
          {title}
        </span>
        <span className="text-xs text-slate-400">{label}</span>
      </div>
      {showStage && (
        <p className="mt-1 text-[11px] text-slate-400">{stage}</p>
      )}
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
        <div className="mt-2 flex items-start gap-2">
          <p className="flex-1 text-xs text-rose-400 break-all">{job.last_error}</p>
          <CopyButton
            text={job.last_error}
            title="Copy error message"
            className="shrink-0"
          />
        </div>
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
  jobStages,
  pendingKind,
  publications,
}: {
  audiobookId: string;
  accessToken: string;
  parentJobs: JobSnapshot[];
  /**
   * Live per-job `stage` strings from the WebSocket. Threaded through
   * to `ParentJobRow` so e.g. narration rows can show "narrating with
   * Eve" — see `chapter_voice_summary` on the backend.
   */
  jobStages: Record<string, string>;
  pendingKind: "chapters" | "tts" | null;
  publications: PublicationRow[];
}): JSX.Element | null {
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
    const failed = parentJobs.some(
      (j) => j.status === "failed" || j.status === "dead",
    );
    return running || previewReady || failed;
  });
  // Completed jobs are noisy once they pile up — hide them by default
  // and let the user opt back in with the toggle.
  const [showCompleted, setShowCompleted] = useState(false);

  const hasJobs = parentJobs.length > 0 || pendingKind !== null;
  const hasPubs = publications.length > 0;
  if (!hasJobs && !hasPubs) return null;

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
  const completedCount = parentJobs.filter(
    (j) => j.status === "completed",
  ).length;
  const visibleParentJobs = showCompleted
    ? parentJobs
    : parentJobs.filter((j) => j.status !== "completed");

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
              {parentJobs.length > 0 && completedCount > 0 && (
                <div className="flex items-center justify-end">
                  <label className="inline-flex cursor-pointer items-center gap-1.5 text-[11px] text-slate-400">
                    <input
                      type="checkbox"
                      checked={showCompleted}
                      onChange={(e) => setShowCompleted(e.target.checked)}
                      className="h-3.5 w-3.5 accent-sky-500"
                    />
                    Show completed ({completedCount})
                  </label>
                </div>
              )}
              {parentJobs.length > 0 ? (
                visibleParentJobs.length > 0 ? (
                  visibleParentJobs.map((j) => (
                    <ParentJobRow
                      key={j.id}
                      job={j}
                      stage={jobStages[j.id] ?? null}
                    />
                  ))
                ) : (
                  <p className="text-center text-xs text-slate-500">
                    All {completedCount} job{completedCount === 1 ? "" : "s"} completed.
                  </p>
                )
              ) : (
                pendingKind && <PendingJobRow kind={pendingKind} />
              )}
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
  isShort,
  animationsReady,
  onClose,
  onQueued,
}: {
  audiobookId: string;
  language: string;
  languageLabel: string;
  accountConnected: boolean;
  isShort: boolean;
  animationsReady: boolean;
  onClose: () => void;
  onQueued: () => void;
}): JSX.Element {
  const [privacy, setPrivacy] = useState<"private" | "unlisted" | "public">(
    "private",
  );
  const [mode, setMode] = useState<"single" | "playlist">("single");
  const [review, setReview] = useState(true);
  // Default to "animated" whenever the per-chapter MP4s are ready and
  // the book isn't a Short. Users who clicked through Animate + waited
  // for every chapter to render almost certainly want those visuals on
  // YouTube — making them remember to tick the box every time meant
  // animations silently got dropped on upload (the original report
  // that motivated this default). The dialog remounts on each open
  // so this initial value tracks the latest readiness state without
  // needing an effect to keep them in sync.
  const [animate, setAnimate] = useState(animationsReady && !isShort);
  const [description, setDescription] = useState("");
  // Tri-state for the like-and-subscribe overlay:
  //   "default" → don't send the field; backend inherits the global setting
  //   "on"      → force `true` on this publication
  //   "off"     → force `false` on this publication
  const [likeSubscribe, setLikeSubscribe] = useState<"default" | "on" | "off">(
    "default",
  );

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
        animate,
        description: description.trim() ? description : null,
        like_subscribe_overlay:
          likeSubscribe === "default"
            ? null
            : likeSubscribe === "on",
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

        <label
          className={
            "mt-3 flex items-start gap-2 rounded-md border bg-slate-900 p-3 " +
            (animationsReady && !isShort
              ? "cursor-pointer border-slate-800"
              : "cursor-not-allowed border-slate-900 opacity-50")
          }
          title={
            isShort
              ? "Shorts are 9:16 vertical and incompatible with the 16:9 chapter renders. Use a horizontal book."
              : !animationsReady
                ? "Click 🎬 Animate above and wait for every chapter to render first."
                : undefined
          }
        >
          <input
            type="checkbox"
            checked={animate}
            disabled={!animationsReady || isShort}
            onChange={(e) => setAnimate(e.target.checked)}
            className="mt-0.5 h-4 w-4 accent-violet-500 disabled:cursor-not-allowed"
          />
          <span className="min-w-0">
            <span className="block text-sm text-slate-100">
              Use animated video
            </span>
            <span className="block text-[11px] text-slate-400">
              {isShort
                ? "Not available for Shorts (9:16 vs the 16:9 chapter renders)."
                : animationsReady
                  ? "Concatenate the per-chapter animation MP4s instead of looping the cover image."
                  : "Run 🎬 Animate above first — this stays disabled until every chapter has been rendered."}
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

        <fieldset className="mt-4">
          <legend className="text-xs font-medium text-slate-300">
            Like &amp; Subscribe overlay
          </legend>
          <p className="mt-1 text-[11px] text-slate-500">
            Burn a centred call-to-action near the bottom of the frame
            for a few seconds early on and again before the end. Leave
            on <strong>Default</strong> to inherit the admin setting; pick{" "}
            <strong>On</strong> or <strong>Off</strong> to override it just
            for this video.
          </p>
          <div className="mt-2 grid grid-cols-3 gap-2">
            {(
              [
                { value: "default", label: "Default" },
                { value: "on", label: "On" },
                { value: "off", label: "Off" },
              ] as const
            ).map((opt) => (
              <label
                key={opt.value}
                className={`flex cursor-pointer items-center justify-center rounded-md border px-3 py-2 text-sm ${
                  likeSubscribe === opt.value
                    ? "border-rose-600 bg-rose-600/10 text-rose-200"
                    : "border-slate-700 bg-slate-950 text-slate-200 hover:border-slate-600"
                }`}
              >
                <input
                  type="radio"
                  name="like-subscribe"
                  value={opt.value}
                  checked={likeSubscribe === opt.value}
                  onChange={() => setLikeSubscribe(opt.value)}
                  className="sr-only"
                />
                {opt.label}
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

function AnimationSection({
  audiobookId,
  chapters,
  jobsByChapter,
  jobStages,
  parentRunning,
  theme,
  onThemeChange,
  accessToken,
  language,
  onRegenerateChapter,
  regeneratingChapter,
  stemDetected,
  stemOverride,
  isStem,
  onSetStemOverride,
  stemPending,
  onClassifyChapterVisuals,
  classifyingChapter,
  classifyError,
  onRegenerateManimCode,
  regeneratingManimCodeChapter,
  regenerateManimCodeError,
}: {
  audiobookId: string;
  chapters: ChapterSummary[];
  jobsByChapter: Map<number, JobSnapshot>;
  /** Latest live `stage` per job_id from the WebSocket. AnimationRow
   * uses it to render "rendering diagram 3/12" instead of just the
   * percentage. Empty/missing → row falls back to the percentage. */
  jobStages: Record<string, string>;
  parentRunning: boolean;
  theme: import("../api").AnimationTheme;
  onThemeChange: (t: import("../api").AnimationTheme) => void;
  accessToken: string;
  language: string;
  onRegenerateChapter: (n: number) => void;
  /** Chapter number currently being re-rendered, if any. Used to
   * disable other rows' buttons mid-mutation so the user can't
   * fire-and-forget a stack of overlapping re-renders. */
  regeneratingChapter: number | null;
  /** LLM verdict on STEM-ness; null until outline runs. */
  stemDetected: boolean | null;
  /** User override; null = trust detection. */
  stemOverride: boolean | null;
  /** Effective STEM flag the renderer uses. */
  isStem: boolean;
  /** Set the override. Pass `null` to clear (return to detection). */
  onSetStemOverride: (next: boolean | null) => void;
  stemPending: boolean;
  /** Phase G — re-run the per-paragraph visual classifier on a
   * chapter that has no diagrams yet. Backfill helper for books
   * generated before STEM was enabled. */
  onClassifyChapterVisuals: (n: number) => void;
  /** Chapter currently being classified, or null. */
  classifyingChapter: number | null;
  /** Last classify failure (chapter + message), or null. The row
   * for that chapter renders the message inline so silent 404 /
   * 400 failures show up. */
  classifyError: { chapter: number | null; message: string } | null;
  /** Phase H — kick the bespoke Manim code-gen LLM for one chapter's
   * `custom_manim` paragraphs. Visible per-row only when the chapter
   * actually has at least one custom_manim paragraph (the button
   * hides itself otherwise). */
  onRegenerateManimCode: (n: number) => void;
  regeneratingManimCodeChapter: number | null;
  regenerateManimCodeError: { chapter: number | null; message: string } | null;
}): JSX.Element {
  const themes: { id: import("../api").AnimationTheme; label: string; hint: string }[] = [
    { id: "library", label: "Library", hint: "Slate + amber. Default." },
    { id: "parchment", label: "Parchment", hint: "Warm cream + burnt orange." },
    { id: "minimal", label: "Minimal", hint: "Editorial mono, sans-serif." },
  ];
  const allReady = chapters.every((ch) => {
    const j = jobsByChapter.get(ch.number);
    return j?.status === "completed";
  });
  return (
    <details
      open={parentRunning || !allReady}
      className="mb-6 rounded-lg border border-slate-800 bg-slate-900/30 p-4"
    >
      <summary className="cursor-pointer text-sm font-semibold text-slate-200">
        🎬 Animation{" "}
        <span className="ml-1 text-xs font-normal text-slate-500">
          ({chapters.length} chapter{chapters.length === 1 ? "" : "s"} · theme: {theme})
        </span>
      </summary>
      <div className="mt-3 flex flex-wrap items-center gap-2 text-xs text-slate-400">
        <span>Theme:</span>
        {themes.map((t) => (
          <button
            key={t.id}
            type="button"
            onClick={() => onThemeChange(t.id)}
            disabled={parentRunning}
            title={t.hint}
            className={
              "rounded-md border px-2 py-1 disabled:cursor-not-allowed disabled:opacity-40 " +
              (theme === t.id
                ? "border-violet-700 bg-violet-950/50 text-violet-100"
                : "border-slate-700 bg-slate-900 text-slate-300 hover:border-slate-600")
            }
          >
            {t.label}
          </button>
        ))}
        <span className="ml-auto text-slate-500">
          {allReady
            ? "All chapters rendered."
            : parentRunning
              ? "Rendering…"
              : "Click Animate above to render."}
        </span>
      </div>
      <StemToggle
        detected={stemDetected}
        override={stemOverride}
        effective={isStem}
        onChange={onSetStemOverride}
        pending={stemPending}
        disabled={parentRunning}
      />
      <ol className="mt-3 space-y-1.5">
        {chapters.map((ch) => (
          <AnimationRow
            key={ch.id}
            audiobookId={audiobookId}
            chapter={ch}
            job={jobsByChapter.get(ch.number)}
            liveStage={(() => {
              const j = jobsByChapter.get(ch.number);
              return j ? jobStages[j.id] ?? null : null;
            })()}
            accessToken={accessToken}
            language={language}
            onRegenerate={() => onRegenerateChapter(ch.number)}
            regenerating={regeneratingChapter === ch.number}
            anyRegenerating={regeneratingChapter !== null}
            isStem={isStem}
            onClassifyVisuals={() => onClassifyChapterVisuals(ch.number)}
            classifying={classifyingChapter === ch.number}
            classifyErrorMessage={
              classifyError && classifyError.chapter === ch.number
                ? classifyError.message
                : null
            }
            onRegenerateManimCode={() => onRegenerateManimCode(ch.number)}
            regeneratingManimCode={regeneratingManimCodeChapter === ch.number}
            regenerateManimCodeErrorMessage={
              regenerateManimCodeError &&
              regenerateManimCodeError.chapter === ch.number
                ? regenerateManimCodeError.message
                : null
            }
          />
        ))}
      </ol>
    </details>
  );
}

/**
 * Three-state STEM control. The user picks one of:
 *   * Auto       — defer to the LLM verdict (override = null)
 *   * STEM       — force STEM mode (override = true)
 *   * Not STEM   — force off (override = false)
 *
 * Effective rendering uses `effective`; the chip alongside Auto shows
 * what the LLM actually said so a user can sanity-check it before
 * overriding.
 */
function StemToggle({
  detected,
  override,
  effective,
  onChange,
  pending,
  disabled,
}: {
  detected: boolean | null;
  override: boolean | null;
  effective: boolean;
  onChange: (next: boolean | null) => void;
  pending: boolean;
  disabled: boolean;
}): JSX.Element {
  type Choice = { id: "auto" | "stem" | "non_stem"; label: string; hint: string };
  const choices: Choice[] = [
    {
      id: "auto",
      label:
        detected == null
          ? "Auto"
          : detected
            ? "Auto (STEM)"
            : "Auto (Not STEM)",
      hint:
        detected == null
          ? "Use the LLM verdict — runs on next outline."
          : detected
            ? "LLM thinks this is STEM — diagram path will activate."
            : "LLM thinks this is non-STEM — prose path only.",
    },
    {
      id: "stem",
      label: "STEM",
      hint: "Force STEM mode — Phase G's Manim diagram path will run.",
    },
    {
      id: "non_stem",
      label: "Not STEM",
      hint: "Force prose-only — skip the diagram path even if the LLM said yes.",
    },
  ];
  const active: Choice["id"] =
    override == null ? "auto" : override ? "stem" : "non_stem";
  const apply = (id: Choice["id"]) => {
    const next = id === "auto" ? null : id === "stem";
    onChange(next);
  };
  return (
    <div className="mt-2 flex flex-wrap items-center gap-2 text-xs text-slate-400">
      <span>STEM:</span>
      {choices.map((c) => (
        <button
          key={c.id}
          type="button"
          onClick={() => apply(c.id)}
          disabled={disabled || pending || active === c.id}
          title={c.hint}
          className={
            "rounded-md border px-2 py-1 disabled:cursor-not-allowed " +
            (active === c.id
              ? "border-emerald-700 bg-emerald-950/40 text-emerald-100 disabled:opacity-100"
              : "border-slate-700 bg-slate-900 text-slate-300 hover:border-slate-600 disabled:opacity-40")
          }
        >
          {c.label}
        </button>
      ))}
      <span className="ml-auto text-slate-500">
        Effective: {effective ? "STEM (diagram path)" : "Prose only"}
      </span>
    </div>
  );
}

function AnimationRow({
  audiobookId,
  chapter,
  job,
  liveStage,
  accessToken,
  language,
  onRegenerate,
  regenerating,
  anyRegenerating,
  isStem,
  onClassifyVisuals,
  classifying,
  classifyErrorMessage,
  onRegenerateManimCode,
  regeneratingManimCode,
  regenerateManimCodeErrorMessage,
}: {
  audiobookId: string;
  chapter: ChapterSummary;
  job: JobSnapshot | undefined;
  /** Latest WebSocket `stage` for this row's job_id (null when none
   * received yet, or after a terminal event clears it). Strings like
   * `rendering diagram 3/12` come from segments::render_chapter via
   * the publisher; we show them while running so the user can see
   * *what* is taking time, not just the bare percentage. */
  liveStage: string | null;
  accessToken: string;
  language: string;
  onRegenerate: () => void;
  /** This row's mutation is the one in flight. */
  regenerating: boolean;
  /** Some row's mutation is in flight (could be this row or another).
   * Used to disable the button so the user can't queue a stack of
   * overlapping re-renders before the first response lands. */
  anyRegenerating: boolean;
  /** Effective is_stem for the book — controls whether the
   * "Classify diagrams" button shows. */
  isStem: boolean;
  /** Backfill the diagram labels for this chapter. */
  onClassifyVisuals: () => void;
  /** This row's classify mutation is in flight. */
  classifying: boolean;
  /** Last classify failure for this chapter, if any. */
  classifyErrorMessage: string | null;
  /** Phase H — re-run the bespoke Manim code-gen LLM for this
   * chapter's `custom_manim` paragraphs. The button is hidden when
   * `customManimCount === 0`; the backend would 400 anyway, so
   * showing it would just lead the user into an error. */
  onRegenerateManimCode: () => void;
  regeneratingManimCode: boolean;
  regenerateManimCodeErrorMessage: string | null;
}): JSX.Element {
  const status = job?.status;
  const pct = Math.round(((job?.progress_pct ?? 0) as number) * 100);
  const ready = status === "completed";
  const inFlight = status === "queued" || status === "running";
  // While running, prefer the richer stage string from the WebSocket
  // ("rendering diagram 3/12") over the bare percentage. Fall back
  // to "Rendering N%" when the stage hasn't arrived yet — the very
  // first frame can land 100–200 ms after the job goes "running",
  // so the bare percent is the holdover during that window.
  const runningLabel =
    liveStage && liveStage.trim().length > 0
      ? `${liveStage} (${pct}%)`
      : `Rendering ${pct}%`;
  const label =
    status === "completed"
      ? "Ready"
      : status === "running"
        ? runningLabel
        : status === "queued"
          ? "Queued"
          : status === "dead" || status === "failed"
            ? "Failed"
            : "Not generated";
  const tone =
    status === "completed"
      ? "text-emerald-300"
      : status === "running"
        ? "text-sky-300"
        : status === "queued"
          ? "text-amber-300"
          : status === "dead" || status === "failed"
            ? "text-rose-300"
            : "text-slate-500";
  // Disable while: this row's mutation is mid-flight, another row's
  // mutation is mid-flight, or the chapter has a live job (queued or
  // running) — the backend would 409 in that last case.
  const buttonDisabled = regenerating || anyRegenerating || inFlight;
  // Phase G — diagrams labelled by the per-paragraph visual classifier
  // for STEM books. Surface the count as a small badge so the user
  // sees at a glance which chapters will get Manim diagrams once that
  // path lands (G.5–G.6).
  const diagramCount = (chapter.paragraphs ?? []).filter(
    (p) => typeof p.visual_kind === "string" && p.visual_kind.trim() !== "",
  ).length;
  // Phase H — separate count for the `custom_manim` paragraphs the
  // bespoke code-gen LLM handles. Drives the "Regen Manim code"
  // button visibility: the backend 400s when this is zero, so we
  // hide it client-side rather than letting the user hit the dead
  // button.
  const customManimCount = (chapter.paragraphs ?? []).filter(
    (p) => p.visual_kind === "custom_manim",
  ).length;
  // Local state for the "Test LLM" dialog. Owned by the row so each
  // chapter has its own dialog instance — opening one doesn't dirty
  // siblings' picker selections.
  const [testOpen, setTestOpen] = useState(false);
  const paragraphsForTest = chapter.paragraphs ?? [];
  return (
    <li className="rounded-md border border-slate-800/60 bg-slate-900/40 px-3 py-2 text-sm">
      <div className="flex items-center gap-3">
        <span className="font-mono text-xs text-slate-500">
          ch{chapter.number.toString().padStart(2, "0")}
        </span>
        <span className="flex-1 truncate text-slate-200">{chapter.title}</span>
        {diagramCount > 0 && (
          <span
            className="rounded border border-emerald-800 bg-emerald-950/40 px-1.5 py-0.5 text-[10px] font-medium text-emerald-300"
            title={`${diagramCount} paragraph${
              diagramCount === 1 ? "" : "s"
            } will render as a diagram via Manim (Phase G)`}
          >
            📐 {diagramCount}
          </span>
        )}
        <span className={`text-xs ${tone}`}>{label}</span>
        {ready && diagramCount > 0 && (
          <span
            className="rounded border border-violet-800 bg-violet-950/40 px-1.5 py-0.5 text-[10px] font-medium text-violet-200"
            title={
              `${diagramCount} diagram${diagramCount === 1 ? "" : "s"} ` +
              `rendered with Manim (per-segment STEM render). ` +
              `Prose paragraphs went through the fast path; the chapter video ` +
              `is the concat of both.`
            }
          >
            🎬 Manim
          </span>
        )}
        {isStem && diagramCount === 0 && (
          <button
            type="button"
            onClick={onClassifyVisuals}
            disabled={classifying}
            title={
              "Run the visual classifier against this chapter's existing " +
              "paragraphs (backfill for books generated before STEM was on). " +
              "Doesn't rewrite the chapter body."
            }
            className="rounded-md border border-emerald-800 bg-emerald-950/40 px-2 py-1 text-xs text-emerald-200 hover:border-emerald-700 disabled:cursor-not-allowed disabled:opacity-40"
          >
            {classifying ? "Classifying…" : "📐 Classify diagrams"}
          </button>
        )}
        {isStem && paragraphsForTest.length > 0 && (
          <button
            type="button"
            onClick={() => setTestOpen(true)}
            title={
              "Audition any enabled LLM on this chapter's Manim code-gen prompt — " +
              "shows the rendered prompt, the model's raw response, cost, and " +
              "wall-clock time. Doesn't write anything back."
            }
            className="rounded-md border border-amber-800 bg-amber-950/40 px-2 py-1 text-xs text-amber-200 hover:border-amber-700"
          >
            🧪 Test LLM
          </button>
        )}
        {customManimCount > 0 && (
          <button
            type="button"
            onClick={onRegenerateManimCode}
            disabled={regeneratingManimCode}
            title={
              `Re-run the bespoke Manim code-gen LLM on this chapter's ${customManimCount} ` +
              `custom_manim paragraph${customManimCount === 1 ? "" : "s"}. ` +
              `Routed through the LlmRole::ManimCode model (set in Admin → LLMs); ` +
              `regenerate after switching that assignment to refresh diagrams ` +
              `with the new model.`
            }
            className="rounded-md border border-violet-800 bg-violet-950/40 px-2 py-1 text-xs text-violet-200 hover:border-violet-700 disabled:cursor-not-allowed disabled:opacity-40"
          >
            {regeneratingManimCode ? "Generating…" : `🛠 Regen Manim code (${customManimCount})`}
          </button>
        )}
        <button
          type="button"
          onClick={onRegenerate}
          disabled={buttonDisabled}
          title={
            inFlight
              ? "Already rendering — wait for current job to finish"
              : "Re-render this chapter only"
          }
          className="rounded-md border border-slate-700 bg-slate-900 px-2 py-1 text-xs text-slate-300 hover:border-slate-600 disabled:cursor-not-allowed disabled:opacity-40"
        >
          {regenerating ? "Queuing…" : "Re-generate"}
        </button>
      </div>
      {status === "running" && (
        <div className="mt-2 h-1 w-full overflow-hidden rounded-full bg-slate-800">
          <div
            className="h-full bg-sky-500 transition-[width] duration-200"
            style={{ width: `${pct}%` }}
          />
        </div>
      )}
      {job?.last_error && status !== "completed" && (
        <p className="mt-1 break-all text-xs text-rose-400">{job.last_error}</p>
      )}
      {classifyErrorMessage && (
        <p className="mt-1 break-all text-xs text-rose-400">
          Classify failed: {classifyErrorMessage}
        </p>
      )}
      {regenerateManimCodeErrorMessage && (
        <p className="mt-1 break-all text-xs text-rose-400">
          Regen Manim code failed: {regenerateManimCodeErrorMessage}
        </p>
      )}
      {ready && (
        <video
          src={`${chapterVideoUrl(audiobookId, chapter.number, accessToken, language)}&v=${job?.id ?? "ready"}`}
          controls
          preload="metadata"
          title={
            diagramCount > 0
              ? `Rendered via Manim diagram path · ${diagramCount} diagram${diagramCount === 1 ? "" : "s"}`
              : undefined
          }
          className="mt-2 w-full rounded border border-slate-800 bg-black"
        />
      )}
      {testOpen && (
        <TestManimLlmDialog
          audiobookId={audiobookId}
          chapter={chapter}
          paragraphs={paragraphsForTest}
          onClose={() => setTestOpen(false)}
        />
      )}
    </li>
  );
}

/**
 * Dialog for auditioning a non-default LLM on the chapter's Manim
 * code-gen prompt. Pure read-only: the response never lands on the
 * chapter row, and no `generation_event` is logged. The prompt body
 * comes back from the backend already rendered (markers substituted)
 * so the user sees exactly what the model received.
 */
function TestManimLlmDialog({
  audiobookId,
  chapter,
  paragraphs,
  onClose,
}: {
  audiobookId: string;
  chapter: ChapterSummary;
  paragraphs: NonNullable<ChapterSummary["paragraphs"]>;
  onClose: () => void;
}): JSX.Element {
  // LLM picker: only enabled, text-capable rows from the user-facing
  // catalog. Image/audio models would error on the chat completions
  // path, so filtering them here saves a confusing 400.
  const llmsQuery = useQuery({
    queryKey: ["catalog", "llms"],
    queryFn: () => catalog.llms(),
  });
  const eligibleLlms: Llm[] = useMemo(() => {
    const items = llmsQuery.data?.items ?? [];
    return items.filter((l) => {
      if (!l.enabled) return false;
      const fn = (l.function ?? "text").toLowerCase();
      return fn === "text" || fn === "multimodal" || fn === "";
    });
  }, [llmsQuery.data]);

  // Sensible default paragraph: first custom_manim, else paragraph 0.
  const defaultIndex = useMemo(() => {
    const cm = paragraphs.find((p) => p.visual_kind === "custom_manim");
    return cm?.index ?? paragraphs[0]?.index ?? 0;
  }, [paragraphs]);

  const [llmId, setLlmId] = useState<string>("");
  const [paragraphIndex, setParagraphIndex] = useState<number>(defaultIndex);

  // Auto-select the first eligible LLM once the catalog loads, but
  // only if the user hasn't already picked something.
  useEffect(() => {
    if (!llmId && eligibleLlms.length > 0) {
      // Prefer one already tagged for manim_code so the dropdown
      // doesn't always default to whatever's alphabetically first.
      const tagged = eligibleLlms.find((l) =>
        l.default_for.includes("manim_code"),
      );
      setLlmId(tagged?.id ?? eligibleLlms[0].id);
    }
  }, [eligibleLlms, llmId]);

  useEffect(() => {
    const onKey = (e: KeyboardEvent): void => {
      if (e.key === "Escape") onClose();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [onClose]);

  const accessToken = useAuth((s) => s.accessToken) ?? "";

  const run = useMutation({
    mutationFn: () =>
      audiobooks.testChapterManimLlm(audiobookId, chapter.number, {
        llm_id: llmId,
        paragraph_index: paragraphIndex,
      }),
    onSuccess: (data) => {
      // Auto-trigger the render the moment we have a response — the
      // user explicitly asked for the preview to follow the LLM call,
      // so don't make them click twice. `parseCodeFromResponse`
      // returns null when the model emitted non-JSON or empty code;
      // in that case the render UI surfaces a parse error instead of
      // silently doing nothing.
      const code = parseCodeFromResponse(data.response);
      if (code) {
        render.mutate(code);
      }
    },
  });
  const render = useMutation({
    mutationFn: (code: string) =>
      audiobooks.renderTestManim(audiobookId, chapter.number, { code }),
  });
  const result = run.data;
  const renderResult = render.data;
  // Bust the `<video>` cache when a fresh render lands so the same
  // dialog instance picks up the new MP4 instead of the previous one.
  const videoUrl =
    renderResult && accessToken
      ? `${audiobookTestManimVideoUrl(audiobookId, renderResult.test_id, accessToken)}&v=${renderResult.test_id}`
      : null;
  const parsedCode = result ? parseCodeFromResponse(result.response) : null;

  return (
    <div
      className="fixed inset-0 z-50 flex items-center justify-center bg-black/60 p-4"
      onClick={onClose}
    >
      <div
        onClick={(e) => e.stopPropagation()}
        className="w-full max-w-3xl rounded-xl border border-slate-800 bg-slate-950 p-5 shadow-xl"
      >
        <div className="mb-4 flex items-baseline justify-between gap-3">
          <div>
            <h2 className="text-base font-semibold text-slate-100">
              🧪 Test Manim LLM
            </h2>
            <p className="mt-0.5 text-xs text-slate-400">
              Chapter {chapter.number}: {chapter.title}
            </p>
          </div>
          <button
            type="button"
            onClick={onClose}
            className="rounded-md border border-slate-800 bg-slate-900 px-2 py-1 text-xs text-slate-300 hover:border-slate-700"
          >
            Close
          </button>
        </div>

        <div className="grid gap-3 sm:grid-cols-[1fr,auto]">
          <label className="block text-xs font-medium text-slate-300">
            <span className="block">Model</span>
            <select
              value={llmId}
              onChange={(e) => setLlmId(e.target.value)}
              disabled={llmsQuery.isLoading || eligibleLlms.length === 0}
              className="mt-1 w-full rounded-md border border-slate-700 bg-slate-900 px-3 py-2 text-sm text-slate-100"
            >
              {eligibleLlms.length === 0 && (
                <option value="">No text-capable LLMs configured</option>
              )}
              {eligibleLlms.map((l) => {
                const tag = l.default_for.includes("manim_code")
                  ? " ★"
                  : "";
                return (
                  <option key={l.id} value={l.id}>
                    {l.name}{tag} — {l.model_id}
                  </option>
                );
              })}
            </select>
          </label>
          <label className="block text-xs font-medium text-slate-300">
            <span className="block">Paragraph</span>
            <select
              value={paragraphIndex}
              onChange={(e) => setParagraphIndex(Number(e.target.value))}
              className="mt-1 w-full rounded-md border border-slate-700 bg-slate-900 px-3 py-2 text-sm text-slate-100"
            >
              {paragraphs.map((p) => {
                const kind = p.visual_kind ? ` · ${p.visual_kind}` : "";
                return (
                  <option key={p.index} value={p.index}>
                    {p.index}
                    {kind}
                  </option>
                );
              })}
            </select>
          </label>
        </div>

        <div className="mt-3 flex items-center justify-between gap-3">
          <p className="text-[11px] text-slate-500">
            ★ = currently routed for{" "}
            <code className="font-mono">manim_code</code> in Admin → LLMs.
          </p>
          <button
            type="button"
            onClick={() => run.mutate()}
            disabled={!llmId || run.isPending}
            className="rounded-md bg-amber-600 px-4 py-2 text-sm font-medium text-white hover:bg-amber-500 disabled:cursor-not-allowed disabled:bg-amber-700/50"
          >
            {run.isPending ? "Running…" : "Run"}
          </button>
        </div>

        {run.error && (
          <p className="mt-3 break-all text-xs text-rose-400">
            {run.error instanceof ApiError
              ? run.error.message
              : "Test failed"}
          </p>
        )}

        {result && (
          <div className="mt-4 space-y-3 text-xs text-slate-300">
            <div className="flex flex-wrap gap-2 text-[11px]">
              <MetricPill label={`⏱ ${formatElapsed(result.elapsed_ms)}`} />
              <MetricPill
                label={`💸 ${result.mocked ? "mocked" : formatCost(result.cost_usd)}`}
              />
              <MetricPill
                label={`🔢 ${result.prompt_tokens.toLocaleString()} in / ${result.completion_tokens.toLocaleString()} out`}
              />
              <MetricPill label={`🤖 ${result.llm_name}`} />
              <MetricPill label={`📐 paragraph ${result.paragraph_index}`} />
            </div>
            <details className="rounded border border-slate-800 bg-slate-900/40 p-2" open>
              <summary className="cursor-pointer text-[11px] font-medium text-slate-400">
                Paragraph (input)
              </summary>
              <p className="mt-1 whitespace-pre-wrap text-slate-300">
                {result.paragraph_preview}
                {result.paragraph_preview.length === 200 && "…"}
              </p>
            </details>
            <details className="rounded border border-slate-800 bg-slate-900/40 p-2">
              <summary className="cursor-pointer text-[11px] font-medium text-slate-400">
                Rendered prompt sent to the LLM
              </summary>
              <pre className="mt-1 max-h-72 overflow-auto whitespace-pre-wrap break-all rounded bg-slate-950 p-2 font-mono text-[11px] text-slate-300">
                {result.prompt}
              </pre>
            </details>
            <details
              className="rounded border border-slate-800 bg-slate-900/40 p-2"
              open
            >
              <summary className="cursor-pointer text-[11px] font-medium text-slate-400">
                Model response
              </summary>
              <pre className="mt-1 max-h-96 overflow-auto whitespace-pre-wrap break-all rounded bg-slate-950 p-2 font-mono text-[11px] text-slate-100">
                {result.response || "(empty)"}
              </pre>
            </details>

            {/* Animation preview. Auto-renders the moment the LLM
                response lands; the user can also re-render manually
                if the first attempt failed (e.g. transient sidecar
                hiccup) without re-running the LLM. */}
            <section className="rounded border border-slate-800 bg-slate-900/40 p-2">
              <header className="mb-1 flex items-baseline justify-between gap-3">
                <span className="text-[11px] font-medium text-slate-400">
                  Animation preview
                </span>
                <div className="flex items-center gap-2 text-[11px] text-slate-500">
                  {renderResult && (
                    <span>
                      Rendered in {formatElapsed(renderResult.elapsed_ms)}
                    </span>
                  )}
                  {parsedCode && (
                    <button
                      type="button"
                      onClick={() => render.mutate(parsedCode)}
                      disabled={render.isPending}
                      className="rounded border border-slate-700 bg-slate-900 px-2 py-0.5 text-slate-200 hover:border-slate-600 disabled:cursor-not-allowed disabled:opacity-40"
                    >
                      {render.isPending
                        ? "Rendering…"
                        : renderResult
                          ? "Re-render"
                          : "Render"}
                    </button>
                  )}
                </div>
              </header>
              {!parsedCode && (
                <p className="text-[11px] text-amber-400">
                  Could not extract <code className="font-mono">code</code>{" "}
                  from the response — the model didn't return the expected{" "}
                  <code className="font-mono">{`{"summary":"…","code":"…"}`}</code>{" "}
                  shape, so there's nothing to render.
                </p>
              )}
              {parsedCode && render.isPending && (
                <p className="text-[11px] text-slate-400">
                  Rendering with the Manim sidecar… (typically 5–15 s)
                </p>
              )}
              {render.error && (
                <p className="text-[11px] text-rose-400 break-all">
                  {render.error instanceof ApiError
                    ? render.error.message
                    : "Render failed"}
                </p>
              )}
              {videoUrl && (
                <video
                  key={videoUrl}
                  src={videoUrl}
                  controls
                  preload="metadata"
                  className="mt-1 w-full rounded border border-slate-800 bg-black"
                />
              )}
            </section>
          </div>
        )}
      </div>
    </div>
  );
}

/**
 * Pull the `code` field out of a manim_code LLM response. Production
 * uses `json_mode: true` so the response should already be valid
 * JSON, but a few models wrap it in markdown fences anyway — strip
 * those before parsing. Returns `null` when the response is empty,
 * not parseable, or has no usable `code` value.
 */
function parseCodeFromResponse(raw: string): string | null {
  const trimmed = raw.trim();
  if (!trimmed) return null;
  // Strip a leading ```json / ``` and trailing ``` if the model
  // ignored the "no markdown fences" instruction.
  const fenced = trimmed
    .replace(/^```(?:json)?\s*/i, "")
    .replace(/```\s*$/, "");
  try {
    const parsed = JSON.parse(fenced);
    if (parsed && typeof parsed === "object" && "code" in parsed) {
      const code = (parsed as { code: unknown }).code;
      if (typeof code === "string" && code.trim().length > 0) {
        return code;
      }
    }
  } catch {
    // Not valid JSON — fall through.
  }
  return null;
}

function MetricPill({ label }: { label: string }): JSX.Element {
  return (
    <span className="rounded-full border border-slate-800 bg-slate-900/60 px-2 py-0.5 tabular-nums text-slate-300">
      {label}
    </span>
  );
}

function formatElapsed(ms: number): string {
  if (ms < 1000) return `${ms} ms`;
  return `${(ms / 1000).toFixed(2)} s`;
}

function formatCost(usd: number): string {
  if (usd === 0) return "$0.00";
  if (usd < 0.001) return "<$0.001";
  if (usd < 0.01) return `$${usd.toFixed(4)}`;
  return `$${usd.toFixed(3)}`;
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
          <summary className="flex cursor-pointer items-center gap-2 text-xs text-sky-400 hover:text-sky-300">
            <span>Read prose ({Math.round(ch.body_md.length / 1024)} KB)</span>
            <CopyButton
              text={() => ch.body_md ?? ""}
              variant="labelled"
              title="Copy chapter prose to clipboard"
            />
          </summary>
          <pre className="mt-2 max-h-80 overflow-auto whitespace-pre-wrap rounded-md border border-slate-800 bg-slate-950 p-3 text-xs leading-relaxed text-slate-200">
            {ch.body_md}
          </pre>
        </details>
      )}
    </li>
  );
}
