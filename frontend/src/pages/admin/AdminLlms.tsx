import { useEffect, useMemo, useState } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { admin, ApiError } from "../../api";
import type {
  AdminLlmRow,
  CreateLlmRequest,
  OpenRouterModelRow,
  UpdateLlmRequest,
  XaiImageModelRow,
  XaiModelRow,
} from "../../api";
import { useDragReorder, DRAG_HANDLE_GLYPH } from "../../lib/llm-reorder";

const ALL_ROLES = [
  "outline",
  "chapter",
  "title",
  "random_topic",
  "moderation",
  "cover_art",
  "translate",
] as const;

const FUNCTIONS: { value: string; label: string; icon: string }[] = [
  { value: "text", label: "Text", icon: "✍️" },
  { value: "image", label: "Image", icon: "🖼️" },
  { value: "audio", label: "Audio", icon: "🔊" },
  { value: "embedding", label: "Embedding", icon: "📐" },
  { value: "multimodal", label: "Multimodal", icon: "✨" },
];

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

function functionInfo(
  value: string | null | undefined,
): { label: string; icon: string } {
  if (!value) return { label: "—", icon: "❓" };
  const m = FUNCTIONS.find((f) => f.value === value.toLowerCase());
  return m ? { label: m.label, icon: m.icon } : { label: value, icon: "🔧" };
}

function langInfo(code: string): { label: string; flag: string } {
  const m = LANGUAGES.find((l) => l.code === code);
  return m ? { label: m.label, flag: m.flag } : { label: code, flag: "🏳️" };
}

export function AdminLlms(): JSX.Element {
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
    // Hide image models — they live on the dedicated /admin/image-llm
    // page where pricing renders as $/megapixel instead of $/1k tokens.
    return [...data.items]
      .filter((row) => (row.function ?? "").toLowerCase() !== "image")
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
          <h1 className="text-xl font-semibold tracking-tight">LLMs</h1>
          <p className="mt-1 text-sm text-slate-400">
            Text models for outline, chapter, title, and moderation. Image
            generation models live on the dedicated{" "}
            <em>Image LLMs</em> page.
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
      <p className="mb-2 text-xs text-slate-500">
        Drag rows by the {DRAG_HANDLE_GLYPH} handle to reorder by priority
        (lower = higher priority in the picker).
      </p>
      <table className="w-full text-sm">
        <thead className="text-left text-xs uppercase tracking-wide text-slate-500">
          <tr>
            <th className="py-2 pr-2 w-6"></th>
            <th className="py-2 pr-4">Pri</th>
            <th className="py-2 pr-4">Name</th>
            <th className="py-2 pr-4">Function</th>
            <th className="py-2 pr-4">Model id</th>
            <th className="py-2 pr-4">Cost / 1k</th>
            <th className="py-2 pr-4">Languages</th>
            <th className="py-2 pr-4">Roles</th>
            <th className="py-2 pr-4 text-right">Status</th>
            <th className="py-2 pr-4 text-right">Actions</th>
          </tr>
        </thead>
        <tbody>
          {sorted.map((row) => {
            const fn = functionInfo(row.function);
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
                <td className="py-3 pr-4 font-medium text-slate-100">{row.name}</td>
                <td className="py-3 pr-4 text-xs text-slate-300">
                  <span className="mr-1">{fn.icon}</span>
                  {fn.label}
                </td>
                <td className="py-3 pr-4 font-mono text-xs text-slate-400">
                  {row.model_id}
                </td>
                <td className="py-3 pr-4 text-xs text-slate-300">
                  ${row.cost_prompt_per_1k.toFixed(3)} /{" "}
                  ${row.cost_completion_per_1k.toFixed(3)}
                </td>
                <td className="py-3 pr-4 text-xs text-slate-400">
                  <LanguagesBadges codes={row.languages ?? []} />
                </td>
                <td className="py-3 pr-4 text-xs text-slate-400">
                  {row.default_for.length ? row.default_for.join(", ") : "—"}
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
                        if (window.confirm(`Delete "${row.name}"?`)) {
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
            );
          })}
        </tbody>
      </table>
      <p className="mt-3 text-xs text-slate-500">
        Lower <em>Priority</em> wins when multiple rows could serve the same
        role and language.
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
          kind="text"
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
          kind="text"
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

function LanguagesBadges({ codes }: { codes: string[] }): JSX.Element {
  if (codes.length === 0) {
    return (
      <span
        title="No language restrictions — used for any language"
        className="text-slate-500"
      >
        any
      </span>
    );
  }
  return (
    <div className="flex flex-wrap gap-1">
      {codes.map((c) => {
        const info = langInfo(c);
        return (
          <span
            key={c}
            title={info.label}
            className="rounded-full border border-slate-800 bg-slate-900/60 px-1.5 py-0.5"
          >
            {info.flag} {c}
          </span>
        );
      })}
    </div>
  );
}

export function LlmDialog({
  kind,
  mode,
  initial,
  onClose,
  onSaved,
}: {
  /** Pricing+role layout: `text` shows $/1k tokens, `image` shows $/megapixel. */
  kind: "text" | "image";
  mode: "create" | "edit";
  initial?: AdminLlmRow;
  onClose: () => void;
  onSaved: () => void;
}): JSX.Element {
  const [id, setId] = useState(initial?.id ?? "");
  const [name, setName] = useState(initial?.name ?? "");
  const [modelId, setModelId] = useState(initial?.model_id ?? "");
  const [contextWindow, setContextWindow] = useState(
    initial?.context_window ?? (kind === "image" ? 4_096 : 200_000),
  );
  const [costPrompt, setCostPrompt] = useState(initial?.cost_prompt_per_1k ?? 0);
  const [costCompletion, setCostCompletion] = useState(
    initial?.cost_completion_per_1k ?? 0,
  );
  const [costPerMp, setCostPerMp] = useState(
    initial?.cost_per_megapixel ?? 0,
  );
  const [defaultFor, setDefaultFor] = useState<string[]>(
    // For new image rows, pre-tick `cover_art` so they immediately enter
    // the picker rotation — image LLMs are useless without that role.
    initial?.default_for ??
      (kind === "image" && !initial ? ["cover_art"] : []),
  );
  const [func, setFunc] = useState<string>(
    initial?.function ?? (kind === "image" ? "image" : "text"),
  );
  const [languages, setLanguages] = useState<string[]>(initial?.languages ?? []);
  const [priority, setPriority] = useState<number>(initial?.priority ?? 100);
  // Which provider tab is active in the picker. Defaults to whatever
  // provider the row currently uses (or open_router for new rows). xAI
  // doesn't ship image models, so the tab is hidden in image kind.
  type ProviderTab = "open_router" | "xai";
  const initialTab: ProviderTab =
    (initial?.provider as ProviderTab | undefined) ?? "open_router";
  const [providerTab, setProviderTab] = useState<ProviderTab>(initialTab);

  useEffect(() => {
    const onKey = (e: KeyboardEvent): void => {
      if (e.key === "Escape") onClose();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [onClose]);

  const isImage = kind === "image";
  const save = useMutation({
    mutationFn: async () => {
      if (mode === "create") {
        const body: CreateLlmRequest = {
          id: id.trim(),
          name: name.trim(),
          model_id: modelId.trim(),
          context_window: contextWindow,
          // Image models can be priced per-token (most), per-image (Gemini),
          // or free (BYOK) — save whichever the admin entered. Text models
          // leave $/MP at 0.
          cost_prompt_per_1k: costPrompt,
          cost_completion_per_1k: costCompletion,
          cost_per_megapixel: isImage ? costPerMp : 0,
          default_for: defaultFor.length ? defaultFor : null,
          function: func || null,
          languages,
          priority,
          provider: providerTab,
        };
        return admin.llms.create(body);
      } else {
        const body: UpdateLlmRequest = {
          name: name.trim(),
          model_id: modelId.trim(),
          context_window: contextWindow,
          cost_prompt_per_1k: costPrompt,
          cost_completion_per_1k: costCompletion,
          cost_per_megapixel: isImage ? costPerMp : 0,
          default_for: defaultFor,
          function: func,
          languages,
          priority,
        };
        return admin.llms.patch(initial!.id, body);
      }
    },
    onSuccess: onSaved,
  });

  const idValid = mode === "edit" || /^[a-z0-9_]+$/.test(id);
  const valid =
    idValid &&
    name.trim().length > 0 &&
    modelId.trim().length > 0 &&
    contextWindow >= 1 &&
    costPrompt >= 0 &&
    costCompletion >= 0 &&
    (!isImage || costPerMp >= 0);

  // Roles offered for selection — filtered when the function is `image`
  // since text-content roles wouldn't sensibly route to an image model.
  const roleOptions = useMemo(() => {
    if (func === "image") {
      return ALL_ROLES.filter((r) => r === "cover_art");
    }
    if (func === "text" || func === "multimodal") {
      return ALL_ROLES;
    }
    return ALL_ROLES;
  }, [func]);

  return (
    <div
      className="fixed inset-0 z-50 flex items-center justify-center bg-black/60 p-4"
      onClick={onClose}
    >
      <form
        onClick={(e) => e.stopPropagation()}
        onSubmit={(e) => {
          e.preventDefault();
          if (!valid || save.isPending) return;
          save.mutate();
        }}
        className="w-full max-w-2xl rounded-xl border border-slate-800 bg-slate-950 p-5 shadow-xl"
      >
        <h2 className="text-base font-semibold text-slate-100">
          {mode === "create" ? "Add LLM" : "Edit LLM"}
        </h2>
        <p className="mt-1 text-xs text-slate-400">
          {mode === "create"
            ? "Register an OpenRouter model so it shows up in the picker."
            : `Editing ${initial?.id}`}
        </p>

        <div className="mt-4 grid gap-3 sm:grid-cols-2">
          {mode === "create" ? (
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
          ) : (
            <Labelled label="Id">
              <input
                type="text"
                value={id}
                disabled
                className={`${inputCls} cursor-not-allowed opacity-60`}
              />
            </Labelled>
          )}
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
          <div className="sm:col-span-2">
            <div className="mb-1 flex items-end justify-between gap-3">
              <p className="text-xs font-medium text-slate-300">
                Browse provider catalog
                <span className="ml-1 text-[10px] text-slate-500">
                  (click a row to fill the fields above)
                </span>
              </p>
              <div className="flex rounded-md border border-slate-800 bg-slate-950 p-0.5 text-xs">
                {(["open_router", "xai"] as const).map((p) => (
                  <button
                    key={p}
                    type="button"
                    onClick={() => setProviderTab(p)}
                    className={`rounded px-2.5 py-1 ${
                      providerTab === p
                        ? "bg-slate-800 text-slate-100"
                        : "text-slate-400 hover:text-slate-200"
                    }`}
                  >
                    {p === "open_router" ? "OpenRouter" : "xAI"}
                  </button>
                ))}
              </div>
            </div>
            {providerTab === "xai" && isImage ? (
              <XaiImageModelPicker
                value={modelId}
                onPick={(m) => {
                  setModelId(m.id);
                  if (!name.trim()) setName(m.id);
                  if (m.context_length && m.context_length > 0) {
                    setContextWindow(m.context_length);
                  }
                  // xAI image models bill per generated image; cost_per_image
                  // is what the cover gen path bills against, so it lands in
                  // the same $/MP column we use for OpenRouter image rows.
                  setCostPerMp(m.cost_per_image);
                  setCostPrompt(0);
                  setCostCompletion(0);
                }}
              />
            ) : providerTab === "xai" ? (
              <XaiModelPicker
                value={modelId}
                onPick={(m) => {
                  setModelId(m.id);
                  if (!name.trim()) setName(m.id);
                  if (m.context_length && m.context_length > 0) {
                    setContextWindow(m.context_length);
                  }
                  setCostPrompt(m.cost_prompt_per_1k);
                  setCostCompletion(m.cost_completion_per_1k);
                }}
              />
            ) : (
              <OpenRouterModelPicker
                kind={kind}
                value={modelId}
                onPick={(m) => {
                  setModelId(m.id);
                  if (!name.trim()) setName(m.name || m.id);
                  if (m.context_length && m.context_length > 0) {
                    setContextWindow(m.context_length);
                  }
                  // Pre-fill every price field the upstream populated. Image
                  // models can charge per-token, per-image, or both — let the
                  // admin see and tweak whichever applies. Per-image price
                  // doubles as the $/MP default since most providers ship
                  // ~1 MP frames.
                  setCostPrompt(m.cost_prompt_per_1k);
                  setCostCompletion(m.cost_completion_per_1k);
                  if (isImage) {
                    setCostPerMp(m.cost_per_image);
                  }
                }}
              />
            )}
          </div>
          {/* Token prices apply to text models always, and to image models
              that bill per-token (most modern OpenRouter image models). */}
          <Labelled label="$ / 1k prompt">
            <input
              type="number"
              min={0}
              step="any"
              value={costPrompt}
              onChange={(e) => setCostPrompt(Number(e.target.value) || 0)}
              className={inputCls}
            />
          </Labelled>
          <Labelled label="$ / 1k completion">
            <input
              type="number"
              min={0}
              step="any"
              value={costCompletion}
              onChange={(e) => setCostCompletion(Number(e.target.value) || 0)}
              className={inputCls}
            />
          </Labelled>
          {isImage && (
            <Labelled
              label="$ / megapixel"
              hint="Per-image price (Gemini-style)"
            >
              <input
                type="number"
                min={0}
                step="any"
                value={costPerMp}
                onChange={(e) => setCostPerMp(Number(e.target.value) || 0)}
                className={inputCls}
              />
            </Labelled>
          )}
          <Labelled label="Function" hint="What this model is for">
            <select
              value={func}
              onChange={(e) => setFunc(e.target.value)}
              className={inputCls}
            >
              {FUNCTIONS.map((f) => (
                <option key={f.value} value={f.value}>
                  {f.icon} {f.label}
                </option>
              ))}
            </select>
          </Labelled>
          <Labelled label="Priority" hint="Lower wins">
            <input
              type="number"
              min={0}
              value={priority}
              onChange={(e) => setPriority(Number(e.target.value) || 0)}
              className={inputCls}
            />
          </Labelled>
        </div>

        <fieldset className="mt-4">
          <legend className="text-xs font-medium text-slate-300">
            Languages
            <span className="ml-1 text-[10px] text-slate-500">
              (no selection = any)
            </span>
          </legend>
          <div className="mt-1 flex flex-wrap gap-1.5">
            {LANGUAGES.map((l) => {
              const on = languages.includes(l.code);
              return (
                <button
                  key={l.code}
                  type="button"
                  onClick={() =>
                    setLanguages((cur) =>
                      cur.includes(l.code)
                        ? cur.filter((c) => c !== l.code)
                        : [...cur, l.code],
                    )
                  }
                  className={`rounded-full border px-2.5 py-0.5 text-xs ${
                    on
                      ? "border-emerald-700 bg-emerald-900/30 text-emerald-200"
                      : "border-slate-700 bg-slate-900 text-slate-400 hover:border-slate-600"
                  }`}
                >
                  {l.flag} {l.label}
                </button>
              );
            })}
          </div>
        </fieldset>

        <fieldset className="mt-4">
          <legend className="text-xs font-medium text-slate-300">
            Default for
          </legend>
          <div className="mt-1 flex flex-wrap gap-1.5">
            {roleOptions.map((role) => {
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

        {save.error && (
          <p className="mt-3 text-xs text-rose-400">
            {save.error instanceof ApiError
              ? save.error.message
              : "Could not save LLM"}
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
            {save.isPending
              ? "Saving…"
              : mode === "create"
                ? "Create"
                : "Save"}
          </button>
        </div>
      </form>
    </div>
  );
}

function OpenRouterModelPicker({
  kind,
  value,
  onPick,
}: {
  kind: "text" | "image";
  /** Currently-picked model id (so the row highlights). */
  value: string;
  onPick: (model: OpenRouterModelRow) => void;
}): JSX.Element {
  const [q, setQ] = useState("");
  // Image admin asks OpenRouter for the filtered catalog: the unfiltered
  // /models response only includes ~7 image-output rows (chat-shaped models),
  // so providers like Sourceful, FLUX and ByteDance never surface without
  // `?output_modalities=image`.
  const orFilter = kind === "image" ? "image" : undefined;
  const { data, isLoading, error } = useQuery({
    queryKey: ["admin", "openrouter", "models", orFilter ?? "all"],
    queryFn: () => admin.openrouter.models(orFilter),
    staleTime: 5 * 60 * 1000,
  });

  const filtered = useMemo(() => {
    const items = data?.items ?? [];
    const ql = q.trim().toLowerCase();
    return items
      .filter((m) => {
        const out = m.output_modalities.map((s) => s.toLowerCase());
        if (kind === "image") {
          // Image admin: only models that *generate* images. The upstream
          // filter already enforces this; double-check defensively.
          return out.includes("image");
        }
        // Text admin: hide image-output-only models so a search like "flash"
        // doesn't surface Stable Diffusion.
        if (out.includes("image") && !out.includes("text")) return false;
        return true;
      })
      .filter((m) => {
        if (!ql) return true;
        return (
          m.id.toLowerCase().includes(ql) ||
          m.name.toLowerCase().includes(ql)
        );
      })
      .slice(0, 200);
  }, [data, kind, q]);

  return (
    <div>
      <input
        type="search"
        value={q}
        onChange={(e) => setQ(e.target.value)}
        placeholder={
          kind === "image"
            ? "Search image-generation models…"
            : "Search OpenRouter text models…"
        }
        className={inputCls}
      />
      <div className="mt-2 max-h-56 overflow-y-auto rounded-md border border-slate-800 bg-slate-950">
        {isLoading && (
          <p className="p-3 text-xs text-slate-500">
            Loading OpenRouter catalog…
          </p>
        )}
        {error && (
          <p className="p-3 text-xs text-rose-400">
            {error instanceof ApiError
              ? error.message
              : "Could not reach OpenRouter."}
          </p>
        )}
        {!isLoading && !error && filtered.length === 0 && (
          <p className="p-3 text-xs text-slate-500">No matching models.</p>
        )}
        {filtered.map((m) => {
          const active = m.id === value;
          const ctx = m.context_length
            ? `${Math.round(m.context_length / 1000)}k ctx`
            : null;
          // Image models on OpenRouter price three different ways:
          //   • per token (prompt/completion) — most modern models, e.g.
          //     gemini-3.1-flash-image-preview, gpt-5.4-image-2.
          //   • per generated image (`pricing.image`) — gemini-2.5-flash-image
          //     and gemini-3-pro-image-preview.
          //   • free (BYOK) — flux.2, riverflow-v2, seedream.
          // Show whichever fields the upstream actually populated so the
          // admin sees the real cost, not a misleading $0.0000/img.
          const price =
            kind === "image"
              ? [
                  m.cost_prompt_per_1k > 0
                    ? `$${m.cost_prompt_per_1k.toFixed(3)}/1k in`
                    : null,
                  m.cost_completion_per_1k > 0
                    ? `$${m.cost_completion_per_1k.toFixed(3)}/1k out`
                    : null,
                  m.cost_per_image > 0
                    ? `$${m.cost_per_image.toFixed(4)}/img`
                    : null,
                ]
                  .filter(Boolean)
                  .join(" · ") || "free / BYOK"
              : `$${m.cost_prompt_per_1k.toFixed(3)}/1k in · $${m.cost_completion_per_1k.toFixed(3)}/1k out`;
          return (
            <button
              key={m.id}
              type="button"
              onClick={() => onPick(m)}
              className={`flex w-full flex-col items-start gap-0.5 border-b border-slate-800 px-3 py-2 text-left last:border-b-0 ${
                active ? "bg-sky-900/30" : "hover:bg-slate-900"
              }`}
            >
              <span className="font-mono text-xs text-slate-200">{m.id}</span>
              <span className="text-[11px] text-slate-400">
                {m.name || "—"}
                <span className="mx-1 text-slate-600">·</span>
                {price}
                {ctx && (
                  <>
                    <span className="mx-1 text-slate-600">·</span>
                    {ctx}
                  </>
                )}
              </span>
            </button>
          );
        })}
      </div>
    </div>
  );
}

function XaiModelPicker({
  value,
  onPick,
}: {
  /** Currently-picked model id (so the row highlights). */
  value: string;
  onPick: (model: XaiModelRow) => void;
}): JSX.Element {
  const [q, setQ] = useState("");
  const { data, isLoading, error } = useQuery({
    queryKey: ["admin", "xai", "models"],
    queryFn: () => admin.xai.models(),
    staleTime: 5 * 60 * 1000,
    // Don't auto-retry — the most common failure is "xai_api_key not
    // configured", and pinging again won't fix that.
    retry: false,
  });

  const filtered = useMemo(() => {
    const items = data?.items ?? [];
    const ql = q.trim().toLowerCase();
    if (!ql) return items;
    return items.filter(
      (m) =>
        m.id.toLowerCase().includes(ql) ||
        m.aliases.some((a) => a.toLowerCase().includes(ql)),
    );
  }, [data, q]);

  return (
    <div>
      <input
        type="search"
        value={q}
        onChange={(e) => setQ(e.target.value)}
        placeholder="Search xAI text models…"
        className={inputCls}
      />
      <div className="mt-2 max-h-56 overflow-y-auto rounded-md border border-slate-800 bg-slate-950">
        {isLoading && (
          <p className="p-3 text-xs text-slate-500">Loading xAI catalog…</p>
        )}
        {error && (
          <p className="p-3 text-xs text-rose-400">
            {error instanceof ApiError
              ? error.message
              : "Could not reach xAI."}
            {error instanceof ApiError && error.status === 400 && (
              <span className="mt-1 block text-slate-500">
                Set <code>xai_api_key</code> in the backend environment to
                enable this tab.
              </span>
            )}
          </p>
        )}
        {!isLoading && !error && filtered.length === 0 && (
          <p className="p-3 text-xs text-slate-500">No matching models.</p>
        )}
        {filtered.map((m) => {
          const active = m.id === value;
          const ctx = m.context_length
            ? `${Math.round(m.context_length / 1000)}k ctx`
            : null;
          const price =
            m.cost_prompt_per_1k > 0 || m.cost_completion_per_1k > 0
              ? `$${m.cost_prompt_per_1k.toFixed(3)}/1k in · $${m.cost_completion_per_1k.toFixed(3)}/1k out`
              : "price unknown";
          return (
            <button
              key={m.id}
              type="button"
              onClick={() => onPick(m)}
              className={`flex w-full flex-col items-start gap-0.5 border-b border-slate-800 px-3 py-2 text-left last:border-b-0 ${
                active ? "bg-sky-900/30" : "hover:bg-slate-900"
              }`}
            >
              <span className="font-mono text-xs text-slate-200">{m.id}</span>
              <span className="text-[11px] text-slate-400">
                {price}
                {ctx && (
                  <>
                    <span className="mx-1 text-slate-600">·</span>
                    {ctx}
                  </>
                )}
                {m.aliases.length > 0 && (
                  <>
                    <span className="mx-1 text-slate-600">·</span>
                    aliases: {m.aliases.join(", ")}
                  </>
                )}
              </span>
            </button>
          );
        })}
      </div>
    </div>
  );
}

function XaiImageModelPicker({
  value,
  onPick,
}: {
  value: string;
  onPick: (model: XaiImageModelRow) => void;
}): JSX.Element {
  const [q, setQ] = useState("");
  const { data, isLoading, error } = useQuery({
    queryKey: ["admin", "xai", "image-models"],
    queryFn: () => admin.xai.imageModels(),
    staleTime: 5 * 60 * 1000,
    retry: false,
  });

  const filtered = useMemo(() => {
    const items = data?.items ?? [];
    const ql = q.trim().toLowerCase();
    if (!ql) return items;
    return items.filter(
      (m) =>
        m.id.toLowerCase().includes(ql) ||
        m.aliases.some((a) => a.toLowerCase().includes(ql)),
    );
  }, [data, q]);

  return (
    <div>
      <input
        type="search"
        value={q}
        onChange={(e) => setQ(e.target.value)}
        placeholder="Search xAI image-generation models…"
        className={inputCls}
      />
      <div className="mt-2 max-h-56 overflow-y-auto rounded-md border border-slate-800 bg-slate-950">
        {isLoading && (
          <p className="p-3 text-xs text-slate-500">Loading xAI image catalog…</p>
        )}
        {error && (
          <p className="p-3 text-xs text-rose-400">
            {error instanceof ApiError
              ? error.message
              : "Could not reach xAI."}
            {error instanceof ApiError && error.status === 400 && (
              <span className="mt-1 block text-slate-500">
                Set <code>xai_api_key</code> in the backend environment to
                enable this tab.
              </span>
            )}
          </p>
        )}
        {!isLoading && !error && filtered.length === 0 && (
          <p className="p-3 text-xs text-slate-500">No matching models.</p>
        )}
        {filtered.map((m) => {
          const active = m.id === value;
          const price =
            m.cost_per_image > 0
              ? `$${m.cost_per_image.toFixed(4)}/img`
              : "price unknown";
          return (
            <button
              key={m.id}
              type="button"
              onClick={() => onPick(m)}
              className={`flex w-full flex-col items-start gap-0.5 border-b border-slate-800 px-3 py-2 text-left last:border-b-0 ${
                active ? "bg-sky-900/30" : "hover:bg-slate-900"
              }`}
            >
              <span className="font-mono text-xs text-slate-200">{m.id}</span>
              <span className="text-[11px] text-slate-400">
                {price}
                {m.aliases.length > 0 && (
                  <>
                    <span className="mx-1 text-slate-600">·</span>
                    aliases: {m.aliases.join(", ")}
                  </>
                )}
              </span>
            </button>
          );
        })}
      </div>
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
