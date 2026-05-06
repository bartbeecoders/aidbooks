import type { Voice } from "../api/types";

// Voices carry a BCP-47-ish `language` from x.ai (e.g. `nl-NL`, `en-GB`,
// `es`) or the literal `multilingual`. The audiobook language is a plain
// 2-letter code (`nl`, `en`). Match by exact code, region prefix, or the
// `multilingual` catch-all so the picker only surfaces speakable voices.
export function voicesForLanguage(voices: Voice[], lang: string | null): Voice[] {
  if (!lang) return voices;
  return voices.filter(
    (v) =>
      v.language === "multilingual" ||
      v.language === lang ||
      v.language.startsWith(`${lang}-`),
  );
}
