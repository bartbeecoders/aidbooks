import { ReactNode } from "react";
import { Navigate, useLocation } from "react-router-dom";
import { useAuth } from "../store/auth";

/**
 * Route wrapper that forwards to `/login` when there is no access token,
 * preserving the intended destination in `location.state.from` so the login
 * page can send the user back where they came from.
 */
export function RequireAuth({ children }: { children: ReactNode }): JSX.Element {
  const accessToken = useAuth((s) => s.accessToken);
  const location = useLocation();
  if (!accessToken) {
    return <Navigate to="/login" replace state={{ from: location.pathname }} />;
  }
  return <>{children}</>;
}
