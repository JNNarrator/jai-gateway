#!/usr/bin/env bash
# UI 静态规范门禁（薄包装，实现在 ui_lint.mjs）。
#
# 把「只写在文档里」的 UI 规范变成可执行检查：字号 ≥11px、图标按钮有可访问名、
# 列表 key 不用下标、命中区不靠伪元素外扩。零依赖，任意目录可跑。
#
# 用法: bash scripts/ui_lint.sh
set -euo pipefail
cd "$(dirname "$0")/.."
exec node scripts/ui_lint.mjs
