#!/usr/bin/env bash
# 发布前自动门禁检查（docs/design/release.md §3 的自动化部分）。
# 用法: bash scripts/release_check.sh
set -euo pipefail

echo "==> 1/6 检查工作区是否干净"
if ! git diff --quiet || ! git diff --cached --quiet; then
  echo "FAIL: 工作区存在未提交/未暂存变更，请先提交。"
  exit 1
fi
echo "OK"

echo "==> 2/6 读取当前版本"
VERSION="$(grep -o '"version"[[:space:]]*:[[:space:]]*"[^"]*"' src-tauri/tauri.conf.json | head -1 | sed -E 's/.*"([^"]+)"$/\1/')"
if [ -z "$VERSION" ]; then
  echo "FAIL: 无法从 src-tauri/tauri.conf.json 读取版本号"
  exit 1
fi
echo "当前版本: $VERSION"

echo "==> 3/6 检查 CHANGELOG 是否有 Unreleased 条目"
if ! grep -q '^## \[Unreleased\]' CHANGELOG.md; then
  echo "FAIL: CHANGELOG.md 缺少 [Unreleased] 条目"
  exit 1
fi
echo "OK"

echo "==> 4/6 检查 tag v$VERSION 是否已存在"
if git rev-parse "v$VERSION" >/dev/null 2>&1; then
  echo "FAIL: tag v$VERSION 已存在，请先升级版本号。"
  exit 1
fi
echo "OK"

echo "==> 5/6 运行全量回归（fmt/clippy/test/frontend build）"
bash scripts/regression.sh

echo "==> 6/6 发布检查结果"
cat <<EOF
发布候选 v$VERSION 已通过自动化门禁：
- 工作区干净
- CHANGELOG [Unreleased] 存在
- tag v$VERSION 尚未创建
- fmt / clippy / test / frontend build 全绿

仍需人工/真实环境完成：
- macOS 签名 + 公证
- Windows 代码签名
- 真机验收（Claude Code / Codex / dsh / zcode / WebDAV / 48h 常驻）
- Tauri Updater feed 发布
- 干净 VM 安装包验证
EOF
