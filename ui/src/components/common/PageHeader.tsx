import type { ReactNode } from "react";
import { cn } from "@/lib/utils";

/** 页头：标题 + 说明 + 右侧页级操作区，统一各页排版节奏 */
export function PageHeader({
  title,
  description,
  actions,
  className,
}: {
  title: string;
  description?: string;
  actions?: ReactNode;
  className?: string;
}) {
  return (
    // data-slot 是**探针契约**（不是装饰）：fold.mjs 用 `[data-slot=page-header] button`
    // 定位「页级主操作」来判断首屏可达性。此前没有这个属性，选择器恒为空，
    // 报告里的「顶部操作区: —」是个**死字段**（假绿），等于这条验收从未生效。
    <div
      data-slot="page-header"
      className={cn("flex flex-wrap items-end justify-between gap-2", className)}
    >
      <div className="space-y-1">
        <h1 className="text-lg font-semibold text-foreground">{title}</h1>
        {description && <p className="text-sm text-muted-foreground">{description}</p>}
      </div>
      {actions && (
        <div data-slot="page-header-actions" className="flex items-center gap-2">
          {actions}
        </div>
      )}
    </div>
  );
}
