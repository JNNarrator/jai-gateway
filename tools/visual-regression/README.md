# 视觉回归探针与 UI 门禁（默认窗口 1180×800，最小 900×600）

配套文档：`docs/视觉回归整改plan.md`（问题清单、验收标准、进度记录）。
证据输出（截图/JSON/日志）落在 `.vr/`，该目录在 `.gitignore` 中只保留在本地。

## 一条命令：UI 门禁

```bash
cd ui && nohup npx vite --host 127.0.0.1 --port 5173 > /tmp/vite.log 2>&1 &   # 起前端
cd .. && node tools/visual-regression/gate.mjs                                 # 双尺寸 × 双主题，单一退出码
```

`gate.mjs` 是**唯一**的 UI 验收入口，跑：

| 检查 | 覆盖 |
|---|---|
| `scripts/ui_lint.sh` | 静态规范：字号 ≥11px、图标按钮有可访问名、列表 key 不用下标、命中区不靠伪元素外扩 |
| `probe-hits.mjs` | **有效命中区** ≥24×24（命中区判据的唯一归属） |
| `fold.mjs` | 主操作首屏可达、无横向溢出、行内控件不被折叠线切半、表格不溢出容器、**吸顶表头真的吸顶**（实测滚动）、弹窗 Esc 可关 |
| `audit.mjs` | 对比度 AA、字号、截断有 title 兜底、横向溢出、弹窗几何、**弹窗 footer 不与 toast 带重叠** |
| `probe-keys.mjs` | 网关**密钥管理**（D9-T6a + 2026-09-23 排版整改）：全部密钥渲染、点前缀就地显示全文、**「⋯」菜单含低频/危险动作**、吊销二次确认（确认前不动数据）、新建插到最前且当场显示全文、全吊销后空态文案；排版硬指标 —— **密钥行不换行**、行内按钮 ≤2、页面宽度吃满窗口、按钮高度档位 ≤2、**页面主体无实心红按钮**（实心红只留在二次确认里） |
| `probe-rules.mjs` | 网关密钥**白/黑名单弹窗**（D9-T6b）：候选清单渲染、回填已保存规则、可访问模型预览随草稿实时变、无可用模型时的 403 后果提示、提交的四个数组与三态逐字一致、保存后关闭并可读回、「已限制」徽标 |
| `probe-endpoint.mjs` | 渠道草稿**端点探测面板**（D9-T1）：逐端点结论行渲染、信息性行灰显、截断摘要自带 title、被拦截地址文案与零延迟、探测不改表单脏状态 |
| `probe-feedback.mjs` | **按钮反馈的落点**（2026-09-24 全局整改）：页级反馈所在容器 `position: sticky` 且**滚到页面最底仍在视口内**（读 computed style + 实测滚动，不是看类名猜）、错误反馈 3s 后仍在且点「关闭提示」才消失、错误 toast 4.6s 后仍在 + 关闭钮 `pointer-events: auto`（Toaster 整体是 `none`）、行级反馈落在**触发它的那一行**里且此时页级吸顶条为空 |
| `scripts/tauri_window_check.mjs` | **最小窗口尺寸在各平台都生效**（零依赖）：平台配置合并（RFC 7396）数组是整体替换，会把基础配置的 `minWidth/minHeight` 静默丢掉 —— macOS 曾因此**完全没有最小尺寸限制**；判据含「平台窗口必须补齐基础配置的每个键」+「解析后最小尺寸 ≥ 本文件 `SIZES` 的最小验收尺寸」 |

判据集中写在 `gate.mjs` 里（不散落到各探针），任何一条不满足即退出码 1，并指出
「哪条判据、哪个尺寸/主题、具体是什么」。它已接进 `scripts/release_check.sh` 第 6 步。

**门禁不会读旧结果假绿**：每次跑探针前先删掉它的输出 JSON，且**探针退出码非 0 直接判失败**
（判据名「探针可运行」）。曾真实踩到 `fold.mjs` 因一次编辑误删变量而**每次都崩**、
`gate.mjs` 却读到上一次的旧 JSON 并报「✓ 全部通过」。
只想重算判据、不重跑探针时用 `--skip-probes`（读 `.vr/*.json`）—— 那是**显式选择**，不是默认路径。

## 前置（不需要改任何路径）

1. 前端依赖：`cd ui && pnpm i`（或 `npm i`）。
   ui 的 devDependency 里有 **`playwright-core`**：它只驱动系统 Chrome、不下载浏览器，
   所以门禁不需要额外的浏览器安装步骤。
2. 探针的 Playwright / Chrome 定位**已统一到 `_env.mjs`**，解析顺序：
   `PLAYWRIGHT_PATH` / `CHROME_PATH` 环境变量 → `ui/node_modules`（`playwright` → `playwright-core`）
   → 仓库根 `node_modules` → 让 Playwright 用它自带的 chromium。
   **不再有写死的绝对路径**（此前 15 个探针各自写死某台机器上的 pnpm 路径，换机/CI 全跑不起来）。
3. 起前端（探针需要 `127.0.0.1:5173`）：

```bash
mkdir -p .vr/tmp .vr/shots
cd ui && nohup npx vite --host 127.0.0.1 --port 5173 > /tmp/vite.log 2>&1 &
```

## 单独跑某个探针（在仓库根目录，调试用）

```bash
export TMPDIR=$PWD/.vr/tmp
node tools/visual-regression/gate.mjs                                     # 全量门禁
node tools/visual-regression/probe-hits.mjs --size=1180x800               # 只查有效命中区
node tools/visual-regression/fold.mjs   --size=900x600                    # 折叠线/横向溢出/弹窗 Esc
node tools/visual-regression/audit.mjs  --size=1180x800 --theme=dark      # 对比度/字号/截断/溢出（light 另跑）
node tools/visual-regression/deep2.mjs  --size=1180x800                   # 弹窗结构：滚动容器、footer、主按钮可见性
node tools/visual-regression/run.mjs    --mode=walk --size=1180x800       # 动态点击遍历（慢，建议后台）
node tools/visual-regression/audit-summarize.mjs .vr/audit-dark-1180x800.json

# 说明性回归（把某个已验收的行为固化成断言，防以后被静默删掉）
node tools/visual-regression/mcp-switches.mjs     --size=1180x800   # MCP 两开关：标签/解释条/hover 详情/点文字能切换
node tools/visual-regression/gateway-endpoints.mjs --size=1180x800  # 网关页：Base URL 复制 + 完整地址 5 条 + 逐条复制内容相符
node tools/visual-regression/sync-intervals.mjs   --size=1180x800   # 同步页：推送/拉取间隔各自独立
```

`gateway-endpoints.mjs` 需要剪贴板读写授权（脚本内 `ctx.grantPermissions([...])`）——
不授权时 `navigator.clipboard.readText()` 抛 `NotAllowedError`，会把「复制成功但读不到」误判成功能失败。

辅助探针（定点复核用，不在门禁里）：

- `probe-colors3.mjs`：某个按钮的真实对比度（`DARK=1` 强制暗色主题）。
- `probe-sticky.mjs` / `probe-fade.mjs` / `probe-models-cols.mjs`：吸顶操作条 / 顶部渐隐遮罩 / 模型表列降级。
- `probe-hits.mjs`：**命中区判据的唯一归属**。从元素中心用 `elementFromPoint` 逐点探测**有效命中区**
  （`::after` 伪元素 / `<label>` 包裹造成的热区外扩，`getBoundingClientRect` 看不到）。
  两个坑：判据不能把**祖先**算命中（否则整行都算可达，数值虚高）；从中心采样时有效尺寸 = `2×可达半径`。
  它还会断言「本轮点名的控件类型确实被扫到」（switch / checkbox / 行内复制 / 官网链接 /
  推理档位值域 / 下拉触发器），**覆盖缺口也算失败** —— 防哪天选择器失效、只测到 3 个元素却报全绿。
- `probe-toast.mjs` / `probe-toast2.mjs`：复核 toast 的真实 fg/bg 与 sonner 主题变量。
  `Toaster` 必须在 `ThemeProvider` 内部，否则 sonner 走错调色板（见 bug 清单第 18 条）。

## 复测要点

- 遮挡统计：`covered`（真被挡住点不到）/ `clippedEdge`（被滚动容器上边缘切开）/ `clippedInScroller`（需滚动）三类已分开，只有 `covered` 才算问题。
- 弹窗验收：`overTop/overBottom = 0`、`footer.visible = true`、主按钮全可见。
- 主题：审计必须在**页面加载前**写入 `localStorage.theme`（`--theme=dark` 时）。
  原先只在加载后 `classList.add("dark")`，next-themes 已初始化完毕，会出现「应用暗色 + 组件库浅色」
  的错配，让自带调色板的组件（toast/richColors）报出假结论。
- 截断：`truncated` 字段曾**只初始化、从不 push**（恒为空，相关问题无法被追踪），现已实现：
  `text-overflow: ellipsis` 或 `.truncate` 且 `scrollWidth > clientWidth` 且无 `title` 才算问题。
- 命中区：**只看 `probe-hits.mjs`**。审计（`audit.mjs`）已**不再**产出命中区判定 ——
  它按 `getBoundingClientRect` 量视觉盒子，看不到伪元素外扩，曾长期报「42 处不达标」
  而 probe-hits 复核后大多合格，两处结论互相矛盾、谁也不能当门禁。现在判据只有一个归属。
  注意 Radix 的 `aria-hidden` 隐藏代理（1×1 `select` 等）不是可点元素，已按假阳性排除。
  反例（说明为什么不能靠伪元素）：模型页「复制模型名」视觉 16×16、`after:-inset-2` 名义 32×32，
  但**有效命中区只有 6×30** —— 右侧紧邻的「推理档位值域」触发器也有伪元素外扩且 DOM 在后、
  paint 在上，把那一侧 8px 全抢走了。所以命中区要靠**真实盒子**（配合负 margin 抵消布局影响）。

- **探针一律读仓库内源文件（已修，2026-09-21）**：此前 `audit.mjs` / `deep.mjs` / `deep2.mjs` /
  `fold.mjs` / `probe-*.mjs` 共 **12 个**探针是从 `path.resolve(".vr/run.mjs")` 读 `installMock` 的
  —— 即读 `.vr/` 下那份**未跟踪的本地镜像**，不是仓库里这份。`.vr/` 在 `.gitignore` 中，
  所以改了源文件而忘了同步时，探针会**静默继续用旧 mock**（实测踩到：改完 mock 后
  `audit` 的 pageerror 依旧，因为用的还是旧副本），排查方向会被彻底带偏。
  现全部改为**相对本脚本解析**（`new URL("./run.mjs", import.meta.url)`），
  与 `fixtures.mjs` 一直以来的写法一致：不依赖 cwd、也不会再被镜像漂移影响。
  **`.vr/` 下那些同名 `.mjs` 副本已无用途，请勿运行**（它们是旧快照，且互相引用 `.vr/` 里的旧副本）。
  输出路径（`.vr/shots/`、`.vr/*.json`）仍按 cwd 解析 —— 那是**证据产出**，不是源码。
- **「无控制台报错」断言曾长期恒假**（2026-09-20 修）：`@tauri-apps/api` 的 `_unlisten`
  直接读 `window.__TAURI_EVENT_PLUGIN_INTERNALS__`（真实运行由 Tauri 注入），
  而 mock 只装了 `__TAURI_INTERNALS__`/`__TAURI__`。于是 `TitleBar` 的
  `win.onResized()` 清理路径在**每个页面**都抛
  `Cannot read properties of undefined (reading 'unregisterListener')`，
  三个断言「无控制台报错」的探针（`mcp-switches` / `gateway-endpoints` / `sync-intervals`）
  一律为红，久而久之被当成「已知噪音」忽略 —— 等于这条断言从未生效。
  现在四个 mock 都补了该内部对象（含 `run.mjs`，从而覆盖从它提取 mock 的探针）。
  **教训**：一条一直红的断言比没有断言更危险，它会训练人忽略红色。

- **「声明 sticky ≠ 真的吸顶」（2026-09-21）**：`fold.mjs` 的 `stickyHeads` 必须**真的滚动**
  最近的纵向滚动容器 150px，再看 `thead` 位移是否 ≤4px。只查 `getComputedStyle(thead).position`
  会**永远通过** —— 日志页就因此假绿很久。坑在 `Table` 基座自带
  `div[data-slot=table-container].overflow-x-auto`，其 `overflow-y` 随之计算成 `auto`，
  于是它是 thead 的最近可滚动祖先且自身无纵向溢出 → `sticky top-0` 完全失效。
  修法：把 `max-h + overflow:auto` 加到**那一层**（`[&_[data-slot=table-container]]:…`），
  不是在外面再套一个 div。（`border-collapse` 与「sticky 放 thead 还是 th」都已排除，无效。）
