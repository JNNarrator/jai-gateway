#!/usr/bin/env bash
# M8 全矩阵回归一键脚本（roadmap M8 验收 1）。
# 用法: bash scripts/regression.sh
set -euo pipefail

echo "==> cargo fmt --check"
cargo fmt --all -- --check

echo "==> cargo clippy (-D warnings)"
cargo clippy --workspace --all-targets -- -D warnings

echo "==> cargo test --workspace"
cargo test --workspace

echo "==> frontend build"
if command -v pnpm >/dev/null 2>&1; then
  (cd ui && pnpm build)
else
  echo "pnpm not found; skip frontend build"
fi

echo "==> regression ok"