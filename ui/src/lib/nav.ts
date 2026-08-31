// 跨页跳转（阶段 0 临时用 CustomEvent，Task 5 换 NavContext，调用点不变）
export type Tab =
  | "gateway" | "sync" | "mcp" | "skills" | "providers"
  | "models" | "stats" | "logs" | "settings";

export function goTab(tab: Tab) {
  window.dispatchEvent(new CustomEvent("jai-goto-tab", { detail: { tab } }));
}
