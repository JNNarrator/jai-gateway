# JAI — 桌面 AI API 网关

> 开箱即用的本地 AI API 网关：把官方与第三方中转的杂牌 token 来源，收敛成一个稳定的本机入口，并让多设备（macOS / Windows）配置保持同步。

**状态**：规划与设计阶段（未开始编码）。欢迎通过 Issue 讨论需求。

## 它解决什么问题

- **token 来源杂**：官方 API、各类第三方中转并存，客户端各自配置难以维护 → JAI 统一代理，客户端只认一个地址
- **设备多且异构**：macOS 与 Windows 各不止一台，供应商/模型配置手工同步成本高 → 配置可导出 + WebDAV 同步（公测前交付）
- **稳定性敏感**：agent 工作流（Claude Code、Codex 等）依赖网关常驻可用 → 稳定性为一票否决项，见路线图全局基线

## 核心特性（MVP 范围）

- 多供应商管理：OpenAI 兼容 / Anthropic / Gemini 三族渠道，密钥存系统钥匙串
- 对外暴露统一网关入口（`127.0.0.1`），支持三种入站协议线：
  - OpenAI `POST /v1/chat/completions`
  - OpenAI Responses API `POST /v1/responses`（Codex CLI 原生接入）
  - Anthropic `POST /v1/messages`（Claude Code 直连）
- 同名模型多渠道路由：按优先级自动故障转移
- 跨协议转换（含 tool calling）：让任意客户端组合任意上游模型
- 请求日志（仅元数据）与用量统计基础

## 技术栈

Tauri 2.0 · Rust (Axum + Reqwest) · React + TypeScript + TailwindCSS · SQLite · OS Keyring · MIT

## 文档索引

| 文档 | 内容 |
| --- | --- |
| [docs/需求主文档](docs/) —— 见仓库根《JAI — 桌面 AI API 网关.md》 | 需求全量定义与评审记录 |
| [docs/design/protocol-ir.md](docs/design/protocol-ir.md) | 协议中间表示、逐字段映射总表、行为规范 |
| [docs/design/storage-schema.md](docs/design/storage-schema.md) | SQLite 表结构、密钥管理、日志策略 |
| [docs/design/roadmap.md](docs/design/roadmap.md) | M0–M9 里程碑路线图与稳定性基线 |

## 第一梯队客户端

Claude Code · Codex · zcode · DeepSeek harness —— 首批适配与验收基准。

## License

[MIT](./LICENSE)
