-- 0002_mcp_and_skills — MCP server 管理 + 技能（skill）管理
-- 说明：当前先提供配置/启停管理，真实 MCP 客户端调用在后续功能迭代接入。

CREATE TABLE mcp_servers (
  id          TEXT PRIMARY KEY,
  name        TEXT NOT NULL UNIQUE,
  kind        TEXT NOT NULL CHECK (kind IN ('stdio','sse','http')),
  command     TEXT,
  args        TEXT,                        -- JSON 数组
  url         TEXT,
  enabled     INTEGER NOT NULL DEFAULT 1,
  created_at  INTEGER NOT NULL,
  updated_at  INTEGER NOT NULL
);

CREATE TABLE skills (
  id          TEXT PRIMARY KEY,
  name        TEXT NOT NULL UNIQUE,
  description TEXT NOT NULL DEFAULT '',
  content     TEXT NOT NULL DEFAULT '',    -- 技能指令/提示词/脚本
  enabled     INTEGER NOT NULL DEFAULT 1,
  created_at  INTEGER NOT NULL,
  updated_at  INTEGER NOT NULL
);