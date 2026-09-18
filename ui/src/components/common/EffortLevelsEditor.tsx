import { useState } from "react";
import { Gauge, Minus } from "lucide-react";
import { Input } from "@/components/ui/input";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuLabel,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";

/**
 * 推理档位值域（reasoning effort）声明编辑器（0011）。
 *
 * 为什么需要它：客户端（zcode / Codex / Claude Code …）会带 `reasoning.effort`
 * （如 `none`），而不同上游只认各自的值域。JAI 默认**原样透传**，上游不认就 400
 * —— 2026-09-18 真机故障即此：zcode 发 `none`，基元律动只认
 * low/medium/high/xhigh/max，客户端只看到一个被包装过的
 * 「Provider rejected the model request.」。声明值域后由网关丢弃/收敛该参数。
 *
 * 语义：留空 = 未声明 ⇒ 原样透传（上游自己认就认）；模型级非空时覆盖供应商级。
 */

/** 常用预设。第一条是本次故障里基元律动对 DeepSeek 的实测值域（取自上游 400 原文）。 */
const PRESETS: { label: string; value: string[] }[] = [
  {
    label: "low / medium / high / xhigh / max",
    value: ["low", "medium", "high", "xhigh", "max"],
  },
  { label: "low / medium / high", value: ["low", "medium", "high"] },
  {
    label: "off / low / medium / high / xhigh / max",
    value: ["off", "low", "medium", "high", "xhigh", "max"],
  },
];

/** 与后端 `gateway_core::effort::parse` 同口径：去空白、小写、保序去重；空 → null。 */
export function parseLevelsDraft(s: string): string[] | null {
  const out: string[] = [];
  for (const tok of s.split(/[,\s]+/)) {
    const t = tok.trim().toLowerCase();
    if (!t || out.includes(t)) continue;
    out.push(t);
  }
  return out.length ? out : null;
}

export function EffortLevelsEditor({
  levels,
  onChange,
  verbose = false,
  scopeLabel,
}: {
  /** 当前声明；null/空 = 未声明 */
  levels?: string[] | null;
  /** 保存（null = 清除声明）。抛错由调用方 toast */
  onChange: (levels: string[] | null) => Promise<void>;
  /** true = 平铺展示完整值域（供应商卡片）；false = 紧凑芯片（模型表格） */
  verbose?: boolean;
  /** 无障碍/提示文案里的作用域，如「模型 deepseek-flash」 */
  scopeLabel: string;
}) {
  const declared = levels && levels.length > 0;
  const [draft, setDraft] = useState(declared ? levels!.join(",") : "");
  const [saving, setSaving] = useState(false);

  const hint = declared
    ? `推理档位值域：${levels!.join(" / ")}`
    : "未声明推理档位值域（原样透传给上游）";

  async function commit(next: string[] | null) {
    setSaving(true);
    try {
      await onChange(next);
      setDraft(next ? next.join(",") : "");
    } finally {
      setSaving(false);
    }
  }

  const scope = `推理档位值域 ${scopeLabel}`;

  return (
    <DropdownMenu
      onOpenChange={(open) => {
        // 每次展开都以最新的声明值起草，避免沿用上次编辑的残留
        if (open) setDraft(declared ? levels!.join(",") : "");
      }}
    >
      <DropdownMenuTrigger asChild>
        <button
          type="button"
          title={hint}
          aria-label={scope}
          className={
            verbose
              ? "relative inline-flex items-center gap-1 text-xs text-muted-foreground underline-offset-2 hover:text-foreground hover:underline after:absolute after:-inset-x-2 after:-inset-y-1.5 after:content-['']"
              : "relative rounded p-0.5 text-muted-foreground/60 hover:bg-muted hover:text-foreground after:absolute after:-inset-2 after:content-['']"
          }
        >
          <Gauge className="size-3" aria-hidden />
          {verbose && (
            <span className={saving ? "opacity-50" : ""}>
              {declared
                ? `档位 ${levels!.join(" / ")}`
                : "档位未声明（原样透传）"}
            </span>
          )}
          {!verbose && (
            <span className="text-[10px]">
              {declared ? `${levels!.length}档` : "档位?"}
            </span>
          )}
        </button>
      </DropdownMenuTrigger>
      <DropdownMenuContent align="center" className="w-72">
        <DropdownMenuLabel className="text-[11px] text-muted-foreground">
          上游认哪些推理档位
        </DropdownMenuLabel>
        <div className="px-2 pb-1">
          <Input
            className="h-8 text-xs"
            value={draft}
            placeholder="low,medium,high,xhigh,max"
            aria-label={`${scope}（逗号分隔，回车保存）`}
            onChange={(e) => setDraft(e.target.value)}
            onKeyDown={(e) => {
              if (e.key === "Enter") {
                e.preventDefault();
                void commit(parseLevelsDraft(draft));
              }
            }}
          />
          <p className="mt-1 text-[10px] text-muted-foreground">
            声明序逗号分隔，回车保存；留空 = 未声明（原样透传）。
          </p>
        </div>
        <DropdownMenuSeparator />
        {PRESETS.map((p) => (
          <DropdownMenuItem key={p.label} onSelect={() => void commit(p.value)}>
            {p.label}
          </DropdownMenuItem>
        ))}
        <DropdownMenuSeparator />
        <DropdownMenuItem onSelect={() => void commit(null)}>
          <Minus className="size-3.5" aria-hidden />
          清除（未声明 = 原样透传）
        </DropdownMenuItem>
      </DropdownMenuContent>
    </DropdownMenu>
  );
}
