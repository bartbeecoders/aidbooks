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
  TranslateRequest,
  TranslateResponse,
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
  UpdateUserRequest,
  UpdateVoiceRequest,
  VoiceList,
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
  patchChapter: (id: string, n: number, body: UpdateChapterRequest) =>
    apiFetch<ChapterSummary>(`/audiobook/${id}/chapter/${n}`, { method: "PATCH", body }),
  regenerateChapter: (id: string, n: number) =>
    apiFetch<ChapterSummary>(`/audiobook/${id}/chapter/${n}/regenerate`, { method: "POST" }),
  regenerateChapterAudio: (id: string, n: number) =>
    apiFetch<ChapterSummary>(`/audiobook/${id}/chapter/${n}/regenerate-audio`, {
      method: "POST",
    }),
};

// --- catalog + topics ----------------------------------------------------
export const catalog = {
  voices: () => apiFetch<VoiceList>("/voices"),
  llms: () => apiFetch<import("./types").LlmList>("/llms"),
};

export const topics = {
  random: (body: RandomTopicRequest) =>
    apiFetch<RandomTopicResponse>("/topics/random", { method: "POST", body }),
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

// --- jobs ----------------------------------------------------------------
export const jobs = {
  listForAudiobook: (id: string) => apiFetch<AudiobookJobList>(`/audiobook/${id}/jobs`),
};

// --- admin ---------------------------------------------------------------
export const admin = {
  system: () => apiFetch<SystemOverview>("/admin/system"),
  llms: {
    list: () => apiFetch<AdminLlmList>("/admin/llm"),
    create: (body: CreateLlmRequest) =>
      apiFetch<AdminLlmRow>("/admin/llm", { method: "POST", body }),
    patch: (id: string, body: UpdateLlmRequest) =>
      apiFetch<AdminLlmRow>(`/admin/llm/${id}`, { method: "PATCH", body }),
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
  },
  test: {
    llm: (body: TestLlmRequest) =>
      apiFetch<TestLlmResponse>("/admin/test/llm", { method: "POST", body }),
    voice: (body: TestVoiceRequest) =>
      apiFetch<TestVoiceResponse>("/admin/test/voice", { method: "POST", body }),
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
