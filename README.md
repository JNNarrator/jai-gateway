<p align="center">
  <img src="ui/public/jai-logo.svg" width="88" alt="JAI logo" />
</p>

# JAI — 桌面 AI API 网关

> 开箱即用的本地 AI API 网关：把官方与第三方中转的杂牌 token 来源，收敛成一个稳定的本机入口，并让多设备（macOS / Windows）配置保持同步。

**状态**：M0–M9 全部里程碑完成，UI 2.0（阶段 0–6）已落地；真机验收矩阵（Claude Code / Codex / zcode 跨族、WebDAV 双机、签名安装包）与 48h 常驻观察按[路线图](docs/design/roadmap.md)推进中。

> **客户端优先级**：优先支持国产 Agent —— **DeepSeek Harness（dsh）** 与 **zcode**。目标是通过 JAI 网关使用时，与直连上游的体验保持一致（协议、流式、工具调用、错误语义均透明兼容）。dsh 已两轮真机联调验证（Chat / Responses / 故障转移），报告见 [docs/test-report-dsh.md](docs/test-report-dsh.md)。

## 它解决什么问题

- **token 来源杂**：官方 API、各类第三方中转并存，客户端各自配置难以维护 → JAI 统一代理，客户端只认一个地址
- **设备多且异构**：macOS 与 Windows 各不止一台，供应商/模型配置手工同步成本高 → 配置可导出 + WebDAV 同步
- **稳定性敏感**：agent 工作流（Claude Code、Codex 等）依赖网关常驻可用 → 稳定性为一票否决项，见路线图全局基线

## 核心特性

- 多供应商管理：OpenAI 兼容 / OpenAI Responses / Anthropic / Gemini 四族渠道，密钥存系统钥匙串（Windows 凭据管理器 / macOS 钥匙串），数据库不落明文
- 对外暴露统一网关入口（`127.0.0.1:1314`），支持四种入站协议线：
  - OpenAI `POST /v1/chat/completions` 与旧版 `POST /v1/completions`
  - OpenAI Responses API `POST /v1/responses`（Codex CLI 原生接入）
  - Anthropic `POST /v1/messages`（Claude Code 直连，含 `count_tokens` 粗估）
  - MCP 元数据服务 `POST /mcp`（Streamable HTTP）：把网关登记的 MCP Server / Skill
    台账以 MCP 协议暴露给 Agent——只提供发现与信息（`list_mcp_servers` /
    `get_mcp_server_detail` / `get_tool_schemas` / `list_skills` / `get_skill_detail`），
    不注入对话链路、不代执行工具；env 仅回键名不回值
- 同名模型多渠道路由：按优先级自动故障转移 + 健康感知排序 + 同优先级权重负载均衡
- 模型别名/映射：每个模型可配置发给上游的真实模型 ID
- 跨协议转换（含 tool calling）：让任意客户端组合任意上游模型
- MCP / 技能（Skill）管理：网关内登记 MCP Server 与技能（连接测试、工具查看、ZIP 批量导入）；**不注入对话链路**——网关只做请求转发与协议转换，工具执行由客户端侧完成
- 请求日志（仅元数据）与用量统计可视化（recharts 堆叠柱状图，近 7/30/90 天）
- **UI 2.0**：明暗双主题（跟随系统）、可折叠侧边栏、shadcn/ui 组件体系、
  表单校验就地展示（react-hook-form + zod）、自绘标题栏 + 窗口毛玻璃特效
- 安全基线：强制鉴权（常量时间比对）、Host/Origin 校验、CORS 默认拒绝、鉴权失败限速、推送前快照

## 技术栈

Tauri 2.0 · Rust (Axum + Reqwest) · React 19 + TypeScript + TailwindCSS 4 + shadcn/ui + recharts + react-hook-form/zod · SQLite · OS Keyring · MIT

## 界面

<div align="center">
  <img src="ui/public/jai-logo.svg" width="40" alt="" />
  <p><sub>明暗双主题 · 可折叠侧边栏 · 全页面语义化组件 —— 主色为蓝紫渐变的「J + AI 星火」标识</sub></p>
</div>

## 国产 Agent 优先支持

JAI 优先保证 **DeepSeek Harness（dsh）** 与 **zcode** 的使用体验：

- **目标**：通过 JAI 网关访问上游时，与直连上游的体验一致——协议、流式、工具调用、错误语义都保持透明兼容。
- **dsh**：已实测打通 OpenAI Chat Completions 与 OpenAI Responses 两条链路，别名映射、故障转移均可正常工作（两轮真机联调，报告见 [docs/test-report-dsh.md](docs/test-report-dsh.md)）。
- **zcode**：协议线待真机确认后纳入同等回归保障。
- 同族直通默认保持字节级透传，跨族（协议不同）时才进入转换路径。
- MCP / Skill 登记后可通过网关 `/mcp` 元数据服务暴露给 Agent（见「网关」页一键复制的
  `mcpServers` 配置）；技能另支持 ZIP 批量导入与 Markdown 导出。

## 文档索引

| 文档 | 内容 |
| --- | --- |
| [docs/需求主文档](docs/) —— 见仓库根《JAI — 桌面 AI API 网关.md》 | 需求全量定义与评审记录 |
| [docs/design/protocol-ir.md](docs/design/protocol-ir.md) | 协议中间表示、逐字段映射总表、行为规范 |
| [docs/design/storage-schema.md](docs/design/storage-schema.md) | SQLite 表结构、密钥管理、日志策略 |
| [docs/design/roadmap.md](docs/design/roadmap.md) | M0–M9 里程碑路线图、稳定性基线、UI 2.0 完成快照 |
| [docs/superpowers/specs/2026-08-31-ui-framework-upgrade-design.md](docs/superpowers/specs/2026-08-31-ui-framework-upgrade-design.md) | UI 2.0 设计 spec（阶段 0–6） |
| [docs/test-report-dsh.md](docs/test-report-dsh.md) | 本机 dsh 真机联调测试报告（两轮） |
| [docs/zcode接入.md](docs/zcode接入.md) | zcode 接入 JAI 与 MCP/Skill 加载指南 |
| [docs/design/release.md](docs/design/release.md) | 签名/公证/更新通道/发布检查单 |

## 第一梯队客户端

Claude Code · Codex · zcode · DeepSeek harness —— 首批适配与验收基准。

## 快速接入

### Claude Code（Anthropic 线，M3 已支持）

在 Claude Code 中把 Base URL 指向 JAI、API Key 换成网关 Key（`sk-jai-*`，见应用「网关」页）：

```bash
ANTHROPIC_BASE_URL=http://127.0.0.1:1314
ANTHROPIC_AUTH_TOKEN=sk-jai-xxxx            # 或 ANTHROPIC_API_KEY=sk-jai-xxxx
claude
```

要求：供应商页添加一个 **Anthropic** 协议族渠道并录入上游 Key，模型列表存在 Claude 型号（如 `claude-sonnet-4-5`）且启用。多轮对话、工具调用、prompt caching 均走字节级直通；`count_tokens` 由网关粗估返回，避免客户端降级。

### 任意 OpenAI 兼容客户端（M1/M2 已支持）

```bash
OPENAI_API_BASE=http://127.0.0.1:1314/v1
OPENAI_API_KEY=sk-jai-xxxx
```

同名模型可配置多个渠道，按优先级自动故障转移（一级 5xx/429/超时 → 顺延下一渠道）。

### DeepSeek Harness（dsh，最高优先级客户端）

dsh 的 provider 配置里 baseURL 填 `http://127.0.0.1:1314/v1`，API Key 用网关 Key；OpenAI Chat 与 Responses 两条线均已实测。详见接入示例（应用「网关」页内置）与[联调报告](docs/test-report-dsh.md)。

### MCP 元数据服务（/mcp）

在任意支持 MCP 的客户端（dsh / Claude Code / Continue 等）的 `mcpServers` 配置中加入（应用「网关」页有同款配置，**点「复制配置」自动填入真实密钥**）：

```json
{
  "mcpServers": {
    "jai-registry": {
      "type": "http",
      "url": "http://127.0.0.1:1314/mcp",
      "headers": { "Authorization": "Bearer <网关密钥>" }
    }
  }
}
```

接入后 Agent 可查询网关登记的 MCP Server / Skill 台账：`list_mcp_servers`、`get_mcp_server_detail`、`get_tool_schemas`、`list_skills`、`get_skill_detail`。该服务只提供发现与信息，不代执行工具。

## 本地开发启动

```bash
pnpm install            # 根工作区依赖
pnpm --dir ui install   # 前端依赖
bash scripts/dev.sh     # 一键启动：Vite + Tauri 桌面壳
```

- Vite 固定监听 `http://127.0.0.1:5173`
- 网关默认监听 `http://127.0.0.1:1314`
- 健康检查：`curl http://127.0.0.1:1314/healthz`
- 前端构建门禁：`pnpm --dir ui build`（`tsc --noEmit` + `vite build` 零错误）
- 全量回归：`bash scripts/regression.sh`（fmt / clippy / test / 前端 build）

## License

[MIT](./LICENSE)
