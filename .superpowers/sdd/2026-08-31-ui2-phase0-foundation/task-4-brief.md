## Task 4: sonner 替换 CustomEvent toast

**Files:**
- Modify: `ui/src/lib/toast.ts`、`ui/src/main.tsx`（挂 Toaster）、`ui/src/App.tsx`（删 toast 状态/监听/UI）

**Interfaces:**
- Consumes: Task 3 的 `components/ui/sonner.tsx` 与 `next-themes`
- Produces: `toast(msg, kind)` 签名不变（22 处调用点零改动），底层改为 sonner；`<Toaster position="bottom-center" richColors />` 挂载在 `main.tsx`

- [ ] **Step 1: 重写 lib/toast.ts**

`ui/src/lib/toast.ts` 全文替换为:

```ts
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
```

- [ ] **Step 2: main.tsx 挂 Toaster**

`ui/src/main.tsx` 中 `</ThemeProvider>` 前加一行:

```tsx
import { Toaster } from "@/components/ui/sonner";
```

```tsx
    </ThemeProvider>
    <Toaster position="bottom-center" richColors />
```

- [ ] **Step 3: 清掉 App.tsx 旧 toast 机制**

`ui/src/App.tsx`：
1. 删除 `toastMsg` state、`onToast` 监听及其 useEffect 逻辑、底部 `{toastMsg && ...}` JSX
2. 删除 `import type { ToastKind } from "./lib/toast"`（不再使用）
3. 保留 `jai-goto-tab` 监听（Task 5 处理）

- [ ] **Step 4: 构建 + 冒烟**

```bash
pnpm --dir ui build
```

Expected: 零错误。`pnpm --dir ui dev` 后触发任意 toast（如点「复制」）确认：右下/底部出现 sonner 成功态（绿色）与失败态（红色）通知，位置在底部居中。

- [ ] **Step 5: Commit**

```bash
git add ui/src/lib/toast.ts ui/src/main.tsx ui/src/App.tsx
git commit -m "feat(ui): sonner 替换 CustomEvent toast（签名不变，调用点零改动）"
```

---

