# Changelog

All notable changes to this project will be documented in this file.

## [Unreleased]

### Added
- M6: OpenAI Responses API 入站（`POST /v1/responses`，Codex 原生线）
  - Responses ↔ IR 编解码、SSE 事件流、错误形状
- M7: 配置导入 + WebDAV 同步
  - `jai-export/v1` 导入、按名称+Base URL 去重 uplift、缺失密钥报告
  - WebDAV 手动推/拉、推送前快照、last-write-wins；UI「同步」页
- M8: 收尾加固
  - 全矩阵回归脚本 `scripts/regression.sh`
  - 500 随机 body 解码器 fuzz 简表
- M9: 发布工程
  - CHANGELOG、发布检查单、tag 触发 CI（签名/公证需真实 secrets）
- MCP Server 管理：`mcp_servers` 表 + CRUD IPC + UI「MCP」页；支持 tools/list、tools/call 客户端调用（stdio/http/sse）
- MCP 工具自动合并 + 自动执行循环：网关把启用 MCP 工具注入请求工具定义；上游发起 MCP 工具调用时自动执行并回填结果继续生成
- 技能（skill）管理：`skills` 表 + CRUD IPC + UI「技能」页；跨族转换请求自动注入启用技能到 system

### Changed
- README/roadmap 同步至 M9 + MCP/Skill 基础管理

### Fixed
- 流式转换首字节含完整 SSE 流时未消费行缓冲的问题
- Anthropic SSE 渲染缺失 `content_block_stop`、交错 tool_calls 顺序问题