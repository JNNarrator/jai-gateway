-- 0005_mcp_env — MCP Server 增加 env 支持（Claude Code mcpServers 格式导入需要）
-- 说明：env 存 JSON 对象字符串（如 {"KEY":"value"}），stdio 启动子进程时注入。

ALTER TABLE mcp_servers ADD COLUMN env TEXT;
