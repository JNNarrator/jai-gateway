#!/usr/bin/env bash
# 开发启动：先起 Vite（127.0.0.1:5173），再启动 Tauri 桌面壳。
# 避免手动只跑 cargo run 导致窗口白屏。
set -euo pipefail

VITE_LOG=$(mktemp)
JAI_LOG=$(mktemp)

cleanup() {
  kill "${JAI_PID:-0}" 2>/dev/null || true
  kill "${VITE_PID:-0}" 2>/dev/null || true
  wait "${JAI_PID:-0}" 2>/dev/null || true
  wait "${VITE_PID:-0}" 2>/dev/null || true
  rm -f "$VITE_LOG" "$JAI_LOG"
}
trap cleanup EXIT INT TERM

echo "==> 启动 Vite: http://127.0.0.1:5173"
pnpm --dir ui dev >"$VITE_LOG" 2>&1 &
VITE_PID=$!

# 等待 Vite 就绪
for _ in $(seq 1 30); do
  if curl -fsS --max-time 1 http://127.0.0.1:5173/ >/dev/null 2>&1; then
    break
  fi
  sleep 0.5
done

echo "==> 启动 JAI (cargo run -p jai)"
cargo run -p jai >"$JAI_LOG" 2>&1 &
JAI_PID=$!

wait "$JAI_PID"