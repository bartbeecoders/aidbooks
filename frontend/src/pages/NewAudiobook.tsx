import { FormEvent, useEffect, useMemo, useRef, useState } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { Link, useNavigate, useSearchParams } from "react-router-dom";
import {
  audiobooks,
  catalog,
  coverArt,
  integrations,
  songbook as songbookApi,
  snippetPreviewUrl,
  topics,
  ApiError,
} from "../api";
import { useAuth } from "../store/auth";
import type {
  AudiobookLength,
  AutoPipeline,
  NarrationIntensity,
  NarrationStyle,
  SongbookPreviewResponse,
  TopicTemplate,
  Voice,
  VoicePreset,
} from "../api";
import {
  NarrationStylePanel,
  VoicePresetPicker,
  presetRoles,
} from "../components/NarrationStylePanel";
import { ArtStyleSelect } from "../components/ArtStylePicker";
import { DEFAULT_ART_STYLE } from "../lib/art-styles";
import { imageCapableLlms } from "../lib/cover-llm";
import { voicesForLanguage } from "../lib/voices";
import {
  useVoicePreview,
  type VoicePreviewState,
} from "../lib/useVoicePreview";
import { VoicePreviewButton } from "../components/VoicePreview";
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
  // Allow other pages to deep-link into the create flow with a
  // pre-filled topic — currently used by the Ideas page's "Start"
  // button. The param is read once on mount; subsequent edits to the
  // input are owned by local state.
  const [searchParams] = useSearchParams();
  const initialTopic = searchParams.get("topic") ?? "";
  const [topic, setTopic] = useState(initialTopic);
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
  // Songbook mode: topic is a song reference, lyrics + artist info are
  // fetched server-side via Tinyfish, and the dedicated outline prompt
  // plans chapters around verses + meaning. Mutually exclusive with
  // `isShort` (the backend rejects the combination with a 400).
  const [isSongbook, setIsSongbook] = useState(false);
  // Songbook-only knob: number of audio clips of the actual song that
  // the publish step splices between chapters. 0 = no clips. The
  // backend caps at 12; we surface a 0..6 slider as the sensible UX
  // range (anything higher feels relentless).
  const [snippetCount, setSnippetCount] = useState(3);
  // Last preview result; rendered as a stack of <audio> tags + the
  // matching YouTube link so the user can verify the song lookup
  // resolved to the right track before committing to a full create.
  const [snippetPreview, setSnippetPreview] = useState<SongbookPreviewResponse | null>(null);
  const accessToken = useAuth((s) => s.accessToken) ?? "";
  const [autoChapters, setAutoChapters] = useState(true);
  const [autoCover, setAutoCover] = useState(true);
  // Tiles per *visual* paragraph (the LLM extract pass picks visualizable
  // paragraphs first; this knob just controls how many tiles per pick).
  // 0 = chapter cover tiles only.
  const [imagesPerParagraph, setImagesPerParagraph] = useState<number>(0);
  const [autoAudio, setAutoAudio] = useState(true);
  // Multi-voice settings the user picks before creation. Mirrors the
  // BookDetail panel's two pieces of state (toggle + role→voice map);
  // both go straight into the create body so the audiobook starts out
  // already configured rather than requiring an extra patch.
  const [multiVoiceEnabled, setMultiVoiceEnabled] = useState(false);
  const [voiceRoles, setVoiceRoles] = useState<Record<string, string>>({});
  // Narrative style overlay + intensity dial. Both are no-op defaults
  // (`null` style + empty intensity); the backend treats those as the
  // legacy genre-driven path so existing flows are unaffected.
  const [narrationStyle, setNarrationStyle] = useState<NarrationStyle | null>(
    null,
  );
  const [narrationIntensity, setNarrationIntensity] = useState<
    NarrationIntensity[]
  >([]);
  // Voice preset is purely a UX hint persisted alongside voice_roles —
  // the backend audio pipeline reads the role map directly. Picking a
  // preset turns multi-voice on automatically; "None" leaves it off.
  const [voicePreset, setVoicePreset] = useState<VoicePreset | null>(null);
  const [autoPublish, setAutoPublish] = useState(false);
  const [publishMode, setPublishMode] = useState<"single" | "playlist">("single");
  const [publishPrivacy, setPublishPrivacy] =
    useState<"private" | "unlisted" | "public">("private");
  const [publishReview, setPublishReview] = useState(true);
  // Off by default — auto-create flows skipping the hyperframes step
  // shouldn't pay for it. Steps null = backend auto-scale.
  const [publishHyperframes, setPublishHyperframes] = useState(false);
  const [publishHyperframesSteps, setPublishHyperframesSteps] = useState<
    number | null
  >(null);

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

  // Drop voice picks that don't match the new language so the form
  // doesn't silently submit a hidden, language-mismatched voice.
  useEffect(() => {
    const all = voicesQuery.data?.items ?? [];
    const allowed = new Set(voicesForLanguage(all, language).map((v) => v.id));
    if (allowed.size === 0) return;
    if (voiceId && !allowed.has(voiceId)) setVoiceId(null);
    setVoiceRoles((prev) => {
      const next = Object.fromEntries(
        Object.entries(prev).filter(([, vid]) => allowed.has(vid)),
      );
      return Object.keys(next).length === Object.keys(prev).length ? prev : next;
    });
  }, [language, voicesQuery.data, voiceId]);

  const applyTemplate = (t: TopicTemplate): void => {
    setTopic(t.topic);
    if (t.genre) setGenre(t.genre);
    if (t.length) setLength(t.length);
    if (t.language) setLanguage(t.language);
    setIsShort(t.is_short);
    // Templates don't carry a songbook bit yet; resetting keeps the form
    // self-consistent if the user toggles songbook after picking one.
    if (t.is_short) setIsSongbook(false);
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

  const previewSnippets = useMutation({
    mutationFn: () =>
      songbookApi.previewSnippets({
        topic: topic.trim(),
        // The backend caps internally; we send what's on the slider
        // and clamp 0 → 1 because previewing 0 doesn't make sense.
        count: Math.max(1, snippetCount),
      }),
    onSuccess: (r) => setSnippetPreview(r),
    onError: () => setSnippetPreview(null),
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

  const buildCreateBody = (enqueue: boolean) => {
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
            hyperframes: publishHyperframes,
            hyperframes_steps: publishHyperframesSteps,
          }
        : null,
    };
    return {
      topic: topic.trim(),
      length,
      genre: genre.trim() || undefined,
      category: category.trim() || undefined,
      language,
      voice_id: voiceId ?? undefined,
      cover_image_base64: cover?.base64,
      art_style: artStyle || undefined,
      cover_llm_id: coverLlmId || undefined,
      // Paragraph slideshow tiles, both for regular books and Shorts —
      // the publisher composites each tile onto its target aspect
      // ratio (9:16 for Shorts, 16:9 otherwise) so the same tile
      // count works either way.
      images_per_paragraph:
        autoCover && imagesPerParagraph > 0 ? imagesPerParagraph : 0,
      is_short: isShort,
      is_songbook: isSongbook,
      // Snippet count is meaningful only in songbook mode; the
      // backend rejects > 0 otherwise, so collapse it here too.
      snippet_count: isSongbook ? snippetCount : 0,
      // Preview adoption only fires on the inline create path — when
      // enqueueing, outline runs much later and the preview dir may
      // be GC'd by then, so we let the SongSnippets job re-fetch.
      preview_id:
        !enqueue &&
        isSongbook &&
        snippetCount > 0 &&
        snippetPreview?.items.length
          ? snippetPreview.preview_id
          : undefined,
      auto_pipeline,
      // Only ship the role map when multi-voice is on: an empty map
      // with the toggle off would otherwise overwrite a future
      // server-side default.
      multi_voice_enabled: multiVoiceEnabled,
      voice_roles: multiVoiceEnabled ? voiceRoles : undefined,
      narration_style: narrationStyle,
      narration_intensity: narrationIntensity,
      voice_preset: voicePreset,
      enqueue,
    };
  };

  const create = useMutation({
    mutationFn: () => audiobooks.create(buildCreateBody(false)),
    onSuccess: (book) => {
      qc.invalidateQueries({ queryKey: ["audiobooks"] });
      navigate(`/app/book/${book.id}`);
    },
    onError: (err) => {
      setError(err instanceof ApiError ? err.message : "Could not create audiobook");
    },
  });

  const enqueue = useMutation({
    mutationFn: () => audiobooks.create(buildCreateBody(true)),
    onSuccess: () => {
      qc.invalidateQueries({ queryKey: ["audiobooks"] });
      qc.invalidateQueries({ queryKey: ["queue"] });
      navigate(`/app/queue`);
    },
    onError: (err) => {
      setError(err instanceof ApiError ? err.message : "Could not add to queue");
    },
  });

  // Whenever topic, genre, art style, the picked image LLM, or the
  // YouTube Short toggle changes, the previewed cover no longer matches
  // — drop it so the user re-generates intentionally rather than
  // shipping a stale image (Shorts need 9:16; books need 1:1).
  useEffect(() => {
    setCover(null);
  }, [topic, genre, artStyle, coverLlmId, isShort]);

  // Same logic for the snippet preview: a different topic or count
  // means the cached clips no longer reflect what the create flow
  // would adopt. Clearing here forces a re-preview before we can
  // pass `preview_id` to the backend.
  useEffect(() => {
    setSnippetPreview(null);
  }, [topic, snippetCount, isSongbook]);

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
              placeholder={
                isSongbook
                  ? "Song — Artist (e.g. Bohemian Rhapsody — Queen)"
                  : "e.g. A short history of tea"
              }
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
          {isSongbook && (
            <p className="mt-1 text-[11px] text-slate-500">
              We'll search Tinyfish for lyrics + artist info before
              planning the chapters.
            </p>
          )}
        </Field>

        <Field label="Format">
          <div className="grid grid-cols-3 gap-2">
            <button
              type="button"
              onClick={() => {
                setIsShort(false);
                setIsSongbook(false);
              }}
              className={`rounded-md border px-3 py-2 text-left text-sm ${
                !isShort && !isSongbook
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
              onClick={() => {
                setIsSongbook(true);
                setIsShort(false);
              }}
              className={`rounded-md border px-3 py-2 text-left text-sm ${
                isSongbook
                  ? "border-fuchsia-600 bg-fuchsia-600/10 text-fuchsia-200"
                  : "border-slate-700 bg-slate-950 text-slate-300 hover:border-slate-600"
              }`}
            >
              <span className="block font-medium">🎵 Songbook</span>
              <span className="block text-[11px] text-slate-400">
                Explains a song; lyrics fetched via Tinyfish
              </span>
            </button>
            <button
              type="button"
              onClick={() => {
                setIsShort(true);
                setIsSongbook(false);
              }}
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

        {isSongbook && (
          <Field label={`Song snippets in audio (${snippetCount})`}>
            <input
              type="range"
              min={0}
              max={6}
              step={1}
              value={snippetCount}
              onChange={(e) => setSnippetCount(Number(e.target.value))}
              className="w-full accent-fuchsia-500"
              aria-label="Number of song snippets"
            />
            <p className="mt-1 text-[11px] text-slate-500">
              The publish step downloads the song from YouTube via
              yt-dlp and splices N short clips of the original
              recording between chapters. 0 disables the snippet
              job. <strong className="text-amber-300/80">Heads up:</strong>{" "}
              you're responsible for clearing copyright before
              publishing public videos containing music you don't own.
            </p>
            <div className="mt-2 flex items-center gap-2">
              <button
                type="button"
                onClick={() => previewSnippets.mutate()}
                disabled={
                  previewSnippets.isPending ||
                  topic.trim().length < 3 ||
                  snippetCount === 0
                }
                className="rounded-md border border-fuchsia-700 bg-fuchsia-700/10 px-3 py-1.5 text-xs text-fuchsia-200 hover:border-fuchsia-500 disabled:cursor-not-allowed disabled:opacity-50"
              >
                {previewSnippets.isPending
                  ? "Fetching…"
                  : "Preview snippets"}
              </button>
              <span className="text-[11px] text-slate-500">
                Runs the same Tinyfish + yt-dlp pipeline so you can
                hear the actual clips before creating the audiobook.
              </span>
            </div>
            {previewSnippets.isError && (
              <p className="mt-2 text-[11px] text-rose-300">
                {previewSnippets.error instanceof ApiError
                  ? previewSnippets.error.message
                  : "Could not preview snippets"}
              </p>
            )}
            {snippetPreview && (
              <div className="mt-3 space-y-2 rounded-md border border-slate-800 bg-slate-950/40 p-3">
                {snippetPreview.youtube_url ? (
                  <p className="text-[11px] text-slate-400">
                    Source:{" "}
                    <a
                      href={snippetPreview.youtube_url}
                      target="_blank"
                      rel="noreferrer"
                      className="text-sky-300 hover:underline"
                    >
                      {snippetPreview.youtube_url}
                    </a>
                  </p>
                ) : (
                  <p className="text-[11px] text-amber-300/80">
                    No YouTube URL was resolved.
                  </p>
                )}
                {snippetPreview.error && (
                  <p className="text-[11px] text-rose-300">
                    {snippetPreview.error}
                  </p>
                )}
                {snippetPreview.items.length === 0 && !snippetPreview.error && (
                  <p className="text-[11px] text-slate-500">
                    No clips were produced.
                  </p>
                )}
                {snippetPreview.items.map((item) => (
                  <div
                    key={item.index}
                    className="flex items-center gap-3 text-[11px] text-slate-400"
                  >
                    <span className="w-12 shrink-0 tabular-nums">
                      #{item.index} · {(item.duration_ms / 1000).toFixed(1)}s
                    </span>
                    <audio
                      controls
                      preload="none"
                      className="h-8 flex-1"
                      src={snippetPreviewUrl(
                        snippetPreview.preview_id,
                        item.index,
                        accessToken,
                      )}
                    />
                  </div>
                ))}
              </div>
            )}
          </Field>
        )}

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
                voices={voicesForLanguage(
                  voicesQuery.data?.items ?? [],
                  language,
                )}
                isLoading={voicesQuery.isLoading}
                selected={voiceId}
                onSelect={setVoiceId}
              />
            </Field>

            <VoicePresetPicker
              value={voicePreset}
              onChange={(next) => {
                setVoicePreset(next);
                // Picking a multi-voice preset (anything beyond a
                // single-voice cast) implies multi-voice mode — the
                // backend ignores `voice_roles` otherwise.
                if (next && presetRoles(next).length > 1) {
                  setMultiVoiceEnabled(true);
                }
              }}
            />

            <MultiVoicePanel
              enabled={multiVoiceEnabled}
              roles={voiceRoles}
              onEnabled={setMultiVoiceEnabled}
              onRoles={setVoiceRoles}
              voices={voicesForLanguage(
                voicesQuery.data?.items ?? [],
                language,
              )}
            />

            <NarrationStylePanel
              style={narrationStyle}
              onStyle={setNarrationStyle}
              intensity={narrationIntensity}
              onIntensity={setNarrationIntensity}
            />
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
          publishHyperframes={publishHyperframes}
          onPublishHyperframes={setPublishHyperframes}
          publishHyperframesSteps={publishHyperframesSteps}
          onPublishHyperframesSteps={setPublishHyperframesSteps}
          youtubeConnected={youtubeConnected}
          coverPreGenerated={cover !== null}
          isShort={isShort}
        />

        {error && <p className="text-sm text-rose-400">{error}</p>}
        <div className="flex flex-wrap items-center gap-3">
          <button
            type="submit"
            disabled={create.isPending || enqueue.isPending}
            className={primaryBtn}
          >
            {submitLabel}
          </button>
          <button
            type="button"
            disabled={
              create.isPending || enqueue.isPending || topic.trim().length < 3
            }
            onClick={() => {
              setError(null);
              enqueue.mutate();
            }}
            className="rounded-md border border-amber-700 bg-amber-700/10 px-4 py-2 text-sm font-medium text-amber-100 hover:border-amber-500 hover:bg-amber-700/20 disabled:cursor-not-allowed disabled:opacity-50"
            title="Create the audiobook in draft state and queue it. The queue runs one book at a time."
          >
            {enqueue.isPending ? "Queueing…" : "Add to queue"}
          </button>
          <p className="text-[11px] text-slate-500">
            Queueing skips the inline outline and saves the request for
            later. View the queue under{" "}
            <Link to="/app/queue" className="underline hover:text-slate-300">
              Queue
            </Link>
            .
          </p>
        </div>
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
  publishHyperframes,
  onPublishHyperframes,
  publishHyperframesSteps,
  onPublishHyperframesSteps,
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
  publishHyperframes: boolean;
  onPublishHyperframes: (v: boolean) => void;
  publishHyperframesSteps: number | null;
  onPublishHyperframesSteps: (v: number | null) => void;
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

      {chapters && cover && (
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
          <div className="mt-3 rounded-md border border-slate-800 bg-slate-950 p-2 text-xs">
            <label className="flex cursor-pointer items-start gap-2">
              <input
                type="checkbox"
                checked={publishHyperframes}
                onChange={(e) => onPublishHyperframes(e.target.checked)}
                className="mt-0.5 h-3.5 w-3.5 accent-cyan-500"
              />
              <span className="min-w-0 flex-1">
                <span className="block text-slate-100">
                  Hyperframes illustrated video
                </span>
                <span className="block text-slate-400">
                  {isShort
                    ? "9:16 composition with chapter art, animated taglines, and a title overlay."
                    : "16:9 composition with a title card, per-chapter intro cards, image + caption scenes, and pull quotes."}
                </span>
              </span>
            </label>
            {publishHyperframes && (
              <div className="mt-2 flex items-center gap-2 pl-6">
                <span className="text-slate-300">Scenes</span>
                <input
                  type="number"
                  min={2}
                  max={isShort ? 12 : 120}
                  placeholder="Auto"
                  value={publishHyperframesSteps ?? ""}
                  onChange={(e) => {
                    const raw = e.target.value;
                    if (raw === "") {
                      onPublishHyperframesSteps(null);
                      return;
                    }
                    const cap = isShort ? 12 : 120;
                    const n = Math.min(cap, Math.max(2, Number(raw) || 0));
                    onPublishHyperframesSteps(
                      Number.isFinite(n) ? n : null,
                    );
                  }}
                  className="w-24 rounded-md border border-slate-700 bg-slate-950 px-2 py-1 text-slate-100"
                />
                <button
                  type="button"
                  onClick={() => onPublishHyperframesSteps(null)}
                  className={
                    "rounded border px-2 py-0.5 text-[10px] " +
                    (publishHyperframesSteps === null
                      ? "border-cyan-500 bg-cyan-500/10 text-cyan-200"
                      : "border-slate-700 bg-slate-950 text-slate-400 hover:text-slate-200")
                  }
                >
                  Auto
                </button>
                <span className="ml-1 text-[10px] text-slate-500">
                  {isShort
                    ? "Auto ≈ 1 / 15 s, max 12."
                    : "Auto ≈ 1 / 15 s, max 120."}
                </span>
              </div>
            )}
          </div>
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
  const preview = useVoicePreview();
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
    <>
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
            previewState={preview.stateFor(v.id)}
            onPreview={() => preview.toggle(v.id)}
          />
        ))}
      </div>
      {preview.error && (
        <p className="mt-2 text-xs text-rose-400">{preview.error}</p>
      )}
    </>
  );
}

function VoiceCard({
  active,
  onSelect,
  icon,
  title,
  subtitle,
  previewState,
  onPreview,
}: {
  active: boolean;
  onSelect: () => void;
  icon: string;
  title: string;
  subtitle: string;
  previewState?: VoicePreviewState;
  onPreview?: () => void;
}): JSX.Element {
  return (
    <div
      className={`relative flex flex-col items-start gap-1 rounded-md border px-3 py-2 pr-9 text-left ${
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
      <span className="relative text-lg leading-none">{icon}</span>
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

const MULTI_VOICE_ROLES: { id: string; label: string; hint: string }[] = [
  { id: "narrator", label: "Narrator", hint: "Descriptive prose & action" },
  { id: "dialogue_male", label: "Male dialogue", hint: "Speech by male characters" },
  {
    id: "dialogue_female",
    label: "Female dialogue",
    hint: "Speech by female characters",
  },
];

/**
 * Multi-voice settings for the create flow — same shape as the
 * BookDetail panel but drives local state instead of patching a saved
 * audiobook. Values flow into `audiobooks.create({ multi_voice_enabled,
 * voice_roles })` so the new book starts out already configured.
 */
function MultiVoicePanel({
  enabled,
  roles,
  onEnabled,
  onRoles,
  voices,
}: {
  enabled: boolean;
  roles: Record<string, string>;
  onEnabled: (v: boolean) => void;
  onRoles: (next: Record<string, string>) => void;
  voices: Voice[];
}): JSX.Element {
  const onPickRole = (roleId: string, voiceId: string | null): void => {
    const next = { ...roles };
    if (voiceId) next[roleId] = voiceId;
    else delete next[roleId];
    onRoles(next);
  };

  return (
    <details
      open={enabled}
      className="overflow-hidden rounded-lg border border-slate-800 bg-slate-900/40"
    >
      <summary className="flex cursor-pointer select-none items-center gap-2 px-4 py-2.5 text-sm text-slate-200 hover:bg-slate-900/70">
        <span aria-hidden="true" className="text-xs text-slate-500">
          {enabled ? "▾" : "▸"}
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
          When on, narration runs an extra LLM pass to split prose by
          speaker, then renders each segment with the role&apos;s mapped
          voice. The voice above is used as the narrator fallback for
          any role you leave unset.
        </p>
        <label className="inline-flex cursor-pointer items-center gap-2 text-sm text-slate-200">
          <input
            type="checkbox"
            checked={enabled}
            onChange={(e) => onEnabled(e.target.checked)}
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
              />
            ))}
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
}: {
  role: { id: string; label: string; hint: string };
  value: string | null;
  voices: Voice[];
  onChange: (voiceId: string | null) => void;
}): JSX.Element {
  // Visual hint: voices whose gender matches the role get a checkmark.
  // Narrator is gender-neutral so no badge.
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
          className="rounded-md border border-slate-700 bg-slate-950 px-2 py-1.5 text-sm text-slate-100 outline-none focus:border-sky-600"
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
