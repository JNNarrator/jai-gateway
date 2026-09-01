import { useState, type ReactNode } from "react";
import { Check, Copy, Eye, EyeOff } from "lucide-react";
import { Button } from "@/components/ui/button";
import {
  Tooltip,
  TooltipContent,
  TooltipProvider,
  TooltipTrigger,
} from "@/components/ui/tooltip";
import { copyText } from "@/lib/clipboard";

function IconTip({
  label,
  onClick,
  disabled,
  children,
}: {
  label: string;
  onClick: () => void;
  disabled?: boolean;
  children: ReactNode;
}) {
  return (
    <TooltipProvider>
      <Tooltip>
        <TooltipTrigger asChild>
          <Button
            variant="outline"
            size="icon"
            className="size-9 shrink-0"
            onClick={onClick}
            disabled={disabled}
            aria-label={label}
          >
            {children}
          </Button>
        </TooltipTrigger>
        <TooltipContent>{label}</TooltipContent>
      </Tooltip>
    </TooltipProvider>
  );
}

/**
 * 只读代码字段 + 复制按钮。
 * - `value` 实际复制内容；`display` 展示文本（掩码/占位场景与 value 不同）
 * - 传 `onToggleReveal` 时渲染 显示/隐藏 图标按钮（`revealed` 指示当前态）
 * - `children` 追加在右侧的操作区（如「轮换密钥」）
 */
export function CopyField({
  value,
  display,
  onToggleReveal,
  revealed,
  copyDisabled,
  onCopy,
  className,
  children,
}: {
  value: string;
  display: string;
  onToggleReveal?: () => void;
  revealed?: boolean;
  copyDisabled?: boolean;
  /** 提供时替代内部复制逻辑（如复制前需异步取全量值），组件仍负责已复制反馈 */
  onCopy?: () => Promise<void> | void;
  className?: string;
  children?: ReactNode;
}) {
  const [copied, setCopied] = useState(false);
  const doCopy = async () => {
    if (onCopy) await onCopy();
    else copyText(value);
    setCopied(true);
    window.setTimeout(() => setCopied(false), 1500);
  };

  return (
    <div className={className}>
      <div className="flex items-center gap-2">
        <code className="min-w-0 flex-1 truncate rounded-md border bg-muted/50 px-3 py-2 font-mono text-sm text-foreground">
          {display}
        </code>
        <IconTip label={copied ? "已复制" : "复制"} onClick={doCopy} disabled={copyDisabled}>
          {copied ? <Check className="size-4 text-emerald-500" /> : <Copy className="size-4" />}
        </IconTip>
        {onToggleReveal && (
          <IconTip label={revealed ? "隐藏" : "显示"} onClick={onToggleReveal}>
            {revealed ? <EyeOff className="size-4" /> : <Eye className="size-4" />}
          </IconTip>
        )}
        {children}
      </div>
    </div>
  );
}
