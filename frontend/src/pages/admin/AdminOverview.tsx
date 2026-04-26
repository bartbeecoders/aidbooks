import { useQuery } from "@tanstack/react-query";
import { Link } from "react-router-dom";
import { admin } from "../../api";
import { ErrorPane, Loading, PageHeader } from "./AdminLlms";

export function AdminOverview(): JSX.Element {
  const { data, isLoading, error } = useQuery({
    queryKey: ["admin", "system"],
    queryFn: () => admin.system(),
    refetchInterval: 10_000,
  });

  if (isLoading) return <Loading />;
  if (error) return <ErrorPane error={error} />;
  if (!data) return <p>No data.</p>;

  return (
    <div>
      <PageHeader
        title="System overview"
        description="Counts, storage footprint, and provider mode. Updated every 10 s."
      />

      <div className="grid gap-3 sm:grid-cols-2 lg:grid-cols-4">
        <Stat label="Users" value={data.users_total} />
        <Stat label="Audiobooks" value={data.audiobooks_total} />
        <Stat label="Chapters" value={data.chapters_total} />
        <Stat label="Storage" value={formatBytes(data.storage_bytes)} />
        <Stat label="Jobs queued" value={data.jobs_queued} hue="amber" />
        <Stat label="Jobs running" value={data.jobs_running} hue="sky" />
        <Stat label="Completed / 24 h" value={data.jobs_completed_24h} hue="emerald" />
        <Stat label="Dead" value={data.jobs_dead} hue="rose" />
      </div>

      <section className="mt-8 rounded-xl border border-slate-800 bg-slate-900/40 p-4 text-sm">
        <h2 className="mb-3 text-xs font-semibold uppercase tracking-wide text-slate-500">
          Runtime
        </h2>
        <dl className="grid gap-2 sm:grid-cols-2">
          <KV k="DB path" v={<code>{data.db_path}</code>} />
          <KV k="Storage path" v={<code>{data.storage_path}</code>} />
          <KV
            k="OpenRouter"
            v={
              data.llm_mock_mode ? (
                <Tag tone="amber">mock mode</Tag>
              ) : (
                <Tag tone="emerald">live</Tag>
              )
            }
          />
          <KV
            k="x.ai"
            v={
              data.tts_mock_mode ? (
                <Tag tone="amber">mock mode</Tag>
              ) : (
                <Tag tone="emerald">live</Tag>
              )
            }
          />
        </dl>
      </section>

      <section className="mt-6 rounded-xl border border-slate-800 bg-slate-900/40 p-4 text-sm">
        <h2 className="mb-3 text-xs font-semibold uppercase tracking-wide text-slate-500">
          Quick actions
        </h2>
        <div className="flex flex-wrap gap-2">
          {data.jobs_dead > 0 && (
            <Link
              to="/admin/jobs"
              className="rounded-md border border-rose-800 bg-rose-950 px-3 py-1.5 text-xs text-rose-200 hover:bg-rose-900"
            >
              {data.jobs_dead} dead job{data.jobs_dead === 1 ? "" : "s"} → Jobs →
            </Link>
          )}
          <Link
            to="/admin/users"
            className="rounded-md border border-slate-700 bg-slate-900 px-3 py-1.5 text-xs text-slate-200 hover:border-slate-600"
          >
            Manage users
          </Link>
          <Link
            to="/admin/llm"
            className="rounded-md border border-slate-700 bg-slate-900 px-3 py-1.5 text-xs text-slate-200 hover:border-slate-600"
          >
            Manage LLMs
          </Link>
        </div>
      </section>
    </div>
  );
}

type Hue = "slate" | "amber" | "sky" | "emerald" | "rose";

const HUES: Record<Hue, string> = {
  slate: "border-slate-800 bg-slate-900/50 text-slate-300",
  amber: "border-amber-900 bg-amber-950/40 text-amber-200",
  sky: "border-sky-900 bg-sky-950/40 text-sky-200",
  emerald: "border-emerald-900 bg-emerald-950/40 text-emerald-200",
  rose: "border-rose-900 bg-rose-950/40 text-rose-200",
};

function Stat({
  label,
  value,
  hue = "slate",
}: {
  label: string;
  value: number | string;
  hue?: Hue;
}): JSX.Element {
  return (
    <div className={`rounded-xl border p-4 ${HUES[hue]}`}>
      <div className="text-[11px] uppercase tracking-wide opacity-75">{label}</div>
      <div className="mt-1 text-2xl font-semibold">{value}</div>
    </div>
  );
}

function KV({ k, v }: { k: string; v: React.ReactNode }): JSX.Element {
  return (
    <div className="flex items-center gap-3">
      <dt className="w-32 text-slate-500">{k}</dt>
      <dd className="min-w-0 flex-1 break-all text-slate-200">{v}</dd>
    </div>
  );
}

function Tag({
  tone,
  children,
}: {
  tone: "emerald" | "amber";
  children: React.ReactNode;
}): JSX.Element {
  const cls =
    tone === "emerald"
      ? "bg-emerald-900 text-emerald-200"
      : "bg-amber-950 text-amber-300";
  return (
    <span className={`inline-block rounded-full px-2 py-0.5 text-[11px] ${cls}`}>
      {children}
    </span>
  );
}

function formatBytes(n: number): string {
  if (n < 1024) return `${n} B`;
  if (n < 1024 * 1024) return `${(n / 1024).toFixed(1)} KB`;
  if (n < 1024 * 1024 * 1024) return `${(n / (1024 * 1024)).toFixed(1)} MB`;
  return `${(n / (1024 * 1024 * 1024)).toFixed(2)} GB`;
}
