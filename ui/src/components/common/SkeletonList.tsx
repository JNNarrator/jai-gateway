import { Skeleton } from "@/components/ui/skeleton";
import { cn } from "@/lib/utils";

/** 列表类页面加载骨架（spec §8：加载态用 Skeleton 替换空白/闪烁） */
export function SkeletonList({
  rows = 3,
  className,
  itemClassName,
}: {
  rows?: number;
  className?: string;
  itemClassName?: string;
}) {
  return (
    <div className={cn("space-y-3", className)} aria-hidden>
      {Array.from({ length: rows }).map((_, i) => (
        <Skeleton key={i} className={cn("h-20 w-full rounded-lg", itemClassName)} />
      ))}
    </div>
  );
}
