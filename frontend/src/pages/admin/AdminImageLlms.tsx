import { useEffect, useMemo, useState } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { admin, ApiError } from "../../api";
import type { AdminLlmRow } from "../../api";
import { ErrorPane, LlmDialog, Loading, Toggle } from "./AdminLlms";
import { useDragReorder, DRAG_HANDLE_GLYPH } from "../../lib/llm-reorder";

// Format a per-image USD cost for the test result chip. Always uses 4
// decimals so a sub-cent fal/OpenRouter call doesn't read as `$0.00`
// and self-hosted mold rows without `cost_per_megapixel` still render
// as `$0.0000` (admin sees the row priced at zero rather than wondering
// if the field is missing).
function formatTestCost(cost: number): string {
  if (!Number.isFinite(cost) || cost <= 0) return "$0.0000";
  return `$${cost.toFixed(4)}`;
}

export function AdminImageLlms(): JSX.Element {
  const qc = useQueryClient();
  const [addOpen, setAddOpen] = useState(false);
  const [editing, setEditing] = useState<AdminLlmRow | null>(null);
  const [testing, setTesting] = useState<AdminLlmRow | null>(null);

  const { data, isLoading, error } = useQuery({
    queryKey: ["admin", "llm"],
    queryFn: () => admin.llms.list(),
  });

  const toggle = useMutation({
    mutationFn: (row: AdminLlmRow) =>
      admin.llms.patch(row.id, { enabled: !row.enabled }),
    onSuccess: () => qc.invalidateQueries({ queryKey: ["admin", "llm"] }),
  });

  const remove = useMutation({
    mutationFn: (row: AdminLlmRow) => admin.llms.remove(row.id),
    onSuccess: () => qc.invalidateQueries({ queryKey: ["admin", "llm"] }),
  });

  // Pull mutation is keyed on `row.id` via `variables` so we can spinner
  // the matching row's button and show its own error inline. Mold pulls
  // can be slow (multi-GB), so the mutation may stay pending for minutes.
  const pull = useMutation({
    mutationFn: (row: AdminLlmRow) => admin.llms.pullModel(row.id),
  });
  // Server-wide unload — frees VRAM. Near-instant; capped at 30s in the
  // backend client. Same per-row UX as pull (spinner on the firing row).
  const unload = useMutation({
    mutationFn: (row: AdminLlmRow) => admin.llms.unloadModels(row.id),
  });

  const sorted = useMemo(() => {
    if (!data) return [];
    return [...data.items]
      .filter((row) => (row.function ?? "").toLowerCase() === "image")
      .sort((a, b) => {
        const ap = a.priority ?? 100;
        const bp = b.priority ?? 100;
        if (ap !== bp) return ap - bp;
        return a.name.localeCompare(b.name);
      });
  }, [data]);

  const drag = useDragReorder(sorted);

  if (isLoading) return <Loading />;
  if (error) return <ErrorPane error={error} />;
  if (!data) return <p>No data.</p>;

  return (
    <div>
      <div className="mb-6 flex items-end justify-between gap-4">
        <div>
          <h1 className="text-xl font-semibold tracking-tight">Image LLMs</h1>
          <p className="mt-1 text-sm text-slate-400">
            Image-generation models used for cover and chapter artwork.
            Pricing is in <strong>$ per megapixel</strong>. The
            lowest-priority enabled row tagged <em>cover_art</em> wins.
          </p>
        </div>
        <button
          type="button"
          onClick={() => setAddOpen(true)}
          className="rounded-md bg-violet-600 px-3 py-2 text-sm font-medium text-white hover:bg-violet-500"
        >
          Add image LLM
        </button>
      </div>

      {sorted.length === 0 ? (
        <p className="rounded-lg border border-dashed border-slate-800 p-6 text-center text-sm text-slate-500">
          No image models registered yet. Click <em>Add image LLM</em> and set
          its function to <em>Image</em>.
        </p>
      ) : (
        <table className="w-full text-sm">
          <thead className="text-left text-xs uppercase tracking-wide text-slate-500">
            <tr>
              <th className="py-2 pr-2 w-6"></th>
              <th className="py-2 pr-4">Pri</th>
              <th className="py-2 pr-4">Name</th>
              <th className="py-2 pr-4">Model id</th>
              <th className="py-2 pr-4">$ / megapixel</th>
              <th className="py-2 pr-4">Roles</th>
              <th className="py-2 pr-4 text-right">Status</th>
              <th className="py-2 pr-4 text-right">Actions</th>
            </tr>
          </thead>
          <tbody>
            {sorted.map((row) => {
              const ds = drag.rowState(row.id);
              return (
                <tr
                  key={row.id}
                  {...drag.rowProps(row.id)}
                  className={`border-t border-slate-800 align-top ${
                    ds.isDragging ? "opacity-40" : ""
                  } ${ds.isOver ? "bg-sky-900/20" : ""}`}
                >
                  <td
                    className="py-3 pr-2 cursor-grab select-none text-center text-slate-600 hover:text-slate-300 active:cursor-grabbing"
                    title="Drag to reorder priority"
                  >
                    {DRAG_HANDLE_GLYPH}
                  </td>
                  <td className="py-3 pr-4 text-xs tabular-nums text-slate-500">
                    {row.priority ?? 100}
                  </td>
                  <td className="py-3 pr-4 font-medium text-slate-100">
                    {row.name}
                  </td>
                  <td className="py-3 pr-4 font-mono text-xs text-slate-400">
                    {row.model_id}
                  </td>
                  <td className="py-3 pr-4 text-xs text-slate-300 tabular-nums">
                    ${(row.cost_per_megapixel ?? 0).toFixed(3)}
                  </td>
                  <td className="py-3 pr-4 text-xs text-slate-400">
                    {row.default_for.length ? row.default_for.join(", ") : "—"}
                  </td>
                  <td className="py-3 pr-4 text-right">
                    <Toggle
                      enabled={row.enabled}
                      onClick={() => toggle.mutate(row)}
                      pending={
                        toggle.isPending && toggle.variables?.id === row.id
                      }
                    />
                  </td>
                  <td className="py-3 pr-4 text-right">
                    <div className="flex justify-end gap-2">
                      <button
                        type="button"
                        onClick={() => setTesting(row)}
                        className="rounded-md border border-emerald-900 bg-emerald-950/40 px-2 py-1 text-xs text-emerald-200 hover:border-emerald-800"
                        title="Generate a small probe image to verify the provider"
                      >
                        Test
                      </button>
                      {row.provider === "mold" && (
                        <>
                          <button
                            type="button"
                            onClick={() => {
                              // Confirm because multi-GB families can take
                              // tens of minutes and there's no cancel UI.
                              if (
                                window.confirm(
                                  `Pull "${row.model_id}" on the mold server? This can take several minutes for multi-GB models.`,
                                )
                              ) {
                                pull.mutate(row);
                              }
                            }}
                            disabled={
                              pull.isPending && pull.variables?.id === row.id
                            }
                            className="rounded-md border border-indigo-900 bg-indigo-950/40 px-2 py-1 text-xs text-indigo-200 hover:border-indigo-800 disabled:opacity-50"
                            title={`Pull ${row.model_id} on the mold server`}
                          >
                            {pull.isPending && pull.variables?.id === row.id
                              ? "Pulling…"
                              : "Pull"}
                          </button>
                          <button
                            type="button"
                            onClick={() => {
                              // Server-wide effect — warn the admin
                              // they're flushing every model from this
                              // mold instance's GPU cache, not just the
                              // one tied to this row.
                              if (
                                window.confirm(
                                  `Unload all models from mold serve at ${row.base_url}? This frees VRAM for every row pointing at the same server. The next generation will reload from disk (slower).`,
                                )
                              ) {
                                unload.mutate(row);
                              }
                            }}
                            disabled={
                              unload.isPending &&
                              unload.variables?.id === row.id
                            }
                            className="rounded-md border border-amber-900 bg-amber-950/40 px-2 py-1 text-xs text-amber-200 hover:border-amber-800 disabled:opacity-50"
                            title="Drop every model from the mold server's GPU cache (frees VRAM)"
                          >
                            {unload.isPending &&
                            unload.variables?.id === row.id
                              ? "Unloading…"
                              : "Unload"}
                          </button>
                        </>
                      )}
                      <button
                        type="button"
                        onClick={() => setEditing(row)}
                        className="rounded-md border border-slate-700 bg-slate-900 px-2 py-1 text-xs text-slate-200 hover:border-slate-600"
                      >
                        Edit
                      </button>
                      <button
                        type="button"
                        onClick={() => {
                          if (window.confirm(`Delete "${row.name}"?`)) {
                            remove.mutate(row);
                          }
                        }}
                        disabled={
                          remove.isPending && remove.variables?.id === row.id
                        }
                        className="rounded-md border border-rose-900 bg-rose-950/40 px-2 py-1 text-xs text-rose-300 hover:border-rose-800 disabled:opacity-40"
                      >
                        Delete
                      </button>
                    </div>
                  </td>
                </tr>
              );
            })}
          </tbody>
        </table>
      )}

      <p className="mt-3 text-xs text-slate-500">
        Drag rows by the {DRAG_HANDLE_GLYPH} handle to reorder by priority
        (lower wins). Edit a row to toggle the <em>cover_art</em> role.
      </p>

      {(toggle.error || remove.error) && (
        <p className="mt-3 text-sm text-rose-400">
          {(toggle.error ?? remove.error) instanceof ApiError
            ? ((toggle.error ?? remove.error) as ApiError).message
            : "Action failed"}
        </p>
      )}
      {pull.isPending && (
        <p className="mt-3 rounded-md border border-indigo-900 bg-indigo-950/30 px-3 py-2 text-sm text-indigo-200">
          Pulling <span className="font-mono">{pull.variables?.model_id}</span>{" "}
          on the mold server — keep this tab open. Multi-GB families can take
          several minutes.
        </p>
      )}
      {pull.error && (
        <pre className="mt-3 whitespace-pre-wrap break-words rounded-md border border-rose-900 bg-rose-950/30 px-3 py-2 text-sm text-rose-300">
          {pull.error instanceof ApiError
            ? pull.error.message
            : (pull.error as Error).message}
        </pre>
      )}
      {pull.data && !pull.isPending && (
        <p className="mt-3 rounded-md border border-emerald-900 bg-emerald-950/30 px-3 py-2 text-sm text-emerald-200">
          {pull.data.message}
        </p>
      )}
      {unload.isPending && (
        <p className="mt-3 rounded-md border border-amber-900 bg-amber-950/30 px-3 py-2 text-sm text-amber-200">
          Unloading models from mold serve…
        </p>
      )}
      {unload.error && (
        <pre className="mt-3 whitespace-pre-wrap break-words rounded-md border border-rose-900 bg-rose-950/30 px-3 py-2 text-sm text-rose-300">
          {unload.error instanceof ApiError
            ? unload.error.message
            : (unload.error as Error).message}
        </pre>
      )}
      {unload.data && !unload.isPending && (
        <p className="mt-3 rounded-md border border-emerald-900 bg-emerald-950/30 px-3 py-2 text-sm text-emerald-200">
          {unload.data.message || "Models unloaded."}
        </p>
      )}

      {addOpen && (
        <LlmDialog
          kind="image"
          mode="create"
          onClose={() => setAddOpen(false)}
          onSaved={() => {
            qc.invalidateQueries({ queryKey: ["admin", "llm"] });
            setAddOpen(false);
          }}
        />
      )}
      {editing && (
        <LlmDialog
          kind="image"
          mode="edit"
          initial={editing}
          onClose={() => setEditing(null)}
          onSaved={() => {
            qc.invalidateQueries({ queryKey: ["admin", "llm"] });
            setEditing(null);
          }}
        />
      )}
      {testing && (
        <TestImageLlmDialog
          row={testing}
          onClose={() => setTesting(null)}
        />
      )}
    </div>
  );
}

/**
 * Generates a single probe image against the selected row and renders the
 * result inline. The default prompt is a deliberately neutral subject so
 * admins can see colour, composition, and prompt adherence at a glance.
 * The prompt is editable so admins can swap in something specific (e.g.
 * a brand-style probe) and re-fire.
 */
function TestImageLlmDialog({
  row,
  onClose,
}: {
  row: AdminLlmRow;
  onClose: () => void;
}): JSX.Element {
  const DEFAULT_PROMPT =
    "A single red apple resting on a clean white surface, soft daylight, photographic, no text";
  const [prompt, setPrompt] = useState(DEFAULT_PROMPT);
  const [isShort, setIsShort] = useState(false);

  const test = useMutation({
    mutationFn: () =>
      admin.test.image_llm({
        llm_id: row.id,
        prompt,
        is_short: isShort,
      }),
  });

  // Auto-fire once on mount with the default prompt. Subsequent retries
  // go through the "Run again" button.
  useEffect(() => {
    test.mutate();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  useEffect(() => {
    const onKey = (e: KeyboardEvent): void => {
      if (e.key === "Escape") onClose();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [onClose]);

  const dataUrl = test.data
    ? `data:${test.data.content_type};base64,${test.data.image_base64}`
    : null;

  return (
    <div
      className="fixed inset-0 z-50 flex items-center justify-center bg-black/60 p-4"
      onClick={onClose}
    >
      <div
        onClick={(e) => e.stopPropagation()}
        className="w-full max-w-xl rounded-xl border border-slate-800 bg-slate-950 p-5 shadow-xl"
      >
        <h2 className="text-base font-semibold text-slate-100">
          Test {row.name}
        </h2>
        <p className="mt-1 text-xs text-slate-400">
          <span className="text-slate-500">{row.provider}</span>
          {" · "}
          <span className="font-mono">{row.model_id}</span>
          {row.base_url && (
            <>
              {" · "}
              <span className="font-mono text-slate-500">{row.base_url}</span>
            </>
          )}
        </p>

        <label className="mt-3 block text-xs font-medium text-slate-300">
          Prompt
          <textarea
            value={prompt}
            onChange={(e) => setPrompt(e.target.value)}
            rows={2}
            className="mt-1 block w-full rounded-md border border-slate-800 bg-slate-900 px-2 py-1.5 text-xs text-slate-100 focus:border-slate-600 focus:outline-none"
          />
        </label>
        <label className="mt-2 flex items-center gap-2 text-xs text-slate-300">
          <input
            type="checkbox"
            checked={isShort}
            onChange={(e) => setIsShort(e.target.checked)}
            className="h-3.5 w-3.5"
          />
          Vertical 9:16 (YouTube Short)
        </label>

        <div className="mt-4 flex min-h-[260px] items-center justify-center rounded-md border border-slate-800 bg-slate-900/40 p-3">
          {test.isPending && (
            <p className="text-sm text-slate-400">
              Generating image — this can take 5–60 seconds…
            </p>
          )}
          {!test.isPending && test.error && (
            <pre className="max-w-full whitespace-pre-wrap break-words text-sm text-rose-300">
              {test.error instanceof ApiError
                ? test.error.message
                : (test.error as Error).message}
            </pre>
          )}
          {!test.isPending && dataUrl && (
            <div className="flex flex-col items-center gap-2">
              <img
                src={dataUrl}
                alt="Probe result"
                className={`rounded border border-slate-800 ${
                  isShort ? "max-h-[360px]" : "max-h-[300px]"
                }`}
              />
              <p className="text-[11px] text-slate-500">
                {test.data?.content_type}
                <span className="ml-2 font-mono text-slate-300">
                  {formatTestCost(test.data?.cost ?? 0)}
                </span>
                {test.data?.mocked && (
                  <span className="ml-2 rounded bg-amber-900/40 px-1.5 py-0.5 text-amber-300">
                    mocked
                  </span>
                )}
              </p>
            </div>
          )}
        </div>

        <div className="mt-4 flex justify-end gap-2">
          <button
            type="button"
            onClick={() => test.mutate()}
            disabled={test.isPending || prompt.trim().length === 0}
            className="rounded-md border border-slate-700 bg-slate-900 px-3 py-2 text-sm text-slate-200 hover:border-slate-600 disabled:opacity-50"
          >
            Run again
          </button>
          <button
            type="button"
            onClick={onClose}
            className="rounded-md bg-sky-600 px-3 py-2 text-sm font-medium text-white hover:bg-sky-500"
          >
            Close
          </button>
        </div>
      </div>
    </div>
  );
}
