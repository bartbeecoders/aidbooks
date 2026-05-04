import { Link, NavLink, Outlet, useNavigate } from "react-router-dom";
import { auth } from "../api";
import { useAuth, logout } from "../store/auth";

export function AppLayout(): JSX.Element {
  const user = useAuth((s) => s.user);
  const refreshToken = useAuth((s) => s.refreshToken);
  const navigate = useNavigate();

  const onLogout = async (): Promise<void> => {
    if (refreshToken) {
      // Best-effort server-side revoke. Navigating home even if it fails —
      // a stale session is less annoying than a stuck button.
      try {
        await auth.logout(refreshToken);
      } catch {
        /* swallow */
      }
    }
    logout();
    navigate("/login", { replace: true });
  };

  return (
    <div className="min-h-screen">
      <header className="border-b border-slate-800 bg-slate-950/80 backdrop-blur">
        <div className="mx-auto flex max-w-[1600px] items-center justify-between px-6 py-3">
          <Link to="/app" className="text-lg font-semibold tracking-tight">
            ListenAI
          </Link>
          <nav className="flex items-center gap-2 text-sm">
            <NavItem to="/app">Library</NavItem>
            <NavItem to="/app/ideas">Ideas</NavItem>
            <NavItem to="/app/settings">Settings</NavItem>
            {user?.role === "admin" && <NavItem to="/admin">Admin</NavItem>}
            <div className="mx-3 h-5 w-px bg-slate-800" />
            {user && (
              <span className="text-slate-400">
                {user.display_name || user.email}
              </span>
            )}
            <button
              onClick={onLogout}
              className="rounded-md border border-slate-800 bg-slate-900 px-2.5 py-1 text-xs text-slate-300 hover:border-slate-700 hover:text-slate-100"
            >
              Sign out
            </button>
          </nav>
        </div>
      </header>
      <main className="mx-auto max-w-[1600px] px-6 py-8">
        <Outlet />
      </main>
    </div>
  );
}

function NavItem({ to, children }: { to: string; children: React.ReactNode }): JSX.Element {
  return (
    <NavLink
      to={to}
      end
      className={({ isActive }) =>
        `rounded-md px-2.5 py-1 ${
          isActive
            ? "bg-slate-800 text-slate-100"
            : "text-slate-400 hover:bg-slate-900 hover:text-slate-200"
        }`
      }
    >
      {children}
    </NavLink>
  );
}
