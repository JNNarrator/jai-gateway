import React from "react";
import ReactDOM from "react-dom/client";
import { ThemeProvider } from "next-themes";
import { Toaster } from "@/components/ui/sonner";
import { NavProvider } from "./lib/nav";
import App from "./App";
import "./index.css";

ReactDOM.createRoot(document.getElementById("root")!).render(
  <React.StrictMode>
    <ThemeProvider
      attribute="class"
      defaultTheme="system"
      enableSystem
      disableTransitionOnChange
    >
      <NavProvider>
        <App />
      </NavProvider>
      {/* Toaster 必须留在 ThemeProvider **内部**：它在 Provider 外时 useTheme() 拿不到主题，
          sonner 会一直用「浅色/系统」调色板（richColors 的成功底色是浅绿），
          而下方的 toastOptions 把文字色写死为 var(--foreground)——暗色主题下就是
          「近白字 + 浅绿底」= 对比度 1.01，成功提示完全不可读（视觉回归审计实测）。
          位置用 bottom-right：bottom-center 会盖住视口底部中央的表格行/表单按钮。
          pointer-events:none 让 toast 只做视觉反馈、永不吞点击（容器 ol 与单条 li 都设）。
          **例外**：错误 toast 不自动消失、需要一个关闭按钮，故在 index.css 里单独把
          `[data-close-button]` 恢复成 pointer-events:auto（只有那个 20×20 的小圆钮可点，
          其余区域依旧穿透，不会挡住页面主操作区）。 */}
      <Toaster
        position="bottom-right"
        richColors
        visibleToasts={2}
        style={{ pointerEvents: "none" }}
        toastOptions={{
          duration: 2400,
          // 只做视觉反馈：不吞点击；文字用 --foreground，避免 richColors 彩色字低于 AA
          style: { pointerEvents: "none", color: "var(--foreground)" },
        }}
      />
    </ThemeProvider>
  </React.StrictMode>,
);
