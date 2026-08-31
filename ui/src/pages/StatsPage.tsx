import { useEffect, useState } from "react";
import { api } from "../api";
import type { UsageStatRow } from "../types";
import { Card, btnGhost } from "../components/common/legacy";

export function StatsPage() {
  const [days, setDays] = useState(7);
  const [rows, setRows] = useState<UsageStatRow[]>([]);

  useEffect(() => {
    api.statsUsage(days).then(setRows).catch(() => {});
  }, [days]);

  const totalRequests = rows.reduce((s, r) => s + r.requests, 0);
  const totalInput = rows.reduce((s, r) => s + r.inputTokens, 0);
  const totalOutput = rows.reduce((s, r) => s + r.outputTokens, 0);
  const totalTokens = totalInput + totalOutput;
  const maxDayTokens = Math.max(1, ...rows.map((r) => r.inputTokens + r.outputTokens));

  return (
    <div className="mx-auto max-w-4xl space-y-4">
      <div className="flex flex-wrap items-center justify-between gap-2">
        <h1 className="text-lg font-semibold">用量统计</h1>
        <div className="flex items-center gap-2 text-xs">
          {[7, 30, 90].map((d) => (
            <button
              key={d}
              className={`${btnGhost} ${days === d ? "bg-neutral-800 text-foreground" : ""}`}
              onClick={() => setDays(d)}
            >
              近 {d} 天
            </button>
          ))}
        </div>
      </div>

      <div className="grid grid-cols-2 gap-3 md:grid-cols-4">
        <Card title="请求数">
          <div className="text-2xl font-semibold">{totalRequests}</div>
        </Card>
        <Card title="输入 Token">
          <div className="text-2xl font-semibold">{totalInput.toLocaleString()}</div>
        </Card>
        <Card title="输出 Token">
          <div className="text-2xl font-semibold">{totalOutput.toLocaleString()}</div>
        </Card>
        <Card title="总 Token">
          <div className="text-2xl font-semibold">{totalTokens.toLocaleString()}</div>
        </Card>
      </div>

      <Card title="每日 Token 用量">
        {rows.length === 0 ? (
          <div className="py-10 text-center text-sm text-neutral-500">
            暂无日志数据。请求经过网关后这里会展示 token 用量。
          </div>
        ) : (
          <div className="flex h-40 items-end gap-1 overflow-x-auto">
            {rows.map((r) => {
              const total = r.inputTokens + r.outputTokens;
              const h = Math.round((total / maxDayTokens) * 100);
              const date = new Date(r.day * 86_400_000).toISOString().slice(0, 10);
              return (
                <div
                  key={r.day}
                  className="flex min-w-6 flex-1 flex-col items-center gap-1"
                  title={`${date}：${r.requests} 请求，${total.toLocaleString()} tokens`}
                >
                  <div className="w-full rounded-t bg-amber-500/70" style={{ height: `${h}%` }} />
                  <span className="text-[10px] text-neutral-500">{date.slice(5)}</span>
                </div>
              );
            })}
          </div>
        )}
      </Card>
    </div>
  );
}
