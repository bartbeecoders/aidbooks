import { useEffect, useRef, useState } from "react";

type Variant = "icon" | "labelled";

/**
 * Copies a string to the system clipboard with a brief "Copied!"
 * confirmation flash. Used in spots like the activity log and the
 * chapter-prose viewer where a user wants to paste the text into a
 * bug report or a translation tool.
 *
 * Falls back to the legacy `document.execCommand("copy")` path when
 * `navigator.clipboard` is unavailable (rare today — only insecure
 * non-localhost origins). The fallback uses a transient hidden
 * textarea: cheap, reliable, and avoids leaking a styled mount.
 */
export function CopyButton({
  text,
  title,
  variant = "icon",
  className,
}: {
  /** Text to copy. Pulled lazily on click so the latest value is
   * captured even if the source string changes after mount. */
  text: string | (() => string);
  /** Tooltip override. Defaults to "Copy to clipboard". */
  title?: string;
  /** `icon` is a 24px square button (good inline). `labelled` adds
   * the word "Copy" alongside the icon for spots where the action
   * needs to be more obvious (top of the prose viewer). */
  variant?: Variant;
  className?: string;
}): JSX.Element {
  const [state, setState] = useState<"idle" | "copied" | "failed">("idle");
  const timer = useRef<number | null>(null);

  // Always clear the pending timer on unmount so a copy near close
  // doesn't fire setState on an unmounted component.
  useEffect(() => {
    return () => {
      if (timer.current) window.clearTimeout(timer.current);
    };
  }, []);

  async function copy(): Promise<void> {
    const value = typeof text === "function" ? text() : text;
    try {
      if (navigator.clipboard && window.isSecureContext) {
        await navigator.clipboard.writeText(value);
      } else {
        legacyCopy(value);
      }
      setState("copied");
    } catch {
      setState("failed");
    }
    if (timer.current) window.clearTimeout(timer.current);
    timer.current = window.setTimeout(() => setState("idle"), 1500);
  }

  const tooltip =
    state === "copied"
      ? "Copied!"
      : state === "failed"
        ? "Copy failed"
        : title ?? "Copy to clipboard";

  const baseStyles =
    "inline-flex items-center gap-1 rounded-md border text-xs transition-colors disabled:cursor-not-allowed disabled:opacity-50";
  const toneStyles =
    state === "copied"
      ? "border-emerald-700 bg-emerald-950/40 text-emerald-200"
      : state === "failed"
        ? "border-rose-700 bg-rose-950/40 text-rose-200"
        : "border-slate-700 bg-slate-900 text-slate-300 hover:border-slate-600 hover:text-slate-100";
  const padding = variant === "icon" ? "px-1.5 py-0.5" : "px-2 py-1";

  return (
    <button
      type="button"
      onClick={(e) => {
        // Stop the click bubbling: the button is often nested in a
        // <summary> or a card whose own click handler would expand
        // / collapse / select; copying shouldn't have that side
        // effect.
        e.preventDefault();
        e.stopPropagation();
        void copy();
      }}
      title={tooltip}
      aria-label={tooltip}
      className={`${baseStyles} ${toneStyles} ${padding} ${className ?? ""}`}
    >
      <span aria-hidden="true">{state === "copied" ? "✓" : "📋"}</span>
      {variant === "labelled" && (
        <span>{state === "copied" ? "Copied" : "Copy"}</span>
      )}
    </button>
  );
}

function legacyCopy(value: string): void {
  const ta = document.createElement("textarea");
  ta.value = value;
  // off-screen but still selectable
  ta.style.position = "fixed";
  ta.style.opacity = "0";
  ta.style.pointerEvents = "none";
  document.body.appendChild(ta);
  ta.select();
  try {
    document.execCommand("copy");
  } finally {
    document.body.removeChild(ta);
  }
}
