import { useMemo, useState } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { admin, ApiError } from "../../api";
import type { AdminLlmRow } from "../../api";
import { ErrorPane, LlmDialog, Loading, Toggle } from "./AdminLlms";
import { useDragReorder, DRAG_HANDLE_GLYPH } from "../../lib/llm-reorder";

export function AdminImageLlms(): JSX.Element {
  const qc = useQueryClient();
  const [addOpen, setAddOpen] = useState(false);
  const [editing, setEditing] = useState<AdminLlmRow | null>(null);

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
    </div>
  );
}
