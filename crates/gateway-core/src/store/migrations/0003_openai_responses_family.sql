-- 0003_openai_responses_family — 支持 OpenAI Responses 上游协议族
-- 用于 dsh / Codex 等 Responses API 客户端直连一个同样说 Responses 的上游
-- （例如 one-model.com），实现字节级 Responses 透传而不是转成 chat/completions。

PRAGMA foreign_keys = OFF;

CREATE TABLE providers_new (
  id            TEXT PRIMARY KEY,            -- UUIDv7 字符串（时间有序）
  name          TEXT NOT NULL,
  base_url      TEXT NOT NULL,
  family        TEXT NOT NULL CHECK (family IN ('openai_compat','openai_responses','anthropic','gemini')),
  enabled       INTEGER NOT NULL DEFAULT 1,
  priority      INTEGER NOT NULL DEFAULT 100,
  extra_headers TEXT,
  keyring_ref   TEXT NOT NULL UNIQUE,
  last_ok_at    INTEGER,
  last_err_at   INTEGER,
  last_err_msg  TEXT,
  created_at    INTEGER NOT NULL,
  updated_at    INTEGER NOT NULL
);

INSERT INTO providers_new (
  id,name,base_url,family,enabled,priority,extra_headers,keyring_ref,
  last_ok_at,last_err_at,last_err_msg,created_at,updated_at
)
SELECT
  id,name,base_url,family,enabled,priority,extra_headers,keyring_ref,
  last_ok_at,last_err_at,last_err_msg,created_at,updated_at
FROM providers;

DROP TABLE providers;
ALTER TABLE providers_new RENAME TO providers;