import { SidebarNav } from "./components/layout/SidebarNav";
import { TitleBar } from "./components/layout/TitleBar";
import { CommandPalette } from "./components/common/CommandPalette";
import { UnsavedGuard } from "./components/common/UnsavedGuard";
import { useNav } from "./lib/nav";
import { GatewayPage } from "./pages/GatewayPage";
import { SyncPage } from "./pages/SyncPage";
import { McpPage } from "./pages/McpPage";
import { SkillsPage } from "./pages/SkillsPage";
import { ProvidersPage } from "./pages/ProvidersPage";
import { ModelsPage } from "./pages/ModelsPage";
import { StatsPage } from "./pages/StatsPage";
import { LogsPage } from "./pages/LogsPage";
import { SettingsPage } from "./pages/SettingsPage";

export default function App() {
  const { tab } = useNav();
  return (
    <div className="flex h-screen flex-col bg-background text-foreground">
      <CommandPalette />
      {/* 未保存改动的全局拦截（切页确认框 + beforeunload），必须常驻、只挂一次 */}
      <UnsavedGuard />
      <TitleBar />
      <div className="flex min-h-0 flex-1">
        <SidebarNav />
        {/* 顶部渐隐遮罩（P3：内容滚过顶部时是硬边）。放在 main **外面**的包裹层上做
            绝对定位：sticky 方案实测贴不到真正的裁切边（滚动容器的 padding 也在可滚动区内，
            内容滚上来后被裁切的是 main 的边框盒顶边，sticky 会低 24px、露出未渐隐的硬边）。
            三个必须点：
            ① `pointer-events-none` —— 否则这条 24px 透明带会吞掉顶部区域的点击
               （历史 P0-3「toast 遮挡导致点不到」就是同类问题）；
            ② 包裹层只加 `relative`、不加 z-index —— 保持 main 内吸顶条（z-10）与
               本遮罩（z-5）处于同一层叠上下文，吸顶条才能盖住遮罩带、
               按钮文字不被渐变冲淡（网关/同步/设置三页有吸顶条）；
            ③ `absolute`（而非占位元素）→ **不占布局高度**，各页高度/折叠线数据不变。 */}
        <div className="relative flex min-w-0 flex-1 flex-col">
          <main className="min-w-0 flex-1 overflow-y-auto p-6">
            {tab === "gateway" && <GatewayPage />}
            {tab === "sync" && <SyncPage />}
            {tab === "mcp" && <McpPage />}
            {tab === "skills" && <SkillsPage />}
            {tab === "providers" && <ProvidersPage />}
            {tab === "models" && <ModelsPage />}
            {tab === "stats" && <StatsPage />}
            {tab === "logs" && <LogsPage />}
            {tab === "settings" && <SettingsPage />}
          </main>
          <div
            aria-hidden
            className="pointer-events-none absolute inset-x-0 top-0 z-[5] h-6 bg-linear-to-b from-background to-transparent"
          />
        </div>
      </div>
    </div>
  );
}
