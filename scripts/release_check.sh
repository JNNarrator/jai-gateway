#!/usr/bin/env bash
# 发布前自动门禁检查（docs/design/release.md §3 的自动化部分）。
# 用法: bash scripts/release_check.sh
#
# 第 6 步是 **UI 门禁**（2026-09-21 加入）：把「双尺寸 × 双主题」的 UI 规范验收
# 收成一条命令（tools/visual-regression/gate.mjs + scripts/ui_lint.sh）。
# 起因：这些规范此前只写在文档里，于是每加一个新功能就静默回退
# （bug 21/24 引入的 0011/0012 编辑器又写出 4 处 text-[10px]、以及只靠伪元素外扩的命中区）。
set -euo pipefail

VITE_PID=""
cleanup() { [ -n "$VITE_PID" ] && kill "$VITE_PID" 2>/dev/null || true; }
trap cleanup EXIT

echo "==> 1/7 检查工作区是否干净"
if ! git diff --quiet || ! git diff --cached --quiet; then
  echo "FAIL: 工作区存在未提交/未暂存变更，请先提交。"
  exit 1
fi
echo "OK"

echo "==> 2/7 读取当前版本"
VERSION="$(grep -o '"version"[[:space:]]*:[[:space:]]*"[^"]*"' src-tauri/tauri.conf.json | head -1 | sed -E 's/.*"([^"]+)"$/\1/')"
if [ -z "$VERSION" ]; then
  echo "FAIL: 无法从 src-tauri/tauri.conf.json 读取版本号"
  exit 1
fi
echo "当前版本: $VERSION"

echo "==> 3/7 检查 CHANGELOG 是否有 Unreleased 条目"
if ! grep -q '^## \[Unreleased\]' CHANGELOG.md; then
  echo "FAIL: CHANGELOG.md 缺少 [Unreleased] 条目"
  exit 1
fi
echo "OK"

echo "==> 4/7 检查 tag v$VERSION 是否已存在"
if git rev-parse "v$VERSION" >/dev/null 2>&1; then
  echo "FAIL: tag v$VERSION 已存在，请先升级版本号。"
  exit 1
fi
echo "OK"

echo "==> 5/7 运行全量回归（fmt/clippy/test/frontend build）"
bash scripts/regression.sh

echo "==> 6/7 UI 门禁（静态规范 + 双尺寸双主题探针）"
# 6a 静态规范：零依赖，任意目录可跑
bash scripts/ui_lint.sh

# 6b 探针需要 vite dev server。若 5173 已在跑就复用，否则临时起一个并在退出时关掉。
if curl -sf -o /dev/null http://127.0.0.1:5173/; then
  echo "复用已在运行的 vite:5173"
else
  echo "启动临时 vite:5173"
  (cd ui && nohup npx vite --host 127.0.0.1 --port 5173 >/tmp/jai-vite-release-check.log 2>&1 & echo $! >/tmp/jai-vite-release-check.pid)
  VITE_PID="$(cat /tmp/jai-vite-release-check.pid)"
  for _ in $(seq 1 40); do
    curl -sf -o /dev/null http://127.0.0.1:5173/ && break
    sleep 0.5
  done
fi
node tools/visual-regression/gate.mjs

echo "==> 7/7 发布检查结果"
cat <<EOF
发布候选 v$VERSION 已通过自动化门禁：
- 工作区干净
- CHANGELOG [Unreleased] 存在
- tag v$VERSION 尚未创建
- fmt / clippy / test / frontend build 全绿
- UI 门禁全绿（ui_lint 静态规范 + 1180×800/900×600 × light/dark 探针）

仍需人工/真实环境完成：
- macOS 签名 + 公证
- Windows 代码签名
- 真机验收（Claude Code / Codex / dsh / zcode / WebDAV / 48h 常驻）
- Tauri Updater feed 发布
- 干净 VM 安装包验证
EOF
