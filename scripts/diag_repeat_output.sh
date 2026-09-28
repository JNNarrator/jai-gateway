#!/bin/bash
# 重复输出诊断：判定「模型复读」里哪一部分是 JAI 的锅，哪一部分是上游的锅。
#
# 背景（docs/bug和优化清单.md 第 11/12 条）：
#   - **上游侧**：基元律动（tokenrhythm.studio）在长上下文 agentic 场景下会退化复读，
#     且会在 max_output_tokens 处截断。这是上游行为，JAI 不制造重复。
#   - **JAI 侧（已修）**：Responses 入站曾经在截断时只写 status:"incomplete" 而不给
#     incomplete_details.reason，只读 status 的严格客户端（PI-Desktop）把它当成可重试错误，
#     于是重发同一 prompt 最多 10 次。每次重试都重新生成 → 客户端把结果拼进同一条消息
#     → 复读份数翻倍滚雪球（2/4/6/…/176）。修后截断按 OpenAI 语义给全 status + reason。
#
# 本脚本做两件事：
#   ① 从 request_logs 里找出**重试风暴指纹**（同一 usage_input 短时间内连续出现多次，
#      且每次 usage_output 顶满上限）；
#   ② 按「结束原因 × 供应商」给出分布，把复读的归属钉到具体渠道上。
#
# 只读：以 mode=ro 打开 DB，不会写入或加锁（WAL 下可安全并发读）。
#
# 用法：
#   scripts/diag_repeat_output.sh                # 最近 7 天
#   scripts/diag_repeat_output.sh 30             # 最近 30 天
#   scripts/diag_repeat_output.sh 7 /path/jai.db # 指定 DB
set -uo pipefail

DAYS="${1:-7}"
DB="${2:-$HOME/Library/Application Support/app.jai.gateway/jai.db}"

# Linux 上的回退路径（Tauri app_data_dir 在 Linux 是 ~/.local/share/<identifier>）
if [ ! -f "$DB" ] && [ -f "$HOME/.local/share/app.jai.gateway/jai.db" ]; then
  DB="$HOME/.local/share/app.jai.gateway/jai.db"
fi

if [ ! -f "$DB" ]; then
  echo "找不到数据库：$DB" >&2
  echo "提示：数据库路径 = <app_data_dir>/jai.db，可用第二个参数显式指定。" >&2
  exit 2
fi

# WAL 模式下只读打开：不加写锁，未 checkpoint 的最新日志也能看到。
# immutable=0 是刻意的 —— 加 immutable=1 会忽略 WAL，最新数据反而看不到。
Q() { sqlite3 -header -column "file:${DB}?mode=ro" ".timeout 5000" "$1"; }

WIN="ts >= (strftime('%s','now') - $((DAYS * 86400))) * 1000"

hr() { printf '%s\n' "────────────────────────────────────────────────────────────────"; }

echo "JAI 重复输出诊断"
echo "DB   : $DB"
echo "窗口 : 最近 ${DAYS} 天"
echo

# ---------------------------------------------------------------- 0. 覆盖度自查
hr
echo "【0】日志覆盖度（判断数据是否够用）"
hr
sqlite3 "file:${DB}?mode=ro" ".timeout 5000" "
SELECT '窗口内日志条数', COUNT(*) FROM request_logs WHERE $WIN
UNION ALL SELECT '窗口内最早', COALESCE(datetime(MIN(ts)/1000,'unixepoch','localtime'),'-') FROM request_logs WHERE $WIN
UNION ALL SELECT '窗口内最新', COALESCE(datetime(MAX(ts)/1000,'unixepoch','localtime'),'-') FROM request_logs WHERE $WIN
UNION ALL SELECT 'stop_reason 已采集', COUNT(*) FROM request_logs WHERE $WIN AND stop_reason IS NOT NULL;" 2>&1
echo
echo "※ 若「stop_reason 已采集」远小于总条数：该列 2026-09-21 才修（bug 清单第 12 条），"
echo "  更早的行本就没有结束原因，属预期，不代表漏采。"
echo

# ---------------------------------------------------------------- 1. 结束原因分布
hr
echo "【1】结束原因分布（max_tokens = 被输出上限截断，复读的温床）"
hr
Q "SELECT COALESCE(stop_reason,'(未采集)') AS stop_reason, COUNT(*) AS n
   FROM request_logs WHERE $WIN
   GROUP BY stop_reason ORDER BY n DESC;" 2>&1
echo

# ---------------------------------------------------------------- 2. 按供应商
hr
echo "【2】被截断的请求落在哪个渠道（责任归属的第一手证据）"
hr
Q "SELECT model_name, COUNT(*) AS truncated,
          MAX(usage_output) AS max_out, MAX(usage_input) AS max_in,
          datetime(MAX(ts)/1000,'unixepoch','localtime') AS last_seen
   FROM request_logs
   WHERE $WIN AND stop_reason='max_tokens'
   GROUP BY model_name ORDER BY truncated DESC;" 2>&1
echo

# ---------------------------------------------------------------- 3. 重试风暴
hr
echo "【3】重试风暴指纹：同一 usage_input 短时间内重复出现"
hr
echo "判据：相同 usage_input（说明是同一 prompt 被重发）+ 出现 ≥2 次。"
echo "      客户端重试会让 usage_input 逐字相同，这个是可靠指纹。"
echo
Q "SELECT usage_input, COUNT(*) AS times,
          GROUP_CONCAT(usage_output) AS outputs,
          CAST((MAX(ts)-MIN(ts))/1000 AS INT) AS span_sec,
          datetime(MIN(ts)/1000,'unixepoch','localtime') AS first_seen,
          GROUP_CONCAT(DISTINCT model_name) AS models
   FROM request_logs
   WHERE $WIN AND stop_reason='max_tokens'
   GROUP BY usage_input HAVING times >= 2
   ORDER BY times DESC, usage_input DESC
   LIMIT 25;" 2>&1
echo
echo "解读：times ≥ 5 且 outputs 多次顶满上限（如 8192）→ 典型的客户端重试风暴。"
echo "      span_sec 小（几十秒内）说明是自动重试，不是用户手动重发。"
echo

# ---------------------------------------------------------------- 4. 风暴汇总
hr
echo "【4】风暴规模汇总（份数翻倍倍数）"
hr
Q "SELECT COUNT(*) AS storm_prompts,
          SUM(times) AS total_requests,
          MAX(times) AS worst_case
   FROM (SELECT usage_input, COUNT(*) AS times
         FROM request_logs
         WHERE $WIN AND stop_reason='max_tokens'
         GROUP BY usage_input HAVING times >= 2);" 2>&1
echo
echo "※ 客户端把 N 次重试结果拼进同一条消息时，复读份数约等于 N 的倍数 —— "
echo "  这解释了用户观测到的「份数恒为偶数（2/4/6/…/176）」。"
echo

# ---------------------------------------------------------------- 4b. 修复前后对照
hr
echo "【4b】修复前后对照（重试风暴是否被压住）"
hr
# 分界**不能**用 commit 时刻：commit 只说明代码进了仓库，真正开始采集要等用户
# 重新构建 + 重启 JAI。本次实测两者差约 15.5 小时（c2f1abf 提交于 09-21 17:19，
# 而第一行 stop_reason 有值出现在 09-22 08:58）—— 用 commit 时刻会把「旧二进制
# 的残留」误读成「修完还有风暴」。
# 故分界取**库里最早一行 stop_reason 有值的时刻**（自校准，跨机器可复现）。
SR_BOUNDARY=$(sqlite3 "file:${DB}?mode=ro" ".timeout 5000" \
  "SELECT COALESCE(MIN(ts), 0) FROM request_logs WHERE stop_reason IS NOT NULL;")
if [ "${SR_BOUNDARY:-0}" -le 0 ] 2>/dev/null; then
  echo "库里还没有任何 stop_reason 有值的行 —— 无法确定分界，跳过本节。"
  echo "（该列 2026-09-21 起才有；若全为 NULL，说明当前二进制早于那次修复。）"
else
  echo "分界：$(( SR_BOUNDARY / 1000 )) → $(date -r $((SR_BOUNDARY / 1000)) '+%Y-%m-%d %H:%M:%S')"
  echo "（口径：库里最早一行 stop_reason 有值的时刻 = 二进制真正开始采集的时刻）"
  echo
  echo "--- 分界前：同 usage_input 连发 ≥3（usage_input>1000 排除 usage 未采集的噪音行）---"
  sqlite3 -header -column "file:${DB}?mode=ro" ".timeout 5000" "
WITH s AS (SELECT usage_input, ts, usage_output,
                  COUNT(*) OVER w AS n,
                  MAX(usage_output) OVER w AS mx,
                  MAX(ts) OVER w - MIN(ts) OVER w AS span
           FROM request_logs WHERE ts < $SR_BOUNDARY AND usage_input > 1000
           WINDOW w AS (PARTITION BY usage_input))
SELECT usage_input, n, mx AS max_out, CAST(span/1000 AS INT) AS span_sec,
       datetime(MIN(ts)/1000,'unixepoch','localtime') AS first_seen
FROM s WHERE n >= 3 GROUP BY usage_input ORDER BY n DESC LIMIT 8;" 2>&1
  echo
  echo "--- 分界后：同 usage_input 连发 ≥3 ---"
  sqlite3 -header -column "file:${DB}?mode=ro" ".timeout 5000" "
WITH s AS (SELECT usage_input, ts, usage_output,
                  COUNT(*) OVER w AS n,
                  MAX(usage_output) OVER w AS mx,
                  MAX(ts) OVER w - MIN(ts) OVER w AS span
           FROM request_logs WHERE ts >= $SR_BOUNDARY AND usage_input > 1000
           WINDOW w AS (PARTITION BY usage_input))
SELECT usage_input, n, mx AS max_out, CAST(span/1000 AS INT) AS span_sec,
       datetime(MIN(ts)/1000,'unixepoch','localtime') AS first_seen
FROM s WHERE n >= 3 GROUP BY usage_input ORDER BY n DESC LIMIT 8;" 2>&1
  echo
  echo "--- 分界后：真·截断（stop_reason=max_tokens）逐条 ---"
  echo "（这是最干净的判据：stop_reason 已采集，不再靠 usage 反推）"
  sqlite3 -header -column "file:${DB}?mode=ro" ".timeout 5000" "
SELECT id, datetime(ts/1000,'unixepoch','localtime') AS t, model_name,
       usage_input, usage_output, COALESCE(error_kind,'-') AS ek
FROM request_logs
WHERE ts >= $SR_BOUNDARY AND stop_reason='max_tokens'
ORDER BY ts LIMIT 20;" 2>&1
fi
echo
echo "※ 判读要点（三条都容易误判，务必先看 usage_output 是否**顶满上限**）："
echo "  ① 真风暴 = 同 usage_input 连发 + 每次 usage_output 顶满（8192/32768）+ span 短；"
echo "  ② 正常长回答 max_out 也可达 2~3 万，但 stop_reason=end_turn 且 usage_input 逐轮递增；"
echo "  ③ **10 分钟整点节奏**的小 usage_input 簇是**供应商健康检查**（每轮探测全部启用渠道），"
echo "     不是风暴 —— 实测 09-22 出现 8 次、间隔 610/630/575/603/632/595/594 秒。"
echo

# ---------------------------------------------------------------- 4c. 诊断缺口
hr
echo "【4c】诊断缺口：stop_reason 为空的行里，有多少是「该有却没有」？"
hr
Q "SELECT route_mode, is_stream,
          SUM(CASE WHEN stop_reason IS NULL THEN 1 ELSE 0 END) AS null_stop_reason,
          COUNT(*) AS total
   FROM request_logs WHERE $WIN
   GROUP BY route_mode, is_stream ORDER BY total DESC;" 2>&1
echo
cat <<'EOF'
**别被上面的 NULL 数字吓到** —— 绝大多数 NULL 是**正确**的：
请求根本没走到「上游给出完成」这一步（限流切渠道 / 余额不足 / 连接失败 / 上游挂死），
本来就没有结束原因可言。实测归类（09-22 08:58 后 1128 行 converted+stream+NULL）：
  RateLimit 599 · InvalidRequest(余额不足) 309 · ProviderOther(连接失败) 94 ·
  CapabilityWarn 95 · Overloaded 18 · stream aborted 7 · **client disconnected 仅 4**

所以真正该问的是：**「有完成」却丢字段** 的行有多少？
判据 = usage 有值（说明终局帧到了、原因算得出来）∧ stop_reason 为空。
EOF
echo
echo "--- 分界之后的「有完成却丢字段」行数（越接近 0 越好）---"
sqlite3 -header -column "file:${DB}?mode=ro" ".timeout 5000" "
SELECT COUNT(*) AS completed_but_no_stop_reason
FROM request_logs
WHERE ts >= $SR_BOUNDARY AND stop_reason IS NULL AND usage_input IS NOT NULL;" 2>&1
echo
echo "--- 分界之前同一口径（对照：那时该列尚未采集，必然很大；不是缺陷）---"
sqlite3 -header -column "file:${DB}?mode=ro" ".timeout 5000" "
SELECT COUNT(*) AS rows_before_feature
FROM request_logs
WHERE ts < $SR_BOUNDARY AND stop_reason IS NULL AND usage_input IS NOT NULL;" 2>&1
echo
echo "已修（2026-09-28，crates/gateway-core/src/server/proxy.rs）："
echo "  • 转换流式「客户端中途断开 mid-stream」分支原先传 stop_reason=None，"
echo "    而 last_stop_reason **已经**算好（同文件另 4 处收尾日志都带上了）。"
echo "  • 直通流式两条断开分支（early / mid-stream）同理（probe 已在累积）。"
echo "  ⇒ 统一带上已见到的结束原因。**坦诚标定影响面**：这些分支同时传 usage=None，"
echo "    故其行呈「usage 与 stop_reason 双空」，与「上游没给终局帧」形态相同 ——"
echo "    无法事后区分，这正是要一并记上的理由；本次真机仅命中 4 行，属小而正确的修补。"
echo

# ---------------------------------------------------------------- 5. 直连对照
hr
echo "【5】直连对照（闭环责任划分的最后一环）"
hr
cat <<'EOF'
本脚本只能看经 JAI 的流量。要彻底钉死「是上游在复读」，还需一次对照：

  1. 从下面复现出的 prompt 里挑一条（建议取 times 最大的那组）；
  2. 把同样的 prompt **直连**基元律动（不经 JAI，用它的原生 base_url + key）；
  3. 观察是否同样出现长响应 + 在 8192/32768 处截断 + 内容复读。

判定：
  • 直连也复读        → 上游行为，JAI 无责（预期结论）
  • 直连正常、经 JAI 复读 → 转入 JAI 侧排查（转换/重放），需进一步深挖

§ 快速取一条直连用的复现样本（最近一次被截断的请求）：
EOF
echo
Q "SELECT id, datetime(ts/1000,'unixepoch','localtime') AS t, model_name,
          usage_input, usage_output, route_mode, error_kind
   FROM request_logs WHERE $WIN AND stop_reason='max_tokens'
   ORDER BY ts DESC LIMIT 5;" 2>&1
echo
echo "※ 注意：request_logs 刻意**不存 prompt 明文**（见 0001_initial_schema.sql 第 66 行注释），"
echo "  所以复现样本要从客户端自己的会话日志里取，DB 只给出行号与 token 数用于对齐。"
echo

# ---------------------------------------------------------------- 6. 空转轮诊断
hr
echo "【6】零可见输出的截断轮（客户端会当成「丢了一轮」而重跑）"
hr
echo "判据：stop_reason=max_tokens 且 tool_calls=0 且 usage_input 很大（说明有长上下文输入），"
echo "      但客户端没拿到可见正文 —— 这类轮次会触发 silent-turn 重跑。"
echo
Q "SELECT id, datetime(ts/1000,'unixepoch','localtime') AS t, model_name,
          usage_input, usage_output, tool_calls, is_stream, error_kind
   FROM request_logs
   WHERE $WIN AND stop_reason='max_tokens' AND tool_calls=0 AND usage_input > 10000
   ORDER BY ts DESC LIMIT 15;" 2>&1
echo

hr
echo "诊断完成。归属速查："
echo "  • max_tokens 集中在某一渠道   → 该上游的截断行为（上游侧）"
echo "  • 同 usage_input 连发多次      → 客户端重试风暴（JAI 侧已修，确认版本 ≥ 修复版）"
echo "  • 两份都命中                   → 「上游复读」× 「JAI 放大」，两者叠加即为原始现象"
hr