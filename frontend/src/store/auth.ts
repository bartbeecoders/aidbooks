// Auth state: persisted access + refresh tokens and the last-known user row.
//
// We persist both tokens in localStorage. Access tokens are short-lived (15
// min) so the blast radius of XSS exfiltration is bounded to that window
// plus one refresh rotation — acceptable for a v1 web client. A production
// hardening pass would migrate refresh tokens to an httpOnly cookie.

import { create } from "zustand";
import { persist, createJSONStorage } from "zustand/middleware";
import type { User } from "../api/types";

type AuthPatch = Partial<AuthState> & Pick<AuthState, "accessToken" | "refreshToken">;

export type AuthState = {
  accessToken: string | null;
  refreshToken: string | null;
  user: User | null;
};

type AuthActions = {
  set: (patch: AuthPatch | { user: User | null }) => void;
  clear: () => void;
};

const initial: AuthState = {
  accessToken: null,
  refreshToken: null,
  user: null,
};

export const useAuth = create<AuthState & AuthActions>()(
  persist(
    (set) => ({
      ...initial,
      set: (patch) =>
        set((prev) => ({
          ...prev,
          ...patch,
        })),
      clear: () => set({ ...initial }),
    }),
    {
      name: "listenai.auth",
      storage: createJSONStorage(() => localStorage),
      // Only persist the tokens + user; action refs must not be serialised.
      partialize: (s) => ({
        accessToken: s.accessToken,
        refreshToken: s.refreshToken,
        user: s.user,
      }),
    },
  ),
);

// --- Imperative helpers for non-React contexts (api/client.ts) -----------
//
// The fetch wrapper lives outside the React tree but still needs to read +
// write tokens. These escape hatches avoid the "use hook outside component"
// error and the brittleness of wrapping the wrapper in a hook.

export function getAuth(): AuthState {
  const { accessToken, refreshToken, user } = useAuth.getState();
  return { accessToken, refreshToken, user };
}

export function setAuth(patch: AuthPatch | { user: User | null }): void {
  useAuth.getState().set(patch);
}

export function logout(): void {
  useAuth.getState().clear();
}

export function selectAccessToken(s: AuthState): string | null {
  return s.accessToken;
}
