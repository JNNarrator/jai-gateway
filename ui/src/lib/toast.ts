// 全局 toast（阶段 0 临时用 CustomEvent，Task 4 换 sonner 实现，签名不变）
export type ToastKind = "ok" | "err";

export function toast(msg: string, kind: ToastKind = "ok") {
  window.dispatchEvent(new CustomEvent("jai-toast", { detail: { msg, kind } }));
}
