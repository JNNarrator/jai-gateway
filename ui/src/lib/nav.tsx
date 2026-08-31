import {
  createContext,
  useCallback,
  useContext,
  useEffect,
  useMemo,
  useState,
  type ReactNode,
} from "react";

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
  const setTab = useCallback((t: Tab) => setTabState(t), []);
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
