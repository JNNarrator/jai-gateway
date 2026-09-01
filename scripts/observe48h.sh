#!/bin/bash
# M8/M9 48h 常驻观察采样：单次执行，追加一行到观察日志。
# 由定时自动化每 30 分钟调用；零崩溃判定 = 日志中不出现 alive=false（用户主动重启除外，标记 restart）。
LOG="/Users/jiangnan/Documents/workspace/JAI/.superpowers/observe48h.log"
PID=$(pgrep -f "target/release/jai" | head -1)
TS=$(date "+%Y-%m-%d %H:%M:%S")
if [ -z "$PID" ]; then
  echo "$TS alive=false healthz=- rss=-" >> "$LOG"
  echo "ALIVE=false — JAI 进程不存在！"
  exit 1
fi
CODE=$(curl -s -o /dev/null -w "%{http_code}" --max-time 3 http://127.0.0.1:1314/healthz 2>/dev/null)
[ "$CODE" != "200" ] && CODE=$(curl -s -o /dev/null -w "%{http_code}" --max-time 3 http://127.0.0.1:1315/healthz 2>/dev/null)
RSS=$(ps -o rss= -p "$PID" 2>/dev/null | tr -d ' ')
echo "$TS alive=true healthz=$CODE rss_kb=$RSS pid=$PID" >> "$LOG"
echo "$TS alive=true healthz=$CODE rss_kb=$RSS"
[ "$CODE" != "200" ] && exit 1
exit 0
