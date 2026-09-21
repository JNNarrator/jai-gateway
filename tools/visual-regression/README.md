# 视觉回归探针（默认窗口 1180×800，最小 900×600）

配套文档：`docs/视觉回归整改plan.md`（问题清单、验收标准、进度记录）。
证据输出（截图/JSON/日志）落在 `.vr/`，该目录在 `.gitignore` 中只保留在本地。

## 前置

1. 前端依赖：`cd ui && npm i`
2. 本目录脚本顶部用 `createRequire` 写死了 Playwright 与 Chrome 的绝对路径，**换机需改成新机器路径**。
3. 建输出目录并起前端：

```bash
mkdir -p .vr/tmp .vr/shots
cd ui && nohup npx vite --host 127.0.0.1 --port 5173 > /tmp/vite.log 2>&1 &
```

## 运行（在仓库根目录）

```bash
export TMPDIR=$PWD/.vr/tmp
node tools/visual-regression/fold.mjs   --size=980x640   # 折叠线/横向溢出/弹窗 Esc 可关闭性
node tools/visual-regression/deep2.mjs  --size=980x640   # 弹窗结构：滚动容器、footer、主按钮可见性
node tools/visual-regression/audit.mjs  --size=980x640 --theme=dark   # 对比度/字号/命中区/截断（light 另跑）
node tools/visual-regression/run.mjs    --mode=walk --size=980x640    # 动态点击遍历（慢，建议后台）
node tools/visual-regression/audit-summarize.mjs .vr/audit-dark-1180x800.json

# MCP 页两个开关的说明性回归（标签/解释条/hover 详情/点文字能切换/无溢出）
node tools/visual-regression/mcp-switches.mjs --size=1180x800
node tools/visual-regression/mcp-switches.mjs --size=900x600

# 网关页接入地址的说明性回归（Base URL 复制仍在 / 完整地址 5 条 / 复制内容逐条相符 / 无溢出）
node tools/visual-regression/gateway-endpoints.mjs --size=1180x800
node tools/visual-regression/gateway-endpoints.mjs --size=900x600

# 同步页「推送间隔 / 拉取间隔各自独立」的说明性回归
# （两个选择器独立 / 各写各的 IPC 字段 / 文案不再声称共用 / 无溢出）
node tools/visual-regression/sync-intervals.mjs --size=1180x800
node tools/visual-regression/sync-intervals.mjs --size=900x600
```

`gateway-endpoints.mjs` 需要剪贴板读写授权（脚本内 `ctx.grantPermissions(["clipboard-read","clipboard-write"])`）——
不授权时 `navigator.clipboard.readText()` 抛 `NotAllowedError`，会把「复制成功但读不到」误判成功能失败。

`probe-colors3.mjs` 用于复核某个按钮的真实对比度（`DARK=1` 强制暗色主题）。本目录新增的两个探针
（本地 `.vr/` 里同名副本，供 `TMPDIR` 口径运行）：

- `probe-hits.mjs`：从元素中心用 `elementFromPoint` 逐点探测**有效命中区**。
  审计按 `getBoundingClientRect` 测量，看不到 `::after` 伪元素与 `<label>` 包裹造成的扩展——
  Switch / 行内复制 / 批量勾选都是这个模式，故只看审计会误判「命中区仍然很小」。
  两个坑：判据不能把**祖先**算命中（否则整行都算可达，数值虚高）；从中心采样时有效尺寸 = `2×可达半径`。
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
- 命中区：审计的 rect 数值**不等于**有效命中区，需用 `probe-hits.mjs` 复核；
  Radix 的 `aria-hidden` 隐藏代理（1×1 `select` 等）不是可点元素，属假阳性。

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
