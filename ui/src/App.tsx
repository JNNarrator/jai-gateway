import { useCallback, useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";

interface GwStatus {
  running: boolean;
  port: number;
  restarts: number;
}

function App() {
  const [status, setStatus] = useState<GwStatus | null>(null);
  const [busy, setBusy] = useState(false);

  const refresh = useCallback(async () => {
    try {
      const s = await invoke<GwStatus>("gateway_status");
      setStatus(s);
    } catch (e) {
      console.error("gateway_status failed:", e);
    }
  }, []);

  useEffect(() => {
    refresh();
    const t = setInterval(refresh, 2000);
    return () => clearInterval(t);
  }, [refresh]);

  const toggle = async (cmd: "gateway_start" | "gateway_stop") => {
    setBusy(true);
    try {
      const s = await invoke<GwStatus>(cmd);
      setStatus(s);
    } finally {
      setBusy(false);
    }
  };

  return (
    <div className="min-h-screen bg-neutral-950 text-neutral-100 p-8">
      <header className="mb-8">
        <h1 className="text-2xl font-semibold tracking-tight">
          JAI <span className="text-neutral-500">· 桌面 AI API 网关</span>
        </h1>
        <p className="mt-1 text-sm text-neutral-500">
          M0 骨架 · 网关监督面已就绪，业务代理自 M1 起挂载
        </p>
      </header>

      <section className="max-w-md rounded-xl border border-neutral-800 bg-neutral-900/60 p-6">
        <h2 className="mb-4 text-sm font-medium text-neutral-400">网关状态</h2>

        {status ? (
          <>
            <div className="flex items-center gap-2 mb-3">
              <span
                className={
                  "inline-block h-2.5 w-2.5 rounded-full " +
                  (status.running ? "bg-emerald-400" : "bg-neutral-600")
                }
              />
              <span className="text-lg font-medium">
                {status.running ? "运行中" : "已停止"}
              </span>
            </div>

            <dl className="grid grid-cols-[auto_1fr] gap-x-4 gap-y-1 text-sm">
              <dt className="text-neutral-500">监听地址</dt>
              <dd className="font-mono">
                http://127.0.0.1:{status.port}
              </dd>
              <dt className="text-neutral-500">看门狗重启</dt>
              <dd className="font-mono">{status.restarts} 次</dd>
            </dl>
          </>
        ) : (
          <p className="text-neutral-500">连接中…</p>
        )}

        <div className="mt-6 flex gap-3">
          <button
            disabled={busy || !!status?.running}
            onClick={() => toggle("gateway_start")}
            className="rounded-lg bg-emerald-600 px-4 py-1.5 text-sm font-medium hover:bg-emerald-500 disabled:opacity-40"
          >
            启动网关
          </button>
          <button
            disabled={busy || !status?.running}
            onClick={() => toggle("gateway_stop")}
            className="rounded-lg bg-neutral-700 px-4 py-1.5 text-sm font-medium hover:bg-neutral-600 disabled:opacity-40"
          >
            停止网关
          </button>
        </div>

        <p className="mt-5 border-t border-neutral-800 pt-3 font-mono text-xs text-neutral-600">
          GET /healthz → {"{"}"ok": true, "version": …{"}"}
        </p>
      </section>
    </div>
  );
}

export default App;
