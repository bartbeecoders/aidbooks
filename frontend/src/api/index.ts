// Typed API surface — one function per endpoint, all call `apiFetch`.
// Keeping the call sites flat makes it obvious which backend routes the UI
// depends on, which matters when diffing schema regenerations.

import { apiFetch } from "./client";
import type {
  AdminJobList,
  AdminLlmList,
  AdminLlmRow,
  AdminUserList,
  AdminUserRow,
  AdminVoiceList,
  AdminVoiceRow,
  ApprovePublicationResponse,
  AnimationTheme,
  AudiobookCostSummary,
  AudiobookDetail,
  AudiobookJobList,
  AudiobookList,
  AudiobookSummary,
  AuthResponse,
  ChapterSummary,
  CoverPreviewRequest,
  CoverPreviewResponse,
  CreateAudiobookRequest,
  CreateLlmRequest,
  CreateTopicTemplateRequest,
  OauthStartResponse,
  AudiobookCategoryList,
  AudiobookCategoryNameList,
  AudiobookCategoryRow,
  CreateAudiobookCategoryRequest,
  OpenRouterModelList,
  UpdateAudiobookCategoryRequest,
  UpsertYoutubeFooterRequest,
  XaiImageModelList,
  XaiModelList,
  YoutubeFooterList,
  YoutubeFooterRow,
  YoutubePublishSettings,
  PublicationList,
  PublishYoutubeRequest,
  PublishYoutubeResponse,
  TopicTemplate,
  TopicTemplateList,
  TranslateRequest,
  TranslateResponse,
  CreateIdeaRequest,
  CreatePodcastRequest,
  IdeaList,
  IdeaRow,
  PodcastList,
  PodcastRow,
  PreviewPodcastImageRequest,
  PreviewPodcastImageResponse,
  SuggestIdeasRequest,
  SuggestIdeasResponse,
  SyncPodcastResponse,
  UpdateIdeaRequest,
  UpdatePodcastRequest,
  LoginRequest,
  MeResponse,
  RandomTopicRequest,
  RandomTopicResponse,
  RegisterRequest,
  RevokeSessionsResponse,
  SystemOverview,
  TestLlmRequest,
  TestLlmResponse,
  TestVoiceRequest,
  TestVoiceResponse,
  UpdateAudiobookRequest,
  UpdateChapterRequest,
  UpdateLlmRequest,
  UpdateTopicTemplateRequest,
  UpdateUserRequest,
  UpdateVoiceRequest,
  VoiceList,
  VoicePreviewResponse,
  YoutubeAccountStatus,
} from "./types";

export { ApiError } from "./client";
export type * from "./types";

// --- auth ----------------------------------------------------------------
export const auth = {
  login: (body: LoginRequest) =>
    apiFetch<AuthResponse>("/auth/login", {
      method: "POST",
      body,
      skipAuth: true,
      retryOnUnauthorized: false,
    }),
  register: (body: RegisterRequest) =>
    apiFetch<AuthResponse>("/auth/register", {
      method: "POST",
      body,
      skipAuth: true,
      retryOnUnauthorized: false,
    }),
  logout: (refresh_token: string) =>
    apiFetch<void>("/auth/logout", {
      method: "POST",
      body: { refresh_token },
    }),
  me: () => apiFetch<MeResponse>("/me"),
};

// --- audiobooks ----------------------------------------------------------
export const audiobooks = {
  list: () => apiFetch<AudiobookList>("/audiobook"),
  get: (id: string, language?: string) =>
    apiFetch<AudiobookDetail>(
      `/audiobook/${id}${language ? `?language=${encodeURIComponent(language)}` : ""}`,
    ),
  create: (body: CreateAudiobookRequest) =>
    apiFetch<AudiobookDetail>("/audiobook", { method: "POST", body }),
  patch: (id: string, body: UpdateAudiobookRequest) =>
    apiFetch<AudiobookDetail>(`/audiobook/${id}`, { method: "PATCH", body }),
  remove: (id: string) => apiFetch<void>(`/audiobook/${id}`, { method: "DELETE" }),
  regenerateCover: (id: string) =>
    apiFetch<AudiobookDetail>(`/audiobook/${id}/cover`, { method: "POST" }),
  costs: (id: string) =>
    apiFetch<AudiobookCostSummary>(`/audiobook/${id}/costs`),
  translate: (id: string, body: TranslateRequest) =>
    apiFetch<TranslateResponse>(`/audiobook/${id}/translate`, {
      method: "POST",
      body,
    }),
  generateChapters: (id: string, idempotencyKey?: string) =>
    apiFetch<void>(`/audiobook/${id}/generate-chapters`, {
      method: "POST",
      headers: idempotencyKey ? { "idempotency-key": idempotencyKey } : undefined,
    }),
  generateAudio: (id: string, language?: string, idempotencyKey?: string) =>
    apiFetch<void>(
      `/audiobook/${id}/generate-audio${language ? `?language=${encodeURIComponent(language)}` : ""}`,
      {
        method: "POST",
        headers: idempotencyKey ? { "idempotency-key": idempotencyKey } : undefined,
      },
    ),
  /** Kick off the per-chapter animation render job. The fan-out
   * progress is delivered via the existing audiobook progress
   * websocket (`useProgressSocket`) under the `animate` and
   * `animate_chapter` job kinds. */
  animate: (
    id: string,
    opts?: { language?: string; theme?: AnimationTheme; idempotencyKey?: string },
  ) => {
    const qs = new URLSearchParams();
    if (opts?.language) qs.set("language", opts.language);
    if (opts?.theme) qs.set("theme", opts.theme);
    const suffix = qs.toString() ? `?${qs.toString()}` : "";
    return apiFetch<void>(`/audiobook/${id}/animate${suffix}`, {
      method: "POST",
      headers: opts?.idempotencyKey
        ? { "idempotency-key": opts.idempotencyKey }
        : undefined,
    });
  },
  /** Re-render a single chapter's animated MP4. The backend busts the
   * F.1e spec-hash cache + deletes the existing ch-N.video.mp4 before
   * enqueueing a single AnimateChapter job, so the client should expect
   * the inline preview to 404 until the new render completes. */
  animateChapter: (
    id: string,
    n: number,
    opts?: { language?: string; theme?: AnimationTheme; idempotencyKey?: string },
  ) => {
    const qs = new URLSearchParams();
    if (opts?.language) qs.set("language", opts.language);
    if (opts?.theme) qs.set("theme", opts.theme);
    const suffix = qs.toString() ? `?${qs.toString()}` : "";
    return apiFetch<void>(
      `/audiobook/${id}/chapter/${n}/animate${suffix}`,
      {
        method: "POST",
        headers: opts?.idempotencyKey
          ? { "idempotency-key": opts.idempotencyKey }
          : undefined,
      },
    );
  },
  patchChapter: (id: string, n: number, body: UpdateChapterRequest) =>
    apiFetch<ChapterSummary>(`/audiobook/${id}/chapter/${n}`, { method: "PATCH", body }),
  regenerateChapter: (id: string, n: number) =>
    apiFetch<ChapterSummary>(`/audiobook/${id}/chapter/${n}/regenerate`, { method: "POST" }),
  regenerateChapterAudio: (id: string, n: number) =>
    apiFetch<ChapterSummary>(`/audiobook/${id}/chapter/${n}/regenerate-audio`, {
      method: "POST",
    }),
  /** Phase G — re-run the per-paragraph visual classifier against
   * an existing chapter without rewriting its body. Backfill helper
   * for books that were generated before STEM was enabled.
   * Requires `is_stem=true` on the book; the backend 400s otherwise.
   * Returns the updated chapter summary so the diagram badge
   * refreshes on success. */
  classifyChapterVisuals: (id: string, n: number) =>
    apiFetch<ChapterSummary>(
      `/audiobook/${id}/chapter/${n}/classify-visuals`,
      { method: "POST" },
    ),
  /** Phase H — re-run the bespoke Manim code-gen LLM against every
   * paragraph in this chapter labelled `custom_manim`. Used as a
   * backfill for books generated before Phase H landed, or to
   * refresh the diagrams after the user reassigns `LlmRole::ManimCode`
   * to a different model. 400s when the chapter has no custom_manim
   * paragraphs (the frontend hides the button in that case). */
  regenerateChapterManimCode: (id: string, n: number) =>
    apiFetch<ChapterSummary>(
      `/audiobook/${id}/chapter/${n}/regenerate-manim-code`,
      { method: "POST" },
    ),
  /** Owner-scoped LLM test for the bespoke Manim code-gen prompt.
   * Runs the prompt against the chosen paragraph through the chosen
   * LLM and returns the rendered prompt + raw response + cost +
   * elapsed ms — without persisting anything. Lets the user audition
   * different LLMs before reassigning `LlmRole::ManimCode`. */
  testChapterManimLlm: (
    id: string,
    n: number,
    body: import("./types").TestChapterManimLlmRequest,
  ) =>
    apiFetch<import("./types").TestChapterManimLlmResponse>(
      `/audiobook/${id}/chapter/${n}/test-manim-llm`,
      { method: "POST", body },
    ),
  /** Render the Manim code returned by `testChapterManimLlm` into a
   * throwaway MP4 we can preview inline. The backend writes to a
   * per-audiobook test-manim/<uuid>.mp4 file; pair the returned
   * `test_id` with `audiobookTestManimVideoUrl(...)` to build the
   * `<video src=…>` URL. */
  renderTestManim: (
    id: string,
    n: number,
    body: import("./types").RenderTestManimRequest,
  ) =>
    apiFetch<import("./types").RenderTestManimResponse>(
      `/audiobook/${id}/chapter/${n}/test-manim-render`,
      { method: "POST", body },
    ),
  regenerateChapterArt: (id: string, n: number) =>
    apiFetch<ChapterSummary>(`/audiobook/${id}/chapter/${n}/art`, {
      method: "POST",
    }),
  cancelPipeline: (id: string) =>
    apiFetch<void>(`/audiobook/${id}/cancel-pipeline`, { method: "POST" }),
};

// --- catalog + topics ----------------------------------------------------
export const catalog = {
  voices: () => apiFetch<VoiceList>("/voices"),
  previewVoice: (id: string) =>
    apiFetch<VoicePreviewResponse>(
      `/voices/${encodeURIComponent(id)}/preview`,
    ),
  llms: () => apiFetch<import("./types").LlmList>("/llms"),
  audiobookCategories: () =>
    apiFetch<AudiobookCategoryNameList>("/audiobook-categories"),
};

export const topics = {
  random: (body: RandomTopicRequest) =>
    apiFetch<RandomTopicResponse>("/topics/random", { method: "POST", body }),
  templates: () => apiFetch<TopicTemplateList>("/topic-templates"),
};

// --- cover art ---------------------------------------------------------
export const coverArt = {
  preview: (body: CoverPreviewRequest) =>
    apiFetch<CoverPreviewResponse>("/cover-art/preview", { method: "POST", body }),
};

/** URL of the saved cover for an audiobook. Used as `<img src>`. */
export function coverImageUrl(audiobookId: string, accessToken: string): string {
  return `/api/audiobook/${audiobookId}/cover?access_token=${encodeURIComponent(accessToken)}`;
}

export function chapterArtUrl(
  audiobookId: string,
  chapter: number,
  accessToken: string,
  language?: string,
): string {
  const qs = new URLSearchParams({ access_token: accessToken });
  if (language) qs.set("language", language);
  return `/api/audiobook/${audiobookId}/chapter/${chapter}/art?${qs.toString()}`;
}

/**
 * URL for the per-chapter animated companion video. 404s until the
 * `animate` job has produced `ch-N.video.mp4` for the requested
 * language.
 */
export function chapterVideoUrl(
  audiobookId: string,
  chapter: number,
  accessToken: string,
  language?: string,
): string {
  const qs = new URLSearchParams({ access_token: accessToken });
  if (language) qs.set("language", language);
  return `/api/audiobook/${audiobookId}/chapter/${chapter}/video?${qs.toString()}`;
}

/** URL for a throwaway test-manim render produced by the LLM test
 * dialog. `testId` is the UUID returned by `renderTestManim`. */
export function audiobookTestManimVideoUrl(
  audiobookId: string,
  testId: string,
  accessToken: string,
): string {
  const qs = new URLSearchParams({ access_token: accessToken });
  return `/api/audiobook/${audiobookId}/test-manim/${testId}?${qs.toString()}`;
}

/**
 * URL for a paragraph illustration tile.
 * `paragraphIndex` matches `chapter.paragraphs[].index`; `ordinal` is
 * the 1-based tile slot (1..=images_per_paragraph).
 */
export function paragraphImageUrl(
  audiobookId: string,
  chapter: number,
  paragraphIndex: number,
  ordinal: number,
  accessToken: string,
  language?: string,
): string {
  const qs = new URLSearchParams({ access_token: accessToken });
  if (language) qs.set("language", language);
  return `/api/audiobook/${audiobookId}/chapter/${chapter}/paragraph/${paragraphIndex}/image/${ordinal}?${qs.toString()}`;
}

// --- jobs ----------------------------------------------------------------
export const jobs = {
  listForAudiobook: (id: string) => apiFetch<AudiobookJobList>(`/audiobook/${id}/jobs`),
};

// --- ideas --------------------------------------------------------------
export const ideas = {
  list: () => apiFetch<IdeaList>("/ideas"),
  create: (body: CreateIdeaRequest) =>
    apiFetch<IdeaRow>("/ideas", { method: "POST", body }),
  patch: (id: string, body: UpdateIdeaRequest) =>
    apiFetch<IdeaRow>(`/ideas/${id}`, { method: "PATCH", body }),
  remove: (id: string) =>
    apiFetch<void>(`/ideas/${id}`, { method: "DELETE" }),
  suggest: (body: SuggestIdeasRequest) =>
    apiFetch<SuggestIdeasResponse>("/ideas/suggest", {
      method: "POST",
      body,
    }),
};

// --- podcasts -----------------------------------------------------------
export const podcasts = {
  list: () => apiFetch<PodcastList>("/podcasts"),
  get: (id: string) => apiFetch<PodcastRow>(`/podcasts/${id}`),
  create: (body: CreatePodcastRequest) =>
    apiFetch<PodcastRow>("/podcasts", { method: "POST", body }),
  patch: (id: string, body: UpdatePodcastRequest) =>
    apiFetch<PodcastRow>(`/podcasts/${id}`, { method: "PATCH", body }),
  remove: (id: string) =>
    apiFetch<void>(`/podcasts/${id}`, { method: "DELETE" }),
  previewImage: (body: PreviewPodcastImageRequest) =>
    apiFetch<PreviewPodcastImageResponse>("/podcasts/preview-image", {
      method: "POST",
      body,
    }),
  syncYoutube: (id: string) =>
    apiFetch<SyncPodcastResponse>(`/podcasts/${id}/sync-youtube`, {
      method: "POST",
    }),
};

/** URL of the saved podcast cover image. Used as `<img src>`. */
export function podcastImageUrl(podcastId: string, accessToken: string): string {
  return `/api/podcasts/${podcastId}/image?access_token=${encodeURIComponent(accessToken)}`;
}

// --- integrations (YouTube) ---------------------------------------------
export const integrations = {
  youtube: {
    /** Start the consent dance. Caller is expected to set
     *  `window.location = res.url` to actually navigate. */
    oauthStart: () =>
      apiFetch<OauthStartResponse>("/integrations/youtube/oauth/start"),
    /** Whether the calling user has connected a YouTube channel. */
    account: () =>
      apiFetch<YoutubeAccountStatus>("/integrations/youtube/account"),
    /** Revoke at Google + delete the local row. */
    disconnect: () =>
      apiFetch<void>("/integrations/youtube/account", { method: "DELETE" }),
    publish: (audiobookId: string, body: PublishYoutubeRequest) =>
      apiFetch<PublishYoutubeResponse>(
        `/audiobook/${audiobookId}/publish/youtube`,
        { method: "POST", body },
      ),
    listPublications: (audiobookId: string) =>
      apiFetch<PublicationList>(`/audiobook/${audiobookId}/publications`),
    approve: (audiobookId: string, publicationId: string) =>
      apiFetch<ApprovePublicationResponse>(
        `/audiobook/${audiobookId}/publications/${publicationId}/approve`,
        { method: "POST" },
      ),
    cancel: (audiobookId: string, publicationId: string) =>
      apiFetch<void>(
        `/audiobook/${audiobookId}/publications/${publicationId}/cancel`,
        { method: "POST" },
      ),
  },
};

/** Streaming URL for the encoded MP4 preview. Pass to a `<video>` tag. */
export function publicationPreviewUrl(
  audiobookId: string,
  publicationId: string,
  accessToken: string,
  chapter?: number,
): string {
  const qs = new URLSearchParams({ access_token: accessToken });
  if (chapter != null) qs.set("chapter", String(chapter));
  return `/api/audiobook/${audiobookId}/publications/${publicationId}/preview?${qs.toString()}`;
}

// --- admin ---------------------------------------------------------------
export const admin = {
  system: () => apiFetch<SystemOverview>("/admin/system"),
  llms: {
    list: () => apiFetch<AdminLlmList>("/admin/llm"),
    create: (body: CreateLlmRequest) =>
      apiFetch<AdminLlmRow>("/admin/llm", { method: "POST", body }),
    patch: (id: string, body: UpdateLlmRequest) =>
      apiFetch<AdminLlmRow>(`/admin/llm/${id}`, { method: "PATCH", body }),
    remove: (id: string) =>
      apiFetch<void>(`/admin/llm/${id}`, { method: "DELETE" }),
  },
  voices: {
    list: () => apiFetch<AdminVoiceList>("/admin/voice"),
    patch: (id: string, body: UpdateVoiceRequest) =>
      apiFetch<AdminVoiceRow>(`/admin/voice/${id}`, { method: "PATCH", body }),
  },
  users: {
    list: (q?: { q?: string; role?: string; tier?: string }) => {
      const qs = new URLSearchParams();
      if (q?.q) qs.set("q", q.q);
      if (q?.role) qs.set("role", q.role);
      if (q?.tier) qs.set("tier", q.tier);
      const suffix = qs.toString() ? `?${qs.toString()}` : "";
      return apiFetch<AdminUserList>(`/admin/users${suffix}`);
    },
    patch: (id: string, body: UpdateUserRequest) =>
      apiFetch<AdminUserRow>(`/admin/users/${id}`, { method: "PATCH", body }),
    revokeSessions: (id: string) =>
      apiFetch<RevokeSessionsResponse>(`/admin/users/${id}/revoke-sessions`, {
        method: "POST",
      }),
  },
  jobs: {
    list: (q?: { status?: string; kind?: string }) => {
      const qs = new URLSearchParams();
      if (q?.status) qs.set("status", q.status);
      if (q?.kind) qs.set("kind", q.kind);
      const suffix = qs.toString() ? `?${qs.toString()}` : "";
      return apiFetch<AdminJobList>(`/admin/jobs${suffix}`);
    },
    retry: (id: string) =>
      apiFetch<void>(`/admin/jobs/${id}/retry`, { method: "POST" }),
    cancel: (id: string) =>
      apiFetch<void>(`/admin/jobs/${id}/cancel`, { method: "POST" }),
    remove: (id: string) =>
      apiFetch<void>(`/admin/jobs/${id}`, { method: "DELETE" }),
  },
  test: {
    llm: (body: TestLlmRequest) =>
      apiFetch<TestLlmResponse>("/admin/test/llm", { method: "POST", body }),
    voice: (body: TestVoiceRequest) =>
      apiFetch<TestVoiceResponse>("/admin/test/voice", { method: "POST", body }),
  },
  openrouter: {
    /**
     * Public OpenRouter catalog used by the LLM-add picker.
     *
     * `outputModalities` is forwarded to OpenRouter — pass `"image"` for the
     * full image-generation catalog, since the unfiltered endpoint hides
     * most image-only providers.
     */
    models: (outputModalities?: string) => {
      const qs = outputModalities
        ? `?output_modalities=${encodeURIComponent(outputModalities)}`
        : "";
      return apiFetch<OpenRouterModelList>(`/admin/openrouter/models${qs}`);
    },
  },
  xai: {
    /**
     * xAI's `/language-models` catalog. Requires the server to have
     * `xai_api_key` configured — returns 400 otherwise.
     */
    models: () => apiFetch<XaiModelList>("/admin/xai/models"),
    /** xAI's `/image-generation-models` catalog (Grok-2-Image et al). */
    imageModels: () => apiFetch<XaiImageModelList>("/admin/xai/image-models"),
  },
  youtubeSettings: {
    list: () => apiFetch<YoutubeFooterList>("/admin/youtube-settings"),
    upsert: (language: string, body: UpsertYoutubeFooterRequest) =>
      apiFetch<YoutubeFooterRow>(
        `/admin/youtube-settings/${encodeURIComponent(language)}`,
        { method: "PUT", body },
      ),
    remove: (language: string) =>
      apiFetch<void>(
        `/admin/youtube-settings/${encodeURIComponent(language)}`,
        { method: "DELETE" },
      ),
    getPublishSettings: () =>
      apiFetch<YoutubePublishSettings>("/admin/youtube-publish-settings"),
    putPublishSettings: (body: YoutubePublishSettings) =>
      apiFetch<YoutubePublishSettings>("/admin/youtube-publish-settings", {
        method: "PUT",
        body,
      }),
  },
  audiobookCategories: {
    list: () =>
      apiFetch<AudiobookCategoryList>("/admin/audiobook-categories"),
    create: (body: CreateAudiobookCategoryRequest) =>
      apiFetch<AudiobookCategoryRow>("/admin/audiobook-categories", {
        method: "POST",
        body,
      }),
    update: (id: string, body: UpdateAudiobookCategoryRequest) =>
      apiFetch<AudiobookCategoryRow>(
        `/admin/audiobook-categories/${encodeURIComponent(id)}`,
        { method: "PATCH", body },
      ),
    remove: (id: string) =>
      apiFetch<void>(
        `/admin/audiobook-categories/${encodeURIComponent(id)}`,
        { method: "DELETE" },
      ),
  },
  topicTemplates: {
    list: () => apiFetch<TopicTemplateList>("/admin/topic-templates"),
    create: (body: CreateTopicTemplateRequest) =>
      apiFetch<TopicTemplate>("/admin/topic-templates", {
        method: "POST",
        body,
      }),
    patch: (id: string, body: UpdateTopicTemplateRequest) =>
      apiFetch<TopicTemplate>(`/admin/topic-templates/${id}`, {
        method: "PATCH",
        body,
      }),
    remove: (id: string) =>
      apiFetch<void>(`/admin/topic-templates/${id}`, { method: "DELETE" }),
  },
};

// --- helpers -------------------------------------------------------------

/** URL of a chapter's audio stream. Used as the `<audio src>`. */
export function chapterAudioUrl(
  audiobookId: string,
  chapter: number,
  language?: string,
): string {
  const lang = language ? `&language=${encodeURIComponent(language)}` : "";
  return `/api/audiobook/${audiobookId}/chapter/${chapter}/audio${lang ? `?${lang.slice(1)}` : ""}`;
}

/** WebSocket URL for live progress, authed via `?access_token`. */
export function progressWebSocketUrl(audiobookId: string, accessToken: string): string {
  const base = window.location.origin.replace(/^http/, "ws");
  return `${base}/api/ws/audiobook/${audiobookId}?access_token=${encodeURIComponent(accessToken)}`;
}

// Suppress "unused" warning on the summary type when it's only used through
// AudiobookList — TS keeps the alias but the compiler doesn't without this.
export type { AudiobookSummary };
