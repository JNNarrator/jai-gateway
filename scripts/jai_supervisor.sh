#!/bin/bash
# JAI 观察期监督器：网关退出时记录退出码并自动拉活（-roadmap 看门狗精神）。
# 退出码含义：0=主动退出 137=SIGKILL 143=SIGTERM 134=SIGABRT
LOG="/Users/jiangnan/Documents/workspace/JAI/.superpowers/observe48h.log"
BIN="/Users/jiangnan/Documents/workspace/JAI/target/release/jai"
while true; do
  "$BIN" >> /tmp/jai-rel.log 2>&1
  CODE=$?
  echo "$(date '+%Y-%m-%d %H:%M:%S') SUPERVISOR: jai exited code=$CODE —— 自动拉活" >> "$LOG"
  echo "$(date '+%H:%M:%S') jai exited code=$CODE, restarting..."
  sleep 2
done
