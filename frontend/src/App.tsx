import { useEffect, useState } from "react";

type Health =
  | { state: "loading" }
  | { state: "ok"; payload: { status: string; service: string; version: string } }
  | { state: "error"; message: string };

export function App() {
  const [health, setHealth] = useState<Health>({ state: "loading" });

  useEffect(() => {
    let cancelled = false;
    fetch("/api/health")
      .then(async (res) => {
        if (!res.ok) throw new Error(`HTTP ${res.status}`);
        return res.json();
      })
      .then((payload) => {
        if (!cancelled) setHealth({ state: "ok", payload });
      })
      .catch((err: unknown) => {
        if (!cancelled) {
          setHealth({
            state: "error",
            message: err instanceof Error ? err.message : String(err),
          });
        }
      });
    return () => {
      cancelled = true;
    };
  }, []);

  return (
    <main className="flex min-h-screen items-center justify-center p-8">
      <div className="max-w-md space-y-6 text-center">
        <h1 className="text-4xl font-semibold tracking-tight">ListenAI</h1>
        <p className="text-slate-400">AI-powered audiobook generator.</p>
        <StatusCard health={health} />
      </div>
    </main>
  );
}

function StatusCard({ health }: { health: Health }) {
  const base = "rounded-lg border px-4 py-3 text-sm";
  if (health.state === "loading") {
    return (
      <div className={`${base} border-slate-700 bg-slate-900 text-slate-400`}>
        Checking backend…
      </div>
    );
  }
  if (health.state === "error") {
    return (
      <div className={`${base} border-rose-700 bg-rose-950 text-rose-200`}>
        Backend unreachable: {health.message}
      </div>
    );
  }
  return (
    <div className={`${base} border-emerald-700 bg-emerald-950 text-emerald-100`}>
      <div className="font-medium">Backend OK</div>
      <div className="text-xs text-emerald-300/80">
        {health.payload.service} v{health.payload.version}
      </div>
    </div>
  );
}
