## Task 5: 导航 CustomEvent 换 NavContext

**Files:**
- Modify: `ui/src/lib/nav.ts`（→ `nav.tsx`，加 Context）、`ui/src/main.tsx`（包 NavProvider）、`ui/src/App.tsx`（用 useNav 取代本地 state 与监听器）

**Interfaces:**
- Consumes: Task 1 的 `goTab(tab)` 调用点（页面内 2 处，保持不变）
- Produces: `NavProvider`、`useNav(): { tab: Tab; setTab: (t: Tab) => void }`；`goTab(tab)` 继续可用（内部改走 provider 注册的 setter）

- [ ] **Step 1: 重写 lib/nav.ts → lib/nav.tsx**

`ui/src/lib/nav.ts` 删除，新建 `ui/src/lib/nav.tsx`:

```tsx
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
```

- [ ] **Step 2: main.tsx 包 NavProvider**

`ui/src/main.tsx`：import `NavProvider` 并包住 `<App />`:

```tsx
import { NavProvider } from "./lib/nav";
```

```tsx
    <ThemeProvider ...>
      <NavProvider>
        <App />
      </NavProvider>
    </ThemeProvider>
    <Toaster position="bottom-center" richColors />
```

- [ ] **Step 3: App.tsx 改用 useNav**

`ui/src/App.tsx`：
1. 删除本地 `const [tab, setTab] = useState<Tab>("gateway")` 与 `jai-goto-tab` 监听及其 useEffect
2. 改为:

```tsx
import { useNav } from "./lib/nav";

export default function App() {
  const { tab, setTab } = useNav();
  ...
}
```

（`tab === k && <XxxPage />` 渲染与 `tabs` 数组不变；`setTab` 来自 useNav。）

- [ ] **Step 4: 构建 + 冒烟**

```bash
pnpm --dir ui build
```

Expected: 零错误。dev 下验证跨页跳转：SyncPage 内「去网关导出」类链接点击后能切到网关页（`goTab` 调用点仍工作）。

- [ ] **Step 5: Commit**

```bash
git add ui/src/lib ui/src/main.tsx ui/src/App.tsx
git commit -m "refactor(ui): 导航 CustomEvent 换 NavContext（goTab 调用点不变）"
```

---

