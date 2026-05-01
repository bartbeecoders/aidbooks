import { useEffect, useState } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { admin, ApiError } from "../../api";
import type {
  AudiobookLength,
  CreateTopicTemplateRequest,
  TopicTemplate,
  UpdateTopicTemplateRequest,
} from "../../api";
import { ErrorPane, Loading, PageHeader, Toggle } from "./AdminLlms";

const LENGTHS: AudiobookLength[] = ["short", "medium", "long"];

const LANGUAGE_CODES: { code: string; label: string; flag: string }[] = [
  { code: "en", label: "English", flag: "🇬🇧" },
  { code: "nl", label: "Dutch", flag: "🇳🇱" },
  { code: "fr", label: "French", flag: "🇫🇷" },
  { code: "de", label: "German", flag: "🇩🇪" },
  { code: "es", label: "Spanish", flag: "🇪🇸" },
  { code: "it", label: "Italian", flag: "🇮🇹" },
  { code: "pt", label: "Portuguese", flag: "🇵🇹" },
  { code: "ru", label: "Russian", flag: "🇷🇺" },
  { code: "zh", label: "Chinese", flag: "🇨🇳" },
  { code: "ja", label: "Japanese", flag: "🇯🇵" },
  { code: "ko", label: "Korean", flag: "🇰🇷" },
];

export function AdminTopicTemplates(): JSX.Element {
  const qc = useQueryClient();
  const [addOpen, setAddOpen] = useState(false);
  const [editing, setEditing] = useState<TopicTemplate | null>(null);

  const { data, isLoading, error } = useQuery({
    queryKey: ["admin", "topic-templates"],
    queryFn: () => admin.topicTemplates.list(),
  });

  const toggle = useMutation({
    mutationFn: (row: TopicTemplate) =>
      admin.topicTemplates.patch(row.id, { enabled: !row.enabled }),
    onSuccess: () => qc.invalidateQueries({ queryKey: ["admin", "topic-templates"] }),
  });

  const remove = useMutation({
    mutationFn: (row: TopicTemplate) => admin.topicTemplates.remove(row.id),
    onSuccess: () => qc.invalidateQueries({ queryKey: ["admin", "topic-templates"] }),
  });

  if (isLoading) return <Loading />;
  if (error) return <ErrorPane error={error} />;

  return (
    <div>
      <div className="mb-6 flex items-end justify-between gap-4">
        <PageHeader
          title="Topic templates"
          description="Pre-canned prompts that show up in the New Audiobook dropdown. Lower sort order comes first."
        />
        <button
          type="button"
          onClick={() => setAddOpen(true)}
          className="rounded-md bg-sky-600 px-3 py-2 text-sm font-medium text-white hover:bg-sky-500"
        >
          Add template
        </button>
      </div>

      {data && data.items.length === 0 ? (
        <p className="rounded-lg border border-dashed border-slate-800 p-6 text-center text-sm text-slate-500">
          No templates yet. Add one to give users a starting point.
        </p>
      ) : (
        <table className="w-full text-sm">
          <thead className="text-left text-xs uppercase tracking-wide text-slate-500">
            <tr>
              <th className="py-2 pr-4">#</th>
              <th className="py-2 pr-4">Title</th>
              <th className="py-2 pr-4">Topic</th>
              <th className="py-2 pr-4">Defaults</th>
              <th className="py-2 pr-4 text-right">Status</th>
              <th className="py-2 pr-4 text-right">Actions</th>
            </tr>
          </thead>
          <tbody>
            {data?.items.map((row) => (
              <tr key={row.id} className="border-t border-slate-800 align-top">
                <td className="py-3 pr-4 text-xs text-slate-500">{row.sort_order}</td>
                <td className="py-3 pr-4 font-medium text-slate-100">{row.title}</td>
                <td className="py-3 pr-4 text-slate-300">
                  <p className="line-clamp-2 max-w-md whitespace-pre-wrap break-words">
                    {row.topic}
                  </p>
                </td>
                <td className="py-3 pr-4 text-xs text-slate-400">
                  <DefaultsBadges row={row} />
                </td>
                <td className="py-3 pr-4 text-right">
                  <Toggle
                    enabled={row.enabled}
                    onClick={() => toggle.mutate(row)}
                    pending={toggle.isPending && toggle.variables?.id === row.id}
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
                        if (window.confirm(`Delete template "${row.title}"?`)) {
                          remove.mutate(row);
                        }
                      }}
                      disabled={remove.isPending && remove.variables?.id === row.id}
                      className="rounded-md border border-rose-900 bg-rose-950/40 px-2 py-1 text-xs text-rose-300 hover:border-rose-800 disabled:opacity-40"
                    >
                      Delete
                    </button>
                  </div>
                </td>
              </tr>
            ))}
          </tbody>
        </table>
      )}

      {(toggle.error || remove.error) && (
        <p className="mt-3 text-sm text-rose-400">
          {(toggle.error ?? remove.error) instanceof ApiError
            ? ((toggle.error ?? remove.error) as ApiError).message
            : "Action failed"}
        </p>
      )}

      {addOpen && (
        <TemplateDialog
          mode="create"
          onClose={() => setAddOpen(false)}
          onSaved={() => {
            qc.invalidateQueries({ queryKey: ["admin", "topic-templates"] });
            setAddOpen(false);
          }}
        />
      )}
      {editing && (
        <TemplateDialog
          mode="edit"
          initial={editing}
          onClose={() => setEditing(null)}
          onSaved={() => {
            qc.invalidateQueries({ queryKey: ["admin", "topic-templates"] });
            setEditing(null);
          }}
        />
      )}
    </div>
  );
}

function DefaultsBadges({ row }: { row: TopicTemplate }): JSX.Element {
  const items: string[] = [];
  if (row.is_short) items.push("YouTube Short");
  if (row.length) items.push(row.length);
  if (row.genre) items.push(row.genre);
  if (row.language) items.push(row.language);
  if (items.length === 0) return <span>—</span>;
  return (
    <div className="flex flex-wrap gap-1">
      {items.map((s) => (
        <span
          key={s}
          className="rounded-full border border-slate-800 bg-slate-900/60 px-2 py-0.5"
        >
          {s}
        </span>
      ))}
    </div>
  );
}

function TemplateDialog({
  mode,
  initial,
  onClose,
  onSaved,
}: {
  mode: "create" | "edit";
  initial?: TopicTemplate;
  onClose: () => void;
  onSaved: () => void;
}): JSX.Element {
  const [title, setTitle] = useState(initial?.title ?? "");
  const [topic, setTopic] = useState(initial?.topic ?? "");
  const [genre, setGenre] = useState(initial?.genre ?? "");
  const [length, setLength] = useState<AudiobookLength | "">(
    initial?.length ?? "",
  );
  const [language, setLanguage] = useState<string>(initial?.language ?? "");
  const [sortOrder, setSortOrder] = useState<number>(initial?.sort_order ?? 0);
  const [enabled, setEnabled] = useState<boolean>(initial?.enabled ?? true);
  const [isShort, setIsShort] = useState<boolean>(initial?.is_short ?? false);

  useEffect(() => {
    const onKey = (e: KeyboardEvent): void => {
      if (e.key === "Escape") onClose();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [onClose]);

  const save = useMutation({
    mutationFn: async () => {
      if (mode === "create") {
        const body: CreateTopicTemplateRequest = {
          title: title.trim(),
          topic: topic.trim(),
          genre: genre.trim() || null,
          length: length || null,
          language: language || null,
          sort_order: sortOrder,
          enabled,
          is_short: isShort,
        };
        return admin.topicTemplates.create(body);
      } else {
        const body: UpdateTopicTemplateRequest = {
          title: title.trim(),
          topic: topic.trim(),
          // Empty string clears genre/language; null on length explicitly clears.
          genre: genre.trim(),
          length: length === "" ? null : length,
          language: language,
          sort_order: sortOrder,
          enabled,
          is_short: isShort,
        };
        return admin.topicTemplates.patch(initial!.id, body);
      }
    },
    onSuccess: onSaved,
  });

  const valid = title.trim().length > 0 && topic.trim().length > 0;

  return (
    <div
      className="fixed inset-0 z-50 flex items-center justify-center bg-black/60 p-4"
      onClick={onClose}
    >
      <form
        onClick={(e) => e.stopPropagation()}
        onSubmit={(e) => {
          e.preventDefault();
          if (valid && !save.isPending) save.mutate();
        }}
        className="w-full max-w-xl rounded-xl border border-slate-800 bg-slate-950 p-5 shadow-xl"
      >
        <h2 className="text-base font-semibold text-slate-100">
          {mode === "create" ? "Add template" : "Edit template"}
        </h2>

        <div className="mt-4 grid gap-3 sm:grid-cols-2">
          <Labelled label="Title" hint="Shown in the dropdown">
            <input
              type="text"
              value={title}
              maxLength={120}
              onChange={(e) => setTitle(e.target.value)}
              placeholder="A short history of …"
              className={inputCls}
            />
          </Labelled>
          <Labelled label="Sort order" hint="Lower comes first">
            <input
              type="number"
              value={sortOrder}
              onChange={(e) => setSortOrder(Number(e.target.value) || 0)}
              className={inputCls}
            />
          </Labelled>
          <div className="sm:col-span-2">
            <Labelled label="Topic" hint="Pre-fills the topic field">
              <textarea
                value={topic}
                maxLength={1000}
                onChange={(e) => setTopic(e.target.value)}
                rows={4}
                placeholder="What should the audiobook cover?"
                className={`${inputCls} resize-y`}
              />
            </Labelled>
          </div>
          <Labelled label="Genre default (optional)">
            <input
              type="text"
              value={genre}
              maxLength={40}
              onChange={(e) => setGenre(e.target.value)}
              placeholder="e.g. History"
              className={inputCls}
            />
          </Labelled>
          <Labelled label="Length default (optional)">
            <select
              value={length}
              onChange={(e) => setLength(e.target.value as AudiobookLength | "")}
              className={inputCls}
            >
              <option value="">—</option>
              {LENGTHS.map((l) => (
                <option key={l} value={l}>
                  {l}
                </option>
              ))}
            </select>
          </Labelled>
          <Labelled label="Language default (optional)">
            <select
              value={language}
              onChange={(e) => setLanguage(e.target.value)}
              className={inputCls}
            >
              <option value="">—</option>
              {LANGUAGE_CODES.map((l) => (
                <option key={l.code} value={l.code}>
                  {l.flag} {l.label}
                </option>
              ))}
            </select>
          </Labelled>
          <Labelled label="Enabled">
            <label className="mt-1 inline-flex cursor-pointer items-center gap-2 text-sm text-slate-200">
              <input
                type="checkbox"
                checked={enabled}
                onChange={(e) => setEnabled(e.target.checked)}
                className="h-4 w-4 cursor-pointer accent-sky-500"
              />
              {enabled ? "Visible to users" : "Hidden from users"}
            </label>
          </Labelled>
          <Labelled label="YouTube Short" hint="Single ≤ 90 s vertical clip">
            <label className="mt-1 inline-flex cursor-pointer items-center gap-2 text-sm text-slate-200">
              <input
                type="checkbox"
                checked={isShort}
                onChange={(e) => setIsShort(e.target.checked)}
                className="h-4 w-4 cursor-pointer accent-rose-500"
              />
              {isShort ? "Short by default" : "Full audiobook"}
            </label>
          </Labelled>
        </div>

        {save.error && (
          <p className="mt-3 text-xs text-rose-400">
            {save.error instanceof ApiError
              ? save.error.message
              : "Save failed"}
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
            disabled={!valid || save.isPending}
            className="rounded-md bg-sky-600 px-3 py-2 text-sm font-medium text-white hover:bg-sky-500 disabled:cursor-not-allowed disabled:bg-sky-700/50"
          >
            {save.isPending ? "Saving…" : mode === "create" ? "Create" : "Save"}
          </button>
        </div>
      </form>
    </div>
  );
}

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

const inputCls =
  "mt-1 w-full rounded-md border border-slate-800 bg-slate-900 px-3 py-2 text-sm text-slate-100 outline-none focus:border-sky-600";
