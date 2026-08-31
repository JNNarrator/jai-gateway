import { useEffect, useState } from "react";
import { Download, RefreshCw, ScrollText, Search } from "lucide-react";
import { api } from "../api";
import type { LogRowView } from "../types";
import { toast } from "../lib/toast";
import { cn } from "@/lib/utils";
import { PageHeader } from "@/components/common/PageHeader";
import { EmptyState } from "@/components/common/EmptyState";
import { SkeletonList } from "@/components/common/SkeletonList";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import { Switch } from "@/components/ui/switch";
import {
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow,
} from "@/components/ui/table";

export function LogsPage() {
  const [rows, setRows] = useState<LogRowView[]>([]);
  const [auto, setAuto] = useState(true);
  const [intervalMs, setIntervalMs] = useState(3000);
  const [modelFilter, setModelFilter] = useState("");
  const [statusFilter, setStatusFilter] = useState("");
  const [limit, setLimit] = useState(500);
  const [initialLoading, setInitialLoading] = useState(true);

  async function refresh() {
    setRows(await api.logsRecent(limit));
  }

  useEffect(() => {
    refresh().finally(() => setInitialLoading(false));
    if (!auto) return;
    const t = setInterval(refresh, intervalMs);
    return () => clearInterval(t);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [auto, intervalMs, limit]);

  const filtered = rows.filter((r) => {
    if (modelFilter && !r.modelName.toLowerCase().includes(modelFilter.toLowerCase()))
      return false;
    if (statusFilter && String(r.httpStatus) !== statusFilter) return false;
    return true;
  });

  function statusClass(s: number) {
    if (s >= 500) return "text-red-600 dark:text-red-400";
    if (s >= 400) return "text-red-500 dark:text-red-300";
    if (s >= 200 && s < 300) return "text-emerald-600 dark:text-emerald-400";
    return "text-muted-foreground";
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
    const blob = new Blob(["\ufeff" + header + body], { type: "text/csv;charset=utf-8" });
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
      <PageHeader
        title="请求日志"
        description="网关转发的每一次请求（仅元数据，不含 prompt 与响应明文）。"
        actions={
          <div className="flex flex-wrap items-center gap-2">
            <Switch
              checked={auto}
              onCheckedChange={setAuto}
              aria-label="自动刷新"
            />
            <span className="text-sm text-muted-foreground">自动刷新</span>
            <Select
              value={String(intervalMs)}
              onValueChange={(v) => setIntervalMs(Number(v))}
              disabled={!auto}
            >
              <SelectTrigger size="sm" className="w-24">
                <SelectValue />
              </SelectTrigger>
              <SelectContent>
                <SelectItem value="3000">3s</SelectItem>
                <SelectItem value="5000">5s</SelectItem>
                <SelectItem value="10000">10s</SelectItem>
                <SelectItem value="30000">30s</SelectItem>
              </SelectContent>
            </Select>
          </div>
        }
      />

      <div className="flex flex-wrap items-center gap-2">
        <div className="relative">
          <Search
            className="pointer-events-none absolute left-2.5 top-1/2 size-4 -translate-y-1/2 text-muted-foreground"
            aria-hidden
          />
          <Input
            className="w-44 pl-8"
            placeholder="筛选模型…"
            value={modelFilter}
            onChange={(e) => setModelFilter(e.target.value)}
          />
        </div>
        <Input
          className="w-24"
          placeholder="状态码"
          value={statusFilter}
          onChange={(e) => setStatusFilter(e.target.value)}
        />
        <div className="ml-auto flex gap-2">
          <Button variant="outline" size="sm" onClick={exportCsv}>
            <Download aria-hidden />
            导出 CSV
          </Button>
          <Button variant="outline" size="sm" onClick={exportJson}>
            <Download aria-hidden />
            导出 JSON
          </Button>
        </div>
      </div>

      {initialLoading ? (
        <SkeletonList rows={8} itemClassName="h-8" />
      ) : (
        <>
      <div className="overflow-x-auto rounded-lg border">
        <Table>
          <TableHeader>
            <TableRow className="bg-muted/50 hover:bg-muted/50">
              <TableHead>时间</TableHead>
              <TableHead>模型</TableHead>
              <TableHead>状态</TableHead>
              <TableHead>耗时</TableHead>
              <TableHead>流式</TableHead>
              <TableHead className="text-right">输入</TableHead>
              <TableHead className="text-right">输出</TableHead>
              <TableHead>错误</TableHead>
            </TableRow>
          </TableHeader>
          <TableBody className="font-mono text-xs">
            {filtered.map((r) => (
              <TableRow
                key={r.id}
                className={cn(
                  r.httpStatus >= 400 && "bg-destructive/5",
                )}
              >
                <TableCell className="whitespace-nowrap text-muted-foreground">
                  {new Date(r.ts).toLocaleTimeString()}
                </TableCell>
                <TableCell className="max-w-48 truncate">{r.modelName}</TableCell>
                <TableCell className={cn("font-semibold", statusClass(r.httpStatus))}>
                  {r.httpStatus}
                </TableCell>
                <TableCell className="text-muted-foreground">{r.durationMs}ms</TableCell>
                <TableCell className="text-muted-foreground">
                  {r.isStream ? <span title="SSE 流式">⇄</span> : "—"}
                </TableCell>
                <TableCell className="text-right text-muted-foreground">
                  {r.usageInput ?? "·"}
                </TableCell>
                <TableCell className="text-right text-muted-foreground">
                  {r.usageOutput ?? "·"}
                </TableCell>
                <TableCell
                  className="max-w-64 truncate text-red-600 dark:text-red-400"
                  title={r.errorSummary ?? ""}
                >
                  {r.errorKind ?? ""}
                </TableCell>
              </TableRow>
            ))}
          </TableBody>
        </Table>
        {filtered.length === 0 && (
          <EmptyState
            icon={ScrollText}
            title="没有匹配的日志"
            description="用任意 OpenAI 兼容客户端打一发试试。"
            className="border-0"
          />
        )}
        {filtered.length > 0 && (
          <div className="flex justify-center border-t py-2">
            <Button variant="ghost" size="sm" onClick={() => setLimit(limit + 500)}>
              <RefreshCw aria-hidden />
              加载更多（当前显示 {rows.length} 条）
            </Button>
          </div>
        )}
      </div>
        </>
      )}
      <p className="text-xs text-muted-foreground">
        日志不含具体内容，保留 30 天或 5 万行，每日自动清理。
      </p>
    </div>
  );
}
