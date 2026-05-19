import { useMemo, useState } from "react";
import { useQuery } from "@tanstack/react-query";
import {
  Bar,
  BarChart,
  CartesianGrid,
  Line,
  LineChart,
  ResponsiveContainer,
  Tooltip,
  XAxis,
  YAxis,
  Legend,
} from "recharts";
import { Link } from "react-router-dom";
import { ApiError, analytics } from "../api";
import type {
  AnalyticsBucket,
  GenerationPoint,
  YoutubeReportPoint,
  YoutubeVideoRow,
} from "../api";

/**
 * Owner-scoped analytics dashboard.
 *
 * Stitches three backend feeds into one page:
 *
 *   1. `/analytics/generation` — local-DB time series (counts, duration,
 *      cost) per content type.
 *   2. `/analytics/youtube/{channel,videos,reports}` — YouTube
 *      performance, surfaced only when the user has connected their
 *      channel. A 409 from any of them renders an inline "connect
 *      YouTube first" surface pointing at Settings.
 *
 * The bucket selector is shared between the local series and the
 * YouTube Analytics report so the two stay synced on screen — every
 * chart redraws on the same x-axis grain.
 */
export function Analytics(): JSX.Element {
  const [bucket, setBucket] = useState<AnalyticsBucket>("day");
  const [rangeDays, setRangeDays] = useState<number>(30);

  return (
    <div className="space-y-8">
      <header className="flex flex-col gap-2 sm:flex-row sm:items-end sm:justify-between">
        <div>
          <h1 className="text-2xl font-semibold tracking-tight">Analytics</h1>
          <p className="text-sm text-slate-400">
            Generation throughput, cost, and YouTube performance for your
            account.
          </p>
        </div>
        <BucketControls
          bucket={bucket}
          rangeDays={rangeDays}
          onBucket={setBucket}
          onRangeDays={setRangeDays}
        />
      </header>

      <GenerationSection bucket={bucket} rangeDays={rangeDays} />
      <YoutubeChannelSection />
      <YoutubeReportSection bucket={bucket} rangeDays={rangeDays} />
      <YoutubeVideosSection />
    </div>
  );
}

// ----------------------------------------------------------------------
// Bucket + range selector
// ----------------------------------------------------------------------

function BucketControls({
  bucket,
  rangeDays,
  onBucket,
  onRangeDays,
}: {
  bucket: AnalyticsBucket;
  rangeDays: number;
  onBucket: (b: AnalyticsBucket) => void;
  onRangeDays: (n: number) => void;
}): JSX.Element {
  return (
    <div className="flex items-center gap-2 text-sm">
      <div className="inline-flex overflow-hidden rounded-md border border-slate-800">
        {(["day", "week", "month"] as AnalyticsBucket[]).map((b) => (
          <button
            key={b}
            onClick={() => onBucket(b)}
            className={
              b === bucket
                ? "bg-slate-700 px-3 py-1 text-slate-100"
                : "bg-slate-900 px-3 py-1 text-slate-400 hover:text-slate-200"
            }
          >
            {b[0].toUpperCase() + b.slice(1)}
          </button>
        ))}
      </div>
      <select
        value={rangeDays}
        onChange={(e) => onRangeDays(Number(e.target.value))}
        className="rounded-md border border-slate-800 bg-slate-900 px-2 py-1 text-slate-200"
      >
        <option value={7}>7 days</option>
        <option value={30}>30 days</option>
        <option value={90}>90 days</option>
        <option value={180}>180 days</option>
        <option value={365}>365 days</option>
      </select>
    </div>
  );
}

// ----------------------------------------------------------------------
// Generation throughput / cost / duration (local DB)
// ----------------------------------------------------------------------

function GenerationSection({
  bucket,
  rangeDays,
}: {
  bucket: AnalyticsBucket;
  rangeDays: number;
}): JSX.Element {
  const { data, isLoading, error } = useQuery({
    queryKey: ["analytics", "generation", bucket, rangeDays],
    queryFn: () => analytics.generation({ bucket, rangeDays }),
  });

  const points: GenerationPoint[] = useMemo(
    () => data?.points ?? [],
    [data],
  );
  const totals = useMemo(() => {
    const t = {
      audiobooks: 0,
      shorts: 0,
      videos: 0,
      cost: 0,
      durationMs: 0,
    };
    for (const p of points) {
      t.audiobooks += p.audiobooks_count;
      t.shorts += p.shorts_count;
      t.videos += p.videos_count;
      t.cost += p.audiobooks_cost_usd + p.shorts_cost_usd;
      t.durationMs +=
        p.audiobooks_duration_ms +
        p.shorts_duration_ms +
        p.videos_duration_ms;
    }
    return t;
  }, [points]);

  return (
    <section className="space-y-4">
      <SectionHeader
        title="Generation"
        subtitle="Audiobooks, shorts, and published videos created in the selected window."
      />
      <div className="grid gap-3 sm:grid-cols-2 lg:grid-cols-4">
        <StatTile label="Audiobooks" value={totals.audiobooks.toLocaleString()} />
        <StatTile label="Shorts" value={totals.shorts.toLocaleString()} />
        <StatTile label="Videos published" value={totals.videos.toLocaleString()} />
        <StatTile
          label="Spend"
          value={`$${totals.cost.toFixed(2)}`}
          hint={`${formatDuration(totals.durationMs)} narrated`}
        />
      </div>
      {error ? (
        <ErrorBanner error={error} />
      ) : isLoading ? (
        <ChartSkeleton />
      ) : (
        <>
          <ChartCard title="Items per bucket">
            <ResponsiveContainer width="100%" height={260}>
              <BarChart data={points}>
                <CartesianGrid strokeDasharray="3 3" stroke="#1f2937" />
                <XAxis dataKey="date" stroke="#94a3b8" fontSize={11} />
                <YAxis stroke="#94a3b8" fontSize={11} allowDecimals={false} />
                <Tooltip
                  contentStyle={tooltipStyle}
                  cursor={{ fill: "rgba(148,163,184,0.08)" }}
                />
                <Legend wrapperStyle={{ color: "#cbd5e1", fontSize: 12 }} />
                <Bar
                  dataKey="audiobooks_count"
                  name="Audiobooks"
                  stackId="content"
                  fill="#6366f1"
                />
                <Bar
                  dataKey="shorts_count"
                  name="Shorts"
                  stackId="content"
                  fill="#22d3ee"
                />
                <Bar
                  dataKey="videos_count"
                  name="Videos"
                  fill="#f97316"
                />
              </BarChart>
            </ResponsiveContainer>
          </ChartCard>

          <div className="grid gap-3 lg:grid-cols-2">
            <ChartCard title="Narration duration (minutes)">
              <ResponsiveContainer width="100%" height={220}>
                <LineChart
                  data={points.map((p) => ({
                    date: p.date,
                    audiobooks: msToMin(p.audiobooks_duration_ms),
                    shorts: msToMin(p.shorts_duration_ms),
                    videos: msToMin(p.videos_duration_ms),
                  }))}
                >
                  <CartesianGrid strokeDasharray="3 3" stroke="#1f2937" />
                  <XAxis dataKey="date" stroke="#94a3b8" fontSize={11} />
                  <YAxis stroke="#94a3b8" fontSize={11} />
                  <Tooltip contentStyle={tooltipStyle} />
                  <Legend wrapperStyle={{ color: "#cbd5e1", fontSize: 12 }} />
                  <Line
                    type="monotone"
                    dataKey="audiobooks"
                    name="Audiobooks"
                    stroke="#6366f1"
                    dot={false}
                  />
                  <Line
                    type="monotone"
                    dataKey="shorts"
                    name="Shorts"
                    stroke="#22d3ee"
                    dot={false}
                  />
                  <Line
                    type="monotone"
                    dataKey="videos"
                    name="Videos"
                    stroke="#f97316"
                    dot={false}
                  />
                </LineChart>
              </ResponsiveContainer>
            </ChartCard>
            <ChartCard title="Spend per bucket (USD)">
              <ResponsiveContainer width="100%" height={220}>
                <LineChart
                  data={points.map((p) => ({
                    date: p.date,
                    audiobooks: round2(p.audiobooks_cost_usd),
                    shorts: round2(p.shorts_cost_usd),
                    total: round2(p.audiobooks_cost_usd + p.shorts_cost_usd),
                  }))}
                >
                  <CartesianGrid strokeDasharray="3 3" stroke="#1f2937" />
                  <XAxis dataKey="date" stroke="#94a3b8" fontSize={11} />
                  <YAxis
                    stroke="#94a3b8"
                    fontSize={11}
                    tickFormatter={(v: number) => `$${v}`}
                  />
                  <Tooltip
                    contentStyle={tooltipStyle}
                    formatter={(v) => `$${Number(v ?? 0).toFixed(2)}`}
                  />
                  <Legend wrapperStyle={{ color: "#cbd5e1", fontSize: 12 }} />
                  <Line
                    type="monotone"
                    dataKey="audiobooks"
                    name="Audiobooks"
                    stroke="#6366f1"
                    dot={false}
                  />
                  <Line
                    type="monotone"
                    dataKey="shorts"
                    name="Shorts"
                    stroke="#22d3ee"
                    dot={false}
                  />
                  <Line
                    type="monotone"
                    dataKey="total"
                    name="Total"
                    stroke="#a3e635"
                    strokeWidth={2}
                    dot={false}
                  />
                </LineChart>
              </ResponsiveContainer>
            </ChartCard>
          </div>
        </>
      )}
    </section>
  );
}

// ----------------------------------------------------------------------
// YouTube channel summary tile
// ----------------------------------------------------------------------

function YoutubeChannelSection(): JSX.Element {
  const { data, error, isLoading } = useQuery({
    queryKey: ["analytics", "yt-channel"],
    queryFn: () => analytics.youtubeChannel(),
    retry: false,
  });

  return (
    <section className="space-y-3">
      <SectionHeader
        title="YouTube channel"
        subtitle="Lifetime totals for the channel connected in Settings."
      />
      {error instanceof ApiError && error.status === 409 ? (
        <ConnectYoutubePrompt />
      ) : error ? (
        <ErrorBanner error={error} />
      ) : isLoading || !data ? (
        <div className="grid gap-3 sm:grid-cols-2 lg:grid-cols-4">
          <StatTile label="Subscribers" value="…" />
          <StatTile label="Channel views" value="…" />
          <StatTile label="Public videos" value="…" />
          <StatTile label="Channel" value="…" />
        </div>
      ) : (
        <div className="grid gap-3 sm:grid-cols-2 lg:grid-cols-4">
          <StatTile
            label="Subscribers"
            value={data.subscriber_count.toLocaleString()}
          />
          <StatTile
            label="Lifetime views"
            value={data.view_count.toLocaleString()}
          />
          <StatTile
            label="Public videos"
            value={data.video_count.toLocaleString()}
          />
          <StatTile label="Channel" value={data.channel_title} />
        </div>
      )}
    </section>
  );
}

// ----------------------------------------------------------------------
// YouTube Analytics report (views/likes/comments/watch-time time series)
// ----------------------------------------------------------------------

function YoutubeReportSection({
  bucket,
  rangeDays,
}: {
  bucket: AnalyticsBucket;
  rangeDays: number;
}): JSX.Element {
  const { data, error, isLoading } = useQuery({
    queryKey: ["analytics", "yt-report", bucket, rangeDays],
    queryFn: () => analytics.youtubeReports({ bucket, rangeDays }),
    retry: false,
  });

  if (error instanceof ApiError && error.status === 409) {
    // Already prompted up in the channel section — don't double the
    // call-to-action down here.
    return <></>;
  }

  const points: YoutubeReportPoint[] = data?.points ?? [];
  return (
    <section className="space-y-3">
      <SectionHeader
        title="YouTube performance"
        subtitle="Views, likes, comments, and watch time across all your videos."
      />
      {error ? (
        <ErrorBanner error={error} />
      ) : isLoading ? (
        <ChartSkeleton />
      ) : (
        <div className="grid gap-3 lg:grid-cols-2">
          <ChartCard title="Views & watch time">
            <ResponsiveContainer width="100%" height={220}>
              <LineChart data={points}>
                <CartesianGrid strokeDasharray="3 3" stroke="#1f2937" />
                <XAxis dataKey="date" stroke="#94a3b8" fontSize={11} />
                <YAxis stroke="#94a3b8" fontSize={11} />
                <Tooltip contentStyle={tooltipStyle} />
                <Legend wrapperStyle={{ color: "#cbd5e1", fontSize: 12 }} />
                <Line
                  type="monotone"
                  dataKey="views"
                  name="Views"
                  stroke="#f97316"
                  dot={false}
                />
                <Line
                  type="monotone"
                  dataKey="estimated_minutes_watched"
                  name="Watch min"
                  stroke="#a3e635"
                  dot={false}
                />
              </LineChart>
            </ResponsiveContainer>
          </ChartCard>
          <ChartCard title="Engagement">
            <ResponsiveContainer width="100%" height={220}>
              <LineChart data={points}>
                <CartesianGrid strokeDasharray="3 3" stroke="#1f2937" />
                <XAxis dataKey="date" stroke="#94a3b8" fontSize={11} />
                <YAxis stroke="#94a3b8" fontSize={11} />
                <Tooltip contentStyle={tooltipStyle} />
                <Legend wrapperStyle={{ color: "#cbd5e1", fontSize: 12 }} />
                <Line
                  type="monotone"
                  dataKey="likes"
                  name="Likes"
                  stroke="#22d3ee"
                  dot={false}
                />
                <Line
                  type="monotone"
                  dataKey="comments"
                  name="Comments"
                  stroke="#e879f9"
                  dot={false}
                />
              </LineChart>
            </ResponsiveContainer>
          </ChartCard>
        </div>
      )}
    </section>
  );
}

// ----------------------------------------------------------------------
// Per-video stats table
// ----------------------------------------------------------------------

type VideoGroupKey = "audiobook" | "short" | "songbook";

const VIDEO_GROUP_ORDER: { key: VideoGroupKey; title: string; subtitle: string }[] = [
  {
    key: "audiobook",
    title: "Audiobooks",
    subtitle: "Long-form audiobook publications.",
  },
  {
    key: "short",
    title: "YouTube Shorts",
    subtitle: "Vertical Shorts cut from one-chapter audiobooks.",
  },
  {
    key: "songbook",
    title: "Songbooks",
    subtitle: "Lyric-driven audiobooks spliced with song snippets.",
  },
];

function classifyVideo(row: YoutubeVideoRow): VideoGroupKey {
  // is_short / is_songbook are mutually exclusive at the audiobook
  // create layer; short wins if both ever land true on legacy rows.
  if (row.is_short) return "short";
  if (row.is_songbook) return "songbook";
  return "audiobook";
}

function YoutubeVideosSection(): JSX.Element {
  const { data, error, isLoading } = useQuery({
    queryKey: ["analytics", "yt-videos"],
    queryFn: () => analytics.youtubeVideos(),
    retry: false,
  });

  const grouped = useMemo(() => {
    const buckets: Record<VideoGroupKey, YoutubeVideoRow[]> = {
      audiobook: [],
      short: [],
      songbook: [],
    };
    for (const v of data?.items ?? []) {
      buckets[classifyVideo(v)].push(v);
    }
    return buckets;
  }, [data]);

  if (error instanceof ApiError && error.status === 409) {
    return <></>;
  }

  return (
    <section className="space-y-3">
      <SectionHeader
        title="Videos"
        subtitle="Every video published from this account, grouped by content type."
      />
      {error ? (
        <ErrorBanner error={error} />
      ) : isLoading ? (
        <ChartSkeleton />
      ) : !data || data.items.length === 0 ? (
        <p className="text-sm text-slate-500">No videos published yet.</p>
      ) : (
        <div className="space-y-6">
          {VIDEO_GROUP_ORDER.map((g) => {
            const rows = grouped[g.key];
            if (rows.length === 0) return null;
            return (
              <VideoGroupTable
                key={g.key}
                title={g.title}
                subtitle={g.subtitle}
                rows={rows}
              />
            );
          })}
          <div className="rounded-md border border-slate-800 bg-slate-900/40 px-4 py-3 text-sm text-slate-200">
            <span className="text-xs uppercase tracking-wide text-slate-500">
              All videos
            </span>
            <div className="mt-1 grid gap-3 sm:grid-cols-4">
              <Tot label="Views" value={data.total_views.toLocaleString()} />
              <Tot label="Likes" value={data.total_likes.toLocaleString()} />
              <Tot
                label="Comments"
                value={data.total_comments.toLocaleString()}
              />
              <Tot
                label="Total cost"
                value={`$${data.total_cost_usd.toFixed(2)}`}
              />
            </div>
          </div>
        </div>
      )}
    </section>
  );
}

function Tot({ label, value }: { label: string; value: string }): JSX.Element {
  return (
    <div>
      <div className="text-xs text-slate-500">{label}</div>
      <div className="text-base font-semibold tabular-nums text-slate-100">
        {value}
      </div>
    </div>
  );
}

function VideoGroupTable({
  title,
  subtitle,
  rows,
}: {
  title: string;
  subtitle: string;
  rows: YoutubeVideoRow[];
}): JSX.Element {
  const subtotal = useMemo(() => {
    const t = { views: 0, likes: 0, comments: 0, cost: 0 };
    for (const r of rows) {
      t.views += r.view_count;
      t.likes += r.like_count;
      t.comments += r.comment_count;
      t.cost += r.cost_usd;
    }
    return t;
  }, [rows]);

  return (
    <div className="overflow-hidden rounded-md border border-slate-800">
      <div className="flex items-baseline justify-between border-b border-slate-800 bg-slate-900/70 px-3 py-2">
        <div>
          <h3 className="text-sm font-semibold text-slate-200">{title}</h3>
          <p className="text-xs text-slate-500">{subtitle}</p>
        </div>
        <span className="text-xs text-slate-500">
          {rows.length} {rows.length === 1 ? "video" : "videos"}
        </span>
      </div>
      <table className="min-w-full divide-y divide-slate-800 text-sm">
        <thead className="bg-slate-900 text-xs uppercase text-slate-400">
          <tr>
            <th className="px-3 py-2 text-left font-medium">Audiobook</th>
            <th className="px-3 py-2 text-left font-medium">Chapter</th>
            <th className="px-3 py-2 text-right font-medium">Views</th>
            <th className="px-3 py-2 text-right font-medium">Likes</th>
            <th className="px-3 py-2 text-right font-medium">Comments</th>
            <th className="px-3 py-2 text-right font-medium">Cost</th>
            <th className="px-3 py-2 text-left font-medium">Published</th>
          </tr>
        </thead>
        <tbody className="divide-y divide-slate-800/50">
          {rows.map((v) => (
            <VideoRow key={v.video_id} row={v} />
          ))}
        </tbody>
        <tfoot className="bg-slate-900 text-xs text-slate-300">
          <tr>
            <td className="px-3 py-2" colSpan={2}>
              <span className="text-slate-500">Subtotal</span>
            </td>
            <td className="px-3 py-2 text-right tabular-nums">
              {subtotal.views.toLocaleString()}
            </td>
            <td className="px-3 py-2 text-right tabular-nums">
              {subtotal.likes.toLocaleString()}
            </td>
            <td className="px-3 py-2 text-right tabular-nums">
              {subtotal.comments.toLocaleString()}
            </td>
            <td className="px-3 py-2 text-right tabular-nums">
              ${subtotal.cost.toFixed(2)}
            </td>
            <td className="px-3 py-2" />
          </tr>
        </tfoot>
      </table>
    </div>
  );
}

function VideoRow({ row }: { row: YoutubeVideoRow }): JSX.Element {
  return (
    <tr className="bg-slate-950">
      <td className="px-3 py-2">
        <a
          href={`https://www.youtube.com/watch?v=${row.video_id}`}
          target="_blank"
          rel="noopener noreferrer"
          className="text-indigo-300 hover:text-indigo-200"
        >
          {row.audiobook_title || row.video_id}
        </a>
      </td>
      <td className="px-3 py-2 text-slate-400">
        {row.chapter_number == null ? "single" : `#${row.chapter_number}`}
      </td>
      <td className="px-3 py-2 text-right tabular-nums">
        {row.view_count.toLocaleString()}
      </td>
      <td className="px-3 py-2 text-right tabular-nums">
        {row.like_count.toLocaleString()}
      </td>
      <td className="px-3 py-2 text-right tabular-nums">
        {row.comment_count.toLocaleString()}
      </td>
      <td className="px-3 py-2 text-right tabular-nums text-slate-300">
        ${row.cost_usd.toFixed(2)}
      </td>
      <td className="px-3 py-2 text-slate-400">
        {row.published_at
          ? new Date(row.published_at).toLocaleDateString()
          : "—"}
      </td>
    </tr>
  );
}

// ----------------------------------------------------------------------
// Shared bits
// ----------------------------------------------------------------------

function SectionHeader({
  title,
  subtitle,
}: {
  title: string;
  subtitle?: string;
}): JSX.Element {
  return (
    <div>
      <h2 className="text-lg font-semibold tracking-tight">{title}</h2>
      {subtitle && <p className="text-xs text-slate-500">{subtitle}</p>}
    </div>
  );
}

function StatTile({
  label,
  value,
  hint,
}: {
  label: string;
  value: string;
  hint?: string;
}): JSX.Element {
  return (
    <div className="rounded-md border border-slate-800 bg-slate-900/40 px-4 py-3">
      <div className="text-xs uppercase tracking-wide text-slate-500">{label}</div>
      <div className="mt-1 text-xl font-semibold text-slate-100">{value}</div>
      {hint && <div className="mt-0.5 text-xs text-slate-500">{hint}</div>}
    </div>
  );
}

function ChartCard({
  title,
  children,
}: {
  title: string;
  children: React.ReactNode;
}): JSX.Element {
  return (
    <div className="rounded-md border border-slate-800 bg-slate-900/20 p-4">
      <h3 className="mb-2 text-sm font-medium text-slate-300">{title}</h3>
      {children}
    </div>
  );
}

function ChartSkeleton(): JSX.Element {
  return (
    <div className="h-[260px] animate-pulse rounded-md border border-slate-800 bg-slate-900/30" />
  );
}

function ErrorBanner({ error }: { error: unknown }): JSX.Element {
  const msg =
    error instanceof ApiError ? error.message : "Could not load data";
  return (
    <div className="rounded-md border border-rose-900/40 bg-rose-950/30 px-3 py-2 text-sm text-rose-200">
      {msg}
    </div>
  );
}

function ConnectYoutubePrompt(): JSX.Element {
  return (
    <div className="rounded-md border border-slate-800 bg-slate-900/30 px-4 py-3 text-sm text-slate-300">
      Connect your YouTube channel in{" "}
      <Link to="/app/settings" className="text-indigo-300 hover:text-indigo-200">
        Settings
      </Link>{" "}
      to surface subscriber, view, and per-video stats here.
    </div>
  );
}

// ----------------------------------------------------------------------
// Local helpers
// ----------------------------------------------------------------------

const tooltipStyle = {
  backgroundColor: "rgb(15 23 42)",
  border: "1px solid rgb(51 65 85)",
  borderRadius: 6,
  fontSize: 12,
  color: "#e2e8f0",
};

function msToMin(ms: number): number {
  return Math.round(ms / 60000);
}

function round2(n: number): number {
  return Math.round(n * 100) / 100;
}

function formatDuration(ms: number): string {
  if (ms <= 0) return "0m";
  const minutes = Math.round(ms / 60000);
  const h = Math.floor(minutes / 60);
  const m = minutes % 60;
  if (h === 0) return `${m}m`;
  return `${h}h ${m}m`;
}
