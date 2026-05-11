import type {
  NarrationIntensity,
  NarrationStyle,
  VoicePreset,
} from "../api";

const STYLE_OPTIONS: {
  value: NarrationStyle;
  label: string;
  hint: string;
  icon: string;
}[] = [
  {
    value: "natural",
    label: "Natural",
    icon: "✨",
    hint: "Genre-driven default — no overlay applied.",
  },
  {
    value: "drama",
    label: "Drama",
    icon: "🎭",
    hint: "Heightened stakes, weighty silences, tense pauses.",
  },
  {
    value: "humor",
    label: "Humor",
    icon: "😄",
    hint: "Light wit, playful asides, comic timing.",
  },
  {
    value: "sketch",
    label: "Sketch",
    icon: "🎤",
    hint: "Snappy beats, exaggerated characters, escalating absurdity.",
  },
  {
    value: "erotic",
    label: "Erotic",
    icon: "🔥",
    hint: "Sensual register for adults — atmosphere over explicit detail.",
  },
  {
    value: "child_friendly",
    label: "Child-friendly",
    icon: "🧸",
    hint: "Ages 5–10. Simple sentences, kindness, no scary cliffhangers.",
  },
  {
    value: "educational",
    label: "Educational",
    icon: "🎓",
    hint: "Patient teacher voice. Defines jargon, gives examples, recaps.",
  },
];

const INTENSITY_OPTIONS: {
  value: NarrationIntensity;
  label: string;
  hint: string;
}[] = [
  { value: "intense", label: "Intense", hint: "Pushes urgency and stakes." },
  { value: "dramatic", label: "Dramatic", hint: "Larger-than-life delivery." },
  { value: "emotional", label: "Emotional", hint: "Lingers on feeling beats." },
  {
    value: "expressive",
    label: "Expressive",
    hint: "Wider tonal range; leans on speech tags.",
  },
];

const VOICE_PRESET_OPTIONS: {
  value: VoicePreset;
  label: string;
  hint: string;
  roles: string[];
}[] = [
  {
    value: "single_narrator",
    label: "1 narrator",
    hint: "One voice for everything (the legacy default).",
    roles: ["narrator"],
  },
  {
    value: "single_male",
    label: "1 male",
    hint: "Single male voice handles all narration.",
    roles: ["narrator"],
  },
  {
    value: "single_female",
    label: "1 female",
    hint: "Single female voice handles all narration.",
    roles: ["narrator"],
  },
  {
    value: "duo_male",
    label: "2 male",
    hint: "Male narrator + male dialogue voice.",
    roles: ["narrator", "dialogue_male"],
  },
  {
    value: "duo_female",
    label: "2 female",
    hint: "Female narrator + female dialogue voice.",
    roles: ["narrator", "dialogue_female"],
  },
  {
    value: "mixed",
    label: "Mixed cast",
    hint: "Narrator + male dialogue + female dialogue.",
    roles: ["narrator", "dialogue_male", "dialogue_female"],
  },
];

export function NarrationStylePanel({
  style,
  onStyle,
  intensity,
  onIntensity,
}: {
  style: NarrationStyle | null;
  onStyle: (next: NarrationStyle | null) => void;
  intensity: NarrationIntensity[];
  onIntensity: (next: NarrationIntensity[]) => void;
}): JSX.Element {
  const toggleIntensity = (tag: NarrationIntensity): void => {
    if (intensity.includes(tag)) {
      onIntensity(intensity.filter((t) => t !== tag));
    } else {
      onIntensity([...intensity, tag]);
    }
  };

  return (
    <details
      open={style !== null || intensity.length > 0}
      className="overflow-hidden rounded-lg border border-slate-800 bg-slate-900/40"
    >
      <summary className="flex cursor-pointer select-none items-center gap-2 px-4 py-2.5 text-sm text-slate-200 hover:bg-slate-900/70">
        <span className="font-medium">Narration style & mood</span>
        {(style && style !== "natural") || intensity.length > 0 ? (
          <span className="rounded-full border border-violet-700 bg-violet-950/40 px-2 py-0.5 text-[11px] uppercase tracking-wide text-violet-200">
            on
          </span>
        ) : (
          <span className="rounded-full border border-slate-700 bg-slate-950 px-2 py-0.5 text-[11px] uppercase tracking-wide text-slate-400">
            default
          </span>
        )}
      </summary>
      <div className="space-y-5 border-t border-slate-800 px-4 py-4">
        <div>
          <p className="mb-2 text-xs font-medium text-slate-300">Style</p>
          <p className="mb-3 text-[11px] text-slate-500">
            Reshapes plot + tone. The chapter writer rewrites the same
            topic to fit (e.g. educational vs. drama tells the same story
            very differently).
          </p>
          <div className="grid grid-cols-2 gap-2 sm:grid-cols-3 lg:grid-cols-4">
            {STYLE_OPTIONS.map((opt) => {
              const selected =
                opt.value === "natural"
                  ? style === null || style === "natural"
                  : style === opt.value;
              return (
                <button
                  key={opt.value}
                  type="button"
                  onClick={() =>
                    onStyle(opt.value === "natural" ? null : opt.value)
                  }
                  className={`rounded-md border px-3 py-2 text-left text-xs ${
                    selected
                      ? "border-violet-600 bg-violet-600/10 text-violet-200"
                      : "border-slate-700 bg-slate-950 text-slate-300 hover:border-slate-600"
                  }`}
                  title={opt.hint}
                >
                  <span className="block font-medium">
                    {opt.icon} {opt.label}
                  </span>
                  <span className="mt-0.5 block text-[10px] leading-tight text-slate-500">
                    {opt.hint}
                  </span>
                </button>
              );
            })}
          </div>
        </div>

        <div>
          <p className="mb-2 text-xs font-medium text-slate-300">
            Intensity (combine freely)
          </p>
          <p className="mb-3 text-[11px] text-slate-500">
            Each tag biases word choice <em>and</em> the speech-tag
            palette so the TTS narrator delivers it. Stack multiple for
            a louder effect.
          </p>
          <div className="flex flex-wrap gap-2">
            {INTENSITY_OPTIONS.map((opt) => {
              const active = intensity.includes(opt.value);
              return (
                <button
                  key={opt.value}
                  type="button"
                  onClick={() => toggleIntensity(opt.value)}
                  title={opt.hint}
                  className={`rounded-full border px-3 py-1 text-xs ${
                    active
                      ? "border-amber-600 bg-amber-600/10 text-amber-200"
                      : "border-slate-700 bg-slate-950 text-slate-400 hover:border-slate-600"
                  }`}
                >
                  {active ? "✓ " : ""}
                  {opt.label}
                </button>
              );
            })}
          </div>
        </div>
      </div>
    </details>
  );
}

export function VoicePresetPicker({
  value,
  onChange,
}: {
  value: VoicePreset | null;
  onChange: (next: VoicePreset | null) => void;
}): JSX.Element {
  return (
    <div className="rounded-md border border-slate-800 bg-slate-950/40 p-3">
      <p className="mb-1 text-xs font-medium text-slate-200">
        Voice cast preset
      </p>
      <p className="mb-3 text-[11px] text-slate-500">
        Picks the layout of voices the multi-voice panel below will use.
        Empty = single-voice narration with whichever voice you picked
        above.
      </p>
      <div className="grid grid-cols-2 gap-2 sm:grid-cols-3">
        <button
          type="button"
          onClick={() => onChange(null)}
          className={`rounded-md border px-2.5 py-1.5 text-left text-xs ${
            value === null
              ? "border-sky-600 bg-sky-600/10 text-sky-200"
              : "border-slate-700 bg-slate-950 text-slate-300 hover:border-slate-600"
          }`}
        >
          <span className="block font-medium">— None —</span>
          <span className="block text-[10px] text-slate-500">
            Use the single voice
          </span>
        </button>
        {VOICE_PRESET_OPTIONS.map((opt) => (
          <button
            key={opt.value}
            type="button"
            onClick={() => onChange(opt.value)}
            className={`rounded-md border px-2.5 py-1.5 text-left text-xs ${
              value === opt.value
                ? "border-sky-600 bg-sky-600/10 text-sky-200"
                : "border-slate-700 bg-slate-950 text-slate-300 hover:border-slate-600"
            }`}
            title={opt.hint}
          >
            <span className="block font-medium">{opt.label}</span>
            <span className="block text-[10px] text-slate-500">
              {opt.hint}
            </span>
          </button>
        ))}
      </div>
    </div>
  );
}

// eslint-disable-next-line react-refresh/only-export-components
export function presetRoles(preset: VoicePreset | null): string[] {
  if (!preset) return [];
  return (
    VOICE_PRESET_OPTIONS.find((o) => o.value === preset)?.roles ?? []
  );
}
