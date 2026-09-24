import { toast as sonnerToast } from "sonner";

export type ToastKind = "ok" | "err";

/**
 * 统一封装层：调整 toast 语义只改这一处。
 *
 * 两种反馈的时长**刻意不同**（2026-09-24）：
 * - 成功 / 确认类：2.4s 自动消失 —— 它只回答「刚才那下点到了」，看过了就不需要留着；
 * - **错误：不自动消失，且带关闭按钮** —— 上游报错原文往往是一大段 JSON/HTML，
 *   2.4s 根本看不完；而用户去查问题（甚至截图）时它已经没了。
 *
 * 注意：`duration: Infinity` 是 sonner 明确支持的写法（它内部专门判了
 * `toast.duration === Infinity` 并跳过 `setTimeout` —— 因为 `setTimeout(fn, Infinity)`
 * 在浏览器里会因延迟溢出被当成 0，即立刻触发）。
 */
export function toast(msg: string, kind: ToastKind = "ok") {
  if (kind === "err") {
    sonnerToast.error(msg, { duration: Infinity, closeButton: true });
  } else {
    sonnerToast.success(msg);
  }
}
