import { useMemo, useState } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { admin, ApiError } from "../../api";
import type { YoutubeFooterRow } from "../../api";
import { ErrorPane, Loading } from "./AdminLlms";

// Same set the rest of the admin uses; keeping it co-located avoids a tiny
// constants module just for one shared list.
const LANGUAGES: { code: string; label: string; flag: string }[] = [
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

function langInfo(code: string): { label: string; flag: string } {
  return (
    LANGUAGES.find((l) => l.code === code) ?? {
      label: code,
      flag: "🌐",
    }
  );
}

export function AdminYoutubeSettings(): JSX.Element {
  const qc = useQueryClient();
  const [addingLang, setAddingLang] = useState<string>("");

  const { data, isLoading, error } = useQuery({
    queryKey: ["admin", "youtube-settings"],
    queryFn: () => admin.youtubeSettings.list(),
  });

  const upsert = useMutation({
    mutationFn: ({ language, text }: { language: string; text: string }) =>
      admin.youtubeSettings.upsert(language, { text }),
    onSuccess: () =>
      qc.invalidateQueries({ queryKey: ["admin", "youtube-settings"] }),
  });

  const remove = useMutation({
    mutationFn: (language: string) =>
      admin.youtubeSettings.remove(language),
    onSuccess: () =>
      qc.invalidateQueries({ queryKey: ["admin", "youtube-settings"] }),
  });

  const items = data?.items ?? [];
  // Languages still available for the "Add language" picker — anything in
  // LANGUAGES that doesn't already have a row.
  const availableToAdd = useMemo(() => {
    const have = new Set(items.map((r) => r.language));
    return LANGUAGES.filter((l) => !have.has(l.code));
  }, [items]);

  if (isLoading) return <Loading />;
  if (error) return <ErrorPane error={error} />;

  return (
    <div>
      <div className="mb-6">
        <h1 className="text-xl font-semibold tracking-tight">
          YouTube settings
        </h1>
        <p className="mt-1 max-w-2xl text-sm text-slate-400">
          Set a per-language footer that gets appended to every YouTube
          description on publish — typically a disclaimer and a link to
          your website. Existing chapter timestamps and the
          auto-generated body are preserved; the footer lands at the
          end of the description.
        </p>
      </div>

      {items.length === 0 && (
        <p className="mb-4 rounded-lg border border-dashed border-slate-800 p-6 text-center text-sm text-slate-500">
          No footers configured yet. Pick a language below to add one.
        </p>
      )}

      <div className="space-y-4">
        {items.map((row) => (
          <FooterCard
            key={row.language}
            row={row}
            onSave={(text) =>
              upsert.mutate({ language: row.language, text })
            }
            onRemove={() => remove.mutate(row.language)}
            saving={upsert.isPending && upsert.variables?.language === row.language}
            removing={remove.isPending && remove.variables === row.language}
          />
        ))}
      </div>

      {availableToAdd.length > 0 && (
        <div className="mt-6 flex items-center gap-3 rounded-lg border border-slate-800 bg-slate-900/40 p-3">
          <label className="text-xs text-slate-400">Add language:</label>
          <select
            value={addingLang}
            onChange={(e) => setAddingLang(e.target.value)}
            className="flex-1 rounded-md border border-slate-700 bg-slate-950 px-3 py-2 text-sm text-slate-100"
          >
            <option value="">— Pick —</option>
            {availableToAdd.map((l) => (
              <option key={l.code} value={l.code}>
                {l.flag} {l.label}
              </option>
            ))}
          </select>
          <button
            type="button"
            disabled={!addingLang || upsert.isPending}
            onClick={() => {
              if (!addingLang) return;
              upsert.mutate(
                {
                  language: addingLang,
                  text: "Default text — edit me.",
                },
                { onSuccess: () => setAddingLang("") },
              );
            }}
            className="rounded-md bg-rose-600 px-3 py-2 text-sm font-medium text-white hover:bg-rose-500 disabled:cursor-not-allowed disabled:opacity-40"
          >
            Add
          </button>
        </div>
      )}

      {(upsert.error || remove.error) && (
        <p className="mt-3 text-sm text-rose-400">
          {(upsert.error ?? remove.error) instanceof ApiError
            ? ((upsert.error ?? remove.error) as ApiError).message
            : "Action failed"}
        </p>
      )}
    </div>
  );
}

function FooterCard({
  row,
  onSave,
  onRemove,
  saving,
  removing,
}: {
  row: YoutubeFooterRow;
  onSave: (text: string) => void;
  onRemove: () => void;
  saving: boolean;
  removing: boolean;
}): JSX.Element {
  const [text, setText] = useState(row.text);
  const info = langInfo(row.language);
  const dirty = text !== row.text;

  return (
    <section className="rounded-lg border border-slate-800 bg-slate-900/40 p-4">
      <div className="mb-2 flex items-baseline justify-between gap-3">
        <h2 className="text-sm font-medium text-slate-100">
          <span className="mr-1.5">{info.flag}</span>
          {info.label}
          <span className="ml-2 font-mono text-[11px] text-slate-500">
            {row.language}
          </span>
        </h2>
        <span className="text-[11px] text-slate-500">
          updated {new Date(row.updated_at).toLocaleString()}
        </span>
      </div>
      <textarea
        value={text}
        onChange={(e) => setText(e.target.value)}
        rows={5}
        maxLength={4000}
        placeholder="Disclaimer, website link, contact info, …"
        className="w-full rounded-md border border-slate-800 bg-slate-950 px-3 py-2 font-mono text-xs text-slate-100 outline-none focus:border-sky-600"
      />
      <div className="mt-2 flex items-center justify-between gap-3">
        <span className="text-[11px] text-slate-500">
          {text.length} / 4000 chars
        </span>
        <div className="flex gap-2">
          <button
            type="button"
            onClick={() => {
              if (window.confirm(`Remove footer for ${info.label}?`)) {
                onRemove();
              }
            }}
            disabled={removing}
            className="rounded-md border border-rose-900 bg-rose-950/40 px-3 py-1.5 text-xs text-rose-300 hover:border-rose-800 disabled:opacity-40"
          >
            Remove
          </button>
          <button
            type="button"
            onClick={() => onSave(text)}
            disabled={!dirty || saving || text.trim().length === 0}
            className="rounded-md bg-sky-600 px-3 py-1.5 text-xs font-medium text-white hover:bg-sky-500 disabled:cursor-not-allowed disabled:opacity-40"
          >
            {saving ? "Saving…" : "Save"}
          </button>
        </div>
      </div>
    </section>
  );
}
