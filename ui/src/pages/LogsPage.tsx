import { useEffect, useState } from "react";
import { Copy, Download, RefreshCw, ScrollText, Search, Terminal } from "lucide-react";
import { api } from "../api";
import type { LogRowView } from "../types";
import { toast } from "../lib/toast";
import { cn } from "@/lib/utils";
import { PageHeader } from "@/components/common/PageHeader";
import { EmptyState } from "@/components/common/EmptyState";
import { SkeletonList } from "@/components/common/SkeletonList";
import { Button } from "@/components/ui/button";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
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
  const [providerFilter, setProviderFilter] = useState("");
  const [sel, setSel] = useState<LogRowView | null>(null);
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
    if (providerFilter && (r.providerId ?? "") !== providerFilter) return false;
    return true;
  });

  const providerOptions = Array.from(
    new Set(rows.map((r) => r.providerId ?? "").filter(Boolean)),
  ).sort();

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

  /** 用行内元数据拼可复现命令（日志按隐私基线不存请求体 → 占位模板）。 */
  function curlFor(r: LogRowView) {
    const path = r.inboundFamily.includes("anthropic") ? "/v1/messages" : "/v1/chat/completions";
    const url = `http://127.0.0.1:1314${path}`;
    const body = JSON.stringify(
      {
        model: r.modelName,
        stream: r.isStream,
        messages: [{ role: "user", content: "粘贴真实请求内容" }],
      },
      null,
      2,
    );
    return [
      `curl -N '${url}' \\`,
      "  -H 'Authorization: Bearer <网关密钥>' \\",
      "  -H 'Content-Type: application/json' \\",
      `  -d '${body}'`,
      "",
      `# 复现第 ${r.id} 条日志（${new Date(r.ts).toLocaleString("zh-CN", { hour12: false })}，状态 ${r.httpStatus}）；`,
      "# 密钥在「接入」页获取；端口以实际网关端口为准",
    ].join("\n");
  }

  async function copyCurl() {
    if (!sel) return;
    try {
      await navigator.clipboard.writeText(curlFor(sel));
      toast("已复制 cURL 复现命令");
    } catch {
      toast("复制失败", "err");
    }
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
        <Select value={providerFilter} onValueChange={setProviderFilter}>
          <SelectTrigger size="sm" className="w-36">
            <SelectValue placeholder="全部供应商" />
          </SelectTrigger>
          <SelectContent>
            <SelectItem value="__all__">全部供应商</SelectItem>
            {providerOptions.map((p) => (
              <SelectItem key={p} value={p}>
                {p}
              </SelectItem>
            ))}
          </SelectContent>
        </Select>
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
                  "cursor-pointer",
                )}
                onClick={() => setSel(r)}
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
        日志不含具体内容，保留 30 天或 5 万行，每日自动清理。点击任意行查看详情并复制复现命令。
      </p>

      <Dialog open={!!sel} onOpenChange={(o) => !o && setSel(null)}>
        <DialogContent className="max-w-xl">
          <DialogHeader>
            <DialogTitle className="flex items-center gap-2">
              <Terminal className="size-4" aria-hidden />
              请求详情
              {sel && (
                <span className={cn("text-sm font-semibold", statusClass(sel.httpStatus))}>
                  {sel.httpStatus}
                </span>
              )}
            </DialogTitle>
            <DialogDescription>
              {sel && new Date(sel.ts).toLocaleString("zh-CN", { hour12: false })} · 日志 #{sel?.id}
            </DialogDescription>
          </DialogHeader>
          {sel && (
            <div className="space-y-3 text-sm">
              <div className="grid grid-cols-2 gap-x-4 gap-y-2">
                <Detail label="入站协议" value={sel.inboundFamily} />
                <Detail label="路由模式" value={sel.routeMode} />
                <Detail label="供应商" value={sel.providerId ?? "—"} />
                <Detail label="模型" value={sel.modelName} />
                <Detail label="耗时" value={`${sel.durationMs}ms`} />
                <Detail
                  label="流式"
                  value={sel.isStream ? "SSE 流式" : "一次性"}
                />
                <Detail label="输入 Token" value={sel.usageInput?.toLocaleString() ?? "—"} />
                <Detail label="输出 Token" value={sel.usageOutput?.toLocaleString() ?? "—"} />
              </div>
              {(sel.errorKind || sel.errorSummary) && (
                <div className="rounded-md border border-red-500/30 bg-red-500/5 px-3 py-2 text-xs">
                  <div className="font-semibold text-red-600 dark:text-red-400">
                    {sel.errorKind ?? "错误"}
                  </div>
                  {sel.errorSummary && (
                    <div className="mt-1 whitespace-pre-wrap break-all text-muted-foreground">
                      {sel.errorSummary}
                    </div>
                  )}
                </div>
              )}
              <div className="rounded-md border bg-muted/30 p-3">
                <div className="mb-2 flex items-center justify-between">
                  <div className="text-xs font-medium text-muted-foreground">
                    复制为 cURL（复现命令，请求体为占位模板）
                  </div>
                  <Button variant="outline" size="sm" className="h-7" onClick={() => void copyCurl()}>
                    <Copy aria-hidden />
                    复制
                  </Button>
                </div>
                <pre className="max-h-56 overflow-auto whitespace-pre-wrap break-all font-mono text-[11px] leading-relaxed text-muted-foreground">
                  {curlFor(sel)}
                </pre>
              </div>
            </div>
          )}
        </DialogContent>
      </Dialog>
    </div>
  );
}

function Detail({ label, value }: { label: string; value: string }) {
  return (
    <div className="min-w-0">
      <div className="text-xs text-muted-foreground">{label}</div>
      <div className="truncate font-mono text-xs">{value}</div>
    </div>
  );
}
