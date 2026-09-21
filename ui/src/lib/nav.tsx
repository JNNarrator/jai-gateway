import {
  createContext,
  useCallback,
  useContext,
  useEffect,
  useMemo,
  useState,
  type ReactNode,
} from "react";
import { requestLeave } from "./dirty";

export type Tab =
  | "gateway" | "sync" | "mcp" | "skills" | "providers"
  | "models" | "stats" | "logs" | "settings";

const NavContext = createContext<{ tab: Tab; setTab: (t: Tab) => void } | null>(null);

let goTabFn: ((t: Tab) => void) | null = null;

// 页面内既有调用点继续使用（Task 1 起未变）
export function goTab(tab: Tab) {
  goTabFn?.(tab);
}

export function NavProvider({ children }: { children: ReactNode }) {
  const [tab, setTabState] = useState<Tab>("gateway");
  // 切页统一走 requestLeave：有未保存的改动时先弹确认框，用户确认「放弃改动并离开」
  // 才真正执行 setTabState。goTab（页面内跳转）复用同一个 setTab，故一并被覆盖。
  const setTab = useCallback((t: Tab) => {
    requestLeave(() => setTabState(t));
  }, []);
  const value = useMemo(() => ({ tab, setTab }), [tab, setTab]);

  useEffect(() => {
    goTabFn = setTab;
    return () => {
      goTabFn = null;
    };
  }, [setTab]);

  return <NavContext.Provider value={value}>{children}</NavContext.Provider>;
}

export function useNav() {
  const ctx = useContext(NavContext);
  if (!ctx) throw new Error("useNav 必须在 NavProvider 内使用");
  return ctx;
}
