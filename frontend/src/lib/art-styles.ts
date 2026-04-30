// Curated artwork-style metadata. Values are sent verbatim to the backend
// (which accepts free text), so the labels here are also the prompt
// fragment the image model receives.

export const ART_STYLES: { value: string; label: string; icon: string }[] = [
  { value: "cinematic", label: "Cinematic", icon: "🎬" },
  { value: "realistic", label: "Realistic photo", icon: "📷" },
  { value: "watercolor", label: "Watercolor", icon: "🎨" },
  { value: "oil painting", label: "Oil painting", icon: "🖼️" },
  { value: "cartoon", label: "Cartoon", icon: "✏️" },
  { value: "anime", label: "Anime", icon: "🌸" },
  { value: "comic book", label: "Comic book", icon: "💥" },
  { value: "abstract", label: "Abstract", icon: "🌀" },
  { value: "minimalist", label: "Minimalist", icon: "◻️" },
  { value: "pixel art", label: "Pixel art", icon: "👾" },
  { value: "sketch", label: "Pencil sketch", icon: "✒️" },
  { value: "vintage poster", label: "Vintage poster", icon: "📜" },
];

export const DEFAULT_ART_STYLE = "cinematic";

export function styleLabel(value: string | null | undefined): string {
  if (!value) return "Default";
  const m = ART_STYLES.find((s) => s.value === value.toLowerCase());
  return m?.label ?? value;
}

export function styleIcon(value: string | null | undefined): string {
  if (!value) return "🖼️";
  const m = ART_STYLES.find((s) => s.value === value.toLowerCase());
  return m?.icon ?? "🖼️";
}
