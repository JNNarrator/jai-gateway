import { useEffect, useState } from "react";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { Minus, Square, X, Copy } from "lucide-react";
import { cn } from "@/lib/utils";

/** 自绘标题栏（阶段 6）：整条为拖拽区（双击最大化），右侧窗口操作按钮。
 *  macOS 走 titleBarStyle overlay（原生红绿灯），此处为其预留左侧安全区。 */
export function TitleBar() {
  const [darwin, setDarwin] = useState(false);
  const [maximized, setMaximized] = useState(false);
  const appWindow = getCurrentWindow();

  useEffect(() => {
    setDarwin(navigator.userAgent.includes("Mac OS X"));
    let unlisten: (() => void) | undefined;
    appWindow.onResized((e) => {
      // payload 为物理尺寸；无法直接区分最大化，改查 isMaximized
      appWindow.isMaximized().then(setMaximized);
      void e;
    });
    appWindow.isMaximized().then(setMaximized);
    return () => unlisten?.();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  return (
    <header
      data-tauri-drag-region
      className={cn(
        "flex h-9 shrink-0 select-none items-center border-b border-border/60 bg-card/60 backdrop-blur-sm",
        darwin ? "pl-20" : "pl-3",
      )}
    >
      <div className="flex items-center gap-1.5" data-tauri-drag-region>
        <span className="text-xs font-semibold tracking-wide text-muted-foreground">
          JAI Gateway
        </span>
      </div>
      {!darwin && (
        <div className="ml-auto flex h-full">
          <button
            className="flex h-full w-11 items-center justify-center text-muted-foreground hover:bg-muted hover:text-foreground"
            onClick={() => appWindow.minimize()}
            aria-label="最小化"
          >
            <Minus className="size-3.5" />
          </button>
          <button
            className="flex h-full w-11 items-center justify-center text-muted-foreground hover:bg-muted hover:text-foreground"
            onClick={() => appWindow.toggleMaximize()}
            aria-label={maximized ? "还原" : "最大化"}
          >
            {maximized ? <Copy className="size-3" /> : <Square className="size-3" />}
          </button>
          <button
            className="flex h-full w-11 items-center justify-center text-muted-foreground hover:bg-destructive hover:text-white"
            onClick={() => appWindow.close()}
            aria-label="关闭"
          >
            <X className="size-3.5" />
          </button>
        </div>
      )}
    </header>
  );
}
