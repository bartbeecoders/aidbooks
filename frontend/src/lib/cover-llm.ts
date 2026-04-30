// Filter the public LLM catalog down to those usable for cover/chapter art.
//
// "Image-capable" means: function explicitly says image (or multimodal),
// OR the LLM is currently default-for the cover_art role (legacy seeds may
// not have a function set). Sorted by priority ASC, name ASC to match the
// backend picker's tiebreaker order.

import type { Llm } from "../api";

export function imageCapableLlms(all: Llm[]): Llm[] {
  return all
    .filter((l) => {
      if (!l.enabled) return false;
      const fn = (l.function ?? "").toLowerCase();
      if (fn === "image" || fn === "multimodal") return true;
      return l.default_for?.includes("cover_art") ?? false;
    })
    .sort((a, b) => {
      const ap = a.priority ?? 100;
      const bp = b.priority ?? 100;
      if (ap !== bp) return ap - bp;
      return a.name.localeCompare(b.name);
    });
}
