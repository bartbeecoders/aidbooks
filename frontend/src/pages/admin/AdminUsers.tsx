import { useState } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { admin, ApiError } from "../../api";
import type { AdminUserRow, UserRole, UserTier } from "../../api";
import { ErrorPane, Loading, PageHeader } from "./AdminLlms";

export function AdminUsers(): JSX.Element {
  const qc = useQueryClient();
  const [search, setSearch] = useState("");
  const [roleFilter, setRoleFilter] = useState<UserRole | "">("");
  const [tierFilter, setTierFilter] = useState<UserTier | "">("");

  const { data, isLoading, error } = useQuery({
    queryKey: ["admin", "users", search, roleFilter, tierFilter],
    queryFn: () =>
      admin.users.list({
        q: search || undefined,
        role: roleFilter || undefined,
        tier: tierFilter || undefined,
      }),
  });

  const patch = useMutation({
    mutationFn: (v: { id: string; body: { role?: UserRole; tier?: UserTier } }) =>
      admin.users.patch(v.id, v.body),
    onSuccess: () => qc.invalidateQueries({ queryKey: ["admin", "users"] }),
  });
  const revoke = useMutation({
    mutationFn: (id: string) => admin.users.revokeSessions(id),
    onSuccess: () => qc.invalidateQueries({ queryKey: ["admin", "users"] }),
  });

  return (
    <div>
      <PageHeader
        title="Users"
        description="Change roles, flip tiers, revoke every active session at once."
      />

      <div className="mb-4 flex flex-wrap items-center gap-2 text-sm">
        <input
          value={search}
          onChange={(e) => setSearch(e.target.value)}
          placeholder="Search email…"
          className="w-64 rounded-md border border-slate-700 bg-slate-950 px-3 py-1.5 text-sm text-slate-100 placeholder-slate-500 focus:border-sky-500"
        />
        <select
          value={roleFilter}
          onChange={(e) => setRoleFilter(e.target.value as UserRole | "")}
          className="rounded-md border border-slate-700 bg-slate-950 px-3 py-1.5 text-sm text-slate-100"
        >
          <option value="">All roles</option>
          <option value="user">user</option>
          <option value="admin">admin</option>
        </select>
        <select
          value={tierFilter}
          onChange={(e) => setTierFilter(e.target.value as UserTier | "")}
          className="rounded-md border border-slate-700 bg-slate-950 px-3 py-1.5 text-sm text-slate-100"
        >
          <option value="">All tiers</option>
          <option value="free">free</option>
          <option value="pro">pro</option>
        </select>
      </div>

      {isLoading && <Loading />}
      {error && <ErrorPane error={error} />}
      {data && (
        <table className="w-full text-sm">
          <thead className="text-left text-xs uppercase tracking-wide text-slate-500">
            <tr>
              <th className="py-2 pr-4">Email</th>
              <th className="py-2 pr-4">Role</th>
              <th className="py-2 pr-4">Tier</th>
              <th className="py-2 pr-4">Sessions</th>
              <th className="py-2 pr-4 text-right">Actions</th>
            </tr>
          </thead>
          <tbody>
            {data.items.map((u) => (
              <Row
                key={u.id}
                u={u}
                onPatch={(body) => patch.mutate({ id: u.id, body })}
                onRevoke={() => revoke.mutate(u.id)}
                revoking={revoke.isPending && revoke.variables === u.id}
              />
            ))}
          </tbody>
        </table>
      )}
      {patch.error && (
        <p className="mt-3 text-sm text-rose-400">
          {patch.error instanceof ApiError ? patch.error.message : "Update failed"}
        </p>
      )}
      {revoke.error && (
        <p className="mt-3 text-sm text-rose-400">
          {revoke.error instanceof ApiError ? revoke.error.message : "Revoke failed"}
        </p>
      )}
    </div>
  );
}

function Row({
  u,
  onPatch,
  onRevoke,
  revoking,
}: {
  u: AdminUserRow;
  onPatch: (body: { role?: UserRole; tier?: UserTier }) => void;
  onRevoke: () => void;
  revoking: boolean;
}): JSX.Element {
  return (
    <tr className="border-t border-slate-800">
      <td className="py-3 pr-4">
        <div className="text-slate-100">{u.email}</div>
        <div className="text-xs text-slate-500">{u.display_name}</div>
      </td>
      <td className="py-3 pr-4">
        <select
          value={u.role}
          onChange={(e) => onPatch({ role: e.target.value as UserRole })}
          className="rounded-md border border-slate-700 bg-slate-950 px-2 py-1 text-xs text-slate-100"
        >
          <option value="user">user</option>
          <option value="admin">admin</option>
        </select>
      </td>
      <td className="py-3 pr-4">
        <select
          value={u.tier}
          onChange={(e) => onPatch({ tier: e.target.value as UserTier })}
          className="rounded-md border border-slate-700 bg-slate-950 px-2 py-1 text-xs text-slate-100"
        >
          <option value="free">free</option>
          <option value="pro">pro</option>
        </select>
      </td>
      <td className="py-3 pr-4 text-slate-300">{u.active_sessions}</td>
      <td className="py-3 pr-4 text-right">
        <button
          onClick={onRevoke}
          disabled={revoking || u.active_sessions === 0}
          className="rounded-md border border-slate-700 bg-slate-900 px-2 py-1 text-xs text-slate-200 hover:border-rose-700 hover:text-rose-300 disabled:cursor-not-allowed disabled:opacity-40"
        >
          {revoking ? "…" : "Revoke sessions"}
        </button>
      </td>
    </tr>
  );
}
