# JAI 视觉回归整改 plan（默认窗口 980×640）

> 触发来源：默认窗口大小下「有些常规操作已不在可视/可点击范围内」。
> 本次对 9 个页面 + 全部弹窗/菜单做了动态点击遍历（303 步）与几何测量，共 715 张截图留证，问题按 P0→P3 排序，每条给出证据数据、根因代码位置、修复方案与验收标准。
>
> 关联文档：`docs/ui优化.md`（历史 66 项）、`docs/bug和优化清单.md`（唯一问题追踪入口，本次已在其中登记）。

---

## 0. 结论摘要

在默认 980×640 下，**没有一个页面能在一屏内完成主要操作**，且存在 3 类"确实点不到"的硬伤：

| 级别 | 问题 | 一句话 |
|---|---|---|
| P0-1 | 供应商「添加/编辑」弹窗主按钮不可见 | 弹窗内容 620–780px 塞进 542px 高的可滚区域，`创建/保存/测试连接/取消` 打开时就在可视框之外；用户必须先在弹窗内滚动才能提交 |
| P0-2 | 弹窗基座没有高度约束 | 基座 `DialogContent` 无 `max-h`/`overflow`，只有供应商弹窗被单独打了补丁；内容一长（长技能正文、MCP env 行、粘贴大 JSON）就整体溢出视口且**无法滚动** |
| P0-3 | toast 覆盖底部控件 | `bottom-center` 的 toaster（z-index 999999999）落在视口底部中央 54px 带，303 步遍历中触发 **150 次"被遮挡且点不到"**（含弹窗内输入框、`检查更新`、`测试连接`） |
| P1-4 | 主操作在首屏之外 | 同步页首屏仅见 38%（22 个控件在折叠线下，`保存配置/测试连接/预览变更/推送/拉取` 全在 y=761）；日志 21%、设置 34%、模型 36%、网关 55% |
| P1-5 | 模型页表格横向溢出 | 7 列 801px 宽表在 760px 窗口下溢出 217px，且行内输入被折叠线切一半（9–14px） |
| P1-6 | 日志页超长 | 内容 2931px，`加载更多` 在 y=2870，表头无 sticky |
| P2-7 | 暗色主题主按钮对比度 2.59:1 | `--primary` 亮青蓝 #00A6F4 + 白字，AA 要求 4.5:1；禁用态更低 |
| P2-8 | 浅色主题小字/状态色对比度不足 | 79 类，最差 badge「缺少凭据」3.2:1、日志状态码「429/499」3.38:1 |
| P2-9 | 命中区过小 | Switch 32×18、弹窗关闭 16×16、行内复制 16×16、批量勾选 16×16、弹窗内协议 `select` 1×1 |
| P2-10 | 10px 字号 + 文本截断 | 2 处 10px；MCP 注册 URL 截 22px、供应商 base URL 截 21px |
| P3-11 | 折叠线硬切 | 卡片被折叠线切半（`客户端接入` 398–660、WebDAV 348–1617），顶部无渐隐遮罩 |
| P3-12 | 默认窗口偏小 | 内容普遍需要 1100–1900px 高，`minHeight=520` 时弹窗必然溢出 |

---

## 1. 方法与证据（含局限）

**可复现命令**（chromium 无头 + mock `__TAURI_INTERNALS__`，用真实长度的中文 fixture 数据渲染全部页面）：

```bash
cd ui && nohup npx vite --host 127.0.0.1 --port 5173 > /tmp/vite.log 2>&1 &   # 起前端
export TMPDIR=$PWD/.vr/tmp
node .vr/run.mjs    --mode=walk --size=980x640     # 动态点击遍历 303 步 → .vr/out-walk-980x640.json
node .vr/fold.mjs   --size=980x640                 # 折叠线/横向溢出/弹窗 Esc 可关闭性 → .vr/fold-980x640.json
node .vr/deep2.mjs  --size=980x640                 # 弹窗结构解剖（滚动容器、footer、主按钮可见性）
node .vr/audit.mjs  --size=980x640 --theme=dark     # 对比度/字号/命中区/截断（浅色把 theme 换成 light）
node .vr/audit-summarize.mjs .vr/audit-dark-980x640.json
```

**测量手段**：`getBoundingClientRect` 几何 + `elementFromPoint` 遮挡命中测试 + 视口外可达性判定（沿祖先找可滚动容器，遇 `position:fixed` 即判定不可达）+ WCAG 相对亮度对比度（自实现 oklch/oklab→sRGB 换算，已用 `oklch(0.985 0 0)=#fafafa`、`oklch(0.145 0 0)=#0a0a0a` 等已知值校验）+ 超长内容注入（长技能正文、+4 行表单项）+ 亮/暗双主题。

**局限（必须声明）**：本机模型不具备图像输入能力（`read_image` 返回 "model does not declare image input"），因此**本次没有做人工目视判读**，结论全部来自上述可量化测量 + 715 张截图留证。这可能漏掉纯观感类问题（配色品味、对齐误差、间距节奏、动效异常）。建议交付前由人或具备视觉能力的模型再过一遍 `.vr/shots/` 中的关键截图（推荐顺序：`fold-*-sync.png`、`fold-*-models.png`、`deep2-980x640-providers-*`、`walk-980x640-155*`）。

---

## 2. 问题清单与整改

### P0-1 供应商「添加/编辑」弹窗：主操作按钮打开即不可见

- **现象**：打开弹窗后底部 `创建/保存/测试连接/取消` 直接看不到，需要先在弹窗里向下滚动；滚动时标题和关闭按钮又被滚走。
- **证据**（`.vr/deep2-980x640.log`）：
  - `980×640`：`添加供应商` 弹窗 rect `48→592`(h=544)，`maxHeight=544px`、`overflowY=auto`，滚动容器就是 `dialog-content` 本身（内容 620 > 可视 542）；footer 在 `609–645`，**全部主按钮 `可见=false`**。`编辑「基元律动」` 同样：内容 636，footer `625–661`，`保存/取消/添加请求头` 均在可视框外。
  - `760×520`：内容 764–780 > 可视 440，footer `744–796`，主按钮全部不可见。
- **根因**：`ui/src/pages/ProvidersPage.tsx:508`
  ```tsx
  <DialogContent className="max-h-[85vh] overflow-y-auto sm:max-w-lg">
  ```
  把「限高 + 滚动」加在整个 `DialogContent` 上 → 头部与 footer 一起滚。基座 `ui/src/components/ui/dialog.tsx:61` 无任何高度约束，这个补丁是唯一的例外处理。
- **修复**：
  1. 基座改为三段式（见 P0-2 的统一改法）；
  2. `ProvidersPage` 用 `<DialogBody>` 包 `<form>`，footer 移出滚动区；
  3. 让 `保存/创建` 常驻可见（footer `shrink-0` + 顶部细分隔线）。
- **验收**：在 980×640 与 760×520 打开该弹窗，footer 底边 ≤ 视口高 − 8px，`deep2` 报告 `primaryButtons.visible=true`；滚动只发生在正文区，标题与关闭按钮保持可见。

### P0-2 弹窗基座缺高度约束（潜在同类问题）

- **证据**：`McpPage` 添加弹窗在 `760×520` 时 h=528 > 视口 520，上下各被切 4px，且 `overflowY=visible`——**没有内部滚动，切掉的像素永久不可达**；`添加技能` 弹窗 h=462（980）/440（760），余量只有约 60px，而技能正文 `textarea` 可 resize、MCP 的 env/args 行可增删、导入可粘贴大 JSON。
- **根因**：`ui/src/components/ui/dialog.tsx:61` 类名里只有 `grid ... p-6 gap-4`，无 `max-h`；9 个 `DialogContent` 中 8 个未做高度处理。
- **修复**（统一基座，一次改完全部弹窗）：
  ```tsx
  // dialog.tsx
  "fixed top-[50%] left-[50%] z-50 flex max-h-[calc(100dvh-3rem)] w-full max-w-[calc(100%-2rem)]
   translate-x-[-50%] translate-y-[-50%] flex-col gap-0 overflow-hidden rounded-lg border
   bg-background p-0 shadow-lg sm:max-w-lg"
  // 新增 DialogBody：(flex-1 overflow-y-auto p-6 pt-0)
  // DialogHeader: shrink-0 px-6 pt-6 ;  DialogFooter: shrink-0 px-6 pb-6 pt-4 border-t bg-background
  ```
  移除 `ProvidersPage.tsx:508` 的 `max-h-[85vh] overflow-y-auto`；`CommandPalette` 的 `top-[18%]` 与 `SyncPage` 的 `max-h-72` 保持。
- **验收**：`audit.mjs` 的超长内容步骤（`dlg!`/`dlg+`）在 760×520、980×640 下对 9 个弹窗全部 `overTop=0, overBottom=0, footer.visible=true, 正文可滚`。

### P0-3 toast 覆盖页面底部控件与弹窗 footer

- **现象**：任何一次操作（复制/保存/导出）弹出底部居中 toast 后，被它盖住的控件在 4 秒内点不到——包括弹窗内的输入框。
- **证据**：
  - `out-walk-980x640.json`：303 个探测步骤中 **150 次**登记"被遮挡且 hit-test 落到别人身上"，其中 149 次盖住者是 sonner 的 `<li>`（`已复制` 144 次、`已导出 JSON` 3 次、`无法读取剪贴板` 2 次、`端口已保存`/`当前已是最新版本`/`已导出 CSV` 各 1 次）。
  - 被遮对象：弹窗内「发给上游时使用的真实模型 ID」(69)、上下文窗口 `128000/256000/512000/1000000`(69)、`代理绕过列表` textarea(4)、`检查更新`(3)、`测试连接`(1)。
  - 几何：toast rect `x 312→668 / y 565→619`（w=356, h=54），距底 24px，`z-index: 999999999`；供应商弹窗 footer 在 `609–645` → 与 toast 带重叠。
- **根因**：`ui/src/main.tsx:21` `<Toaster position="bottom-center" richColors />` + sonner 默认极高 z-index。
- **修复**：改 `position="bottom-right"`（macOS 通知位置，且避开表格中轴）；`toastOptions={{ duration: 2000, style: { zIndex: 40 } }}`（低于弹窗 z=50，避免盖住弹窗 footer）；`expand={false}`、`visibleToasts={1}` 减少堆叠。
- **验收**：复跑 `walk` 后 toast 遮挡计数 = 0；手动验证：在「编辑供应商」弹窗里点复制，弹出 toast 后 footer 的 `保存` 仍可点击。

### P1-4 主操作在首屏之外（用户诉求本体的最大头）

- **数据**（`.vr/fold-980x640.log` / `fold-760x520.log`，`首屏可见% / 折叠线下控件数`）：

| 页面 | 980×640 | 760×520 | 折叠线下的关键操作（980×640，y 坐标） |
|---|---|---|---|
| 网关 | 55% / 1 | 42% | `复制配置` y=797 |
| 同步 | 38% / 22 | 28% | `保存配置`·`测试连接`·`预览变更`·`推送`·`拉取` y=761；`自动推送` y=846；`自动拉取` y=917；`从快照恢复` y=1126；`刷新` y=1200 |
| MCP | 100% / 0 | 79% | — |
| 技能 | 100% / 0 | 100% | — |
| 供应商 | 82% / 0 | 39% | 760 下 `编辑/删除/测试/拉取模型` y=842 |
| 模型 | 36% / 105 | 27% | 行内输入与 `保存` 被折叠线切 9–14px |
| 统计 | 93% / 0 | 63% | — |
| 日志 | 21% / 1 | 16% | `加载更多` y=2870 |
| 设置 | 34% / 8 | 26% | `日志记录开关` y=698；`编辑保留策略` y=769；`保存` y=1115；`添加域名` y=1117；`检查更新` y=1749 |

- **根因**：`ui/src/App.tsx:23` 全应用只有一个滚动容器 `<main className="min-w-0 flex-1 overflow-y-auto p-6">`，页面没有常驻操作区；WebDAV/代理/日志等长表单平铺直叙，主操作落在表单末尾。
- **修复（按性价比排序）**：
  1. **页面操作条 sticky**：页头 `sticky top-0 z-10 -mx-6 -mt-6 mb-4 border-b bg-background/95 px-6 py-3 backdrop-blur`，把该页主操作（保存配置/测试连接/预览变更/推送/拉取；日志：刷新/清空/自动刷新；设置：保存）搬进去；
  2. **同步页**重构为「当前状态摘要卡 + 展开编辑」：默认只显示 WebDAV 地址/账号/同步开关与三个主按钮，`高级（自动推送/间隔/备份列表/快照恢复）` 折叠；
  3. **设置页**按卡片加锚点导航（端口/日志/代理/来源/更新），每张卡片的保存按钮随卡片走；
  4. macOS 下滚动条是 overlay 不可见，建议 `main` 常驻细滚动条或页面顶部加内容高度指示，避免用户以为"页面就这些"。
- **验收**：`fold.mjs` 输出中 9 个页面的 `foldPct ≤ 25%`，且主操作按钮 `visible=true`；740 高度的默认窗口下同样成立。

### P1-5 模型页表格横向溢出与被切行

- **数据**：7 列表格宽 801px；980 窗口容器 804（勉强）；**760 窗口容器 584 → 溢出 217px**（`fold-760x520.log`），需横向滚动而没有任何提示；行内 `同模型名`/`INPUT`/`保存` 被折叠线切 9–14px；行内复制按钮仅 16×16。
- **修复**：<960px 时列降级（`协议/上下文/最大输出` 合并进「详情」弹窗或 tooltip）；`表格外层 overflow-x-auto` + 右缘渐隐提示；行高固定并 `sticky thead`；行内输入改 `w-20` 并右对齐数字。
- **验收**：760×520 下 `hScroll = 0`（或出现可见横向滚动提示），无被切半的行。

### P1-6 日志页超长与表头丢失

- **数据**：内容 2931px / 首屏 21%；8 列宽 754（980）/595（760，溢出 11px）；`加载更多（当前显示 80 条）` 在 y=2870；表头随页面滚走。
- **修复**：日志表放进 `max-h-[calc(100dvh-16rem)] overflow-auto` 容器 + `thead sticky top-0 bg-background`；列优先级（时间/模型/状态/耗时 常显，其余收进行展开）；`加载更多` 改为容器内分页。
- **验收**：日志页整页高度 ≤ 视口，表头常驻，滚动发生在表格内部。

### P2-7 暗色主题主按钮对比度 2.59:1（AA 不达标）

- **证据**（`.vr/probe-colors3.mjs`，暗色强制）：
  ```
  --primary: oklch(0.685 0.169 237.323) → rgb(0,166,244)
  --primary-foreground: oklch(0.985 0 0) → #FAFAFA
  对比度 = 2.59:1   （AA 小字要求 4.5:1；禁用态 opacity .5 更低）
  ```
  受影响：`保存/创建/导入/恢复/删除/添加/测试连接` 等全部 primary 按钮（`audit-dark-980x640.json` 共 17 类，含弹窗内 footer 按钮）。浅色主题同按钮为 5.03:1，合格。
- **修复**：暗色下 `--primary-foreground` 改为深色（`oklch(0.145 0 0)` → 约 8:1），或把 `--primary` 调深到 `oklch(0.55 0.19 250)` 一级；禁用态单独给 `disabled:opacity-60` + `disabled:text-foreground/70`，避免半透明叠加后崩到 1.6:1。
- **验收**：暗色主题下所有按钮文本对比度 ≥ 4.5（禁用态 ≥ 3.0）。

### P2-8 浅色主题小字/状态色对比度不足

- **证据**（`audit-light-980x640.json`，共 79 类，按最差排序）：
  - badge「缺少凭据」**3.2**、「最近成功」3.65（12px）；
  - 日志表状态码「429/499」**3.38**、「200」3.54、「500」/`rate_limited`/`upstream_5xx` 4.24（12px）；
  - 同步页「成功」3.55（`text-emerald-600`）；
  - 设置页说明文字 3.65（12px）、「✓ 已是最新版本」3.65；
  - toast 文案 4.26（13px）。
- **修复**：状态色统一提级（`emerald-600→emerald-700`、`amber-600→amber-700`、`rose-600→rose-700`），badge 文本用 `--foreground`，说明文字改用 `text-foreground/80`，toast 文本色改 `--foreground`。
- **验收**：浅色主题下 audit 的 `contrast` 计数 ≤ 少数可解释项，且全部 ≥ 4.5。

### P2-9 命中区过小

- **数据**（`audit-*-980x640.json`，14 类）：
  `Switch 32×18`（同步自动推送、MCP 启用/允许代理、供应商启用、模型启用、设置网络代理/自动刷新）、`弹窗关闭 16×16`、`批量勾选 checkbox 16×16`、`复制模型名 16×16`、`供应商「官网」40×16`、`「显示」40×24`、**弹窗内协议 `select` 1×1**（当前几乎不可点）。
- **修复**：交互控件命中区统一 ≥ 24×24（`after:absolute after:-inset-1` 扩热区或 `py-1.5`）；图标按钮 `size-6` + `aria-label`；`select` 用 `className="sr-only"` 配自定义触发器，或改回原生 `Select`。
- **验收**：audit `smallTargets` 中除 `sr-only` 元素外为 0。

### P2-10 字号与文本截断

- **数据**：10px 两处（同步「当前」标签、模型徽标「文本」）；截断：MCP 注册 URL 少 22px、供应商 base URL 少 21px、日志模型单元格 `max-w-48 truncate`。
- **修复**：最小字号提到 11px；截断元素补 `title`（或行内复制按钮），base URL 允许两行 `break-all`。
- **验收**：`tiny` 计数 = 0，且截断项均有可发现全文的手段。

### P3-11 折叠线硬切与顶部遮挡

- **数据**：卡片被折叠线硬切（网关「客户端接入」`398–660`、同步「WebDAV 同步」`348–1617`、统计「每日 Token 用量」`254–660`）；内容滚到顶时被标题栏切开 3 次（`main p-6` 顶边无渐隐）。
- **修复**：折叠线上方加 8px 渐隐（`bg-gradient-to-b`）或卡片 `break-inside-avoid`；重要卡片整块入首屏，避免"半张卡"。

### P3-12 默认窗口尺寸与滚动条可见性

- **现状**：`src-tauri/tauri.conf.json` → `width 980 / height 640 / minWidth 760 / minHeight 520`，而内容普遍需要 1100–1900px 高（模型 1682、设置 1794、日志 2931），520 高度下**所有超 520px 的弹窗必然溢出**。
- **建议**：默认改为 `1080×740`，`minHeight` 提到 `560`；启动时按屏幕可用高度取 `min(available*0.85, 1080)`；配合 P0-2 的弹窗限高，保证"任何窗口尺寸下弹窗都自洽"。

---

## 3. 实施顺序与批次

| 批次 | 内容 | 说明 |
|---|---|---|
| 第 1 批（半天） | P0-1 + P0-2 | 弹窗基座三件套（Header/Body/Footer）+ 供应商弹窗改造，**收益最大、改动集中** |
| 第 2 批（1 小时） | P0-3 | Toaster 位置与 z-index 两行改动 |
| 第 3 批（1 天） | P1-4 | 同步/设置/日志页 sticky 操作条与折叠分区（用户抱怨的主要来源） |
| 第 4 批（半天） | P1-5 + P1-6 | 模型表列降级、日志表内滚 + sticky 表头 |
| 第 5 批（半天） | P2-7 + P2-8 + P2-9 + P2-10 | 主题 token 与尺寸类全局调整，配 audit 复测 |
| 第 6 批（可选） | P3-11 + P3-12 | 渐隐遮罩、默认窗口尺寸 |

---

## 4. 验收与回归

1. `node .vr/fold.mjs --size=980x640` 与 `--size=760x520`：9 页 `foldPct ≤ 25%`、主操作首屏可见、无横向溢出、弹窗 Esc 可关且关后可导航；
2. `node .vr/deep2.mjs --size=980x640` 与 `--size=760x520`：所有弹窗 `overTop/overBottom = 0`、`footer.visible = true`、`primaryButtons` 全可见（不再出现"需内部滚动"）；
3. `node .vr/run.mjs --mode=walk --size=980x640`：`covered` 中 `hitBy.tag==='li'`（toast）计数 = 0，`unreachable` = 0；
4. `node .vr/audit.mjs --size=980x640 --theme=dark` 与 `--theme=light`：`contrast`、`tiny`、`smallTargets` 全部收敛到 0（或仅剩可解释项）；
5. 人工（或具备视觉能力的模型）复核 `.vr/shots/` 关键截图 —— 见第 1 节"局限"。

---

## 5. 证据文件索引

| 文件 | 内容 |
|---|---|
| `.vr/out-walk-980x640.json` | 303 步动态点击遍历原始探针数据 |
| `.vr/walk-980.log` | 遍历过程日志（每步 out/covered/dlg/menus/BAD） |
| `.vr/fold-980x640.json` / `fold-760x520.json` | 各页折叠线、可见率、横向溢出、弹窗 Esc 可关闭性 |
| `.vr/deep2-980x640.json` / `deep2-760x520.json` | 弹窗结构解剖（滚动容器、footer、主按钮可见性） |
| `.vr/audit-dark-980x640.json` / `audit-light-980x640.json` / `audit-dark-760x520.json` | 对比度/字号/命中区/截断/半行截断 |
| `.vr/shots/*.png` | 715 张截图（`walk-`、`fold-`、`deep2-`、`audit-` 前缀） |
| `.vr/*.mjs` | 可复现脚本（run / fold / deep2 / audit / summarize） |

---

## 6. 进度记录（交接用）

> 本次会话到此，换机后从这里接着做。探针脚本随仓库提交（`.vr/*.mjs`），体积大的证据（截图/JSON/日志）未纳入 git。

### 已完成并验证

| 项 | 提交 | 验证证据 |
|---|---|---|
| P0-1 供应商弹窗主按钮可见 | `9ce86c4` | deep2 980×640：footer 546–599 可见，`创建/保存/测试连接` 全可见；760×520：footer 426–479 可见，滚动只发生在 `dialog-body`（980：458>427；760：602>307） |
| P0-2 弹窗基座限高 + `DialogBody`（Mcp/Skills 已接入） | `9ce86c4` | audit 长内容注入（长技能正文 / +4 行表单项）在 980×640 与 760×520 均 `dlgBad=0` |
| P0-3 toast 移右下 + 点击穿透 | `9ce86c4` | walk 遍历：980×640（88 步）、760×520（300 步）toast 遮挡均为 **0** |
| P1-6 日志表内部滚动 + 吸顶表头 | `bc38627` | fold：日志页内容 2931→604，首屏 21%→100%，折叠线下控件 0 |
| P1-4（部分）同步页主操作条上移吸顶 | `bc38627` | fold：`保存配置/测试连接/预览变更/推送/拉取` 不再出现在折叠线下清单 |
| P2-7 暗色主按钮对比度 | 本次提交 | probe-colors3（强制暗色）：**2.59 → 7.32:1**（`--primary-foreground` 改 `oklch(0.145 0 0)`） |

### 待做（按优先级）

- **P1-4 其余页面**：设置页（`保存` y=1115、日志开关 698、编辑保留策略 769、检查更新 1749）、网关页（`复制配置` y=797）——用同法把主操作上移或吸顶，或加卡片锚点导航；设置页首屏 34%、网关 55%。
- **P1-5 模型页表格**：7 列 801px，760 宽窗口横向溢出 217px；行内输入/保存被折叠线切 9–14px。方案：次要列降级进详情、外层 `overflow-x-auto` + 右缘渐隐、sticky thead。
- **P2-8 浅色小字/状态色 79 类**（最差 badge「缺少凭据」3.2、日志状态码 3.38、同步「成功」3.55、设置说明 3.65、toast 文案 4.26）：状态色提级到 -700，badge 文本用 `--foreground`。
- **P2-9 命中区**：Switch 32×18、弹窗关闭 16×16、行内复制 16×16、批量勾选 16×16、弹窗内协议 `select` 1×1 → 统一 ≥24×24 或改自定义触发器。
- **P2-10 字号与截断**：2 处 10px；MCP 注册 URL 截 22px、供应商 base URL 截 21px。
- **P3-11** 折叠线硬切加渐隐；**P3-12** 默认窗口 1080×740 / `minHeight` 560（`src-tauri/tauri.conf.json`）。

### 复现环境（换机后）

1. 前端依赖：`cd ui && npm i`。
2. 探针脚本开头用 `createRequire` 引 Playwright，路径写死在
   `/Users/jiangnan/Documents/workspace/deepseek-harness/node_modules/.pnpm/playwright@1.61.1/node_modules/playwright/`，
   **换机需改成新机器上的 Playwright 路径**（Chrome 可执行文件路径同理）。
3. 起前端：`cd ui && nohup npx vite --host 127.0.0.1 --port 5173 > /tmp/vite.log 2>&1 &`
4. 跑探针：`export TMPDIR=$PWD/.vr/tmp && node .vr/deep2.mjs --size=980x640`
   - `fold.mjs` 折叠线/横向溢出/弹窗 Esc；`deep2.mjs` 弹窗结构解剖；`audit.mjs` 对比度/字号/命中区/截断；`run.mjs` 动态点击遍历（较慢，建议后台跑）；`audit-summarize.mjs` 汇总。
5. 证据：`.vr/*.json`、`.vr/*.log`、`.vr/shots/`（715+ 张截图）；这些未入库，需要时单独拷贝。

### 第二轮（本次会话）新增完成

| 项 | 结果 / 验证 |
|---|---|
| P2-8 浅色主题对比度 | 状态色 600→700 级（16 处/6 文件）、日志状态码 4xx/5xx→700/800（行底纹上更严）、同步提示条与「当前」标签改 `text-foreground`、toast 文本色改 `--foreground`；audit（light, 980×640）不合格类 **79 → 10 → 预计 ≤2 类**（剩余为 4.27 的个别主按钮，待 DevTools 复核） |
| P2-9 命中区 | `switch.tsx` 用 `after:` 伪元素把点击区从 32×18 扩到约 44×36（视觉不变）；`dialog.tsx` 关闭按钮 16×16 → **24×24**（达 WCAG 2.2 AA 最小 24） |
| P2-10 字号 | 2 处 10px → 11px（同步「当前」标签、模型徽标） |
| P1-4（部分）设置页 | 7 张卡片加 `id` + `scroll-mt-16`，页头加**吸顶锚点导航**（端口/代理/日志/跨域/凭据/数据/更新），常规操作一键可达 |
| P1-5 模型表 | 确认 7 列表格由 `Table` 基座自带 `overflow-x-auto` 承载横向滚动（980 下 801/804 勉强放下，760 下溢出 217 属容器内滚动）；补 `max-lg` 可见提示文案，避免"内容被挤出却不知道能滑" |
| 探针增强 | 遮挡分桶细化为 `covered`（真遮）/`clippedEdge`（上边缘裁切）/`clippedInScroller`（需滚动）/`underSticky`（被吸顶条压住），只有 `covered` 计为问题 |

### 仍然待做

- **P1-4 网关页**：`复制配置`（MCP 元数据卡）仍在折叠线下（y=797），可考虑与「客户端接入」合并或上移。
- **P1-4 设置页深化**：锚点已加，但页面仍 1794px 高（首屏 34%）；若要"首屏能看到全部卡片"，需把长卡片改手风琴（默认折叠高级项）。
- **P2-8 剩余**：设置页 `保存`/弹窗 `添加` 实测 4.27:1（浅色主按钮），需在 DevTools 确认真实取色后决定是改 token 还是改按钮样式。
- **P2-9 剩余**：行内复制按钮、批量勾选 16×16、供应商「官网」40×16、「显示」40×24（可按同样伪元素手法逐个扩）；`1×1 select` 经查是 Radix `Select` 的隐藏 bubble select，属误报，无需处理。
- **P2-10 剩余**：MCP 注册 URL 截 22px、供应商 base URL 截 21px → 加 `title` 或允许换行。
- **P3-11** 折叠线渐隐遮罩；**P3-12** 默认窗口 1080×740 / `minHeight` 560。

---

## 7. 第三轮（2026-09-21）：双尺寸验收清零 + UI 规范门禁化

> 触发：用户提出「做一次 UI 优化专题」。先跑现有探针取基线，结论是**本 plan 的 P0–P3
> 在默认窗口 1180×800 下已基本闭环**（双主题对比度 0 类不达标、横向溢出 0、
> 弹窗主按钮全可见、日志/MCP/技能/统计首屏 100%）。剩下的问题分两类：
> ① 最小窗口 900×600 从未被当成一等公民验收；② 一批规范**只写在文档里**，新功能一加就回退。
> 故本轮重点是把规范变成**可执行门禁**。详细条目与实测证据见
> [`docs/bug和优化清单.md`](bug和优化清单.md) §4。

### 7.1 本轮修掉的 8 项

| # | 项 | 结果 |
|---|---|---|
| 1 | 模型页是唯一既无操作区、又无列降级的「最长页」 | 表格改**只读** + 右侧**详情抽屉**（新建 `ui/src/components/ui/sheet.tsx`）+ **吸顶表头**；行内控件 **194 → 68**。顺带发现**日志页的吸顶表头此前从未真正生效**（见 7.2） |
| 2 | 新功能引入 4 处 `text-[10px]`（P2-10 曾清零过一次） | 改 `text-[11px]`，并变成 `ui_lint` 的硬检查 |
| 3 | 命中区「已修」项实测不达标（伪元素热区被相邻元素抢走） | 「复制模型名」有效命中区 **6×30 → ≥24×24**；「官网」**54×18 → ≥24×24**；改用**真实盒子** + `z-10` |
| 4 | 模型页表在 900×600 横向溢出 8px（P1-5 回归） | 随第 1 项解决：表 674/724，溢出 **-50** |
| 5 | 探针自身不可信：`headerBtns` 死字段 + `clippedRows` 噪音 + 路径不可移植 | 见 7.2 |
| 6 | 900×600 下 toast 视觉压住弹窗 footer 主按钮 | `dialog.tsx` 的 `max-h` 改 `calc(100dvh-8rem)`（默认窗口行为不变：672 > 最高弹窗 639） |
| 7 | 无脏状态 guard（改表单不保存就切页/关窗 → 静默丢失） | 新增 `lib/dirty.ts` + `UnsavedGuard.tsx`，供应商/MCP/技能/设置**四处表单**接入 |
| 8 | 日志轮询不随可见性暂停 | `LogsPage` 随 `document.hidden` 停/恢复（不误开用户关掉的开关） |

### 7.2 门禁化（本轮最重要的产出）

| 门禁 | 覆盖 |
|---|---|
| `scripts/ui_lint.sh` | 字号 ≥11px、图标按钮有可访问名、列表 key 不用下标、**命中区不靠伪元素外扩** |
| `tools/visual-regression/probe-hits.mjs` | **有效命中区 ≥24×24**（命中区判据的唯一归属）+ 控件类型覆盖缺口 |
| `tools/visual-regression/gate.mjs` | 双尺寸 × 双主题一把收口，**单一退出码**；判据集中在本文件 |

- 已接进 `scripts/release_check.sh` **第 6 步**（CI 接入留待下一步）。
- **探针可移植**：新增 `_env.mjs` 统一 Playwright/Chrome 定位
  （`PLAYWRIGHT_PATH`/`CHROME_PATH` → `ui/node_modules` → 仓库根 → Playwright 自带 chromium），
  15 个探针全部改为从它取；**本 README 第 1 节「换机需改路径」的前提已删除**。
  `ui` 的 devDependency 增加 `playwright-core`（只驱动系统 Chrome、不下载浏览器）。
- **死字段与噪音清零**：`PageHeader` 补 `data-slot="page-header"`、吸顶条补 `data-slot="page-actions"`
  （都写成探针契约并注释），`fold.mjs` 的「顶部操作区」从此能列出真实主操作，
  并额外断言 `primaryActionCount > 0`；`clippedRows` 不再把滚动容器误报成 `html`（45/102 步 → 0）；
  `audit.mjs` 不再给命中区判定（避免两处结论互相矛盾）。
- **四条负控制已实证**门禁会红（字号 / 命中区 / 表格溢出 / 吸顶表头各一次），见 bug 清单 §4.3 ⑤。
- **门禁自身可信**（本轮末段补）：跑探针前先删掉它的输出 JSON，且**探针退出码非 0 直接判失败**
  （判据「探针可运行」）。曾真实踩到 `fold.mjs` 因一次编辑误删变量 `tables` 而**每次都崩**，
  `gate.mjs` 却读到上次的旧 JSON 并报「✓ 全部通过、退出码 0」。
  `--skip-probes` 保留为「手工重跑单个探针后复算判据」的显式选项。
- **「吸顶表头」这条判据本轮才真正生效**：先前只断言 `getComputedStyle(thead).position === "sticky"`，
  是**恒真**的。改成真实滚动容器 150px 再量 `thead` 位移后实测发现 ——
  **日志页的吸顶表头此前从未真正生效**（根因：`Table` 基座自带
  `div[data-slot=table-container].overflow-x-auto`，其 `overflow-y` 随之计算成 `auto`，
  它是 thead 的最近可滚动祖先且自身无纵向溢出 → `sticky` 完全失效）。
  两页统一改为把 `max-h + overflow:auto` 加到**那一层**；实测滚动 150px 后位移 **0px**（双尺寸）。
  另：我自己新写的那段检查一度被误插进 `if (cutPrimary.length) {…}` 内部（语法合法、语义错），
  导致判据**跑不到** —— 又是一次「恒真的断言」，已移到正确层级并加负控制。
- **两条判据被修订并留档**（原判据不合理）：折叠线 `partialCut` 由「失败」改为「信息」
  （长页面必然有控件跨在视口底边，且那是正常的滚动提示），替代判据是「**主操作**不得跨线」
  +「跨线控件可滚动到达」+「表格不溢出容器」；命中区判据由两处归属收敛为一处。
  理由写在 bug 清单 §4.3 ⑨。

### 7.3 仍待做

- **CI 接入**：`.github/workflows/ci.yml` 加一条 UI 门禁 job（需 `playwright-core` + 系统 Chrome）。
- **`release_check.sh` 第 1 步**（工作区干净）与本地 `node` 版本：本机默认 `node v18.9.0`
  会让 `pnpm build` 因 corepack 报 `URL.canParse is not a function` 而失败（需 ≥18.17），
  建议把 node 版本写进文档或加 `engines`。
- 最小窗口 900×600 下仍有若干页面「首屏可见率」偏低（设置 31% / 同步 34% / 网关 38%）——
  但**主操作都已首屏可达**（吸顶条/锚点导航），故按本 plan 的实质目标已验收；
  若要进一步压缩，可做「摘要卡 + 展开高级项」（原 P1-4 的第 2 项，至今未做）。
- 模型页抽屉的**「关闭抽屉」不拦截未保存改动**（只拦了切页/关窗）；若要拦截需在
  `Sheet onOpenChange` 里再走一次 `requestLeave`，会改变既有「取消」语义，待评估。
- Tauri **原生关窗**路径：`src-tauri/src/main.rs` 的 `CloseRequested` 是 `prevent_close + hide`
  （不卸载 WebView），托盘「退出」是 `app.exit(0)` —— 两者都不会触发 `beforeunload`，
  所以**「真正退出应用」时弹窗表单的未保存改动仍会静默丢失**。要堵住需改 Rust 侧，超出本轮范围。
