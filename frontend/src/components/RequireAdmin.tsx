import { ReactNode } from "react";
import { Navigate } from "react-router-dom";
import { useAuth } from "../store/auth";

/**
 * Route wrapper that forwards non-admins to `/app`. The backend rejects admin
 * routes with 403 for non-admins regardless, so this is a convenience + UX
 * guard, not a security boundary.
 */
export function RequireAdmin({ children }: { children: ReactNode }): JSX.Element {
  const user = useAuth((s) => s.user);
  const accessToken = useAuth((s) => s.accessToken);
  if (!accessToken) return <Navigate to="/login" replace />;
  if (user?.role !== "admin") return <Navigate to="/app" replace />;
  return <>{children}</>;
}
