import { useEffect, useState } from "react";
import { BarChart3 } from "lucide-react";
import {
  Bar,
  BarChart,
  CartesianGrid,
  ResponsiveContainer,
  Tooltip as ChartTooltip,
  XAxis,
  YAxis,
} from "recharts";
import { api } from "../api";
import type { UsageStatRow } from "../types";
import { cn } from "@/lib/utils";
import { PageHeader } from "@/components/common/PageHeader";
import { EmptyState } from "@/components/common/EmptyState";
import { Skeleton } from "@/components/ui/skeleton";
import { Button } from "@/components/ui/button";
import {
  Card,
  CardContent,
  CardDescription,
  CardHeader,
  CardTitle,
} from "@/components/ui/card";

const RANGES = [7, 30, 90] as const;

function dayLabel(day: number) {
  return new Date(day * 86_400_000).toISOString().slice(5, 10); // MM-DD
}

export function StatsPage() {
  const [days, setDays] = useState<number>(7);
  const [rows, setRows] = useState<UsageStatRow[]>([]);
  const [initialLoading, setInitialLoading] = useState(true);

  useEffect(() => {
    api
      .statsUsage(days)
      .then(setRows)
      .catch(() => {})
      .finally(() => setInitialLoading(false));
  }, [days]);

  const totalRequests = rows.reduce((s, r) => s + r.requests, 0);
  const totalInput = rows.reduce((s, r) => s + r.inputTokens, 0);
  const totalOutput = rows.reduce((s, r) => s + r.outputTokens, 0);
  const totalTokens = totalInput + totalOutput;

  const chartData = rows.map((r) => ({
    ...r,
    label: dayLabel(r.day),
  }));

  const kpis: [string, string][] = [
    ["请求数", totalRequests.toLocaleString()],
    ["输入 Token", totalInput.toLocaleString()],
    ["输出 Token", totalOutput.toLocaleString()],
    ["总 Token", totalTokens.toLocaleString()],
  ];

  return (
    <div className="mx-auto max-w-4xl space-y-4">
      <PageHeader
        title="用量统计"
        description="按天汇总经过网关的请求与 token 用量。"
        actions={
          <div className="flex items-center gap-1">
            {RANGES.map((d) => (
              <Button
                key={d}
                variant={days === d ? "secondary" : "ghost"}
                size="sm"
                className={cn(days === d && "font-medium text-foreground")}
                onClick={() => setDays(d)}
                aria-pressed={days === d}
              >
                近 {d} 天
              </Button>
            ))}
          </div>
        }
      />

      {initialLoading ? (
        <div className="space-y-3">
          <div className="grid grid-cols-2 gap-3 md:grid-cols-4">
            {[0, 1, 2, 3].map((i) => (
              <Skeleton key={i} className="h-28" />
            ))}
          </div>
          <Skeleton className="h-80 w-full rounded-lg" />
        </div>
      ) : (
        <>
      <div className="grid grid-cols-2 gap-3 md:grid-cols-4">
        {kpis.map(([title, value]) => (
          <Card key={title}>
            <CardHeader>
              <CardDescription>{title}</CardDescription>
              <CardTitle className="text-2xl tabular-nums">{value}</CardTitle>
            </CardHeader>
          </Card>
        ))}
      </div>

      <Card>
        <CardHeader>
          <CardTitle>每日 Token 用量</CardTitle>
          <CardDescription>输入与输出分色堆叠；悬停查看单日明细。</CardDescription>
        </CardHeader>
        <CardContent>
          {rows.length === 0 ? (
            <EmptyState
              icon={BarChart3}
              title="暂无日志数据"
              description="请求经过网关后，这里会按天展示 token 用量。"
              className="border-0 py-8"
            />
          ) : (
            <div className="h-72 w-full">
              <ResponsiveContainer width="100%" height="100%">
                <BarChart data={chartData} margin={{ top: 8, right: 8, left: 0, bottom: 0 }}>
                  <CartesianGrid vertical={false} stroke="var(--border)" />
                  <XAxis
                    dataKey="label"
                    tickLine={false}
                    axisLine={false}
                    tick={{ fill: "var(--muted-foreground)", fontSize: 11 }}
                  />
                  <YAxis
                    width={56}
                    tickLine={false}
                    axisLine={false}
                    tick={{ fill: "var(--muted-foreground)", fontSize: 11 }}
                    tickFormatter={(v: number) => v.toLocaleString()}
                  />
                  <ChartTooltip
                    cursor={{ fill: "var(--accent)" }}
                    content={({ active, payload }) => {
                      if (!active || !payload?.length) return null;
                      const row = payload[0].payload as UsageStatRow;
                      const total = row.inputTokens + row.outputTokens;
                      return (
                        <div className="rounded-md border bg-popover px-3 py-2 text-xs shadow-md">
                          <div className="mb-1 font-medium text-popover-foreground">
                            {dayLabel(row.day)}
                          </div>
                          <div className="space-y-0.5 text-muted-foreground">
                            <div>请求：{row.requests.toLocaleString()}</div>
                            <div>输入：{row.inputTokens.toLocaleString()}</div>
                            <div>输出：{row.outputTokens.toLocaleString()}</div>
                            <div className="font-medium text-foreground">
                              合计：{total.toLocaleString()}
                            </div>
                          </div>
                        </div>
                      );
                    }}
                  />
                  <Bar dataKey="inputTokens" name="输入" stackId="tokens" fill="var(--primary)" radius={[0, 0, 0, 0]} />
                  <Bar dataKey="outputTokens" name="输出" stackId="tokens" fill="var(--primary)" fillOpacity={0.45} radius={[4, 4, 0, 0]} />
                </BarChart>
              </ResponsiveContainer>
            </div>
          )}
        </CardContent>
      </Card>
        </>
      )}
    </div>
  );
}
