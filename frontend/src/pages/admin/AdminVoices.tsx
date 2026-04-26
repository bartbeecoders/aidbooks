import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { admin, ApiError } from "../../api";
import type { AdminVoiceRow } from "../../api";
import { ErrorPane, Loading, PageHeader, Toggle } from "./AdminLlms";

export function AdminVoices(): JSX.Element {
  const qc = useQueryClient();
  const { data, isLoading, error } = useQuery({
    queryKey: ["admin", "voice"],
    queryFn: () => admin.voices.list(),
  });

  const toggleEnabled = useMutation({
    mutationFn: (row: AdminVoiceRow) =>
      admin.voices.patch(row.id, { enabled: !row.enabled }),
    onSuccess: () => qc.invalidateQueries({ queryKey: ["admin", "voice"] }),
  });
  const togglePremium = useMutation({
    mutationFn: (row: AdminVoiceRow) =>
      admin.voices.patch(row.id, { premium_only: !row.premium_only }),
    onSuccess: () => qc.invalidateQueries({ queryKey: ["admin", "voice"] }),
  });

  if (isLoading) return <Loading />;
  if (error) return <ErrorPane error={error} />;
  if (!data) return <p>No data.</p>;

  return (
    <div>
      <PageHeader
        title="Voices"
        description="x.ai narrator catalogue. Disable a voice to hide it from the create wizard; premium-only flags it for paid tiers."
      />
      <table className="w-full text-sm">
        <thead className="text-left text-xs uppercase tracking-wide text-slate-500">
          <tr>
            <th className="py-2 pr-4">Name</th>
            <th className="py-2 pr-4">Provider id</th>
            <th className="py-2 pr-4">Profile</th>
            <th className="py-2 pr-4">Premium</th>
            <th className="py-2 pr-4 text-right">Status</th>
          </tr>
        </thead>
        <tbody>
          {data.items.map((row) => (
            <tr key={row.id} className="border-t border-slate-800">
              <td className="py-3 pr-4 font-medium text-slate-100">{row.name}</td>
              <td className="py-3 pr-4 font-mono text-xs text-slate-400">
                {row.provider}:{row.provider_voice_id}
              </td>
              <td className="py-3 pr-4 text-xs text-slate-400">
                {row.gender} · {row.accent || "—"} · {row.language}
              </td>
              <td className="py-3 pr-4">
                <Toggle
                  enabled={row.premium_only}
                  onClick={() => togglePremium.mutate(row)}
                  pending={
                    togglePremium.isPending && togglePremium.variables?.id === row.id
                  }
                />
              </td>
              <td className="py-3 pr-4 text-right">
                <Toggle
                  enabled={row.enabled}
                  onClick={() => toggleEnabled.mutate(row)}
                  pending={
                    toggleEnabled.isPending && toggleEnabled.variables?.id === row.id
                  }
                />
              </td>
            </tr>
          ))}
        </tbody>
      </table>
      {(toggleEnabled.error || togglePremium.error) && (
        <p className="mt-3 text-sm text-rose-400">
          {(toggleEnabled.error || togglePremium.error) instanceof ApiError
            ? (toggleEnabled.error || togglePremium.error)!.message
            : "Toggle failed"}
        </p>
      )}
    </div>
  );
}
