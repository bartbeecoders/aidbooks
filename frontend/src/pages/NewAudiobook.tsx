import { FormEvent, useEffect, useMemo, useRef, useState } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { Link, useNavigate } from "react-router-dom";
import {
  audiobooks,
  catalog,
  coverArt,
  integrations,
  topics,
  ApiError,
} from "../api";
import type {
  AudiobookLength,
  AutoPipeline,
  TopicTemplate,
  Voice,
} from "../api";
import { ArtStyleSelect } from "../components/ArtStylePicker";
import { DEFAULT_ART_STYLE } from "../lib/art-styles";
import { imageCapableLlms } from "../lib/cover-llm";
import { Field, inputClass, primaryBtn } from "./Login";

const GENRE_PRESETS: { label: string; icon: string }[] = [
  { label: "Sci-fi", icon: "🛸" },
  { label: "Fantasy", icon: "🐉" },
  { label: "Mystery", icon: "🔍" },
  { label: "Thriller", icon: "🗡️" },
  { label: "Romance", icon: "💕" },
  { label: "Historical", icon: "🏰" },
  { label: "Horror", icon: "👻" },
  { label: "Adventure", icon: "🧭" },
  { label: "Comedy", icon: "🎭" },
  { label: "Drama", icon: "🎬" },
  { label: "Children", icon: "🧸" },
  { label: "Self-help", icon: "🌱" },
  { label: "Biography", icon: "📜" },
  { label: "Non-fiction", icon: "📚" },
];

function genreIcon(label: string): string {
  const match = GENRE_PRESETS.find(
    (g) => g.label.toLowerCase() === label.trim().toLowerCase(),
  );
  return match?.icon ?? "🏷️";
}

const VOICE_ICONS: Record<string, string> = {
  female: "👩",
  male: "👨",
  neutral: "🧑",
};

const LANGUAGES: { code: string; label: string; flag: string }[] = [
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

export function NewAudiobook(): JSX.Element {
  const qc = useQueryClient();
  const navigate = useNavigate();
  const [topic, setTopic] = useState("");
  const [length, setLength] = useState<AudiobookLength>("short");
  const [genre, setGenre] = useState("");
  const [category, setCategory] = useState("");
  const [language, setLanguage] = useState("en");
  const [voiceId, setVoiceId] = useState<string | null>(null);
  const [artStyle, setArtStyle] = useState<string>(DEFAULT_ART_STYLE);
  const [coverLlmId, setCoverLlmId] = useState<string>("");
  const [error, setError] = useState<string | null>(null);
  const [cover, setCover] = useState<{ base64: string; mime: string } | null>(null);
  // Auto-pipeline: chapters + cover + audio default ON. Publish defaults
  // OFF because it depends on the user having a YouTube channel connected.
  const [isShort, setIsShort] = useState(false);
  const [autoChapters, setAutoChapters] = useState(true);
  const [autoCover, setAutoCover] = useState(true);
  // Tiles per *visual* paragraph (the LLM extract pass picks visualizable
  // paragraphs first; this knob just controls how many tiles per pick).
  // 0 = chapter cover tiles only.
  const [imagesPerParagraph, setImagesPerParagraph] = useState<number>(0);
  const [autoAudio, setAutoAudio] = useState(true);
  const [autoPublish, setAutoPublish] = useState(false);
  const [publishMode, setPublishMode] = useState<"single" | "playlist">("single");
  const [publishPrivacy, setPublishPrivacy] =
    useState<"private" | "unlisted" | "public">("private");
  const [publishReview, setPublishReview] = useState(true);

  const voicesQuery = useQuery({
    queryKey: ["voices"],
    queryFn: () => catalog.voices(),
  });
  const templatesQuery = useQuery({
    queryKey: ["topic-templates"],
    queryFn: () => topics.templates(),
  });
  const llmsQuery = useQuery({
    queryKey: ["llms"],
    queryFn: () => catalog.llms(),
  });
  const youtubeAccount = useQuery({
    queryKey: ["integrations", "youtube", "account"],
    queryFn: () => integrations.youtube.account(),
  });
  const coverLlms = llmsQuery.data
    ? imageCapableLlms(llmsQuery.data.items)
    : [];

  // Categories live in an admin-curated table now. We pull them from
  // the public catalog endpoint; admins manage the list via
  // /admin/categories.
  const categoriesQuery = useQuery({
    queryKey: ["audiobook-categories"],
    queryFn: () => catalog.audiobookCategories(),
  });
  const categoryNames = useMemo(
    () => (categoriesQuery.data?.items ?? []).map((c) => c.name),
    [categoriesQuery.data],
  );
  const youtubeConnected = youtubeAccount.data?.connected ?? false;

  const applyTemplate = (t: TopicTemplate): void => {
    setTopic(t.topic);
    if (t.genre) setGenre(t.genre);
    if (t.length) setLength(t.length);
    if (t.language) setLanguage(t.language);
    setIsShort(t.is_short);
  };

  const generateCover = useMutation({
    mutationFn: () =>
      coverArt.preview({
        topic: topic.trim(),
        genre: genre.trim() || undefined,
        art_style: artStyle || undefined,
        llm_id: coverLlmId || undefined,
        is_short: isShort,
      }),
    onSuccess: (r) => setCover({ base64: r.image_base64, mime: r.mime_type }),
  });

  const surprise = useMutation({
    mutationFn: () => topics.random({ seed: null, language }),
    onSuccess: (r) => {
      setTopic(r.topic);
      setGenre(r.genre ?? "");
      setLength(r.length);
    },
  });

  // Highest-priority enabled LLM tagged `default_for: ["chapter"]` whose
  // languages list either includes the selected language or is empty
  // (= any). Mirrors `pick_llm_for_roles_lang(state, [Chapter], lang)` on
  // the backend so the user sees the same row that'll actually run. We
  // surface the *chapter* row because that's the model that does the bulk
  // of the work; outline/topic typically use the same default.
  const pickedTextLlm = (() => {
    const items = llmsQuery.data?.items ?? [];
    const candidates = items
      .filter(
        (l) =>
          l.default_for?.includes("chapter") &&
          (!l.languages?.length || l.languages.includes(language)),
      )
      .sort((a, b) => {
        const ap = a.priority ?? 100;
        const bp = b.priority ?? 100;
        if (ap !== bp) return ap - bp;
        return a.name.localeCompare(b.name);
      });
    return candidates[0] ?? null;
  })();

  const create = useMutation({
    mutationFn: () => {
      // Effective publish step is gated on the audio step + a connected
      // YouTube channel — UI mirrors the backend's `normalise_auto_pipeline`.
      const effectiveAudio = autoChapters && autoAudio;
      const effectivePublish =
        effectiveAudio && autoPublish && youtubeConnected;
      const auto_pipeline: AutoPipeline = {
        chapters: autoChapters,
        cover: autoCover,
        audio: effectiveAudio,
        publish: effectivePublish
          ? {
              mode: publishMode,
              privacy_status: publishPrivacy,
              review: publishReview,
            }
          : null,
      };
      return audiobooks.create({
        topic: topic.trim(),
        length,
        genre: genre.trim() || undefined,
        category: category.trim() || undefined,
        language,
        voice_id: voiceId ?? undefined,
        cover_image_base64: cover?.base64,
        art_style: artStyle || undefined,
        cover_llm_id: coverLlmId || undefined,
        // Shorts: single chapter, no paragraph slideshow.
        images_per_paragraph:
          !isShort && autoCover && imagesPerParagraph > 0
            ? imagesPerParagraph
            : 0,
        is_short: isShort,
        auto_pipeline,
      });
    },
    onSuccess: (book) => {
      qc.invalidateQueries({ queryKey: ["audiobooks"] });
      navigate(`/app/book/${book.id}`);
    },
    onError: (err) => {
      setError(err instanceof ApiError ? err.message : "Could not create audiobook");
    },
  });

  // Whenever topic, genre, art style, the picked image LLM, or the
  // YouTube Short toggle changes, the previewed cover no longer matches
  // — drop it so the user re-generates intentionally rather than
  // shipping a stale image (Shorts need 9:16; books need 1:1).
  useEffect(() => {
    setCover(null);
  }, [topic, genre, artStyle, coverLlmId, isShort]);

  // Shorts always upload as a single vertical clip — playlist mode
  // doesn't apply because the whole story fits in one ≤ 90 s video.
  useEffect(() => {
    if (isShort) setPublishMode("single");
  }, [isShort]);

  function submit(e: FormEvent<HTMLFormElement>): void {
    e.preventDefault();
    setError(null);
    create.mutate();
  }

  // The submit-button label tells the user how far we'll auto-run. Order
  // mirrors the backend's pipeline: outline (always) → chapters → audio
  // → publish.
  const submitLabel = (() => {
    if (create.isPending) return "Starting…";
    const noun = isShort ? "short" : "book";
    if (autoChapters && autoAudio && autoPublish && youtubeConnected) {
      return publishReview ? "Generate + publish (review)" : "Generate + publish";
    }
    if (autoChapters && autoAudio) return `Generate ${noun} + audio`;
    if (autoChapters) return `Generate ${noun}`;
    return "Draft outline";
  })();

  return (
    <section className="mx-auto max-w-5xl">
      <h1 className="text-2xl font-semibold tracking-tight">New audiobook</h1>
      <p className="mt-1 text-sm text-slate-400">
        Pick a topic and any extras you want; by default we&apos;ll draft the
        outline, write the chapters, and narrate them in one go.
      </p>

      <form onSubmit={submit} className="mt-6 space-y-6">
        <Field label="Language">
          <div className="flex items-center gap-2">
            <select
              value={language}
              onChange={(e) => setLanguage(e.target.value)}
              className={`${inputClass} flex-1`}
            >
              {LANGUAGES.map((l) => (
                <option key={l.code} value={l.code}>
                  {l.flag}  {l.label}
                </option>
              ))}
            </select>
            <LanguageLlmHint
              language={language}
              llm={pickedTextLlm}
              loading={llmsQuery.isLoading}
            />
          </div>
        </Field>

        {templatesQuery.data && templatesQuery.data.items.length > 0 && (
          <Field label="Start from a template (optional)">
            <select
              defaultValue=""
              onChange={(e) => {
                const t = templatesQuery.data?.items.find(
                  (x) => x.id === e.target.value,
                );
                if (t) applyTemplate(t);
                e.target.value = "";
              }}
              className={inputClass}
            >
              <option value="">— Pick a template —</option>
              {templatesQuery.data.items.map((t) => (
                <option key={t.id} value={t.id}>
                  {t.title}
                </option>
              ))}
            </select>
          </Field>
        )}

        <Field label="Topic">
          <div className="flex gap-2">
            <input
              type="text"
              required
              minLength={3}
              maxLength={500}
              value={topic}
              onChange={(e) => setTopic(e.target.value)}
              placeholder="e.g. A short history of tea"
              className={inputClass}
            />
            <button
              type="button"
              onClick={() => surprise.mutate()}
              disabled={surprise.isPending}
              className="whitespace-nowrap rounded-md border border-slate-700 bg-slate-900 px-3 py-2 text-xs text-slate-300 hover:border-slate-600 hover:text-slate-100"
              title="Generate a random topic via the LLM"
            >
              {surprise.isPending ? "…" : "Surprise me"}
            </button>
          </div>
        </Field>

        <Field label="Format">
          <div className="grid grid-cols-2 gap-2">
            <button
              type="button"
              onClick={() => setIsShort(false)}
              className={`rounded-md border px-3 py-2 text-left text-sm ${
                !isShort
                  ? "border-sky-600 bg-sky-600/10 text-sky-200"
                  : "border-slate-700 bg-slate-950 text-slate-300 hover:border-slate-600"
              }`}
            >
              <span className="block font-medium">📚 Audiobook</span>
              <span className="block text-[11px] text-slate-400">
                Multi-chapter, square cover
              </span>
            </button>
            <button
              type="button"
              onClick={() => setIsShort(true)}
              className={`rounded-md border px-3 py-2 text-left text-sm ${
                isShort
                  ? "border-rose-600 bg-rose-600/10 text-rose-200"
                  : "border-slate-700 bg-slate-950 text-slate-300 hover:border-slate-600"
              }`}
            >
              <span className="block font-medium">🎬 YouTube Short</span>
              <span className="block text-[11px] text-slate-400">
                Single ≤ 90 s clip, 9:16 vertical cover
              </span>
            </button>
          </div>
        </Field>

        <div className="grid gap-6 md:grid-cols-2">
          <div className="space-y-4">
            <Field label={isShort ? "Length (locked for Shorts)" : "Length"}>
              <div className="grid grid-cols-3 gap-2">
                {(["short", "medium", "long"] as AudiobookLength[]).map((l) => (
                  <button
                    key={l}
                    type="button"
                    onClick={() => setLength(l)}
                    disabled={isShort}
                    className={`rounded-md border px-3 py-2 text-sm capitalize ${
                      isShort
                        ? "cursor-not-allowed border-slate-800 bg-slate-950/40 text-slate-500"
                        : length === l
                          ? "border-sky-600 bg-sky-600/10 text-sky-200"
                          : "border-slate-700 bg-slate-950 text-slate-300 hover:border-slate-600"
                    }`}
                  >
                    {l}
                  </button>
                ))}
              </div>
              {isShort && (
                <p className="mt-1 text-[11px] text-slate-500">
                  Shorts always render as a single ≤ 90 s chapter (~225 words),
                  ignoring this preset.
                </p>
              )}
            </Field>

            <Field label="Genre (optional)">
              <GenreCombo value={genre} onChange={setGenre} />
            </Field>

            <Field label="Category (optional)">
              <select
                value={category}
                onChange={(e) => setCategory(e.target.value)}
                disabled={categoriesQuery.isLoading}
                className={inputClass}
              >
                <option value="">— Uncategorized —</option>
                {categoryNames.map((c) => (
                  <option key={c} value={c}>
                    {c}
                  </option>
                ))}
              </select>
              <p className="mt-1 text-xs text-slate-500">
                Used to group books in the library sidebar. Manage the
                list under <em>Admin → Categories</em>.
              </p>
            </Field>

            <Field label="Voice (optional)">
              <VoicePicker
                voices={voicesQuery.data?.items ?? []}
                isLoading={voicesQuery.isLoading}
                selected={voiceId}
                onSelect={setVoiceId}
              />
            </Field>
          </div>

          <div className="space-y-4">
            <Field label="Art style">
              <ArtStyleSelect value={artStyle} onChange={setArtStyle} />
              <p className="mt-1 text-xs text-slate-500">
                Applied to both the cover and per-chapter artwork. You can
                change it later from the book detail page.
              </p>
            </Field>

            {coverLlms.length > 1 && (
              <Field label="Image model">
                <select
                  value={coverLlmId}
                  onChange={(e) => setCoverLlmId(e.target.value)}
                  className={inputClass}
                >
                  <option value="">Server default</option>
                  {coverLlms.map((l) => (
                    <option key={l.id} value={l.id}>
                      {l.name}
                    </option>
                  ))}
                </select>
                <p className="mt-1 text-xs text-slate-500">
                  Used for cover and chapter artwork. Empty = whichever model
                  is marked default-for cover_art in admin settings.
                </p>
              </Field>
            )}

            <Field label="Cover art (optional)">
              <CoverArtPicker
                cover={cover}
                canGenerate={topic.trim().length >= 3}
                generating={generateCover.isPending}
                onGenerate={() => generateCover.mutate()}
                onClear={() => setCover(null)}
                vertical={isShort}
                error={
                  generateCover.error
                    ? generateCover.error instanceof ApiError
                      ? generateCover.error.message
                      : "Cover generation failed"
                    : null
                }
              />
            </Field>
          </div>
        </div>

        <PipelinePanel
          chapters={autoChapters}
          onChapters={setAutoChapters}
          cover={autoCover}
          onCover={setAutoCover}
          imagesPerParagraph={imagesPerParagraph}
          onImagesPerParagraph={setImagesPerParagraph}
          audio={autoAudio}
          onAudio={setAutoAudio}
          publish={autoPublish}
          onPublish={setAutoPublish}
          publishMode={publishMode}
          onPublishMode={setPublishMode}
          publishPrivacy={publishPrivacy}
          onPublishPrivacy={setPublishPrivacy}
          publishReview={publishReview}
          onPublishReview={setPublishReview}
          youtubeConnected={youtubeConnected}
          coverPreGenerated={cover !== null}
          isShort={isShort}
        />

        {error && <p className="text-sm text-rose-400">{error}</p>}
        <button type="submit" disabled={create.isPending} className={primaryBtn}>
          {submitLabel}
        </button>
      </form>
    </section>
  );
}

function LanguageLlmHint({
  language,
  llm,
  loading,
}: {
  language: string;
  llm: { name: string; model_id: string; provider?: string | null } | null;
  loading: boolean;
}): JSX.Element {
  const langLabel =
    LANGUAGES.find((l) => l.code === language)?.label ?? language;
  if (loading) {
    return (
      <span className="rounded-md border border-slate-800 bg-slate-900/40 px-2.5 py-1.5 text-[11px] text-slate-500">
        loading…
      </span>
    );
  }
  if (!llm) {
    return (
      <span
        title={`No LLM tagged default-for chapter is enabled for ${langLabel}. Configure one in admin → LLMs.`}
        className="rounded-md border border-amber-900/60 bg-amber-950/30 px-2.5 py-1.5 text-[11px] text-amber-200"
      >
        ⚠ no model for {langLabel}
      </span>
    );
  }
  return (
    <span
      title={`Highest-priority LLM for ${langLabel}: ${llm.model_id}${
        llm.provider ? ` via ${llm.provider}` : ""
      }. Used for outline, chapters, translation, and the surprise-me topic.`}
      className="rounded-md border border-slate-800 bg-slate-900/40 px-2.5 py-1.5 text-[11px] text-slate-300"
    >
      🤖 {llm.name}
    </span>
  );
}

function PipelinePanel({
  chapters,
  onChapters,
  cover,
  onCover,
  imagesPerParagraph,
  onImagesPerParagraph,
  audio,
  onAudio,
  publish,
  onPublish,
  publishMode,
  onPublishMode,
  publishPrivacy,
  onPublishPrivacy,
  publishReview,
  onPublishReview,
  youtubeConnected,
  coverPreGenerated,
  isShort,
}: {
  chapters: boolean;
  onChapters: (v: boolean) => void;
  cover: boolean;
  onCover: (v: boolean) => void;
  imagesPerParagraph: number;
  onImagesPerParagraph: (v: number) => void;
  audio: boolean;
  onAudio: (v: boolean) => void;
  publish: boolean;
  onPublish: (v: boolean) => void;
  publishMode: "single" | "playlist";
  onPublishMode: (v: "single" | "playlist") => void;
  publishPrivacy: "private" | "unlisted" | "public";
  onPublishPrivacy: (v: "private" | "unlisted" | "public") => void;
  publishReview: boolean;
  onPublishReview: (v: boolean) => void;
  youtubeConnected: boolean;
  coverPreGenerated: boolean;
  isShort: boolean;
}): JSX.Element {
  const audioDisabled = !chapters;
  const publishDisabled = !chapters || !audio;
  return (
    <div className="rounded-lg border border-slate-800 bg-slate-900/40 p-4">
      <div className="mb-3 flex items-baseline justify-between">
        <h2 className="text-sm font-semibold text-slate-100">Pipeline</h2>
        <span className="text-[11px] text-slate-500">
          What runs after the outline drafts
        </span>
      </div>

      <div className="grid gap-3 sm:grid-cols-2 lg:grid-cols-4">
        <PipelineStep
          step="1"
          title="Write chapters"
          subtitle="LLM expands the outline into prose."
          enabled={chapters}
          onChange={(v) => {
            onChapters(v);
            if (!v) {
              onAudio(false);
              onPublish(false);
            }
          }}
        />
        <PipelineStep
          step="2"
          title="Generate cover art"
          subtitle={
            coverPreGenerated
              ? "Already pre-generated above — step skipped."
              : "Image model paints a 1:1 cover from topic + style."
          }
          enabled={cover && !coverPreGenerated}
          onChange={(v) => onCover(v)}
          disabled={coverPreGenerated}
          disabledReason="Cover already pre-generated"
        />
        <PipelineStep
          step="3"
          title="Narrate audio"
          subtitle="TTS generates one WAV per chapter."
          enabled={audio}
          onChange={(v) => {
            onAudio(v);
            if (!v) onPublish(false);
          }}
          disabled={audioDisabled}
          disabledReason="Enable Write chapters first"
        />
        <PipelineStep
          step="4"
          title="Publish to YouTube"
          subtitle={
            youtubeConnected
              ? "Encode + upload the finished audio."
              : "Connect YouTube in Settings to enable."
          }
          enabled={publish && youtubeConnected}
          onChange={(v) => onPublish(v)}
          disabled={publishDisabled || !youtubeConnected}
          disabledReason={
            publishDisabled
              ? "Enable narration first"
              : "Connect a YouTube channel from Settings"
          }
        />
      </div>

      {chapters && cover && !isShort && (
        <div className="mt-4 rounded-md border border-slate-800 bg-slate-950/60 p-3">
          <div className="flex items-baseline justify-between">
            <div>
              <p className="text-xs font-semibold text-slate-100">
                Paragraph illustrations
              </p>
              <p className="text-[11px] text-slate-400">
                An LLM picks the visual paragraphs in each chapter; the
                image model then renders {imagesPerParagraph === 0
                  ? "no"
                  : imagesPerParagraph}{" "}
                tile{imagesPerParagraph === 1 ? "" : "s"} per pick. Tiles
                are crossfaded during playback in reading order.
              </p>
            </div>
            <span className="text-[11px] text-slate-500">
              {imagesPerParagraph === 0
                ? "off"
                : `${imagesPerParagraph}× per paragraph`}
            </span>
          </div>
          <div className="mt-2 grid grid-cols-4 gap-2">
            {[0, 1, 2, 3].map((n) => (
              <button
                key={n}
                type="button"
                onClick={() => onImagesPerParagraph(n)}
                className={`rounded-md border px-3 py-2 text-xs ${
                  imagesPerParagraph === n
                    ? "border-rose-600 bg-rose-600/10 text-rose-200"
                    : "border-slate-700 bg-slate-950 text-slate-300 hover:border-slate-600"
                }`}
              >
                {n === 0 ? "Off" : `${n}×`}
              </button>
            ))}
          </div>
          {imagesPerParagraph > 0 && (
            <p className="mt-2 text-[11px] text-slate-500">
              Capped at 12 visual paragraphs per chapter. With{" "}
              {imagesPerParagraph}× tiles, a 6-chapter book makes ≤
              {" "}
              {6 * 12 * imagesPerParagraph} image calls (typically ~half
              that — non-visual paragraphs are skipped).
            </p>
          )}
        </div>
      )}

      {publish && youtubeConnected && (
        <div className="mt-4 rounded-md border border-slate-800 bg-slate-950/60 p-3">
          <div className="grid gap-3 sm:grid-cols-2">
            <Field label="Video format">
              {isShort ? (
                <div className="rounded-md border border-rose-900/50 bg-rose-950/20 px-3 py-2 text-xs text-rose-200">
                  Single vertical Short (9:16, ≤ 90 s)
                </div>
              ) : (
                <div className="grid grid-cols-2 gap-2">
                  {(
                    [
                      { value: "single", label: "Single video" },
                      { value: "playlist", label: "Playlist" },
                    ] as const
                  ).map((opt) => (
                    <button
                      key={opt.value}
                      type="button"
                      onClick={() => onPublishMode(opt.value)}
                      className={`rounded-md border px-3 py-2 text-xs ${
                        publishMode === opt.value
                          ? "border-rose-600 bg-rose-600/10 text-rose-200"
                          : "border-slate-700 bg-slate-950 text-slate-300 hover:border-slate-600"
                      }`}
                    >
                      {opt.label}
                    </button>
                  ))}
                </div>
              )}
            </Field>
            <Field label="Visibility">
              <div className="grid grid-cols-3 gap-2">
                {(["private", "unlisted", "public"] as const).map((p) => (
                  <button
                    key={p}
                    type="button"
                    onClick={() => onPublishPrivacy(p)}
                    className={`rounded-md border px-3 py-2 text-xs capitalize ${
                      publishPrivacy === p
                        ? "border-rose-600 bg-rose-600/10 text-rose-200"
                        : "border-slate-700 bg-slate-950 text-slate-300 hover:border-slate-600"
                    }`}
                  >
                    {p}
                  </button>
                ))}
              </div>
            </Field>
          </div>
          <label className="mt-3 flex cursor-pointer items-start gap-2 rounded-md border border-slate-800 bg-slate-950 p-2 text-xs">
            <input
              type="checkbox"
              checked={publishReview}
              onChange={(e) => onPublishReview(e.target.checked)}
              className="mt-0.5 h-3.5 w-3.5 accent-rose-600"
            />
            <span className="min-w-0 flex-1">
              <span className="block text-slate-100">Stop for review</span>
              <span className="block text-slate-400">
                Encode but don&apos;t upload to YouTube until you approve from
                the audiobook detail page.
              </span>
            </span>
          </label>
        </div>
      )}

      {!youtubeConnected && publish && (
        <p className="mt-3 rounded-md border border-amber-900/60 bg-amber-950/30 p-2 text-xs text-amber-200">
          Connect a YouTube channel from{" "}
          <Link to="/app/settings" className="underline hover:text-amber-100">
            Settings
          </Link>{" "}
          to enable the publish step.
        </p>
      )}
    </div>
  );
}

function PipelineStep({
  step,
  title,
  subtitle,
  enabled,
  onChange,
  disabled,
  disabledReason,
}: {
  step: string;
  title: string;
  subtitle: string;
  enabled: boolean;
  onChange: (v: boolean) => void;
  disabled?: boolean;
  disabledReason?: string;
}): JSX.Element {
  return (
    <label
      className={`flex cursor-pointer items-start gap-3 rounded-md border p-3 ${
        disabled
          ? "cursor-not-allowed border-slate-800 bg-slate-950/40 opacity-60"
          : enabled
            ? "border-sky-600 bg-sky-600/10"
            : "border-slate-700 bg-slate-950 hover:border-slate-600"
      }`}
      title={disabled ? disabledReason : undefined}
    >
      <input
        type="checkbox"
        checked={enabled}
        disabled={disabled}
        onChange={(e) => onChange(e.target.checked)}
        className="mt-0.5 h-4 w-4 accent-sky-500 disabled:cursor-not-allowed"
      />
      <span className="min-w-0 flex-1">
        <span className="flex items-center gap-1.5">
          <span className="text-[10px] uppercase tracking-wide text-slate-500">
            Step {step}
          </span>
        </span>
        <span className="block text-sm font-medium text-slate-100">{title}</span>
        <span className="block text-xs text-slate-400">{subtitle}</span>
      </span>
    </label>
  );
}

function GenreCombo({
  value,
  onChange,
}: {
  value: string;
  onChange: (v: string) => void;
}): JSX.Element {
  const [open, setOpen] = useState(false);
  const wrapRef = useRef<HTMLDivElement>(null);

  // Close on click-outside.
  useEffect(() => {
    if (!open) return;
    const onDoc = (e: MouseEvent): void => {
      if (wrapRef.current && !wrapRef.current.contains(e.target as Node)) {
        setOpen(false);
      }
    };
    document.addEventListener("mousedown", onDoc);
    return () => document.removeEventListener("mousedown", onDoc);
  }, [open]);

  const trimmed = value.trim();
  const filtered = trimmed
    ? GENRE_PRESETS.filter((g) =>
        g.label.toLowerCase().includes(trimmed.toLowerCase()),
      )
    : GENRE_PRESETS;

  return (
    <div ref={wrapRef} className="relative">
      <div className="flex items-stretch gap-2">
        <div className="relative flex flex-1 items-center">
          <span className="pointer-events-none absolute left-3 text-base">
            {genreIcon(value)}
          </span>
          <input
            type="text"
            maxLength={40}
            value={value}
            onChange={(e) => {
              onChange(e.target.value);
              if (!open) setOpen(true);
            }}
            onFocus={() => setOpen(true)}
            placeholder="Pick a genre or type your own"
            className={`${inputClass} pl-10`}
          />
        </div>
        <button
          type="button"
          onClick={() => setOpen((v) => !v)}
          aria-label="Toggle genre list"
          className="rounded-md border border-slate-700 bg-slate-900 px-3 text-slate-300 hover:border-slate-600"
        >
          <svg width="14" height="14" viewBox="0 0 16 16" fill="currentColor">
            <path d="M3 6l5 5 5-5H3z" />
          </svg>
        </button>
      </div>
      {open && filtered.length > 0 && (
        <ul className="absolute z-20 mt-1 max-h-72 w-full overflow-auto rounded-md border border-slate-800 bg-slate-950 py-1 text-sm shadow-lg">
          {filtered.map((g) => {
            const selected = g.label.toLowerCase() === trimmed.toLowerCase();
            return (
              <li key={g.label}>
                <button
                  type="button"
                  onClick={() => {
                    onChange(g.label);
                    setOpen(false);
                  }}
                  className={`flex w-full items-center gap-3 px-3 py-1.5 text-left ${
                    selected
                      ? "bg-sky-600/15 text-sky-200"
                      : "text-slate-200 hover:bg-slate-800"
                  }`}
                >
                  <span className="w-5 text-center text-base">{g.icon}</span>
                  {g.label}
                </button>
              </li>
            );
          })}
        </ul>
      )}
    </div>
  );
}

function VoicePicker({
  voices,
  isLoading,
  selected,
  onSelect,
}: {
  voices: Voice[];
  isLoading: boolean;
  selected: string | null;
  onSelect: (id: string | null) => void;
}): JSX.Element {
  if (isLoading) {
    return <p className="text-xs text-slate-500">Loading voices…</p>;
  }
  if (voices.length === 0) {
    return (
      <p className="text-xs text-slate-500">
        No voices available — narration will use the server default.
      </p>
    );
  }
  return (
    <div className="grid grid-cols-2 gap-2 sm:grid-cols-3">
      <VoiceCard
        active={selected === null}
        onSelect={() => onSelect(null)}
        icon="✨"
        title="Default"
        subtitle="Server pick"
      />
      {voices.map((v) => (
        <VoiceCard
          key={v.id}
          active={selected === v.id}
          onSelect={() => onSelect(v.id)}
          icon={VOICE_ICONS[v.gender] ?? "🎙️"}
          title={v.name}
          subtitle={v.accent}
        />
      ))}
    </div>
  );
}

function VoiceCard({
  active,
  onSelect,
  icon,
  title,
  subtitle,
}: {
  active: boolean;
  onSelect: () => void;
  icon: string;
  title: string;
  subtitle: string;
}): JSX.Element {
  return (
    <button
      type="button"
      onClick={onSelect}
      className={`flex flex-col items-start gap-1 rounded-md border px-3 py-2 text-left ${
        active
          ? "border-sky-600 bg-sky-600/10"
          : "border-slate-700 bg-slate-950 hover:border-slate-600"
      }`}
    >
      <span className="text-lg leading-none">{icon}</span>
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

function CoverArtPicker({
  cover,
  canGenerate,
  generating,
  onGenerate,
  onClear,
  vertical,
  error,
}: {
  cover: { base64: string; mime: string } | null;
  canGenerate: boolean;
  generating: boolean;
  onGenerate: () => void;
  onClear: () => void;
  vertical: boolean;
  error: string | null;
}): JSX.Element {
  // Match the preview frame to the requested aspect so a Short's 9:16
  // cover doesn't get letterboxed inside a square placeholder (and a
  // book's 1:1 cover doesn't stretch into a tall slot).
  const frameSize = vertical ? "h-48 w-[6.75rem]" : "h-32 w-32";
  return (
    <div className="flex flex-col gap-2 sm:flex-row sm:items-start">
      <div
        className={`flex shrink-0 items-center justify-center overflow-hidden rounded-md border border-slate-700 bg-slate-950 ${frameSize}`}
      >
        {cover ? (
          <img
            src={`data:${cover.mime};base64,${cover.base64}`}
            alt="Generated cover preview"
            className="h-full w-full object-cover"
          />
        ) : (
          <span className="text-[11px] text-slate-500">
            {vertical ? "No cover (9:16)" : "No cover"}
          </span>
        )}
      </div>
      <div className="flex-1 space-y-2">
        <div className="flex flex-wrap gap-2">
          <button
            type="button"
            onClick={onGenerate}
            disabled={!canGenerate || generating}
            className="rounded-md bg-violet-600 px-3 py-2 text-sm font-medium text-white hover:bg-violet-500 disabled:cursor-not-allowed disabled:bg-violet-700/50"
            title={
              canGenerate
                ? "Generate cover art from the topic + genre"
                : "Type a topic first"
            }
          >
            {generating ? "Generating…" : cover ? "Regenerate" : "Generate cover"}
          </button>
          {cover && (
            <button
              type="button"
              onClick={onClear}
              className="rounded-md border border-slate-700 bg-slate-900 px-3 py-2 text-sm text-slate-300 hover:border-slate-600"
            >
              Remove
            </button>
          )}
        </div>
        <p className="text-xs text-slate-500">
          Uses the OpenRouter model marked <em>cover_art</em> in admin settings.
          Editing topic or genre will clear the preview.
        </p>
        {error && <p className="text-xs text-rose-400">{error}</p>}
      </div>
    </div>
  );
}
