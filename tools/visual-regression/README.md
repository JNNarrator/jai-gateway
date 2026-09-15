# 视觉回归探针（默认窗口 980×640）

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
node tools/visual-regression/audit-summarize.mjs .vr/audit-dark-980x640.json
```

`probe-colors3.mjs` 用于复核某个按钮的真实对比度（`DARK=1` 强制暗色主题）。

## 复测要点

- 遮挡统计：`covered`（真被挡住点不到）/ `clippedEdge`（被滚动容器上边缘切开）/ `clippedInScroller`（需滚动）三类已分开，只有 `covered` 才算问题。
- 弹窗验收：`overTop/overBottom = 0`、`footer.visible = true`、主按钮全可见。
