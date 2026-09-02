-- 0006_secrets_in_db — 密钥迁入 SQLite，keyring_ref 退场
-- providers 增 api_key（明文，与网关 key/MCP env 同级安全模型）与 website（官网，可空）；
-- keyring_ref 带 NOT NULL UNIQUE 约束（SQLite 禁止 DROP 带 UNIQUE 的列），按惯例重建表移除。
-- 存量钥匙串凭据由 Rust 侧 migrate_keyring_secrets（keyring account = jai/provider/{id}，
-- id 即本表主键）填入 api_key，随后删除钥匙串项并置 meta.keyring_migrated 标记。

PRAGMA foreign_keys = OFF;

CREATE TABLE providers_new (
  id            TEXT PRIMARY KEY,            -- UUIDv7 字符串（时间有序）
  name          TEXT NOT NULL,
  base_url      TEXT NOT NULL,
  family        TEXT NOT NULL CHECK (family IN ('openai_compat','openai_responses','anthropic','gemini')),
  enabled       INTEGER NOT NULL DEFAULT 1,
  priority      INTEGER NOT NULL DEFAULT 100,-- 同名模型渠道路由顺序，小者先试
  weight        INTEGER NOT NULL DEFAULT 1,  -- 同 priority 内加权随机
  extra_headers TEXT,                        -- JSON；OpenRouter 类站点扩展头透传
  api_key       TEXT,                        -- 上游凭据明文；NULL = 未录入
  website       TEXT,                        -- 供应商官网（可空，UI 点击跳转）
  last_ok_at    INTEGER,                     -- 健康徽标数据源（重启后仍在）
  last_err_at   INTEGER,
  last_err_msg  TEXT,                        -- 截断 300 字符
  created_at    INTEGER NOT NULL,
  updated_at    INTEGER NOT NULL
);

INSERT INTO providers_new (
  id,name,base_url,family,enabled,priority,weight,extra_headers,
  last_ok_at,last_err_at,last_err_msg,created_at,updated_at
)
SELECT
  id,name,base_url,family,enabled,priority,weight,extra_headers,
  last_ok_at,last_err_at,last_err_msg,created_at,updated_at
FROM providers;

DROP TABLE providers;
ALTER TABLE providers_new RENAME TO providers;
