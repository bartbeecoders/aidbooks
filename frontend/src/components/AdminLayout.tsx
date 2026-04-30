import { NavLink, Outlet } from "react-router-dom";

const SECTIONS: Array<{ to: string; label: string }> = [
  { to: "/admin", label: "Overview" },
  { to: "/admin/llm", label: "LLMs" },
  { to: "/admin/image-llm", label: "Image LLMs" },
  { to: "/admin/voice", label: "Voices" },
  { to: "/admin/topic-templates", label: "Topic templates" },
  { to: "/admin/youtube-settings", label: "YouTube settings" },
  { to: "/admin/test-llm", label: "Test LLM" },
  { to: "/admin/test-voice", label: "Test voice" },
  { to: "/admin/users", label: "Users" },
  { to: "/admin/jobs", label: "Jobs" },
];

export function AdminLayout(): JSX.Element {
  return (
    <div className="grid gap-8 md:grid-cols-[200px,1fr]">
      <aside className="space-y-1 text-sm">
        <p className="px-2 pb-2 text-xs font-semibold uppercase tracking-wide text-slate-500">
          Admin
        </p>
        {SECTIONS.map((s) => (
          <NavLink
            key={s.to}
            to={s.to}
            end={s.to === "/admin"}
            className={({ isActive }) =>
              `block rounded-md px-3 py-1.5 ${
                isActive
                  ? "bg-slate-800 text-slate-100"
                  : "text-slate-400 hover:bg-slate-900 hover:text-slate-200"
              }`
            }
          >
            {s.label}
          </NavLink>
        ))}
      </aside>
      <section>
        <Outlet />
      </section>
    </div>
  );
}
