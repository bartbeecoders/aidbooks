import type { VoicePreviewState } from "../lib/useVoicePreview";

export function VoicePreviewButton({
  state,
  onToggle,
}: {
  state: VoicePreviewState;
  onToggle: () => void;
}): JSX.Element {
  return (
    <button
      type="button"
      onClick={(e) => {
        e.stopPropagation();
        onToggle();
      }}
      disabled={state === "loading"}
      aria-label={state === "playing" ? "Stop preview" : "Play preview"}
      title={state === "playing" ? "Stop preview" : "Play preview"}
      className={`absolute right-1.5 top-1.5 z-10 inline-flex h-7 w-7 items-center justify-center rounded-full border shadow-sm ${
        state === "playing"
          ? "border-sky-400 bg-sky-600 text-white"
          : "border-slate-600 bg-slate-800 text-slate-100 hover:border-sky-400 hover:bg-slate-700 hover:text-sky-200"
      } disabled:cursor-wait disabled:opacity-60`}
    >
      {state === "loading" ? (
        <svg
          className="h-3.5 w-3.5 animate-spin"
          viewBox="0 0 24 24"
          fill="none"
          aria-hidden="true"
        >
          <circle
            cx="12"
            cy="12"
            r="9"
            stroke="currentColor"
            strokeWidth="3"
            strokeOpacity="0.3"
          />
          <path
            d="M21 12a9 9 0 0 0-9-9"
            stroke="currentColor"
            strokeWidth="3"
            strokeLinecap="round"
          />
        </svg>
      ) : state === "playing" ? (
        <svg
          className="h-3 w-3"
          viewBox="0 0 24 24"
          fill="currentColor"
          aria-hidden="true"
        >
          <rect x="6" y="5" width="4" height="14" rx="1" />
          <rect x="14" y="5" width="4" height="14" rx="1" />
        </svg>
      ) : (
        <svg
          className="h-3.5 w-3.5"
          viewBox="0 0 24 24"
          fill="currentColor"
          aria-hidden="true"
        >
          <path d="M8 5v14l11-7z" />
        </svg>
      )}
    </button>
  );
}
