import { toast as sonnerToast } from "sonner";

export type ToastKind = "ok" | "err";

// 统一封装层：阶段 2–5 调整样式只改这一处
export function toast(msg: string, kind: ToastKind = "ok") {
  if (kind === "err") {
    sonnerToast.error(msg);
  } else {
    sonnerToast.success(msg);
  }
}
