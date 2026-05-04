import { FormEvent, useState } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useNavigate } from "react-router-dom";
import { ApiError, ideas } from "../api";
import type {
  IdeaRow,
  IdeaStatus,
  SuggestedIdea,
} from "../api";
import { CopyButton } from "../components/CopyButton";

const STATUS_LABELS: Record<IdeaStatus, string> = {
  pending: "Pending",
  in_progress: "In progress",
  completed: "Completed",
};

const STATUS_TONE: Record<IdeaStatus, string> = {
  pending: "border-slate-700 bg-slate-900 text-slate-300",
  in_progress: "border-amber-800 bg-amber-950/40 text-amber-200",
  completed: "border-emerald-800 bg-emerald-950/40 text-emerald-200",
};

const SUGGEST_LANGUAGES: { code: string; label: string }[] = [
  { code: "en", label: "English" },
  { code: "nl", label: "Dutch" },
  { code: "fr", label: "French" },
  { code: "de", label: "German" },
  { code: "es", label: "Spanish" },
];

export function Ideas(): JSX.Element {
  const qc = useQueryClient();
  const navigate = useNavigate();
  // Tracks which library hits have been imported this session so the
  // result card flips to "Imported" without waiting for the backlog
  // refetch — and lets us show an in-flight indicator on the right card.
  const [importedKeys, setImportedKeys] = useState<Set<string>>(new Set());
  const [importingKey, setImportingKey] = useState<string | null>(null);

  const list = useQuery({
    queryKey: ["ideas"],
    queryFn: () => ideas.list(),
  });

  const create = useMutation({
    mutationFn: (body: { title: string; audiobook_prompt: string; source?: string }) =>
      ideas.create(body),
    onSuccess: () => qc.invalidateQueries({ queryKey: ["ideas"] }),
  });

  const patch = useMutation({
    mutationFn: ({ id, status }: { id: string; status: IdeaStatus }) =>
      ideas.patch(id, { status }),
    onSuccess: () => qc.invalidateQueries({ queryKey: ["ideas"] }),
  });

  const remove = useMutation({
    mutationFn: (id: string) => ideas.remove(id),
    onSuccess: () => qc.invalidateQueries({ queryKey: ["ideas"] }),
  });

  const items = list.data?.items ?? [];

  return (
    <div className="space-y-8">
      <header>
        <h1 className="text-2xl font-semibold tracking-tight text-slate-100">
          Ideas
        </h1>
        <p className="mt-1 max-w-2xl text-sm text-slate-400">
          A backlog of audiobook ideas. Add your own, or pull a fresh batch
          of LLM-suggested trends — pick the ones worth keeping and
          generate the audiobook later from the prompt.
        </p>
      </header>

      <AddIdeaForm
        onSubmit={(title, prompt) =>
          create.mutate({ title, audiobook_prompt: prompt, source: "manual" })
        }
        pending={create.isPending}
      />

      <SuggestPanel
        onKeep={(suggestion) =>
          create.mutate({
            title: suggestion.title,
            audiobook_prompt: suggestion.audiobook_prompt,
            source: "trend",
          })
        }
        existingTitles={new Set(items.map((i) => i.title.toLowerCase()))}
      />

      <LibrarySearchPanel
        existingTitles={new Set(items.map((i) => i.title.toLowerCase()))}
        importedKeys={importedKeys}
        importing={importingKey}
        onImport={(hit) => {
          setImportingKey(hit.key);
          create.mutate(
            {
              title: buildImportTitle(hit),
              audiobook_prompt: buildImportPrompt(hit),
              source: "library",
            },
            {
              onSuccess: () => {
                setImportedKeys((prev) => {
                  const next = new Set(prev);
                  next.add(hit.key);
                  return next;
                });
              },
              onSettled: () => setImportingKey(null),
            },
          );
        }}
      />

      <section>
        <h2 className="mb-3 text-sm font-semibold uppercase tracking-wide text-slate-400">
          Backlog
        </h2>

        {list.isLoading ? (
          <p className="text-sm text-slate-500">Loading…</p>
        ) : list.error ? (
          <p className="text-sm text-rose-400">
            {list.error instanceof ApiError
              ? list.error.message
              : "Failed to load ideas"}
          </p>
        ) : items.length === 0 ? (
          <p className="rounded-lg border border-dashed border-slate-800 p-6 text-center text-sm text-slate-500">
            No ideas yet. Add one above or pull some trends.
          </p>
        ) : (
          <div className="overflow-x-auto rounded-lg border border-slate-800">
            <table className="w-full text-sm">
              <thead className="bg-slate-900/60 text-left text-xs uppercase tracking-wide text-slate-500">
                <tr>
                  <th className="px-3 py-2">Idea</th>
                  <th className="px-3 py-2">Audiobook prompt</th>
                  <th className="px-3 py-2 whitespace-nowrap">Status</th>
                  <th className="px-3 py-2 whitespace-nowrap">Date added</th>
                  <th className="px-3 py-2 whitespace-nowrap">Date completed</th>
                  <th className="px-3 py-2 text-right whitespace-nowrap">Actions</th>
                </tr>
              </thead>
              <tbody>
                {items.map((row) => (
                  <IdeaRowView
                    key={row.id}
                    row={row}
                    onSetStatus={(status) =>
                      patch.mutate({ id: row.id, status })
                    }
                    onStart={() => {
                      // Flip status first so the backlog reflects the
                      // user's intent even if they bail out of the
                      // create flow without saving.
                      patch.mutate({ id: row.id, status: "in_progress" });
                      const seed =
                        row.audiobook_prompt.trim() || row.title.trim();
                      navigate(
                        `/app/new?topic=${encodeURIComponent(seed)}`,
                      );
                    }}
                    onDelete={() => {
                      if (window.confirm(`Delete idea "${row.title}"?`)) {
                        remove.mutate(row.id);
                      }
                    }}
                    busy={
                      (patch.isPending && patch.variables?.id === row.id) ||
                      (remove.isPending && remove.variables === row.id)
                    }
                  />
                ))}
              </tbody>
            </table>
          </div>
        )}

        {(create.error || patch.error || remove.error) && (
          <p className="mt-3 text-sm text-rose-400">
            {(() => {
              const e = create.error ?? patch.error ?? remove.error;
              return e instanceof ApiError ? e.message : "Action failed";
            })()}
          </p>
        )}
      </section>
    </div>
  );
}

// --- Add idea form ------------------------------------------------------

function AddIdeaForm({
  onSubmit,
  pending,
}: {
  onSubmit: (title: string, prompt: string) => void;
  pending: boolean;
}): JSX.Element {
  const [title, setTitle] = useState("");
  const [prompt, setPrompt] = useState("");

  return (
    <form
      onSubmit={(e: FormEvent) => {
        e.preventDefault();
        const t = title.trim();
        if (!t) return;
        onSubmit(t, prompt.trim());
        setTitle("");
        setPrompt("");
      }}
      className="space-y-2 rounded-lg border border-slate-800 bg-slate-900/40 p-4"
    >
      <h2 className="text-sm font-semibold text-slate-200">Add your own idea</h2>
      <input
        type="text"
        value={title}
        onChange={(e) => setTitle(e.target.value)}
        maxLength={300}
        placeholder="Short label, e.g. ‘The 1848 cholera outbreak in London’"
        className="w-full rounded-md border border-slate-700 bg-slate-950 px-3 py-1.5 text-sm text-slate-100 outline-none focus:border-sky-600"
      />
      <textarea
        value={prompt}
        onChange={(e) => setPrompt(e.target.value)}
        maxLength={4000}
        rows={3}
        placeholder="Audiobook prompt — angle, hook, what makes this listen worth it. Optional."
        className="w-full rounded-md border border-slate-700 bg-slate-950 px-3 py-1.5 text-sm text-slate-100 outline-none focus:border-sky-600"
      />
      <div className="flex justify-end">
        <button
          type="submit"
          disabled={!title.trim() || pending}
          className="rounded-md bg-sky-600 px-3 py-1.5 text-sm font-medium text-white hover:bg-sky-500 disabled:cursor-not-allowed disabled:opacity-40"
        >
          {pending ? "Adding…" : "Add idea"}
        </button>
      </div>
    </form>
  );
}

// --- Trend suggestions panel --------------------------------------------

function SuggestPanel({
  onKeep,
  existingTitles,
}: {
  onKeep: (suggestion: SuggestedIdea) => void;
  existingTitles: Set<string>;
}): JSX.Element {
  const [seed, setSeed] = useState("");
  const [language, setLanguage] = useState("en");
  const [count, setCount] = useState(8);
  const [results, setResults] = useState<SuggestedIdea[] | null>(null);

  const fetchTrends = useMutation({
    mutationFn: () =>
      ideas.suggest({
        seed: seed.trim() || null,
        language,
        count,
      }),
    onSuccess: (data) => setResults(data.items),
  });

  return (
    <section className="space-y-3 rounded-lg border border-slate-800 bg-slate-900/40 p-4">
      <header className="flex flex-wrap items-end justify-between gap-3">
        <div>
          <h2 className="text-sm font-semibold text-slate-200">
            Search trends with AI
          </h2>
          <p className="mt-1 text-xs text-slate-500">
            Asks the configured random-topic LLM what's currently capturing
            public curiosity (X / Reddit / news cycles) and proposes
            audiobook-ready angles. Suggestions aren't saved until you keep them.
          </p>
        </div>
      </header>

      <div className="flex flex-wrap items-end gap-2">
        <label className="flex-1 min-w-[200px] text-xs text-slate-400">
          Theme hint (optional)
          <input
            type="text"
            value={seed}
            onChange={(e) => setSeed(e.target.value)}
            maxLength={300}
            placeholder="e.g. AI, climate, ancient history…"
            className="mt-1 w-full rounded-md border border-slate-700 bg-slate-950 px-3 py-1.5 text-sm text-slate-100 outline-none focus:border-sky-600"
          />
        </label>
        <label className="text-xs text-slate-400">
          Language
          <select
            value={language}
            onChange={(e) => setLanguage(e.target.value)}
            className="mt-1 block rounded-md border border-slate-700 bg-slate-950 px-2 py-1.5 text-sm text-slate-100 outline-none focus:border-sky-600"
          >
            {SUGGEST_LANGUAGES.map((l) => (
              <option key={l.code} value={l.code}>
                {l.label}
              </option>
            ))}
          </select>
        </label>
        <label className="text-xs text-slate-400">
          Count
          <input
            type="number"
            min={1}
            max={12}
            value={count}
            onChange={(e) =>
              setCount(Math.max(1, Math.min(12, Number(e.target.value) || 1)))
            }
            className="mt-1 w-16 rounded-md border border-slate-700 bg-slate-950 px-2 py-1.5 text-sm text-slate-100 outline-none focus:border-sky-600"
          />
        </label>
        <button
          type="button"
          onClick={() => fetchTrends.mutate()}
          disabled={fetchTrends.isPending}
          className="rounded-md bg-sky-600 px-3 py-1.5 text-sm font-medium text-white hover:bg-sky-500 disabled:cursor-not-allowed disabled:opacity-40"
        >
          {fetchTrends.isPending ? "Searching…" : "Search trends"}
        </button>
      </div>

      {fetchTrends.error && (
        <p className="text-sm text-rose-400">
          {fetchTrends.error instanceof ApiError
            ? fetchTrends.error.message
            : "Trend search failed"}
        </p>
      )}

      {results && results.length > 0 && (
        <ul className="grid grid-cols-1 gap-2 md:grid-cols-2">
          {results.map((s, i) => {
            const dupe = existingTitles.has(s.title.trim().toLowerCase());
            return (
              <li
                key={i}
                className="space-y-1 rounded-md border border-slate-800 bg-slate-950/60 p-3"
              >
                <div className="flex items-start justify-between gap-2">
                  <div className="text-sm font-medium text-slate-100">
                    {s.title}
                  </div>
                  <button
                    type="button"
                    onClick={() => onKeep(s)}
                    disabled={dupe}
                    title={dupe ? "Already in your backlog" : "Save to backlog"}
                    className="shrink-0 rounded-md border border-emerald-800 bg-emerald-950/40 px-2 py-1 text-xs text-emerald-200 hover:border-emerald-700 disabled:cursor-not-allowed disabled:opacity-40"
                  >
                    {dupe ? "Already kept" : "Keep"}
                  </button>
                </div>
                <p className="text-xs leading-relaxed text-slate-400">
                  {s.audiobook_prompt}
                </p>
              </li>
            );
          })}
        </ul>
      )}
      {results && results.length === 0 && (
        <p className="text-xs text-slate-500">
          The LLM returned no suggestions. Try a different theme hint.
        </p>
      )}
    </section>
  );
}

// --- Public-domain library search ---------------------------------------

const LIBRARY_CATEGORIES: { value: string; label: string }[] = [
  { value: "", label: "Any genre" },
  { value: "historical fiction", label: "Historical fiction" },
  { value: "fiction", label: "Fiction" },
  { value: "non-fiction", label: "Non-fiction" },
  { value: "biography", label: "Biography & memoir" },
  { value: "science", label: "Science" },
  { value: "philosophy", label: "Philosophy" },
  { value: "poetry", label: "Poetry" },
  { value: "drama", label: "Drama" },
  { value: "children's", label: "Children's" },
  { value: "mystery", label: "Mystery" },
  { value: "adventure", label: "Adventure" },
];

type LibrarySourceKey = "openlibrary" | "gutenberg" | "archive";

interface LibraryHit {
  // Stable cross-source id for keying lists and dedupe.
  key: string;
  source: LibrarySourceKey;
  sourceLabel: string;
  title: string;
  authors: string[];
  year?: number | null;
  // Open original record in a new tab (covers users who want to verify).
  externalUrl: string;
  coverUrl?: string | null;
  subjects?: string[];
  // Short blurb if the API exposes one (Gutendex doesn't, OL doesn't, IA does).
  blurb?: string | null;
}

const SOURCE_BADGE: Record<LibrarySourceKey, string> = {
  openlibrary:
    "border-emerald-900 bg-emerald-950/40 text-emerald-200",
  gutenberg: "border-amber-900 bg-amber-950/40 text-amber-200",
  archive: "border-sky-900 bg-sky-950/40 text-sky-200",
};

async function searchOpenLibrary(q: string): Promise<LibraryHit[]> {
  const url = `https://openlibrary.org/search.json?q=${encodeURIComponent(q)}&limit=10&fields=key,title,author_name,first_publish_year,cover_i,subject`;
  const res = await fetch(url);
  if (!res.ok) throw new Error(`Open Library: ${res.status}`);
  const json = (await res.json()) as {
    docs?: Array<{
      key?: string;
      title?: string;
      author_name?: string[];
      first_publish_year?: number;
      cover_i?: number;
      subject?: string[];
    }>;
  };
  return (json.docs ?? [])
    .filter((d) => d.title)
    .map<LibraryHit>((d) => ({
      key: `openlibrary:${d.key ?? d.title!}`,
      source: "openlibrary",
      sourceLabel: "Open Library",
      title: d.title!,
      authors: d.author_name ?? [],
      year: d.first_publish_year ?? null,
      externalUrl: d.key
        ? `https://openlibrary.org${d.key}`
        : `https://openlibrary.org/search?q=${encodeURIComponent(q)}`,
      coverUrl: d.cover_i
        ? `https://covers.openlibrary.org/b/id/${d.cover_i}-S.jpg`
        : null,
      subjects: (d.subject ?? []).slice(0, 6),
    }));
}

async function searchGutenberg(q: string): Promise<LibraryHit[]> {
  // Gutendex is the unofficial JSON API in front of Project Gutenberg's
  // catalogue. Public, CORS-enabled, no key required.
  const url = `https://gutendex.com/books?search=${encodeURIComponent(q)}`;
  const res = await fetch(url);
  if (!res.ok) throw new Error(`Gutenberg: ${res.status}`);
  const json = (await res.json()) as {
    results?: Array<{
      id: number;
      title: string;
      authors?: Array<{ name: string }>;
      subjects?: string[];
      formats?: Record<string, string>;
    }>;
  };
  return (json.results ?? []).slice(0, 10).map<LibraryHit>((b) => ({
    key: `gutenberg:${b.id}`,
    source: "gutenberg",
    sourceLabel: "Project Gutenberg",
    title: b.title,
    authors: (b.authors ?? []).map((a) => a.name),
    year: null,
    externalUrl: `https://www.gutenberg.org/ebooks/${b.id}`,
    coverUrl: b.formats?.["image/jpeg"] ?? null,
    subjects: (b.subjects ?? []).slice(0, 6),
  }));
}

async function searchInternetArchive(q: string): Promise<LibraryHit[]> {
  const queryString = `(${q}) AND mediatype:texts`;
  const url =
    `https://archive.org/advancedsearch.php?q=${encodeURIComponent(queryString)}` +
    `&fl[]=identifier&fl[]=title&fl[]=creator&fl[]=year&fl[]=description&fl[]=subject&output=json&rows=10`;
  const res = await fetch(url);
  if (!res.ok) throw new Error(`Archive.org: ${res.status}`);
  const json = (await res.json()) as {
    response?: {
      docs?: Array<{
        identifier: string;
        title?: string;
        creator?: string | string[];
        year?: string | number;
        description?: string | string[];
        subject?: string | string[];
      }>;
    };
  };
  const toArray = (v?: string | string[]): string[] =>
    v == null ? [] : Array.isArray(v) ? v : [v];
  return (json.response?.docs ?? [])
    .filter((d) => d.title)
    .map<LibraryHit>((d) => ({
      key: `archive:${d.identifier}`,
      source: "archive",
      sourceLabel: "Internet Archive",
      title: d.title!,
      authors: toArray(d.creator),
      year: d.year ? Number(d.year) || null : null,
      externalUrl: `https://archive.org/details/${d.identifier}`,
      coverUrl: `https://archive.org/services/img/${d.identifier}`,
      subjects: toArray(d.subject).slice(0, 6),
      blurb: toArray(d.description)[0] ?? null,
    }));
}

function buildImportPrompt(hit: LibraryHit): string {
  const parts: string[] = [];
  const author = hit.authors.join(", ");
  if (author) {
    parts.push(`Adapt "${hit.title}" by ${author}.`);
  } else {
    parts.push(`Adapt "${hit.title}".`);
  }
  if (hit.year) parts.push(`Originally published ${hit.year}.`);
  parts.push(`Source: ${hit.sourceLabel} (${hit.externalUrl}).`);
  if (hit.blurb) {
    // Strip HTML and clamp — IA descriptions are sometimes huge HTML blobs.
    const flat = hit.blurb.replace(/<[^>]+>/g, " ").replace(/\s+/g, " ").trim();
    if (flat) parts.push(flat.length > 600 ? `${flat.slice(0, 600)}…` : flat);
  }
  if (hit.subjects && hit.subjects.length) {
    parts.push(`Subjects: ${hit.subjects.slice(0, 4).join(", ")}.`);
  }
  return parts.join(" ");
}

function buildImportTitle(hit: LibraryHit): string {
  const author = hit.authors[0];
  const base = author ? `${hit.title} — ${author}` : hit.title;
  return base.length > 290 ? `${base.slice(0, 287)}…` : base;
}

function LibrarySearchPanel({
  onImport,
  importing,
  importedKeys,
  existingTitles,
}: {
  onImport: (hit: LibraryHit) => void;
  importing: string | null;
  importedKeys: Set<string>;
  existingTitles: Set<string>;
}): JSX.Element {
  const [query, setQuery] = useState("");
  const [category, setCategory] = useState("");
  const [results, setResults] = useState<LibraryHit[] | null>(null);
  const [errors, setErrors] = useState<string[]>([]);
  const [loading, setLoading] = useState(false);

  const combined = [query.trim(), category].filter(Boolean).join(" ").trim();

  const runSearch = async (): Promise<void> => {
    if (!combined) return;
    setLoading(true);
    setErrors([]);
    setResults(null);
    const settled = await Promise.allSettled([
      searchOpenLibrary(combined),
      searchGutenberg(combined),
      searchInternetArchive(combined),
    ]);
    const hits: LibraryHit[] = [];
    const errs: string[] = [];
    for (const r of settled) {
      if (r.status === "fulfilled") hits.push(...r.value);
      else errs.push(r.reason instanceof Error ? r.reason.message : String(r.reason));
    }
    // Interleave by source so the first screen isn't dominated by one
    // catalogue. A simple round-robin merge works since each list is
    // already in relevance order from its API.
    const buckets = new Map<LibrarySourceKey, LibraryHit[]>();
    for (const h of hits) {
      const list = buckets.get(h.source) ?? [];
      list.push(h);
      buckets.set(h.source, list);
    }
    const interleaved: LibraryHit[] = [];
    let added = true;
    while (added) {
      added = false;
      for (const list of buckets.values()) {
        const next = list.shift();
        if (next) {
          interleaved.push(next);
          added = true;
        }
      }
    }
    setResults(interleaved);
    setErrors(errs);
    setLoading(false);
  };

  return (
    <section className="space-y-3 rounded-lg border border-slate-800 bg-slate-900/40 p-4">
      <header>
        <h2 className="text-sm font-semibold text-slate-200">
          Import from public-domain libraries
        </h2>
        <p className="mt-1 text-xs text-slate-500">
          Search Open Library, Project Gutenberg, and the Internet Archive.
          Importing creates an idea seeded with the book's title, author,
          and source link — adapt it to an audiobook later from the backlog.
        </p>
      </header>

      <form
        onSubmit={(e) => {
          e.preventDefault();
          void runSearch();
        }}
        className="flex flex-wrap items-end gap-2"
      >
        <label className="flex-1 min-w-[200px] text-xs text-slate-400">
          Keyword
          <input
            type="text"
            value={query}
            onChange={(e) => setQuery(e.target.value)}
            maxLength={300}
            placeholder="e.g. Napoleon, Brontë, voyages…"
            className="mt-1 w-full rounded-md border border-slate-700 bg-slate-950 px-3 py-1.5 text-sm text-slate-100 outline-none focus:border-sky-600"
          />
        </label>
        <label className="text-xs text-slate-400">
          Genre
          <select
            value={category}
            onChange={(e) => setCategory(e.target.value)}
            className="mt-1 block rounded-md border border-slate-700 bg-slate-950 px-2 py-1.5 text-sm text-slate-100 outline-none focus:border-sky-600"
          >
            {LIBRARY_CATEGORIES.map((c) => (
              <option key={c.label} value={c.value}>
                {c.label}
              </option>
            ))}
          </select>
        </label>
        <button
          type="submit"
          disabled={!combined || loading}
          className="rounded-md bg-sky-600 px-3 py-1.5 text-sm font-medium text-white hover:bg-sky-500 disabled:cursor-not-allowed disabled:opacity-40"
        >
          {loading ? "Searching…" : "Search"}
        </button>
      </form>

      {errors.length > 0 && (
        <p className="text-xs text-rose-400">
          Some sources failed: {errors.join("; ")}
        </p>
      )}

      {results && results.length === 0 && !loading && (
        <p className="text-xs text-slate-500">
          No matches across the searched catalogues.
        </p>
      )}

      {results && results.length > 0 && (
        <ul className="grid grid-cols-1 gap-2 md:grid-cols-2">
          {results.map((hit) => {
            const titleKey = buildImportTitle(hit).trim().toLowerCase();
            const dupe =
              importedKeys.has(hit.key) || existingTitles.has(titleKey);
            const isImporting = importing === hit.key;
            return (
              <li
                key={hit.key}
                className="flex gap-3 rounded-md border border-slate-800 bg-slate-950/60 p-3"
              >
                {hit.coverUrl ? (
                  // External image — let it fail gracefully if the cover
                  // URL is missing (common on IA records without a thumb).
                  <img
                    src={hit.coverUrl}
                    alt=""
                    loading="lazy"
                    onError={(e) => {
                      (e.currentTarget as HTMLImageElement).style.display = "none";
                    }}
                    className="h-20 w-14 shrink-0 rounded border border-slate-800 object-cover"
                  />
                ) : (
                  <div className="flex h-20 w-14 shrink-0 items-center justify-center rounded border border-slate-800 bg-slate-900 text-lg text-slate-700">
                    📖
                  </div>
                )}
                <div className="min-w-0 flex-1 space-y-1">
                  <div className="flex items-start justify-between gap-2">
                    <div className="min-w-0">
                      <p className="truncate text-sm font-medium text-slate-100">
                        {hit.title}
                      </p>
                      {hit.authors.length > 0 && (
                        <p className="truncate text-xs text-slate-400">
                          {hit.authors.join(", ")}
                          {hit.year ? ` · ${hit.year}` : ""}
                        </p>
                      )}
                    </div>
                    <button
                      type="button"
                      onClick={() => onImport(hit)}
                      disabled={dupe || isImporting}
                      title={
                        dupe
                          ? "Already in your backlog"
                          : "Import as an idea"
                      }
                      className="shrink-0 rounded-md border border-emerald-800 bg-emerald-950/40 px-2 py-1 text-xs text-emerald-200 hover:border-emerald-700 disabled:cursor-not-allowed disabled:opacity-40"
                    >
                      {isImporting
                        ? "Importing…"
                        : dupe
                          ? "Imported"
                          : "Import"}
                    </button>
                  </div>
                  <div className="flex flex-wrap items-center gap-1.5">
                    <span
                      className={`rounded-md border px-1.5 py-0.5 text-[10px] uppercase tracking-wide ${SOURCE_BADGE[hit.source]}`}
                    >
                      {hit.sourceLabel}
                    </span>
                    <a
                      href={hit.externalUrl}
                      target="_blank"
                      rel="noopener noreferrer"
                      className="text-[10px] text-slate-500 hover:text-sky-300"
                    >
                      view source ↗
                    </a>
                  </div>
                  {hit.subjects && hit.subjects.length > 0 && (
                    <p className="line-clamp-2 text-[11px] text-slate-500">
                      {hit.subjects.slice(0, 4).join(" · ")}
                    </p>
                  )}
                </div>
              </li>
            );
          })}
        </ul>
      )}
    </section>
  );
}

// --- Single backlog row -------------------------------------------------

function IdeaRowView({
  row,
  onSetStatus,
  onStart,
  onDelete,
  busy,
}: {
  row: IdeaRow;
  onSetStatus: (status: IdeaStatus) => void;
  onStart: () => void;
  onDelete: () => void;
  busy: boolean;
}): JSX.Element {
  return (
    <tr className="border-t border-slate-800 align-top">
      <td className="px-3 py-3">
        <div className="font-medium text-slate-100">{row.title}</div>
        {row.source === "trend" && (
          <span className="mt-1 inline-block rounded-md border border-violet-900 bg-violet-950/40 px-1.5 py-0.5 text-[10px] uppercase tracking-wide text-violet-300">
            trend
          </span>
        )}
        {row.source === "library" && (
          <span className="mt-1 inline-block rounded-md border border-emerald-900 bg-emerald-950/40 px-1.5 py-0.5 text-[10px] uppercase tracking-wide text-emerald-300">
            library
          </span>
        )}
      </td>
      <td className="px-3 py-3 text-xs text-slate-300">
        {row.audiobook_prompt ? (
          <div className="flex items-start gap-2">
            <p className="max-w-md whitespace-pre-wrap text-slate-300">
              {row.audiobook_prompt}
            </p>
            <CopyButton text={row.audiobook_prompt} title="Copy prompt" />
          </div>
        ) : (
          <span className="text-slate-600">—</span>
        )}
      </td>
      <td className="px-3 py-3 whitespace-nowrap">
        <span
          className={`inline-block rounded-md border px-1.5 py-0.5 text-[11px] ${STATUS_TONE[row.status]}`}
        >
          {STATUS_LABELS[row.status]}
        </span>
      </td>
      <td className="px-3 py-3 whitespace-nowrap text-xs text-slate-500">
        {new Date(row.created_at).toLocaleDateString()}
      </td>
      <td className="px-3 py-3 whitespace-nowrap text-xs text-slate-500">
        {row.completed_at
          ? new Date(row.completed_at).toLocaleDateString()
          : "—"}
      </td>
      <td className="px-3 py-3 text-right">
        <div className="flex justify-end gap-2">
          {row.status !== "in_progress" && row.status !== "completed" && (
            <button
              type="button"
              onClick={onStart}
              disabled={busy}
              title="Mark in progress and open the New Audiobook screen with this prompt"
              className="rounded-md border border-amber-800 bg-amber-950/40 px-2 py-1 text-xs text-amber-200 hover:border-amber-700 disabled:opacity-40"
            >
              Start
            </button>
          )}
          {row.status !== "completed" && (
            <button
              type="button"
              onClick={() => onSetStatus("completed")}
              disabled={busy}
              className="rounded-md border border-emerald-800 bg-emerald-950/40 px-2 py-1 text-xs text-emerald-200 hover:border-emerald-700 disabled:opacity-40"
            >
              Mark completed
            </button>
          )}
          {row.status === "completed" && (
            <button
              type="button"
              onClick={() => onSetStatus("pending")}
              disabled={busy}
              className="rounded-md border border-slate-700 bg-slate-900 px-2 py-1 text-xs text-slate-200 hover:border-slate-600 disabled:opacity-40"
            >
              Re-open
            </button>
          )}
          <button
            type="button"
            onClick={onDelete}
            disabled={busy}
            className="rounded-md border border-rose-900 bg-rose-950/40 px-2 py-1 text-xs text-rose-300 hover:border-rose-800 disabled:opacity-40"
          >
            Delete
          </button>
        </div>
      </td>
    </tr>
  );
}
