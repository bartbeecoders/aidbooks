import { useMemo, useState } from "react";
import { useMutation, useQuery } from "@tanstack/react-query";
import { admin, ApiError } from "../../api";
import { ErrorPane, Loading, PageHeader } from "./AdminLlms";

const DEFAULT_SYSTEM =
  "You are a concise assistant. Answer in one short paragraph.";
const DEFAULT_PROMPT = "Write one sentence that showcases your writing voice.";

export function AdminTestLlm(): JSX.Element {
  const { data, isLoading, error } = useQuery({
    queryKey: ["admin", "llm"],
    queryFn: () => admin.llms.list(),
  });

  const [llmId, setLlmId] = useState<string>("");
  const [systemPrompt, setSystemPrompt] = useState<string>(DEFAULT_SYSTEM);
  const [userPrompt, setUserPrompt] = useState<string>(DEFAULT_PROMPT);
  const [temperature, setTemperature] = useState<string>("0.7");
  const [maxTokens, setMaxTokens] = useState<string>("400");

  const run = useMutation({
    mutationFn: () =>
      admin.test.llm({
        llm_id: llmId,
        prompt: userPrompt,
        system: systemPrompt.trim() || undefined,
        temperature: parseOptionalFloat(temperature),
        max_tokens: parseOptionalInt(maxTokens),
      }),
  });

  // Auto-pick the first enabled LLM once the list loads.
  const enabled = useMemo(
    () => data?.items.filter((r) => r.enabled) ?? [],
    [data],
  );
  if (enabled.length > 0 && !llmId) {
    setLlmId(enabled[0].id);
  }

  if (isLoading) return <Loading />;
  if (error) return <ErrorPane error={error} />;
  if (!data) return <p>No data.</p>;

  const selected = data.items.find((r) => r.id === llmId);
  const disabled = run.isPending || !llmId || userPrompt.trim().length === 0;

  return (
    <div>
      <PageHeader
        title="Test LLM"
        description="Send a one-shot prompt through the OpenRouter client and inspect the raw completion. Useful for sanity-checking keys, models, and system prompts."
      />
      <form
        className="space-y-4"
        onSubmit={(e) => {
          e.preventDefault();
          if (!disabled) run.mutate();
        }}
      >
        <div className="grid gap-4 md:grid-cols-[1fr,140px,140px]">
          <Field label="Model">
            <select
              value={llmId}
              onChange={(e) => setLlmId(e.target.value)}
              className="w-full rounded-md border border-slate-700 bg-slate-950 px-3 py-2 text-sm text-slate-100"
            >
              {data.items.map((row) => (
                <option key={row.id} value={row.id}>
                  {row.name} — {row.model_id}
                  {row.enabled ? "" : " (disabled)"}
                </option>
              ))}
            </select>
            {selected && (
              <p className="mt-1 text-xs text-slate-500">
                context {selected.context_window.toLocaleString()} · $
                {selected.cost_prompt_per_1k.toFixed(3)} in / $
                {selected.cost_completion_per_1k.toFixed(3)} out per 1k
              </p>
            )}
          </Field>
          <Field label="Temperature">
            <input
              type="number"
              step="0.1"
              min="0"
              max="2"
              value={temperature}
              onChange={(e) => setTemperature(e.target.value)}
              className="w-full rounded-md border border-slate-700 bg-slate-950 px-3 py-2 text-sm text-slate-100"
            />
          </Field>
          <Field label="Max tokens">
            <input
              type="number"
              min="1"
              max="4000"
              value={maxTokens}
              onChange={(e) => setMaxTokens(e.target.value)}
              className="w-full rounded-md border border-slate-700 bg-slate-950 px-3 py-2 text-sm text-slate-100"
            />
          </Field>
        </div>

        <Field label="System prompt (optional)">
          <textarea
            rows={2}
            value={systemPrompt}
            onChange={(e) => setSystemPrompt(e.target.value)}
            className="w-full rounded-md border border-slate-700 bg-slate-950 px-3 py-2 font-mono text-xs text-slate-100"
          />
        </Field>

        <Field label="User prompt">
          <textarea
            rows={5}
            value={userPrompt}
            onChange={(e) => setUserPrompt(e.target.value)}
            className="w-full rounded-md border border-slate-700 bg-slate-950 px-3 py-2 font-mono text-xs text-slate-100"
          />
        </Field>

        <div className="flex items-center gap-3">
          <button
            type="submit"
            disabled={disabled}
            className="inline-flex items-center gap-2 rounded-md border border-emerald-800 bg-emerald-950 px-4 py-2 text-sm font-medium text-emerald-200 hover:bg-emerald-900 disabled:opacity-50"
          >
            {run.isPending ? "Running…" : "Run test"}
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

      {run.data && (
        <div className="mt-6 rounded-lg border border-slate-800 bg-slate-950/60 p-4">
          <div className="mb-3 flex flex-wrap items-center gap-3 text-xs text-slate-400">
            <Badge mocked={run.data.mocked} />
            <span>prompt {run.data.prompt_tokens} tok</span>
            <span>·</span>
            <span>completion {run.data.completion_tokens} tok</span>
          </div>
          <pre className="whitespace-pre-wrap font-mono text-sm text-slate-100">
            {run.data.content}
          </pre>
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

function parseOptionalFloat(s: string): number | undefined {
  const n = Number.parseFloat(s);
  return Number.isFinite(n) ? n : undefined;
}
function parseOptionalInt(s: string): number | undefined {
  const n = Number.parseInt(s, 10);
  return Number.isFinite(n) ? n : undefined;
}
