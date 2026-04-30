import { ART_STYLES } from "../lib/art-styles";

/** Compact `<select>` styled to match the rest of the form inputs. */
export function ArtStyleSelect({
  value,
  onChange,
  className,
}: {
  value: string;
  onChange: (next: string) => void;
  className?: string;
}): JSX.Element {
  return (
    <select
      value={value}
      onChange={(e) => onChange(e.target.value)}
      className={
        className ??
        "w-full rounded-md border border-slate-800 bg-slate-900 px-3 py-2 text-sm text-slate-100 outline-none focus:border-sky-600"
      }
    >
      {ART_STYLES.map((s) => (
        <option key={s.value} value={s.value}>
          {s.icon} {s.label}
        </option>
      ))}
    </select>
  );
}
