import { useState } from "react";
import { SlidersHorizontal, Minus } from "lucide-react";
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
 * 工具声明数上限（max_tools）声明编辑器（0012）。
 *
 * 为什么需要它：JAI 曾按 128 硬拦工具声明数（GodeX 参考值），而 2026-09-18 实测
 * 基元律动接受 140 / 300 个工具均 200 —— 网关**自己发明的限制**把 zcode 的 140 个工具
 * 直接 400 掉（`tools_limit_exceeded`），而这是上游完全能跑的配置。
 *
 * 新语义：**没声明就不拦**（放行，由上游裁决，上游真报错时其原文照常回给客户端）；
 * **声明了就严格执行**（超限 400）。上限只有真的知道时才填。
 */

/** 常用预设：128 是历史上被硬编码的值，留着方便「确实需要拦」的渠道一键设置。 */
const PRESETS: number[] = [128, 256, 512];

export function MaxToolsEditor({
  maxTools,
  onChange,
  verbose = false,
  scopeLabel,
}: {
  /** 当前声明；null/undefined = 未声明 */
  maxTools?: number | null;
  /** 保存（null = 清除声明）。抛错由调用方 toast */
  onChange: (maxTools: number | null) => Promise<void>;
  /** true = 平铺展示（供应商卡片）；false = 紧凑芯片（模型表格） */
  verbose?: boolean;
  /** 无障碍/提示文案里的作用域，如「模型 deepseek-flash」 */
  scopeLabel: string;
}) {
  const declared = typeof maxTools === "number" && maxTools > 0;
  const [draft, setDraft] = useState(declared ? String(maxTools) : "");
  const [saving, setSaving] = useState(false);

  const hint = declared
    ? `工具声明数上限：${maxTools}（超限 JAI 直接 400）`
    : "未声明工具数上限：不拦，交给上游裁决";

  async function commit(next: number | null) {
    setSaving(true);
    try {
      await onChange(next);
      setDraft(next ? String(next) : "");
    } finally {
      setSaving(false);
    }
  }

  const scope = `工具声明数上限 ${scopeLabel}`;

  return (
    <DropdownMenu
      onOpenChange={(open) => {
        if (open) setDraft(declared ? String(maxTools) : "");
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
          <SlidersHorizontal className="size-3" aria-hidden />
          {verbose && (
            <span className={saving ? "opacity-50" : ""}>
              {declared ? `工具上限 ${maxTools}` : "工具上限未声明（不拦）"}
            </span>
          )}
          {!verbose && (
            <span className="text-[10px]">{declared ? `≤${maxTools}` : "上限?"}</span>
          )}
        </button>
      </DropdownMenuTrigger>
      <DropdownMenuContent align="center" className="w-64">
        <DropdownMenuLabel className="text-[11px] text-muted-foreground">
          工具声明数上限（超限即 400）
        </DropdownMenuLabel>
        <div className="px-2 pb-1">
          <Input
            className="h-8 text-xs"
            type="number"
            min={0}
            value={draft}
            placeholder="留空 = 不拦"
            aria-label={`${scope}（回车保存）`}
            onChange={(e) => setDraft(e.target.value)}
            onKeyDown={(e) => {
              if (e.key === "Enter") {
                e.preventDefault();
                const n = Number(draft);
                void commit(Number.isFinite(n) && n > 0 ? Math.floor(n) : null);
              }
            }}
          />
          <p className="mt-1 text-[10px] text-muted-foreground">
            回车保存；留空 = 未声明（不拦，由上游裁决）。
          </p>
        </div>
        <DropdownMenuSeparator />
        {PRESETS.map((n) => (
          <DropdownMenuItem key={n} onSelect={() => void commit(n)}>
            {n} 个
          </DropdownMenuItem>
        ))}
        <DropdownMenuSeparator />
        <DropdownMenuItem onSelect={() => void commit(null)}>
          <Minus className="size-3.5" aria-hidden />
          清除（未声明 = 不拦）
        </DropdownMenuItem>
      </DropdownMenuContent>
    </DropdownMenu>
  );
}
