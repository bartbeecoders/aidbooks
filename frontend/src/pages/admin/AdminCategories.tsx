import { useState } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { admin, ApiError } from "../../api";
import type { AudiobookCategoryRow } from "../../api";
import { ErrorPane, Loading } from "./AdminLlms";

export function AdminCategories(): JSX.Element {
  const qc = useQueryClient();
  const [adding, setAdding] = useState("");
  const [editing, setEditing] = useState<AudiobookCategoryRow | null>(null);

  const { data, isLoading, error } = useQuery({
    queryKey: ["admin", "audiobook-categories"],
    queryFn: () => admin.audiobookCategories.list(),
  });

  const create = useMutation({
    mutationFn: (name: string) => admin.audiobookCategories.create({ name }),
    onSuccess: () => {
      setAdding("");
      qc.invalidateQueries({ queryKey: ["admin", "audiobook-categories"] });
      // The user-facing /audiobook-categories list also caches; refresh
      // it so the New Audiobook + BookDetail pickers see the new entry.
      qc.invalidateQueries({ queryKey: ["audiobook-categories"] });
    },
  });

  const update = useMutation({
    mutationFn: ({ id, name }: { id: string; name: string }) =>
      admin.audiobookCategories.update(id, { name }),
    onSuccess: () => {
      setEditing(null);
      qc.invalidateQueries({ queryKey: ["admin", "audiobook-categories"] });
      qc.invalidateQueries({ queryKey: ["audiobook-categories"] });
      qc.invalidateQueries({ queryKey: ["audiobooks"] });
    },
  });

  const remove = useMutation({
    mutationFn: (id: string) => admin.audiobookCategories.remove(id),
    onSuccess: () => {
      qc.invalidateQueries({ queryKey: ["admin", "audiobook-categories"] });
      qc.invalidateQueries({ queryKey: ["audiobook-categories"] });
      qc.invalidateQueries({ queryKey: ["audiobooks"] });
    },
  });

  if (isLoading) return <Loading />;
  if (error) return <ErrorPane error={error} />;

  const items = data?.items ?? [];

  return (
    <div>
      <div className="mb-6">
        <h1 className="text-xl font-semibold tracking-tight">
          Audiobook categories
        </h1>
        <p className="mt-1 max-w-2xl text-sm text-slate-400">
          Curated buckets users can pick from when creating or editing an
          audiobook. Renaming a category cascades to every book using it.
          Deleting a category moves its books to <em>Uncategorized</em>.
        </p>
      </div>

      <form
        onSubmit={(e) => {
          e.preventDefault();
          const name = adding.trim();
          if (!name) return;
          create.mutate(name);
        }}
        className="mb-6 flex items-center gap-2 rounded-lg border border-slate-800 bg-slate-900/40 p-3"
      >
        <input
          type="text"
          value={adding}
          onChange={(e) => setAdding(e.target.value)}
          maxLength={60}
          placeholder="New category name…"
          className="flex-1 rounded-md border border-slate-700 bg-slate-950 px-3 py-1.5 text-sm text-slate-100 outline-none focus:border-sky-600"
        />
        <button
          type="submit"
          disabled={!adding.trim() || create.isPending}
          className="rounded-md bg-sky-600 px-3 py-1.5 text-sm font-medium text-white hover:bg-sky-500 disabled:cursor-not-allowed disabled:opacity-40"
        >
          {create.isPending ? "Adding…" : "Add"}
        </button>
      </form>

      {items.length === 0 ? (
        <p className="rounded-lg border border-dashed border-slate-800 p-6 text-center text-sm text-slate-500">
          No categories yet. Add the first one above.
        </p>
      ) : (
        <table className="w-full text-sm">
          <thead className="text-left text-xs uppercase tracking-wide text-slate-500">
            <tr>
              <th className="py-2 pr-4">Name</th>
              <th className="py-2 pr-4">In use</th>
              <th className="py-2 pr-4">Updated</th>
              <th className="py-2 pr-4 text-right">Actions</th>
            </tr>
          </thead>
          <tbody>
            {items.map((row) =>
              editing?.id === row.id ? (
                <EditingRow
                  key={row.id}
                  row={row}
                  onSave={(name) => update.mutate({ id: row.id, name })}
                  onCancel={() => setEditing(null)}
                  saving={update.isPending}
                />
              ) : (
                <tr
                  key={row.id}
                  className="border-t border-slate-800 align-top"
                >
                  <td className="py-3 pr-4 font-medium text-slate-100">
                    {row.name}
                  </td>
                  <td className="py-3 pr-4 text-xs text-slate-400 tabular-nums">
                    {row.usage_count} book{row.usage_count === 1 ? "" : "s"}
                  </td>
                  <td className="py-3 pr-4 text-xs text-slate-500">
                    {new Date(row.updated_at).toLocaleString()}
                  </td>
                  <td className="py-3 pr-4 text-right">
                    <div className="flex justify-end gap-2">
                      <button
                        type="button"
                        onClick={() => setEditing(row)}
                        className="rounded-md border border-slate-700 bg-slate-900 px-2 py-1 text-xs text-slate-200 hover:border-slate-600"
                      >
                        Rename
                      </button>
                      <button
                        type="button"
                        onClick={() => {
                          const msg = row.usage_count
                            ? `Delete "${row.name}"? ${row.usage_count} book(s) will move to Uncategorized.`
                            : `Delete "${row.name}"?`;
                          if (window.confirm(msg)) remove.mutate(row.id);
                        }}
                        disabled={
                          remove.isPending && remove.variables === row.id
                        }
                        className="rounded-md border border-rose-900 bg-rose-950/40 px-2 py-1 text-xs text-rose-300 hover:border-rose-800 disabled:opacity-40"
                      >
                        Delete
                      </button>
                    </div>
                  </td>
                </tr>
              ),
            )}
          </tbody>
        </table>
      )}

      {(create.error || update.error || remove.error) && (
        <p className="mt-3 text-sm text-rose-400">
          {(create.error ?? update.error ?? remove.error) instanceof ApiError
            ? ((create.error ?? update.error ?? remove.error) as ApiError).message
            : "Action failed"}
        </p>
      )}
    </div>
  );
}

function EditingRow({
  row,
  onSave,
  onCancel,
  saving,
}: {
  row: AudiobookCategoryRow;
  onSave: (name: string) => void;
  onCancel: () => void;
  saving: boolean;
}): JSX.Element {
  const [draft, setDraft] = useState(row.name);
  return (
    <tr className="border-t border-slate-800 bg-slate-900/40 align-top">
      <td className="py-3 pr-4">
        <input
          type="text"
          value={draft}
          onChange={(e) => setDraft(e.target.value)}
          maxLength={60}
          autoFocus
          className="w-full rounded-md border border-slate-700 bg-slate-950 px-3 py-1.5 text-sm text-slate-100 outline-none focus:border-sky-600"
        />
      </td>
      <td className="py-3 pr-4 text-xs text-slate-400">
        {row.usage_count} book{row.usage_count === 1 ? "" : "s"}
      </td>
      <td className="py-3 pr-4 text-xs text-slate-500">—</td>
      <td className="py-3 pr-4 text-right">
        <div className="flex justify-end gap-2">
          <button
            type="button"
            onClick={onCancel}
            className="rounded-md border border-slate-700 bg-slate-900 px-2 py-1 text-xs text-slate-300 hover:border-slate-600"
          >
            Cancel
          </button>
          <button
            type="button"
            onClick={() => {
              const trimmed = draft.trim();
              if (trimmed && trimmed !== row.name) onSave(trimmed);
              else onCancel();
            }}
            disabled={saving || !draft.trim()}
            className="rounded-md bg-sky-600 px-2 py-1 text-xs font-medium text-white hover:bg-sky-500 disabled:opacity-40"
          >
            {saving ? "Saving…" : "Save"}
          </button>
        </div>
      </td>
    </tr>
  );
}
