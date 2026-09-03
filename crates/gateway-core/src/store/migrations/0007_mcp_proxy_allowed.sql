-- 0007_mcp_proxy_allowed — MCP Server 增加代理执行开关（jai-registry 动态工具转发）
-- 说明：proxy_allowed=1 的 Server 才会进入 jai-registry 的动态工具列表（server__tool），
-- 并允许网关 tools/call 转发调用；默认 0 = 只读台账，保持最小权限。

ALTER TABLE mcp_servers ADD COLUMN proxy_allowed INTEGER NOT NULL DEFAULT 0;
