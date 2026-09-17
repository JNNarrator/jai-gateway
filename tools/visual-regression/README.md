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
```

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
