import type { DraftProbeReport, ProbeCategory, ProbeOutcome } from "@/types";
import { cn } from "@/lib/utils";
import { CheckCircle2, CircleSlash, Info, XCircle } from "lucide-react";

/** 失败分类 → 中文短语（不要把枚举直接摆给用户看） */
const CATEGORY_LABEL: Record<ProbeCategory, string> = {
  authentication: "鉴权失败",
  model: "模型不存在",
  endpoint_unsupported: "端点不支持",
  request: "请求被拒",
  rate_limit: "被限速",
  overloaded: "上游过载",
  timeout: "超时",
  network: "连不上",
  protocol: "响应格式异常",
  url_blocked: "地址被拦截",
  no_model: "未填模型",
};

/** 端点名 → 展示名 */
const ENDPOINT_LABEL: Record<string, string> = {
  chat_completions: "chat/completions",
  responses: "responses",
  messages: "messages",
  gemini_generate: "generateContent",
  unknown: "未知端点",
};

function StatusIcon({ o }: { o: ProbeOutcome }) {
  if (o.status === "passed") {
    return (
      <CheckCircle2
        className="size-4 shrink-0 text-emerald-600 dark:text-emerald-400"
        aria-hidden
      />
    );
  }
  if (o.status === "skipped") {
    return <CircleSlash className="size-4 shrink-0 text-muted-foreground" aria-hidden />;
  }
  return <XCircle className="size-4 shrink-0 text-destructive" aria-hidden />;
}

function statusText(o: ProbeOutcome): string {
  if (o.status === "passed") return "通过";
  if (o.status === "skipped") return "跳过";
  return o.category ? CATEGORY_LABEL[o.category] : "失败";
}

/**
 * 端点探测结果面板（D9-T1）。
 *
 * 只负责展示：探测的调用与状态由父组件持有（探测不改变表单，也不参与脏状态判定）。
 * 失败行的完整 message 用 `title` 提供 hover 全文，行内只显示截断后的摘要。
 */
export function ProbePanel({
  report,
  busy,
  error,
}: {
  report: DraftProbeReport | null;
  busy: boolean;
  error: string;
}) {
  if (busy) {
    return (
      <div className="rounded-md border bg-muted/30 px-3 py-2 text-xs text-muted-foreground">
        正在对每个端点发一次最小推理请求（最多 60 秒）…
      </div>
    );
  }
  if (error) {
    return (
      <div className="rounded-md border border-destructive/40 px-3 py-2 text-xs text-destructive">
        {error}
      </div>
    );
  }
  if (!report) return null;

  const billed = report.results.some((r) => r.costPossible);
  return (
    <div className="space-y-1.5 rounded-md border bg-muted/30 px-3 py-2">
      <div className="flex items-center gap-1.5 text-xs text-muted-foreground">
        <Info className="size-3.5" aria-hidden />
        <span>
          探测结论（run {report.runId.slice(0, 8)}）
          {billed ? " · 已真实调用模型，可能产生计费" : ""}
        </span>
      </div>
      <ul className="space-y-1">
        {report.results.map((o) => (
          <li
            key={`${o.endpoint}-${o.informational ? "info" : "main"}`}
            className={cn(
              "flex items-baseline gap-2 text-xs",
              // 信息性探测（openai_compat 的 /responses）灰显：它只回答
              // 「这个中转站能不能接 Codex」，失败不算主探测失败。
              // 用 opacity 而不只是文字颜色 —— 否则与主探测行的 muted 文案无视觉差别。
              o.informational && "text-muted-foreground opacity-60",
            )}
            title={o.message}
          >
            <StatusIcon o={o} />
            <span
              className="w-32 shrink-0 truncate font-mono"
              title={ENDPOINT_LABEL[o.endpoint] ?? o.endpoint}
            >
              {ENDPOINT_LABEL[o.endpoint] ?? o.endpoint}
            </span>
            <span
              className={cn(
                "w-16 shrink-0",
                o.status === "failed" && "text-destructive",
                o.status === "passed" && "text-emerald-700 dark:text-emerald-400",
              )}
            >
              {statusText(o)}
            </span>
            <span className="w-14 shrink-0 tabular-nums text-muted-foreground">
              {o.latencyMs ? `${o.latencyMs}ms` : "—"}
            </span>
            {/* 截断的行内摘要必须自带 title：UI 门禁的「截断有 title 兜底」判据
                是逐元素看的（父级有 title 不算） */}
            <span
              className="min-w-0 flex-1 truncate text-muted-foreground"
              title={`${o.informational ? "（信息性）" : ""}${o.message}`}
            >
              {o.informational ? "（信息性）" : ""}
              {o.message}
            </span>
          </li>
        ))}
      </ul>
    </div>
  );
}
