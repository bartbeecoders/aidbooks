import { useEffect, useState } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { admin, ApiError } from "../../api";
import type { AdminLlmRow, CreateLlmRequest } from "../../api";

const COVER_ROLE = "cover_art";
const ALL_ROLES = [
  "outline",
  "chapter",
  "title",
  "random_topic",
  "moderation",
  "cover_art",
] as const;

export function AdminLlms(): JSX.Element {
  const qc = useQueryClient();
  const [addOpen, setAddOpen] = useState(false);
  const { data, isLoading, error } = useQuery({
    queryKey: ["admin", "llm"],
    queryFn: () => admin.llms.list(),
  });

  const toggle = useMutation({
    mutationFn: (row: AdminLlmRow) =>
      admin.llms.patch(row.id, { enabled: !row.enabled }),
    onSuccess: () => qc.invalidateQueries({ queryKey: ["admin", "llm"] }),
  });

  // Pick the cover-art LLM. Mutually exclusive across rows: the picked row
  // gets `cover_art` added to default_for; every other row that currently
  // claims it gets it removed. Both PATCHes go in parallel so the DB never
  // sees two cover_art models at once for longer than one network round-trip.
  const setCoverArt = useMutation({
    mutationFn: async (target: AdminLlmRow) => {
      if (!data) return;
      const updates: Promise<unknown>[] = [];
      for (const row of data.items) {
        const has = row.default_for.includes(COVER_ROLE);
        if (row.id === target.id && !has) {
          updates.push(
            admin.llms.patch(row.id, {
              default_for: [...row.default_for, COVER_ROLE],
            }),
          );
        } else if (row.id !== target.id && has) {
          updates.push(
            admin.llms.patch(row.id, {
              default_for: row.default_for.filter((r) => r !== COVER_ROLE),
            }),
          );
        }
      }
      await Promise.all(updates);
    },
    onSuccess: () => qc.invalidateQueries({ queryKey: ["admin", "llm"] }),
  });

  if (isLoading) return <Loading />;
  if (error) return <ErrorPane error={error} />;
  if (!data) return <p>No data.</p>;

  return (
    <div>
      <div className="mb-6 flex items-end justify-between gap-4">
        <div>
          <h1 className="text-xl font-semibold tracking-tight">LLMs</h1>
          <p className="mt-1 text-sm text-slate-400">
            OpenRouter models available to the generation pipeline. Disabled
            rows are skipped as defaults.
          </p>
        </div>
        <button
          type="button"
          onClick={() => setAddOpen(true)}
          className="rounded-md bg-sky-600 px-3 py-2 text-sm font-medium text-white hover:bg-sky-500"
        >
          Add LLM
        </button>
      </div>
      <table className="w-full text-sm">
        <thead className="text-left text-xs uppercase tracking-wide text-slate-500">
          <tr>
            <th className="py-2 pr-4">Name</th>
            <th className="py-2 pr-4">Model id</th>
            <th className="py-2 pr-4">Cost / 1k</th>
            <th className="py-2 pr-4">Default for</th>
            <th className="py-2 pr-4 text-center">Cover art</th>
            <th className="py-2 pr-4 text-right">Status</th>
          </tr>
        </thead>
        <tbody>
          {data.items.map((row) => {
            const isCoverArt = row.default_for.includes(COVER_ROLE);
            return (
              <tr key={row.id} className="border-t border-slate-800">
                <td className="py-3 pr-4 font-medium text-slate-100">{row.name}</td>
                <td className="py-3 pr-4 font-mono text-xs text-slate-400">
                  {row.model_id}
                </td>
                <td className="py-3 pr-4 text-slate-300">
                  ${row.cost_prompt_per_1k.toFixed(3)} /{" "}
                  ${row.cost_completion_per_1k.toFixed(3)}
                </td>
                <td className="py-3 pr-4 text-xs text-slate-400">
                  {row.default_for.length ? row.default_for.join(", ") : "—"}
                </td>
                <td className="py-3 pr-4 text-center">
                  <input
                    type="radio"
                    name="cover-art-llm"
                    checked={isCoverArt}
                    onChange={() => setCoverArt.mutate(row)}
                    disabled={!row.enabled || setCoverArt.isPending}
                    className="h-4 w-4 cursor-pointer accent-violet-500 disabled:cursor-not-allowed disabled:opacity-30"
                    title={
                      row.enabled
                        ? "Use this model for cover-art generation"
                        : "Enable the model first"
                    }
                  />
                </td>
                <td className="py-3 pr-4 text-right">
                  <Toggle
                    enabled={row.enabled}
                    onClick={() => toggle.mutate(row)}
                    pending={toggle.isPending && toggle.variables?.id === row.id}
                  />
                </td>
              </tr>
            );
          })}
        </tbody>
      </table>
      <p className="mt-3 text-xs text-slate-500">
        The model marked <em>Cover art</em> handles image generation for the
        “Generate cover” button on the New audiobook page. It must be an
        image-capable OpenRouter model (e.g. <code>google/gemini-2.5-flash-image</code>).
      </p>
      {toggle.error && (
        <p className="mt-3 text-sm text-rose-400">
          {toggle.error instanceof ApiError ? toggle.error.message : "Toggle failed"}
        </p>
      )}
      {setCoverArt.error && (
        <p className="mt-1 text-sm text-rose-400">
          {setCoverArt.error instanceof ApiError
            ? setCoverArt.error.message
            : "Could not set cover-art model"}
        </p>
      )}

      {addOpen && (
        <AddLlmDialog
          onClose={() => setAddOpen(false)}
          onCreated={() => {
            qc.invalidateQueries({ queryKey: ["admin", "llm"] });
            setAddOpen(false);
          }}
        />
      )}
    </div>
  );
}

function AddLlmDialog({
  onClose,
  onCreated,
}: {
  onClose: () => void;
  onCreated: () => void;
}): JSX.Element {
  const [id, setId] = useState("");
  const [name, setName] = useState("");
  const [modelId, setModelId] = useState("");
  const [contextWindow, setContextWindow] = useState(200_000);
  const [costPrompt, setCostPrompt] = useState(0);
  const [costCompletion, setCostCompletion] = useState(0);
  const [defaultFor, setDefaultFor] = useState<string[]>([]);

  useEffect(() => {
    const onKey = (e: KeyboardEvent): void => {
      if (e.key === "Escape") onClose();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [onClose]);

  const create = useMutation({
    mutationFn: (body: CreateLlmRequest) => admin.llms.create(body),
    onSuccess: onCreated,
  });

  const idValid = /^[a-z0-9_]+$/.test(id);
  const valid =
    idValid &&
    name.trim().length > 0 &&
    modelId.trim().length > 0 &&
    contextWindow >= 1 &&
    costPrompt >= 0 &&
    costCompletion >= 0;

  return (
    <div
      className="fixed inset-0 z-50 flex items-center justify-center bg-black/60 p-4"
      onClick={onClose}
    >
      <form
        onClick={(e) => e.stopPropagation()}
        onSubmit={(e) => {
          e.preventDefault();
          if (!valid || create.isPending) return;
          create.mutate({
            id: id.trim(),
            name: name.trim(),
            model_id: modelId.trim(),
            context_window: contextWindow,
            cost_prompt_per_1k: costPrompt,
            cost_completion_per_1k: costCompletion,
            default_for: defaultFor.length ? defaultFor : null,
          });
        }}
        className="w-full max-w-lg rounded-xl border border-slate-800 bg-slate-950 p-5 shadow-xl"
      >
        <h2 className="text-base font-semibold text-slate-100">Add LLM</h2>
        <p className="mt-1 text-xs text-slate-400">
          Register an OpenRouter model so it shows up in the picker. The id is
          used as the SurrealDB record id; pick something stable.
        </p>

        <div className="mt-4 grid gap-3 sm:grid-cols-2">
          <Labelled label="Id" hint="lowercase letters, digits, _">
            <input
              type="text"
              value={id}
              onChange={(e) => setId(e.target.value.toLowerCase())}
              placeholder="gemini_flash_image"
              className={inputCls}
            />
            {id && !idValid && (
              <p className="mt-1 text-xs text-rose-400">
                Only a–z, 0–9 and underscores.
              </p>
            )}
          </Labelled>
          <Labelled label="Display name">
            <input
              type="text"
              value={name}
              onChange={(e) => setName(e.target.value)}
              placeholder="Gemini 2.5 Flash Image"
              className={inputCls}
            />
          </Labelled>
          <Labelled label="Model id" hint="OpenRouter slug">
            <input
              type="text"
              value={modelId}
              onChange={(e) => setModelId(e.target.value)}
              placeholder="google/gemini-2.5-flash-image"
              className={`${inputCls} font-mono text-xs`}
            />
          </Labelled>
          <Labelled label="Context window">
            <input
              type="number"
              min={1}
              value={contextWindow}
              onChange={(e) => setContextWindow(Number(e.target.value) || 0)}
              className={inputCls}
            />
          </Labelled>
          <Labelled label="$ / 1k prompt">
            <input
              type="number"
              min={0}
              step="0.01"
              value={costPrompt}
              onChange={(e) => setCostPrompt(Number(e.target.value) || 0)}
              className={inputCls}
            />
          </Labelled>
          <Labelled label="$ / 1k completion">
            <input
              type="number"
              min={0}
              step="0.01"
              value={costCompletion}
              onChange={(e) => setCostCompletion(Number(e.target.value) || 0)}
              className={inputCls}
            />
          </Labelled>
        </div>

        <fieldset className="mt-4">
          <legend className="text-xs font-medium text-slate-300">
            Default for
          </legend>
          <div className="mt-1 flex flex-wrap gap-1.5">
            {ALL_ROLES.map((role) => {
              const on = defaultFor.includes(role);
              return (
                <button
                  key={role}
                  type="button"
                  onClick={() =>
                    setDefaultFor((cur) =>
                      cur.includes(role)
                        ? cur.filter((r) => r !== role)
                        : [...cur, role],
                    )
                  }
                  className={`rounded-full border px-2.5 py-0.5 text-xs ${
                    on
                      ? "border-sky-600 bg-sky-600/15 text-sky-200"
                      : "border-slate-700 bg-slate-900 text-slate-400 hover:border-slate-600"
                  }`}
                >
                  {role}
                </button>
              );
            })}
          </div>
        </fieldset>

        {create.error && (
          <p className="mt-3 text-xs text-rose-400">
            {create.error instanceof ApiError
              ? create.error.message
              : "Could not create LLM"}
          </p>
        )}

        <div className="mt-5 flex justify-end gap-2">
          <button
            type="button"
            onClick={onClose}
            className="rounded-md border border-slate-800 bg-slate-900 px-3 py-2 text-sm text-slate-200 hover:border-slate-700"
          >
            Cancel
          </button>
          <button
            type="submit"
            disabled={!valid || create.isPending}
            className="rounded-md bg-sky-600 px-3 py-2 text-sm font-medium text-white hover:bg-sky-500 disabled:cursor-not-allowed disabled:bg-sky-700/50"
          >
            {create.isPending ? "Creating…" : "Create"}
          </button>
        </div>
      </form>
    </div>
  );
}

const inputCls =
  "mt-1 w-full rounded-md border border-slate-800 bg-slate-900 px-3 py-2 text-sm text-slate-100 outline-none focus:border-sky-600";

function Labelled({
  label,
  hint,
  children,
}: {
  label: string;
  hint?: string;
  children: React.ReactNode;
}): JSX.Element {
  return (
    <label className="block text-xs font-medium text-slate-300">
      <span className="flex items-baseline justify-between">
        <span>{label}</span>
        {hint && <span className="text-[10px] text-slate-500">{hint}</span>}
      </span>
      {children}
    </label>
  );
}

export function PageHeader({
  title,
  description,
}: {
  title: string;
  description: string;
}): JSX.Element {
  return (
    <div className="mb-6">
      <h1 className="text-xl font-semibold tracking-tight">{title}</h1>
      <p className="mt-1 text-sm text-slate-400">{description}</p>
    </div>
  );
}

export function Loading(): JSX.Element {
  return <p className="text-sm text-slate-400">Loading…</p>;
}

export function ErrorPane({ error }: { error: unknown }): JSX.Element {
  return (
    <p className="text-sm text-rose-400">
      {(error as Error).message ?? String(error)}
    </p>
  );
}

export function Toggle({
  enabled,
  onClick,
  pending,
}: {
  enabled: boolean;
  onClick: () => void;
  pending?: boolean;
}): JSX.Element {
  return (
    <button
      onClick={onClick}
      disabled={pending}
      className={`inline-flex items-center gap-2 rounded-full border px-3 py-1 text-xs font-medium ${
        enabled
          ? "border-emerald-800 bg-emerald-950 text-emerald-200 hover:bg-emerald-900"
          : "border-slate-700 bg-slate-900 text-slate-400 hover:bg-slate-800"
      } disabled:opacity-50`}
    >
      <span
        className={`inline-block h-1.5 w-1.5 rounded-full ${
          enabled ? "bg-emerald-400" : "bg-slate-600"
        }`}
      />
      {enabled ? "Enabled" : "Disabled"}
    </button>
  );
}
