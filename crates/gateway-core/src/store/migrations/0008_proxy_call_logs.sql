-- 0008_proxy_call_logs — jai-registry 代理转发调用的审计日志（独立表）
-- 说明：与 request_logs 隔离，避免污染正常的 LLM 请求统计（request_logs 的
-- route_mode 有 CHECK 强约束、且会被 usage_stats/logs_recent 聚合）。
-- 只记审计所需的最小字段，不含 prompt/响应明文，不含 env 值。

CREATE TABLE proxy_call_logs (
  id          INTEGER PRIMARY KEY,   -- rowid 自增
  ts          INTEGER NOT NULL,      -- unix ms
  server_name TEXT NOT NULL,         -- 目标 MCP Server
  tool_name   TEXT NOT NULL,         -- 被调用工具
  kind        TEXT NOT NULL,         -- 'stdio' | 'http' | 'sse'
  status      TEXT NOT NULL CHECK (status IN ('ok','error')),  -- 转发结果
  duration_ms INTEGER NOT NULL,
  error       TEXT                   -- 出错时的截断信息（不含敏感内容）
);
CREATE INDEX idx_proxy_call_logs_ts ON proxy_call_logs (ts DESC);