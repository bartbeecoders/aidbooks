import { useMemo, useState } from "react";
import { useMutation, useQuery } from "@tanstack/react-query";
import { admin, ApiError } from "../../api";
import { ErrorPane, Loading, PageHeader } from "./AdminLlms";

const DEFAULT_TEXT =
  "Hello — this is a short sample for testing the voice. If you can hear this clearly, the pipeline is working end to end.";

export function AdminTestVoice(): JSX.Element {
  const { data, isLoading, error } = useQuery({
    queryKey: ["admin", "voice"],
    queryFn: () => admin.voices.list(),
  });

  const [voiceId, setVoiceId] = useState<string>("");
  const [text, setText] = useState<string>(DEFAULT_TEXT);

  const run = useMutation({
    mutationFn: () => admin.test.voice({ voice_id: voiceId, text }),
  });

  const enabled = useMemo(
    () => data?.items.filter((r) => r.enabled) ?? [],
    [data],
  );
  if (enabled.length > 0 && !voiceId) {
    setVoiceId(enabled[0].id);
  }

  if (isLoading) return <Loading />;
  if (error) return <ErrorPane error={error} />;
  if (!data) return <p>No data.</p>;

  const selected = data.items.find((r) => r.id === voiceId);
  const trimmed = text.trim();
  const disabled = run.isPending || !voiceId || trimmed.length === 0;
  const audioSrc = run.data
    ? `data:audio/wav;base64,${run.data.audio_wav_base64}`
    : undefined;

  return (
    <div>
      <PageHeader
        title="Test voice"
        description="Synthesise a short sample with the selected voice. Useful for auditioning narrators and verifying the x.ai realtime connection."
      />
      <form
        className="space-y-4"
        onSubmit={(e) => {
          e.preventDefault();
          if (!disabled) run.mutate();
        }}
      >
        <div className="grid gap-4 md:grid-cols-[1fr,auto]">
          <Field label="Voice">
            <select
              value={voiceId}
              onChange={(e) => setVoiceId(e.target.value)}
              className="w-full rounded-md border border-slate-700 bg-slate-950 px-3 py-2 text-sm text-slate-100"
            >
              {data.items.map((row) => (
                <option key={row.id} value={row.id}>
                  {row.name} — {row.provider}:{row.provider_voice_id}
                  {row.enabled ? "" : " (disabled)"}
                </option>
              ))}
            </select>
            {selected && (
              <p className="mt-1 text-xs text-slate-500">
                {selected.gender} · {selected.accent || "—"} ·{" "}
                {selected.language}
                {selected.premium_only ? " · premium" : ""}
              </p>
            )}
          </Field>
          <Field label="Length">
            <p className="rounded-md border border-slate-800 bg-slate-900 px-3 py-2 text-xs text-slate-400">
              {trimmed.length} / 1000 chars
            </p>
          </Field>
        </div>

        <Field label="Text">
          <textarea
            rows={6}
            value={text}
            onChange={(e) => setText(e.target.value)}
            maxLength={1000}
            className="w-full rounded-md border border-slate-700 bg-slate-950 px-3 py-2 text-sm text-slate-100"
          />
        </Field>

        <div className="flex items-center gap-3">
          <button
            type="submit"
            disabled={disabled}
            className="inline-flex items-center gap-2 rounded-md border border-emerald-800 bg-emerald-950 px-4 py-2 text-sm font-medium text-emerald-200 hover:bg-emerald-900 disabled:opacity-50"
          >
            {run.isPending ? "Synthesising…" : "Synthesise sample"}
          </button>
          {run.error && (
            <span className="text-sm text-rose-400">
              {run.error instanceof ApiError
                ? run.error.message
                : "Test failed"}
            </span>
          )}
        </div>
      </form>

      {run.data && audioSrc && (
        <div className="mt-6 rounded-lg border border-slate-800 bg-slate-950/60 p-4">
          <div className="mb-3 flex flex-wrap items-center gap-3 text-xs text-slate-400">
            <Badge mocked={run.data.mocked} />
            <span>{formatDuration(run.data.duration_ms)}</span>
            <span>·</span>
            <span>{run.data.sample_rate_hz.toLocaleString()} Hz</span>
          </div>
          {/* key forces the <audio> element to remount when a new clip
              arrives so the previous one is unloaded cleanly. */}
          <audio
            key={audioSrc}
            controls
            src={audioSrc}
            autoPlay
            className="w-full"
          />
          <div className="mt-2">
            <a
              href={audioSrc}
              download={`voice-test-${voiceId}.wav`}
              className="text-xs text-slate-400 underline decoration-dotted hover:text-slate-200"
            >
              download .wav
            </a>
          </div>
        </div>
      )}
    </div>
  );
}

function Field({
  label,
  children,
}: {
  label: string;
  children: React.ReactNode;
}): JSX.Element {
  return (
    <label className="block text-sm">
      <span className="mb-1 block text-xs font-medium uppercase tracking-wide text-slate-500">
        {label}
      </span>
      {children}
    </label>
  );
}

function Badge({ mocked }: { mocked: boolean }): JSX.Element {
  return mocked ? (
    <span className="rounded-full border border-amber-800 bg-amber-950 px-2 py-0.5 text-amber-200">
      MOCK
    </span>
  ) : (
    <span className="rounded-full border border-emerald-800 bg-emerald-950 px-2 py-0.5 text-emerald-200">
      LIVE
    </span>
  );
}

function formatDuration(ms: number): string {
  if (ms < 1000) return `${ms} ms`;
  const s = ms / 1000;
  return s < 60 ? `${s.toFixed(1)} s` : `${Math.floor(s / 60)}m ${Math.round(s % 60)}s`;
}
