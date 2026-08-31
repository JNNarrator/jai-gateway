import type { ReactNode } from "react";
import { cn } from "@/lib/utils";

/** 状态点 + 文案徽章：ok=运行中（绿）/ idle=已停止（灰） */
export function StatusBadge({
  tone,
  children,
  className,
}: {
  tone: "ok" | "idle";
  children: ReactNode;
  className?: string;
}) {
  return (
    <span
      className={cn(
        "inline-flex items-center gap-1.5 rounded-full border px-2.5 py-0.5 text-xs font-medium",
        tone === "ok"
          ? "border-emerald-500/30 bg-emerald-500/10 text-emerald-600 dark:text-emerald-400"
          : "border-border bg-muted text-muted-foreground",
        className,
      )}
    >
      <span
        aria-hidden
        className={cn(
          "size-1.5 rounded-full",
          tone === "ok" ? "bg-emerald-500" : "bg-muted-foreground/60",
        )}
      />
      {children}
    </span>
  );
}
