import { useCallback, useEffect, useMemo, useRef, useState, type ReactNode } from "react";
import { CircleAlert, CircleCheck, X } from "lucide-react";
import { cn } from "@/lib/utils";

/**
 * 反馈的落点规则（2026-09-23 全局整改）。
 *
 * 起因（用户反馈「有些反馈都在最顶部……还得滚到上面或者下面去看提示」）：
 * 页级 `msg/err` 横幅此前直接渲染在 `PageHeader` 之后，而触发它的按钮常在列表下方 ——
 * 点完必须**滚回页面顶部**才看得到；右下角的 toast 又在另一个对角，视线来回跳。
 *
 * 现在的规则（`ProvidersPage` 早就这么做了，只是没推广开 —— 它把反馈放在产生它的卡片里）：
 * - **行级按钮**（列表某一行的「列出工具」等）→ 反馈落在**该行内**（`rowFeedback`）；
 * - **页级 / 卡片级按钮** → 反馈**吸顶**（`StickyFeedback` 或并入页面已有的吸顶操作条），
 *   跟着滚动条走，永远在视口里；
 * - **错误不自动消失**（上游报错原文往往很长，2.4s 看不完就没了），成功类 2.4s 自动收。
 *
 * 「吸顶」为什么必须和页面已有的 `page-actions` 合成**同一个** sticky 容器：两个
 * `top-0` 的 sticky 元素会叠在一起，DOM 靠后的那个把前面那个盖住（同步页的 WebDAV
 * 操作条就踩过这个坑），所以每个页面只保留一个吸顶区。
 */
export type FeedbackKind = "ok" | "err";

export interface Feedback {
  kind: FeedbackKind;
  text: string;
}

/** 成功类自动消失时长。错误不设 TTL —— 见文件头。 */
const OK_TTL_MS = 2400;

/** 页级反馈在 `useFeedback` 内部用的 key（行级用调用方给的 id）。 */
const PAGE = "__page__";

/** 一行反馈（吸顶条与行内共用）。 */
export function FeedbackLine({
  feedback,
  onDismiss,
  className,
  testId = "page-feedback",
  children,
}: {
  feedback: Feedback | null;
  onDismiss: () => void;
  className?: string;
  testId?: string;
  children?: ReactNode;
}) {
  if (!feedback) return null;
  const err = feedback.kind === "err";
  return (
    <div
      role={err ? "alert" : "status"}
      data-testid={testId}
      data-kind={feedback.kind}
      className={cn(
        "flex items-start gap-2 rounded-md border px-3 py-2 text-sm",
        err
          ? "border-destructive/50 bg-destructive/10 text-destructive"
          : "border-primary/40 bg-primary/10 text-primary",
        className,
      )}
    >
      {err ? (
        <CircleAlert className="mt-0.5 size-4 shrink-0" aria-hidden />
      ) : (
        <CircleCheck className="mt-0.5 size-4 shrink-0" aria-hidden />
      )}
      <span className="min-w-0 flex-1 break-words">{feedback.text}</span>
      {children}
      {/* 真实盒子 24×24：UI 门禁要求命中区 ≥24×24（`size-6` = 24px） */}
      <button
        type="button"
        aria-label="关闭提示"
        onClick={onDismiss}
        className="-my-0.5 -mr-1 inline-flex size-6 shrink-0 items-center justify-center rounded text-current opacity-70 hover:bg-current/10 hover:opacity-100"
      >
        <X className="size-4" aria-hidden />
      </button>
    </div>
  );
}

/**
 * 页级反馈条：**吸顶**。
 *
 * 只给「页面上没有别的吸顶操作条」的页面用（MCP / 技能 / 供应商）。有 `page-actions`
 * 的页面请把 `FeedbackLine` 放进那个 sticky 容器里 —— 两个 `top-0` 的 sticky 会叠。
 */
export function StickyFeedback({
  feedback,
  onDismiss,
}: {
  feedback: Feedback | null;
  onDismiss: () => void;
}) {
  if (!feedback) return null;
  return (
    <div
      data-slot="page-sticky-feedback"
      className="sticky top-0 z-10 -mx-2 border-b border-border/60 bg-card px-2 py-3"
    >
      <FeedbackLine feedback={feedback} onDismiss={onDismiss} />
    </div>
  );
}

/**
 * 页面反馈状态：一个 `map`（页级用 `PAGE`，行级用行 id）+ 自动收成功的定时器。
 *
 * 行级与页级共用一套实现，是为了让「错误不自动消失」这条规则**只写一次** ——
 * 分成两套很容易只改一边。
 */
export function useFeedback() {
  const [map, setMap] = useState<Record<string, Feedback>>({});
  const timers = useRef(new Map<string, number>());

  const dismiss = useCallback((key: string) => {
    const t = timers.current.get(key);
    if (t !== undefined) {
      window.clearTimeout(t);
      timers.current.delete(key);
    }
    setMap((cur) => {
      if (!(key in cur)) return cur;
      const next = { ...cur };
      delete next[key];
      return next;
    });
  }, []);

  const show = useCallback(
    (key: string, kind: FeedbackKind, text: string) => {
      const t = timers.current.get(key);
      if (t !== undefined) {
        window.clearTimeout(t);
        timers.current.delete(key);
      }
      setMap((cur) => ({ ...cur, [key]: { kind, text } }));
      // 成功类自动收；错误留着等用户关（或下一次同 key 的反馈顶掉它）
      if (kind === "ok") {
        timers.current.set(
          key,
          window.setTimeout(() => dismiss(key), OK_TTL_MS),
        );
      }
    },
    [dismiss],
  );

  // 卸载时清掉所有待触发的定时器（StrictMode 下会挂载两次，留着会对着已卸载的
  // 组件 setState）
  useEffect(() => {
    const t = timers.current;
    return () => {
      for (const id of t.values()) window.clearTimeout(id);
      t.clear();
    };
  }, []);

  return useMemo(
    () => ({
      /** 页级反馈（吸顶） */
      page: map[PAGE] ?? null,
      pageOk: (text: string) => show(PAGE, "ok", text),
      pageErr: (text: string) => show(PAGE, "err", text),
      clearPage: () => dismiss(PAGE),
      /** 行级反馈（落在该行内）；key 用行 id */
      row: (id: string) => map[id] ?? null,
      rowOk: (id: string, text: string) => show(id, "ok", text),
      rowErr: (id: string, text: string) => show(id, "err", text),
      dismiss,
    }),
    [map, show, dismiss],
  );
}
