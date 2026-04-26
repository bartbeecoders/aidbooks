import { useEffect, useRef, useState } from "react";
import { useMutation, useQueryClient } from "@tanstack/react-query";
import { audiobooks, ApiError } from "../api";
import type { AudiobookSummary } from "../api";

export function RenameAudiobookDialog({
  book,
  onClose,
}: {
  book: AudiobookSummary;
  onClose: () => void;
}): JSX.Element {
  const qc = useQueryClient();
  const [title, setTitle] = useState(book.title);
  const [genre, setGenre] = useState(book.genre ?? "");
  const inputRef = useRef<HTMLInputElement>(null);

  useEffect(() => {
    inputRef.current?.focus();
    inputRef.current?.select();
  }, []);

  useEffect(() => {
    const onKey = (e: KeyboardEvent): void => {
      if (e.key === "Escape") onClose();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [onClose]);

  const mutation = useMutation({
    mutationFn: () => {
      const trimmedTitle = title.trim();
      const trimmedGenre = genre.trim();
      const body: { title?: string; genre?: string } = {};
      if (trimmedTitle && trimmedTitle !== book.title) body.title = trimmedTitle;
      if (trimmedGenre !== (book.genre ?? "")) body.genre = trimmedGenre;
      return audiobooks.patch(book.id, body);
    },
    onSuccess: () => {
      qc.invalidateQueries({ queryKey: ["audiobooks"] });
      qc.invalidateQueries({ queryKey: ["audiobook", book.id] });
      onClose();
    },
  });

  const titleTrim = title.trim();
  const dirty =
    titleTrim !== book.title || genre.trim() !== (book.genre ?? "");
  const valid = titleTrim.length >= 1 && titleTrim.length <= 200;

  return (
    <div
      className="fixed inset-0 z-50 flex items-center justify-center bg-black/60 p-4"
      onClick={onClose}
    >
      <form
        onClick={(e) => e.stopPropagation()}
        onSubmit={(e) => {
          e.preventDefault();
          if (dirty && valid && !mutation.isPending) mutation.mutate();
        }}
        className="w-full max-w-md rounded-xl border border-slate-800 bg-slate-950 p-5 shadow-xl"
      >
        <h2 className="text-base font-semibold text-slate-100">Rename audiobook</h2>
        <p className="mt-1 text-xs text-slate-400">
          Update the title and genre. Topic is locked once generated.
        </p>

        <label className="mt-4 block text-xs font-medium text-slate-300">
          Title
          <input
            ref={inputRef}
            type="text"
            value={title}
            onChange={(e) => setTitle(e.target.value)}
            maxLength={200}
            className="mt-1 w-full rounded-md border border-slate-800 bg-slate-900 px-3 py-2 text-sm text-slate-100 outline-none focus:border-sky-600"
          />
        </label>

        <label className="mt-3 block text-xs font-medium text-slate-300">
          Genre <span className="text-slate-500">(optional)</span>
          <input
            type="text"
            value={genre}
            onChange={(e) => setGenre(e.target.value)}
            maxLength={40}
            className="mt-1 w-full rounded-md border border-slate-800 bg-slate-900 px-3 py-2 text-sm text-slate-100 outline-none focus:border-sky-600"
          />
        </label>

        {mutation.error && (
          <p className="mt-3 text-xs text-rose-400">
            {mutation.error instanceof ApiError
              ? mutation.error.message
              : "Could not save changes"}
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
            disabled={!dirty || !valid || mutation.isPending}
            className="rounded-md bg-sky-600 px-3 py-2 text-sm font-medium text-white hover:bg-sky-500 disabled:cursor-not-allowed disabled:bg-sky-700/50"
          >
            {mutation.isPending ? "Saving…" : "Save"}
          </button>
        </div>
      </form>
    </div>
  );
}
