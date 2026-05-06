// Auth-aware JSON fetch wrapper.
//
// Single chokepoint that every UI call flows through so refresh-token rotation
// can be implemented once and future concerns (retry, correlation IDs, error
// surfaces) land here rather than in individual screens.

import { getAuth, setAuth, logout, selectAccessToken } from "../store/auth";
import type { ErrorBody } from "./types";

const BASE = "/api";

export class ApiError extends Error {
  constructor(
    public status: number,
    public code: string,
    public requestId: string | null,
    message: string,
  ) {
    super(message);
    this.name = "ApiError";
  }
}

type RequestOptions = {
  method?: "GET" | "POST" | "PUT" | "PATCH" | "DELETE";
  body?: unknown;
  headers?: Record<string, string>;
  // Skip the Authorization header even if we have a token. Used by login /
  // refresh themselves so we don't send a dead token on the way in.
  skipAuth?: boolean;
  // If true, a 401 triggers a one-shot refresh-and-retry. We disable this on
  // the refresh endpoint itself to avoid loops.
  retryOnUnauthorized?: boolean;
};

/**
 * Core fetch that knows about our auth tokens + error shape. Response body is
 * parsed as JSON unless the server returns 204 (`No Content`) or a non-JSON
 * `content-type` (binary streams for audio/waveform use a different helper).
 */
export async function apiFetch<T>(path: string, opts: RequestOptions = {}): Promise<T> {
  const {
    method = "GET",
    body,
    headers = {},
    skipAuth = false,
    retryOnUnauthorized = true,
  } = opts;

  const res = await doFetch(path, { method, body, headers, skipAuth });

  if (res.status === 401 && retryOnUnauthorized && !skipAuth) {
    const refreshed = await tryRefresh();
    if (refreshed) {
      const retried = await doFetch(path, { method, body, headers, skipAuth });
      return unpack<T>(retried);
    }
    logout();
  }

  return unpack<T>(res);
}

async function doFetch(
  path: string,
  { method, body, headers, skipAuth }: Required<Pick<RequestOptions, "method" | "skipAuth">> & {
    body?: unknown;
    headers: Record<string, string>;
  },
): Promise<Response> {
  const finalHeaders: Record<string, string> = {
    Accept: "application/json",
    ...headers,
  };
  if (body !== undefined) finalHeaders["Content-Type"] = "application/json";
  if (!skipAuth) {
    const token = selectAccessToken(getAuth());
    if (token) finalHeaders.Authorization = `Bearer ${token}`;
  }
  return fetch(`${BASE}${path}`, {
    method,
    headers: finalHeaders,
    body: body !== undefined ? JSON.stringify(body) : undefined,
  });
}

async function unpack<T>(res: Response): Promise<T> {
  if (res.status === 204 || res.status === 205) return undefined as T;
  const reqId = res.headers.get("x-request-id");

  // Error path: parse the ErrorBody contract and throw a typed ApiError. Any
  // 4xx/5xx that doesn't return the expected JSON falls back to a bare
  // message so the UI never shows "Unexpected token < in JSON …".
  if (!res.ok) {
    let body: Partial<ErrorBody> = {};
    try {
      body = (await res.json()) as ErrorBody;
    } catch {
      /* swallow; use defaults */
    }
    throw new ApiError(
      res.status,
      body.code ?? "http_error",
      reqId,
      body.message ?? `HTTP ${res.status}`,
    );
  }
  const contentType = res.headers.get("content-type") ?? "";
  const contentLength = res.headers.get("content-length");
  if (contentLength === "0" || !contentType.includes("application/json")) {
    return undefined as T;
  }
  return (await res.json()) as T;
}

/**
 * Single-flight refresh. If two 401s hit at once they share the same network
 * round-trip rather than each stealing the other's rotated refresh token.
 */
let refreshInflight: Promise<boolean> | null = null;
function tryRefresh(): Promise<boolean> {
  if (refreshInflight) return refreshInflight;
  refreshInflight = (async () => {
    const auth = getAuth();
    if (!auth.refreshToken) return false;
    try {
      const res = await fetch(`${BASE}/auth/refresh`, {
        method: "POST",
        headers: { "Content-Type": "application/json", Accept: "application/json" },
        body: JSON.stringify({ refresh_token: auth.refreshToken }),
      });
      if (!res.ok) return false;
      const next = (await res.json()) as {
        access_token: string;
        refresh_token: string;
        user: unknown;
      };
      setAuth({
        accessToken: next.access_token,
        refreshToken: next.refresh_token,
      });
      return true;
    } catch {
      return false;
    } finally {
      // Allow the next 401 cycle to trigger another refresh.
      refreshInflight = null;
    }
  })();
  return refreshInflight;
}
