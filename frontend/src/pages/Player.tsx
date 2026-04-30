import { useEffect, useMemo, useRef, useState } from "react";
import { useQuery } from "@tanstack/react-query";
import { Link, useParams } from "react-router-dom";
import {
  audiobooks,
  chapterArtUrl,
  coverImageUrl,
  paragraphImageUrl,
} from "../api";
import type { ChapterSummary, ParagraphSummary } from "../api";
import { useAuth } from "../store/auth";

const SPEEDS = [0.75, 1, 1.25, 1.5, 2];
const LOCAL_KEY = (id: string) => `listenai.player.${id}`;

type Persisted = { chapter: number; time: number; speed: number };

const LANG_NAMES: Record<string, { label: string; flag: string }> = {
  en: { label: "English", flag: "🇬🇧" },
  nl: { label: "Dutch", flag: "🇳🇱" },
  fr: { label: "French", flag: "🇫🇷" },
  de: { label: "German", flag: "🇩🇪" },
  es: { label: "Spanish", flag: "🇪🇸" },
  it: { label: "Italian", flag: "🇮🇹" },
  pt: { label: "Portuguese", flag: "🇵🇹" },
  ru: { label: "Russian", flag: "🇷🇺" },
  zh: { label: "Chinese", flag: "🇨🇳" },
  ja: { label: "Japanese", flag: "🇯🇵" },
  ko: { label: "Korean", flag: "🇰🇷" },
};

export function Player(): JSX.Element {
  const { id } = useParams<{ id: string }>();
  const accessToken = useAuth((s) => s.accessToken) ?? "";
  const [activeLang, setActiveLang] = useState<string | null>(null);
  const { data, isLoading, error } = useQuery({
    queryKey: ["audiobook", id, activeLang ?? "primary"],
    queryFn: () => audiobooks.get(id!, activeLang ?? undefined),
    enabled: !!id,
  });

  const playable = useMemo(
    () => (data?.chapters ?? []).filter((c) => c.status === "audio_ready"),
    [data],
  );

  const stored = readPersisted(id);
  const [active, setActive] = useState<number>(
    stored?.chapter ?? playable[0]?.number ?? 1,
  );
  const [speed, setSpeed] = useState<number>(stored?.speed ?? 1);
  const [currentTime, setCurrentTime] = useState(0);
  const [chapterDuration, setChapterDuration] = useState(0);
  const audioRef = useRef<HTMLAudioElement | null>(null);

  // When the list of playable chapters fills in for the first time, pick the
  // persisted chapter if it's available — otherwise the first playable one.
  useEffect(() => {
    if (!data) return;
    const known = playable.find((c) => c.number === active);
    if (!known && playable.length > 0) {
      setActive(playable[0].number);
    }
  }, [data, playable, active]);

  // Persist on pause / time update. Keeping this coarse avoids hammering
  // localStorage 30 times a second; we rely on `timeupdate` + a 5 s budget.
  useEffect(() => {
    if (!id) return;
    const el = audioRef.current;
    if (!el) return;
    let last = 0;
    function save(): void {
      if (!id) return;
      const t = Math.floor(el!.currentTime);
      if (t === last) return;
      last = t;
      writePersisted(id, { chapter: active, time: t, speed });
    }
    const iv = window.setInterval(save, 5_000);
    el.addEventListener("pause", save);
    return () => {
      window.clearInterval(iv);
      el.removeEventListener("pause", save);
    };
  }, [id, active, speed]);

  // Restore position on load.
  useEffect(() => {
    const el = audioRef.current;
    if (!el || !id) return;
    const s = readPersisted(id);
    if (s && s.chapter === active) el.currentTime = s.time;
    el.playbackRate = speed;
  }, [id, active, speed]);

  // Track the current playhead + chapter duration so the whole-book bar can
  // update at whatever cadence the browser fires `timeupdate` (~4Hz).
  useEffect(() => {
    const el = audioRef.current;
    if (!el) return;
    const onTime = (): void => setCurrentTime(el.currentTime);
    const onMeta = (): void =>
      setChapterDuration(Number.isFinite(el.duration) ? el.duration : 0);
    el.addEventListener("timeupdate", onTime);
    el.addEventListener("loadedmetadata", onMeta);
    el.addEventListener("durationchange", onMeta);
    return () => {
      el.removeEventListener("timeupdate", onTime);
      el.removeEventListener("loadedmetadata", onMeta);
      el.removeEventListener("durationchange", onMeta);
    };
  }, [active]);

  // Reset the chapter clock when switching chapters so the bar doesn't lag a
  // frame behind on chapter change.
  useEffect(() => {
    setCurrentTime(0);
    setChapterDuration(0);
  }, [active]);

  // When a chapter finishes, auto-advance.
  useEffect(() => {
    const el = audioRef.current;
    if (!el) return;
    function onEnded(): void {
      const idx = playable.findIndex((c) => c.number === active);
      const next = idx >= 0 && playable[idx + 1];
      if (next) setActive(next.number);
    }
    el.addEventListener("ended", onEnded);
    return () => el.removeEventListener("ended", onEnded);
  }, [playable, active]);

  // Keyboard shortcuts — scoped to when the player is mounted so shortcuts
  // never swallow typing in inputs elsewhere in the app.
  useEffect(() => {
    function onKey(e: KeyboardEvent): void {
      const el = audioRef.current;
      if (!el) return;
      const target = e.target as HTMLElement | null;
      if (target && ["INPUT", "TEXTAREA"].includes(target.tagName)) return;
      if (e.code === "Space") {
        e.preventDefault();
        if (el.paused) void el.play();
        else el.pause();
      } else if (e.key === "j") {
        el.currentTime = Math.max(0, el.currentTime - 15);
      } else if (e.key === "l") {
        el.currentTime = el.currentTime + 15;
      } else if (e.key === ",") {
        setActive((n) => Math.max(1, n - 1));
      } else if (e.key === ".") {
        setActive((n) => Math.min(playable.length, n + 1));
      }
    }
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [playable.length]);

  if (!id) return <p>Missing id.</p>;
  if (isLoading) return <p className="text-sm text-slate-400">Loading…</p>;
  if (error) return <p className="text-sm text-rose-400">{(error as Error).message}</p>;
  if (!data) return <p className="text-sm text-slate-400">Not found.</p>;

  if (playable.length === 0) {
    return (
      <section>
        <h1 className="text-2xl font-semibold tracking-tight">{data.title}</h1>
        <p className="mt-2 text-sm text-slate-400">
          No chapters have been narrated yet.{" "}
          <Link to={`/app/book/${id}`} className="text-sky-400 hover:text-sky-300">
            Go narrate →
          </Link>
        </p>
      </section>
    );
  }

  const langForUrl = activeLang ?? data.language;
  const src = `/api/audiobook/${id}/chapter/${active}/audio?access_token=${encodeURIComponent(accessToken)}&language=${encodeURIComponent(langForUrl)}`;
  const activeChapter = data.chapters.find((c) => c.number === active);

  return (
    <section className="grid gap-6 md:grid-cols-[1fr,280px]">
      <div className="min-w-0">
        <h1 className="text-2xl font-semibold tracking-tight">{data.title}</h1>
        <p className="mt-1 text-sm text-slate-400">
          Chapter {active}: {activeChapter?.title ?? ""}
        </p>

        {data.available_languages.length > 1 && (
          <div className="mt-3 flex flex-wrap items-center gap-1.5 text-xs">
            <span className="text-slate-500">Language:</span>
            {[
              data.language,
              ...data.available_languages.filter((l) => l !== data.language),
            ].map((code) => {
              const info = LANG_NAMES[code] ?? { label: code, flag: "🏳️" };
              const isActive = (activeLang ?? data.language) === code;
              return (
                <button
                  key={code}
                  type="button"
                  onClick={() =>
                    setActiveLang(code === data.language ? null : code)
                  }
                  className={`rounded-md px-2 py-0.5 ${
                    isActive
                      ? "bg-sky-600/15 text-sky-200"
                      : "text-slate-400 hover:bg-slate-900 hover:text-slate-200"
                  }`}
                >
                  {info.flag} {info.label}
                </button>
              );
            })}
          </div>
        )}

        <ChapterSlideshow
          audiobookId={id}
          chapter={active}
          accessToken={accessToken}
          language={langForUrl}
          paragraphs={activeChapter?.paragraphs ?? []}
          hasArt={activeChapter?.has_art ?? false}
          hasCover={data.has_cover}
          currentTime={currentTime}
          chapterDuration={chapterDuration}
        />

        <audio
          ref={audioRef}
          key={src}
          src={src}
          autoPlay
          controls
          className="mt-4 w-full"
        />

        <BookProgress
          playable={playable}
          active={active}
          currentTime={currentTime}
          fallbackChapterDuration={chapterDuration}
          onSeek={(chapterNumber, seconds) => {
            if (chapterNumber !== active) {
              setActive(chapterNumber);
              writePersisted(id, { chapter: chapterNumber, time: seconds, speed });
              return;
            }
            const el = audioRef.current;
            if (el) el.currentTime = seconds;
          }}
        />

        <div className="mt-3 flex flex-wrap items-center gap-2 text-xs text-slate-300">
          <span className="text-slate-500">Speed</span>
          {SPEEDS.map((s) => (
            <button
              key={s}
              onClick={() => {
                setSpeed(s);
                if (audioRef.current) audioRef.current.playbackRate = s;
              }}
              className={`rounded-md border px-2 py-0.5 ${
                speed === s
                  ? "border-sky-600 bg-sky-600/10 text-sky-200"
                  : "border-slate-700 bg-slate-900 hover:border-slate-600"
              }`}
            >
              {s}×
            </button>
          ))}
          <span className="ml-4 text-slate-500">
            Shortcuts: space · j / l (±15 s) · , / . (chapters)
          </span>
        </div>

        <ChapterText chapter={activeChapter} />
      </div>

      <aside className="rounded-xl border border-slate-800 bg-slate-900/40 p-3">
        <h2 className="px-1 pb-2 text-xs font-semibold uppercase tracking-wide text-slate-500">
          Chapters
        </h2>
        <ol className="space-y-1 text-sm">
          {data.chapters.map((c) => (
            <li key={c.id}>
              <ChapterLink
                ch={c}
                active={c.number === active}
                disabled={c.status !== "audio_ready"}
                onSelect={() => setActive(c.number)}
              />
            </li>
          ))}
        </ol>
        <Link
          to={`/app/book/${id}`}
          className="mt-3 block rounded-md border border-slate-800 bg-slate-950 px-3 py-1.5 text-center text-xs text-slate-400 hover:border-slate-700 hover:text-slate-200"
        >
          Back to book
        </Link>
      </aside>
    </section>
  );
}

/**
 * Slideshow shown above the audio element. Walks through the chapter's
 * illustrations in playback order:
 *   - the chapter cover tile (if any) holds the screen until the first
 *     visual paragraph's time-slot,
 *   - each visual paragraph then takes a slot proportional to its
 *     character count over the chapter total — the assumption being
 *     that TTS narrates roughly at a constant char/sec rate. Within a
 *     paragraph that has multiple tiles, the slot is divided evenly.
 *   - falls back to the audiobook cover when the chapter has no art at
 *     all, so the player isn't blank.
 */
function ChapterSlideshow({
  audiobookId,
  chapter,
  accessToken,
  language,
  paragraphs,
  hasArt,
  hasCover,
  currentTime,
  chapterDuration,
}: {
  audiobookId: string;
  chapter: number;
  accessToken: string;
  language: string;
  paragraphs: ParagraphSummary[];
  hasArt: boolean;
  hasCover: boolean;
  currentTime: number;
  chapterDuration: number;
}): JSX.Element | null {
  // Build (slide URL, time window) pairs. Slides without a known
  // duration get equal share so the early-load case still renders.
  const timeline = useMemo<{ url: string; start: number; end: number }[]>(() => {
    const visual = paragraphs.filter((p) => p.is_visual && p.image_count > 0);
    const totalChars = visual.reduce((sum, p) => sum + Math.max(1, p.char_count), 0);
    const dur = chapterDuration > 0 ? chapterDuration : 0;

    const slides: { url: string; weight: number }[] = [];
    if (hasArt) {
      // Cover tile gets a small lead-in slot proportional to one
      // average paragraph; falls back to a 10% slice when paragraphs
      // are missing entirely.
      const lead =
        visual.length > 0
          ? Math.max(1, Math.round(totalChars / Math.max(1, visual.length)))
          : Math.max(1, Math.round(totalChars * 0.1));
      slides.push({
        url: chapterArtUrl(audiobookId, chapter, accessToken, language),
        weight: lead,
      });
    }
    for (const p of visual) {
      // Divide the paragraph's char-count budget evenly across its tiles.
      const per = Math.max(1, Math.floor(Math.max(1, p.char_count) / p.image_count));
      for (let ord = 1; ord <= p.image_count; ord++) {
        slides.push({
          url: paragraphImageUrl(
            audiobookId,
            chapter,
            p.index,
            ord,
            accessToken,
            language,
          ),
          weight: per,
        });
      }
    }
    if (slides.length === 0 && hasCover) {
      slides.push({
        url: coverImageUrl(audiobookId, accessToken),
        weight: 1,
      });
    }
    if (slides.length === 0) return [];

    // Convert weights into time windows. With a known duration, scale
    // weights to seconds; without, fall back to equal slots so the
    // first slide always renders.
    const totalWeight = slides.reduce((s, x) => s + x.weight, 0);
    const out: { url: string; start: number; end: number }[] = [];
    if (dur > 0 && totalWeight > 0) {
      let acc = 0;
      for (const s of slides) {
        const span = (s.weight / totalWeight) * dur;
        out.push({ url: s.url, start: acc, end: acc + span });
        acc += span;
      }
      // Fix any rounding drift on the last slide.
      if (out.length > 0) out[out.length - 1].end = dur;
    } else {
      for (let i = 0; i < slides.length; i++) {
        out.push({ url: slides[i].url, start: i, end: i + 1 });
      }
    }
    return out;
  }, [
    audiobookId,
    chapter,
    accessToken,
    language,
    paragraphs,
    hasArt,
    hasCover,
    chapterDuration,
  ]);

  if (timeline.length === 0) return null;

  // Pick the slide whose time window contains the playhead. With a
  // known duration the windows are seconds; without, they're indices
  // and currentTime won't advance through them — so we stick to slot 0.
  const idx =
    chapterDuration > 0
      ? Math.max(
          0,
          timeline.findIndex((s) => currentTime < s.end),
        )
      : 0;
  const safeIdx = idx === -1 ? timeline.length - 1 : idx;

  return (
    <div className="relative mt-4 aspect-square max-w-md overflow-hidden rounded-xl border border-slate-800 bg-slate-900/40">
      {timeline.map((s, i) => (
        <img
          key={s.url}
          src={s.url}
          alt=""
          className={`absolute inset-0 h-full w-full object-cover transition-opacity duration-700 ${
            i === safeIdx ? "opacity-100" : "opacity-0"
          }`}
        />
      ))}
      {timeline.length > 1 && (
        <div className="absolute bottom-2 left-1/2 flex -translate-x-1/2 gap-1.5">
          {timeline.map((_, i) => (
            <span
              key={i}
              className={`h-1.5 w-1.5 rounded-full ${
                i === safeIdx ? "bg-white/90" : "bg-white/30"
              }`}
            />
          ))}
        </div>
      )}
    </div>
  );
}

function BookProgress({
  playable,
  active,
  currentTime,
  fallbackChapterDuration,
  onSeek,
}: {
  playable: ChapterSummary[];
  active: number;
  currentTime: number;
  fallbackChapterDuration: number;
  onSeek: (chapterNumber: number, seconds: number) => void;
}): JSX.Element {
  // Server-reported duration_ms is the source of truth; for the active chapter,
  // fall back to whatever the <audio> element exposes if the DB value is
  // missing (e.g. older books narrated before duration_ms was tracked).
  const durationsSec = useMemo(
    () =>
      playable.map((c) => {
        const fromDb = (c.duration_ms ?? 0) / 1000;
        if (c.number === active && fromDb === 0 && fallbackChapterDuration > 0) {
          return fallbackChapterDuration;
        }
        return fromDb;
      }),
    [playable, active, fallbackChapterDuration],
  );

  const totalSec = durationsSec.reduce((a, b) => a + b, 0);
  const activeIdx = Math.max(
    0,
    playable.findIndex((c) => c.number === active),
  );
  const elapsedSec =
    durationsSec.slice(0, activeIdx).reduce((a, b) => a + b, 0) + currentTime;
  const pct = totalSec > 0 ? Math.min(100, (elapsedSec / totalSec) * 100) : 0;

  // Click → translate the click position into (chapter, offset within chapter).
  const trackRef = useRef<HTMLDivElement | null>(null);
  const seekFromEvent = (clientX: number): void => {
    const el = trackRef.current;
    if (!el || totalSec <= 0) return;
    const rect = el.getBoundingClientRect();
    const ratio = Math.max(0, Math.min(1, (clientX - rect.left) / rect.width));
    let target = ratio * totalSec;
    for (let i = 0; i < playable.length; i++) {
      const d = durationsSec[i];
      if (target <= d || i === playable.length - 1) {
        onSeek(playable[i].number, Math.max(0, Math.min(d, target)));
        return;
      }
      target -= d;
    }
  };

  // Tick marks at chapter boundaries — visual landmarks make the bar feel
  // navigable rather than a single opaque blob.
  const markerOffsets = useMemo(() => {
    if (totalSec <= 0) return [];
    const out: number[] = [];
    let acc = 0;
    for (let i = 0; i < durationsSec.length - 1; i++) {
      acc += durationsSec[i];
      out.push((acc / totalSec) * 100);
    }
    return out;
  }, [durationsSec, totalSec]);

  return (
    <div className="mt-4">
      <div className="mb-1 flex items-center justify-between text-xs text-slate-400">
        <span>{formatTime(elapsedSec)}</span>
        <span className="text-slate-500">
          Chapter {active} / {playable.length}
        </span>
        <span>{formatTime(totalSec)}</span>
      </div>
      <div
        ref={trackRef}
        role="slider"
        tabIndex={0}
        aria-label="Audiobook progress"
        aria-valuemin={0}
        aria-valuemax={Math.round(totalSec)}
        aria-valuenow={Math.round(elapsedSec)}
        onClick={(e) => seekFromEvent(e.clientX)}
        onKeyDown={(e) => {
          // Arrow keys nudge the playhead by 5 s of book time.
          if (e.key === "ArrowLeft" || e.key === "ArrowRight") {
            e.preventDefault();
            const delta = e.key === "ArrowLeft" ? -5 : 5;
            const next = Math.max(0, Math.min(totalSec, elapsedSec + delta));
            // Map back to (chapter, offset).
            let acc = 0;
            for (let i = 0; i < playable.length; i++) {
              if (next <= acc + durationsSec[i] || i === playable.length - 1) {
                onSeek(playable[i].number, Math.max(0, next - acc));
                return;
              }
              acc += durationsSec[i];
            }
          }
        }}
        className="relative h-2 cursor-pointer overflow-hidden rounded-full bg-slate-800 outline-none ring-sky-600/40 focus-visible:ring-2"
      >
        <div
          className="h-full bg-sky-500 transition-[width] duration-100"
          style={{ width: `${pct}%` }}
        />
        {markerOffsets.map((m, i) => (
          <div
            key={i}
            className="absolute top-0 h-full w-px bg-slate-950/70"
            style={{ left: `${m}%` }}
          />
        ))}
      </div>
    </div>
  );
}

function ChapterText({ chapter }: { chapter: ChapterSummary | undefined }): JSX.Element {
  if (!chapter) return <p className="mt-6 text-sm text-slate-500">Chapter not found.</p>;
  if (!chapter.body_md) {
    return (
      <p className="mt-6 text-sm text-slate-500">
        This chapter has no text on file.
      </p>
    );
  }
  return (
    <article className="mt-6 rounded-xl border border-slate-800 bg-slate-900/30 p-5">
      <h3 className="text-base font-semibold text-slate-100">
        Chapter {chapter.number}: {chapter.title}
      </h3>
      {chapter.synopsis && (
        <p className="mt-1 text-xs italic text-slate-400">{chapter.synopsis}</p>
      )}
      <pre className="mt-4 max-h-[60vh] overflow-auto whitespace-pre-wrap font-sans text-sm leading-relaxed text-slate-200">
        {chapter.body_md}
      </pre>
    </article>
  );
}

function ChapterLink({
  ch,
  active,
  disabled,
  onSelect,
}: {
  ch: ChapterSummary;
  active: boolean;
  disabled: boolean;
  onSelect: () => void;
}): JSX.Element {
  return (
    <button
      disabled={disabled}
      onClick={onSelect}
      className={`w-full truncate rounded-md px-2 py-1.5 text-left ${
        active
          ? "bg-sky-600/15 text-sky-200"
          : disabled
            ? "cursor-not-allowed text-slate-600"
            : "text-slate-300 hover:bg-slate-800"
      }`}
    >
      <span className="mr-2 text-xs text-slate-500">{ch.number}.</span>
      {ch.title}
    </button>
  );
}

function formatTime(seconds: number): string {
  if (!Number.isFinite(seconds) || seconds <= 0) return "0:00";
  const total = Math.floor(seconds);
  const h = Math.floor(total / 3600);
  const m = Math.floor((total % 3600) / 60);
  const s = total % 60;
  if (h > 0) return `${h}:${String(m).padStart(2, "0")}:${String(s).padStart(2, "0")}`;
  return `${m}:${String(s).padStart(2, "0")}`;
}

function readPersisted(id: string | undefined): Persisted | null {
  if (!id) return null;
  try {
    const raw = localStorage.getItem(LOCAL_KEY(id));
    if (!raw) return null;
    return JSON.parse(raw) as Persisted;
  } catch {
    return null;
  }
}

function writePersisted(id: string, p: Persisted): void {
  try {
    localStorage.setItem(LOCAL_KEY(id), JSON.stringify(p));
  } catch {
    /* quota exceeded — ignore */
  }
}
