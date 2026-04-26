import { FormEvent, useEffect, useRef, useState } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useNavigate } from "react-router-dom";
import { audiobooks, catalog, coverArt, topics, ApiError } from "../api";
import type { AudiobookLength, Voice } from "../api";
import { Field, inputClass, primaryBtn } from "./Login";

const GENRE_PRESETS: { label: string; icon: string }[] = [
  { label: "Sci-fi", icon: "🛸" },
  { label: "Fantasy", icon: "🐉" },
  { label: "Mystery", icon: "🔍" },
  { label: "Thriller", icon: "🗡️" },
  { label: "Romance", icon: "💕" },
  { label: "Historical", icon: "🏰" },
  { label: "Horror", icon: "👻" },
  { label: "Adventure", icon: "🧭" },
  { label: "Comedy", icon: "🎭" },
  { label: "Drama", icon: "🎬" },
  { label: "Children", icon: "🧸" },
  { label: "Self-help", icon: "🌱" },
  { label: "Biography", icon: "📜" },
  { label: "Non-fiction", icon: "📚" },
];

function genreIcon(label: string): string {
  const match = GENRE_PRESETS.find(
    (g) => g.label.toLowerCase() === label.trim().toLowerCase(),
  );
  return match?.icon ?? "🏷️";
}

const VOICE_ICONS: Record<string, string> = {
  female: "👩",
  male: "👨",
  neutral: "🧑",
};

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

export function NewAudiobook(): JSX.Element {
  const qc = useQueryClient();
  const navigate = useNavigate();
  const [topic, setTopic] = useState("");
  const [length, setLength] = useState<AudiobookLength>("short");
  const [genre, setGenre] = useState("");
  const [language, setLanguage] = useState("en");
  const [voiceId, setVoiceId] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [cover, setCover] = useState<{ base64: string; mime: string } | null>(null);

  const voicesQuery = useQuery({
    queryKey: ["voices"],
    queryFn: () => catalog.voices(),
  });

  const generateCover = useMutation({
    mutationFn: () =>
      coverArt.preview({
        topic: topic.trim(),
        genre: genre.trim() || undefined,
      }),
    onSuccess: (r) => setCover({ base64: r.image_base64, mime: r.mime_type }),
  });

  const surprise = useMutation({
    mutationFn: () => topics.random({ seed: null }),
    onSuccess: (r) => {
      setTopic(r.topic);
      setGenre(r.genre ?? "");
      setLength(r.length);
    },
  });

  const create = useMutation({
    mutationFn: () =>
      audiobooks.create({
        topic: topic.trim(),
        length,
        genre: genre.trim() || undefined,
        language,
        voice_id: voiceId ?? undefined,
        cover_image_base64: cover?.base64,
      }),
    onSuccess: (book) => {
      qc.invalidateQueries({ queryKey: ["audiobooks"] });
      navigate(`/app/book/${book.id}`);
    },
    onError: (err) => {
      setError(err instanceof ApiError ? err.message : "Could not create audiobook");
    },
  });

  // Whenever topic or genre changes, the previewed cover no longer matches —
  // drop it so the user re-generates intentionally rather than shipping a
  // stale image.
  useEffect(() => {
    setCover(null);
  }, [topic, genre]);

  function submit(e: FormEvent<HTMLFormElement>): void {
    e.preventDefault();
    setError(null);
    create.mutate();
  }

  return (
    <section className="max-w-xl">
      <h1 className="text-2xl font-semibold tracking-tight">New audiobook</h1>
      <p className="mt-1 text-sm text-slate-400">
        Pick a topic, length, and optional genre. We&apos;ll draft the outline
        immediately; narration happens on the next screen.
      </p>

      <form onSubmit={submit} className="mt-6 space-y-4">
        <Field label="Topic">
          <div className="flex gap-2">
            <input
              type="text"
              required
              minLength={3}
              maxLength={500}
              value={topic}
              onChange={(e) => setTopic(e.target.value)}
              placeholder="e.g. A short history of tea"
              className={inputClass}
            />
            <button
              type="button"
              onClick={() => surprise.mutate()}
              disabled={surprise.isPending}
              className="whitespace-nowrap rounded-md border border-slate-700 bg-slate-900 px-3 py-2 text-xs text-slate-300 hover:border-slate-600 hover:text-slate-100"
              title="Generate a random topic via the LLM"
            >
              {surprise.isPending ? "…" : "Surprise me"}
            </button>
          </div>
        </Field>

        <Field label="Length">
          <div className="grid grid-cols-3 gap-2">
            {(["short", "medium", "long"] as AudiobookLength[]).map((l) => (
              <button
                key={l}
                type="button"
                onClick={() => setLength(l)}
                className={`rounded-md border px-3 py-2 text-sm capitalize ${
                  length === l
                    ? "border-sky-600 bg-sky-600/10 text-sky-200"
                    : "border-slate-700 bg-slate-950 text-slate-300 hover:border-slate-600"
                }`}
              >
                {l}
              </button>
            ))}
          </div>
        </Field>

        <Field label="Language">
          <select
            value={language}
            onChange={(e) => setLanguage(e.target.value)}
            className={inputClass}
          >
            {LANGUAGES.map((l) => (
              <option key={l.code} value={l.code}>
                {l.flag}  {l.label}
              </option>
            ))}
          </select>
        </Field>

        <Field label="Genre (optional)">
          <GenreCombo value={genre} onChange={setGenre} />
        </Field>

        <Field label="Voice (optional)">
          <VoicePicker
            voices={voicesQuery.data?.items ?? []}
            isLoading={voicesQuery.isLoading}
            selected={voiceId}
            onSelect={setVoiceId}
          />
        </Field>

        <Field label="Cover art (optional)">
          <CoverArtPicker
            cover={cover}
            canGenerate={topic.trim().length >= 3}
            generating={generateCover.isPending}
            onGenerate={() => generateCover.mutate()}
            onClear={() => setCover(null)}
            error={
              generateCover.error
                ? generateCover.error instanceof ApiError
                  ? generateCover.error.message
                  : "Cover generation failed"
                : null
            }
          />
        </Field>

        {error && <p className="text-sm text-rose-400">{error}</p>}
        <button type="submit" disabled={create.isPending} className={primaryBtn}>
          {create.isPending ? "Drafting outline…" : "Create outline"}
        </button>
      </form>
    </section>
  );
}

function GenreCombo({
  value,
  onChange,
}: {
  value: string;
  onChange: (v: string) => void;
}): JSX.Element {
  const [open, setOpen] = useState(false);
  const wrapRef = useRef<HTMLDivElement>(null);

  // Close on click-outside.
  useEffect(() => {
    if (!open) return;
    const onDoc = (e: MouseEvent): void => {
      if (wrapRef.current && !wrapRef.current.contains(e.target as Node)) {
        setOpen(false);
      }
    };
    document.addEventListener("mousedown", onDoc);
    return () => document.removeEventListener("mousedown", onDoc);
  }, [open]);

  const trimmed = value.trim();
  const filtered = trimmed
    ? GENRE_PRESETS.filter((g) =>
        g.label.toLowerCase().includes(trimmed.toLowerCase()),
      )
    : GENRE_PRESETS;

  return (
    <div ref={wrapRef} className="relative">
      <div className="flex items-stretch gap-2">
        <div className="relative flex flex-1 items-center">
          <span className="pointer-events-none absolute left-3 text-base">
            {genreIcon(value)}
          </span>
          <input
            type="text"
            maxLength={40}
            value={value}
            onChange={(e) => {
              onChange(e.target.value);
              if (!open) setOpen(true);
            }}
            onFocus={() => setOpen(true)}
            placeholder="Pick a genre or type your own"
            className={`${inputClass} pl-10`}
          />
        </div>
        <button
          type="button"
          onClick={() => setOpen((v) => !v)}
          aria-label="Toggle genre list"
          className="rounded-md border border-slate-700 bg-slate-900 px-3 text-slate-300 hover:border-slate-600"
        >
          <svg width="14" height="14" viewBox="0 0 16 16" fill="currentColor">
            <path d="M3 6l5 5 5-5H3z" />
          </svg>
        </button>
      </div>
      {open && filtered.length > 0 && (
        <ul className="absolute z-20 mt-1 max-h-72 w-full overflow-auto rounded-md border border-slate-800 bg-slate-950 py-1 text-sm shadow-lg">
          {filtered.map((g) => {
            const selected = g.label.toLowerCase() === trimmed.toLowerCase();
            return (
              <li key={g.label}>
                <button
                  type="button"
                  onClick={() => {
                    onChange(g.label);
                    setOpen(false);
                  }}
                  className={`flex w-full items-center gap-3 px-3 py-1.5 text-left ${
                    selected
                      ? "bg-sky-600/15 text-sky-200"
                      : "text-slate-200 hover:bg-slate-800"
                  }`}
                >
                  <span className="w-5 text-center text-base">{g.icon}</span>
                  {g.label}
                </button>
              </li>
            );
          })}
        </ul>
      )}
    </div>
  );
}

function VoicePicker({
  voices,
  isLoading,
  selected,
  onSelect,
}: {
  voices: Voice[];
  isLoading: boolean;
  selected: string | null;
  onSelect: (id: string | null) => void;
}): JSX.Element {
  if (isLoading) {
    return <p className="text-xs text-slate-500">Loading voices…</p>;
  }
  if (voices.length === 0) {
    return (
      <p className="text-xs text-slate-500">
        No voices available — narration will use the server default.
      </p>
    );
  }
  return (
    <div className="grid grid-cols-2 gap-2 sm:grid-cols-3">
      <VoiceCard
        active={selected === null}
        onSelect={() => onSelect(null)}
        icon="✨"
        title="Default"
        subtitle="Server pick"
      />
      {voices.map((v) => (
        <VoiceCard
          key={v.id}
          active={selected === v.id}
          onSelect={() => onSelect(v.id)}
          icon={VOICE_ICONS[v.gender] ?? "🎙️"}
          title={v.name}
          subtitle={v.accent}
        />
      ))}
    </div>
  );
}

function VoiceCard({
  active,
  onSelect,
  icon,
  title,
  subtitle,
}: {
  active: boolean;
  onSelect: () => void;
  icon: string;
  title: string;
  subtitle: string;
}): JSX.Element {
  return (
    <button
      type="button"
      onClick={onSelect}
      className={`flex flex-col items-start gap-1 rounded-md border px-3 py-2 text-left ${
        active
          ? "border-sky-600 bg-sky-600/10"
          : "border-slate-700 bg-slate-950 hover:border-slate-600"
      }`}
    >
      <span className="text-lg leading-none">{icon}</span>
      <span
        className={`text-sm font-medium ${
          active ? "text-sky-200" : "text-slate-100"
        }`}
      >
        {title}
      </span>
      <span className="text-[11px] capitalize text-slate-400">{subtitle}</span>
    </button>
  );
}

function CoverArtPicker({
  cover,
  canGenerate,
  generating,
  onGenerate,
  onClear,
  error,
}: {
  cover: { base64: string; mime: string } | null;
  canGenerate: boolean;
  generating: boolean;
  onGenerate: () => void;
  onClear: () => void;
  error: string | null;
}): JSX.Element {
  return (
    <div className="flex flex-col gap-2 sm:flex-row sm:items-start">
      <div className="flex h-32 w-32 shrink-0 items-center justify-center overflow-hidden rounded-md border border-slate-700 bg-slate-950">
        {cover ? (
          <img
            src={`data:${cover.mime};base64,${cover.base64}`}
            alt="Generated cover preview"
            className="h-full w-full object-cover"
          />
        ) : (
          <span className="text-[11px] text-slate-500">No cover</span>
        )}
      </div>
      <div className="flex-1 space-y-2">
        <div className="flex flex-wrap gap-2">
          <button
            type="button"
            onClick={onGenerate}
            disabled={!canGenerate || generating}
            className="rounded-md bg-violet-600 px-3 py-2 text-sm font-medium text-white hover:bg-violet-500 disabled:cursor-not-allowed disabled:bg-violet-700/50"
            title={
              canGenerate
                ? "Generate cover art from the topic + genre"
                : "Type a topic first"
            }
          >
            {generating ? "Generating…" : cover ? "Regenerate" : "Generate cover"}
          </button>
          {cover && (
            <button
              type="button"
              onClick={onClear}
              className="rounded-md border border-slate-700 bg-slate-900 px-3 py-2 text-sm text-slate-300 hover:border-slate-600"
            >
              Remove
            </button>
          )}
        </div>
        <p className="text-xs text-slate-500">
          Uses the OpenRouter model marked <em>cover_art</em> in admin settings.
          Editing topic or genre will clear the preview.
        </p>
        {error && <p className="text-xs text-rose-400">{error}</p>}
      </div>
    </div>
  );
}
