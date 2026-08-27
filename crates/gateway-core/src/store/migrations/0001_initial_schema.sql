-- 0001_initial_schema — 依据 docs/design/storage-schema.md §3 定稿 DDL
PRAGMA foreign_keys = ON;

CREATE TABLE meta (
  key   TEXT PRIMARY KEY,
  value TEXT NOT NULL                        -- JSON 编码值
);

CREATE TABLE providers (
  id            TEXT PRIMARY KEY,            -- UUIDv7 字符串（时间有序）
  name          TEXT NOT NULL,
  base_url      TEXT NOT NULL,
  family        TEXT NOT NULL CHECK (family IN ('openai_compat','anthropic','gemini')),
  enabled       INTEGER NOT NULL DEFAULT 1,
  priority      INTEGER NOT NULL DEFAULT 100,-- 同名模型渠道路由顺序，小者先试
  extra_headers TEXT,                        -- JSON；OpenRouter 类站点扩展头透传
  keyring_ref   TEXT NOT NULL UNIQUE,        -- 形如 jai/provider/{uuid}
  last_ok_at    INTEGER,                     -- 健康徽标数据源（重启后仍在）
  last_err_at   INTEGER,
  last_err_msg  TEXT,                        -- 截断 300 字符
  created_at    INTEGER NOT NULL,
  updated_at    INTEGER NOT NULL
);

CREATE TABLE models (
  id                TEXT PRIMARY KEY,
  provider_id       TEXT NOT NULL REFERENCES providers(id) ON DELETE CASCADE,
  model_name        TEXT NOT NULL,           -- 路由键 = 客户端请求中的 model
  upstream_model_id TEXT,                    -- 上游真实 id；NULL ⇒ 同名（二期别名映射预留位）
  context_window    INTEGER,                 -- NULL ⇒ 快照值或保守默认 128k
  max_output_tokens INTEGER NOT NULL DEFAULT 4096,
  enabled           INTEGER NOT NULL DEFAULT 1,
  UNIQUE (provider_id, model_name)
);
CREATE INDEX idx_models_route ON models(model_name, enabled);
CREATE INDEX idx_models_prov  ON models(provider_id);

CREATE TABLE gateway_keys (
  id           TEXT PRIMARY KEY,
  key          TEXT NOT NULL UNIQUE,         -- 明文存储（storage §6 已拍板），常态脱敏展示前缀
  prefix       TEXT NOT NULL,                -- 如 sk-jai-aB12cd → UI 常态仅显示前缀
  label        TEXT,
  created_at   INTEGER NOT NULL,
  revoked_at   INTEGER,                      -- 非空即吊销；认证查询排除
  last_used_at INTEGER
);

CREATE TABLE request_logs (
  id                INTEGER PRIMARY KEY,     -- rowid 自增，优于 UUID
  ts                INTEGER NOT NULL,        -- unix ms
  inbound_family    TEXT NOT NULL,           -- 'openai' | 'anthropic' | 'responses'
  route_mode        TEXT NOT NULL CHECK (route_mode IN ('passthrough','converted')),
  model_name        TEXT NOT NULL,
  provider_id       TEXT,                    -- 刻意不设外键，允许悬空
  upstream_model_id TEXT,                    -- 多渠道顺延时记录最终命中的渠道模型 id
  http_status       INTEGER NOT NULL,        -- 返回给客户端的状态码
  stop_reason       TEXT,                    -- IR StopReason 枚举名
  usage_input       INTEGER,
  usage_output      INTEGER,
  usage_cache_read  INTEGER,
  usage_cache_write INTEGER,
  duration_ms       INTEGER NOT NULL,
  is_stream         INTEGER NOT NULL DEFAULT 0,
  tool_calls        INTEGER NOT NULL DEFAULT 0,  -- assistant 发起的调用次数
  error_kind        TEXT,                    -- IR 错误 kind 枚举名（protocol-ir §6）
  error_summary     TEXT                     -- 截断 300 字符；永不含 prompt/响应明文
);
CREATE INDEX idx_logs_ts       ON request_logs(ts DESC);
CREATE INDEX idx_logs_model_ts ON request_logs(model_name, ts DESC);

CREATE TABLE tool_id_map (                   -- 协议 IR §5-B 超长 id 的兜底映射
  outbound_id  TEXT PRIMARY KEY,
  canonical_id TEXT NOT NULL,
  created_at   INTEGER NOT NULL,
  expires_at   INTEGER NOT NULL              -- 7 天滚动过期
);
