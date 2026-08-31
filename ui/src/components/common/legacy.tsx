// 过渡期共享样式，阶段 2–5 逐页迁移后删除
export function Card({
  title,
  children,
}: {
  title: string;
  children: React.ReactNode;
}) {
  return (
    <div className="rounded-lg border border-neutral-800 bg-neutral-900/60 p-4">
      <h2 className="mb-3 text-sm font-semibold tracking-wide text-neutral-300">
        {title}
      </h2>
      {children}
    </div>
  );
}

export const inputCls =
  "w-full rounded border border-neutral-700 bg-neutral-950 px-2 py-1.5 text-sm text-neutral-100 outline-none focus:border-amber-500";
export const btnCls =
  "rounded px-3 py-1.5 text-sm font-medium transition-colors disabled:cursor-not-allowed disabled:opacity-40";
export const btnPrimary = `${btnCls} bg-primary text-primary-foreground hover:bg-primary/90`;
export const btnGhost = `${btnCls} border border-neutral-700 text-neutral-300 hover:border-neutral-500`;
export const btnDanger = `${btnCls} border border-red-900/60 text-red-400 hover:bg-red-950`;
