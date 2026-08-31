import { useEffect, useState } from "react";
import { api } from "../api";
import type { LogRowView } from "../types";
import { toast } from "../lib/toast";
import { inputCls, btnGhost } from "../components/common/legacy";

export function LogsPage() {
  const [rows, setRows] = useState<LogRowView[]>([]);
  const [auto, setAuto] = useState(true);
  const [intervalMs, setIntervalMs] = useState(3000);
  const [modelFilter, setModelFilter] = useState("");
  const [statusFilter, setStatusFilter] = useState("");
  const [limit, setLimit] = useState(500);

  async function refresh() {
    setRows(await api.logsRecent(limit));
  }

  useEffect(() => {
    refresh();
    if (!auto) return;
    const t = setInterval(refresh, intervalMs);
    return () => clearInterval(t);
  }, [auto, intervalMs, limit]);

  const filtered = rows.filter((r) => {
    if (modelFilter && !r.modelName.toLowerCase().includes(modelFilter.toLowerCase()))
      return false;
    if (statusFilter && String(r.httpStatus) !== statusFilter) return false;
    return true;
  });

  function statusClass(s: number) {
    if (s >= 500) return "text-red-700";
    if (s >= 400) return "text-red-400";
    if (s >= 200 && s < 300) return "text-emerald-400";
    return "text-neutral-400";
  }

  function exportCsv() {
    const header = "时间,模型,状态,耗时,流式,输入,输出,错误\n";
    const body = filtered
      .map((r) =>
        [
          new Date(r.ts).toISOString(),
          `"${r.modelName.replaceAll('"', '""')}"`,
          r.httpStatus,
          r.durationMs,
          r.isStream ? "SSE" : "",
          r.usageInput ?? "",
          r.usageOutput ?? "",
          `"${(r.errorKind ?? "").replaceAll('"', '""')}"`,
        ].join(",")
      )
      .join("\n");
    const blob = new Blob([header + body], { type: "text/csv;charset=utf-8" });
    const a = document.createElement("a");
    a.href = URL.createObjectURL(blob);
    a.download = `jai-logs-${Date.now()}.csv`;
    a.click();
    URL.revokeObjectURL(a.href);
    toast("已导出 CSV");
  }

  function exportJson() {
    const blob = new Blob([JSON.stringify(filtered, null, 2)], {
      type: "application/json",
    });
    const a = document.createElement("a");
    a.href = URL.createObjectURL(blob);
    a.download = `jai-logs-${Date.now()}.json`;
    a.click();
    URL.revokeObjectURL(a.href);
    toast("已导出 JSON");
  }

  return (
    <div className="mx-auto max-w-5xl space-y-4">
      <div className="flex flex-wrap items-center justify-between gap-2">
        <h1 className="text-lg font-semibold">请求日志</h1>
        <div className="flex flex-wrap items-center gap-2">
          <label className="flex items-center gap-2 text-sm text-neutral-400">
            <input type="checkbox" checked={auto} onChange={(e) => setAuto(e.target.checked)} className="accent-amber-500" />
            自动刷新
          </label>
          <select
            className={`${inputCls} w-28`}
            value={intervalMs}
            onChange={(e) => setIntervalMs(Number(e.target.value))}
            disabled={!auto}
          >
            <option value={3000}>3s</option>
            <option value={5000}>5s</option>
            <option value={10000}>10s</option>
            <option value={30000}>30s</option>
          </select>
          <input
            className={`${inputCls} w-36`}
            placeholder="筛选模型…"
            value={modelFilter}
            onChange={(e) => setModelFilter(e.target.value)}
          />
          <input
            className={`${inputCls} w-24`}
            placeholder="状态码"
            value={statusFilter}
            onChange={(e) => setStatusFilter(e.target.value)}
          />
          <button className={btnGhost} onClick={exportCsv}>导出 CSV</button>
          <button className={btnGhost} onClick={exportJson}>导出 JSON</button>
        </div>
      </div>
      <div className="overflow-x-auto rounded-lg border border-neutral-800">
        <table className="w-full text-xs">
          <thead className="bg-neutral-900 text-left uppercase tracking-wider text-neutral-500">
            <tr>
              <th className="px-3 py-2">时间</th>
              <th className="px-3 py-2">模型</th>
              <th className="px-3 py-2">状态</th>
              <th className="px-3 py-2">耗时</th>
              <th className="px-3 py-2">流式</th>
              <th className="px-3 py-2 text-right">输入</th>
              <th className="px-3 py-2 text-right">输出</th>
              <th className="px-3 py-2">错误</th>
            </tr>
          </thead>
          <tbody className="divide-y divide-neutral-800/70 font-mono">
            {filtered.map((r) => (
              <tr key={r.id} className={r.httpStatus >= 400 ? "bg-red-950/20" : ""}>
                <td className="whitespace-nowrap px-3 py-1.5 text-neutral-500">
                  {new Date(r.ts).toLocaleTimeString()}
                </td>
                <td className="max-w-48 truncate px-3 py-1.5">{r.modelName}</td>
                <td className={`px-3 py-1.5 font-semibold ${statusClass(r.httpStatus)}`}>
                  {r.httpStatus}
                </td>
                <td className="px-3 py-1.5 text-neutral-400">{r.durationMs}ms</td>
                <td className="px-3 py-1.5 text-neutral-500">
                  {r.isStream ? <span title="SSE 流式">⇄</span> : "—"}
                </td>
                <td className="px-3 py-1.5 text-right text-neutral-400">{r.usageInput ?? "·"}</td>
                <td className="px-3 py-1.5 text-right text-neutral-400">{r.usageOutput ?? "·"}</td>
                <td className="max-w-64 truncate px-3 py-1.5 text-red-400" title={r.errorSummary ?? ""}>
                  {r.errorKind ?? ""}
                </td>
              </tr>
            ))}
          </tbody>
        </table>
        {filtered.length === 0 && (
          <div className="px-4 py-10 text-center text-sm text-neutral-500">
            没有匹配的日志。用任意 OpenAI 兼容客户端打一发试试。
          </div>
        )}
        {filtered.length > 0 && (
          <div className="flex justify-center border-t border-neutral-800/70 py-2">
            <button className={btnGhost} onClick={() => setLimit(limit + 500)}>
              加载更多（当前显示 {rows.length} 条）
            </button>
          </div>
        )}
      </div>
      <p className="text-xs text-neutral-600">
        日志不含具体内容，保留 30 天或 5 万行，每日自动清理。
      </p>
    </div>
  );
}
