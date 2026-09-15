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
    </ThemeProvider>
    {/* position 用 bottom-right：bottom-center 会盖住视口底部中央的表格行/表单按钮。
        pointer-events:none 让 toast 只做视觉反馈、永不吞点击（容器 ol 与单条 li 都设）；
        若将来要加可点击的 toast action，需单独把该条设为 pointerEvents:auto，
        并确保它不覆盖页面主操作区。 */}
    <Toaster
      position="bottom-right"
      richColors
      visibleToasts={2}
      style={{ pointerEvents: "none" }}
      toastOptions={{ duration: 2400, style: { pointerEvents: "none" } }}
    />
  </React.StrictMode>,
);
