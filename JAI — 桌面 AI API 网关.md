# JAI — 桌面 AI API 网关

## 项目概述

JAI 是一款**开箱即用的桌面 AI API 网关**，纯本地运行，无服务器依赖。旨在为个人开发者提供统一管理多个 AI 模型供应商的入口，对外暴露**兼容 OpenAI 与 Anthropic 双协议**的代理接口，让任何支持 OpenAI API 的客户端（如 ChatGPT Next Web、Continue 等）以及 **Claude Code**（Anthropic 协议直连）都能无缝接入。

> **立项动机（2025-08 补记）**：实际使用中 token 来源繁杂——既有官方 API，也有大量第三方小作坊中转渠道；且开发设备为 macOS 与 Windows 各不止一台，供应商/模型配置在多机间手工同步成本极高。JAI 的核心价值即：**把杂乱的 token 来源收敛为一个本地稳定入口，并让多设备配置可导出、可同步。**

## 重点客户端（第一梯队）

以下客户端是首批适配与验收基准，优先保证其真实工作流经 JAI 可用：

| 客户端 | 对接协议线 | 承接里程碑 |
| --- | --- | --- |
| Claude Code | Anthropic Messages 直通（`ANTHROPIC_BASE_URL`） | M3 |
| Codex | OpenAI **Responses API**（`POST /v1/responses`）；过渡期可用 provider 配置 `wire_api="chat"` 走 chat completions | M6 |
| DeepSeek harness | OpenAI 兼容 `POST /v1/chat/completions` | M1 |
| zcode | 协议待实测确认（OpenAI chat 或 Anthropic 二者之一，两条线均在覆盖范围内） | M1 或 M3 |

## 参考项目

| 项目                                                 | 参考价值                                                     |
| ---------------------------------------------------- | ------------------------------------------------------------ |
| [New-API](https://github.com/QuantumNous/new-api)    | 协议转换（OpenAI ↔ Claude ↔ Gemini）、渠道加权随机、失败自动重试、用户级限流 |
| [SwitchAI](https://github.com/BlankLifeMan/SwitchAI) | **桌面架构参考**（Tauri 2.0 + Rust + React）、智能路由与故障转移、模型别名与映射、健康检查、Token 统计与仪表盘 |
| [CC-Switch](https://github.com/farion1231/cc-switch) | **开发者工具集成**思路（为 Claude Code、Codex 等工具提供一键配置） |
| [LiteLLM](https://github.com/BerriAI/litellm)        | 模型元数据（上下文窗口 / 最大输出）注册表快照来源             |

## 技术栈

| 层级      | 技术选型                      | 说明                                      |
| --------- | ----------------------------- | ----------------------------------------- |
| 桌面框架  | **Tauri 2.0**                 | 跨平台（Windows + macOS），体积小，性能好 |
| 后端语言  | **Rust**                      | 高性能，内存安全，适合网关场景            |
| HTTP 网关 | **Axum** + **Reqwest**        | Rust 生态成熟方案                         |
| 前端      | **React + TypeScript + TailwindCSS + shadcn/ui + Zustand** | SwitchAI 已验证组合；shadcn 组件生态最强；配合 TanStack Query 走 Tauri IPC |
| 数据库    | **SQLite**（通过 rusqlite）   | 轻量级，零配置，**唯一事实源（见数据持久化）** |
| 密钥存储  | **操作系统密钥环**（Keyring） | 敏感信息不入库                            |

> UI 框架已定案（原候选 React/Svelte/Vue 中选定 React），Vue/Svelte 不再评估。

## 平台支持

- ✅ Windows
- ✅ macOS

## 架构核心：协议转换矩阵

协议转换层按**资源类型**建模统一中间表示（消息 → tools → 参数 → finish_reason），各协议只需实现「该协议 ⇄ 中间表示」两个适配器，避免 N×M 的转换函数组合爆炸。

> 📐 类型定义、逐字段映射总表与行为规范：见 [docs/design/protocol-ir.md](docs/design/protocol-ir.md)（已定稿 v1）。
> 📐 表结构、密钥环集成与日志保留策略：见 [docs/design/storage-schema.md](docs/design/storage-schema.md)（已定稿 v1）。

MVP 必须支持的转换链路（含 tool/function calling 映射）：

```
入站 OpenAI   ──→ 出站 OpenAI     近透传（system 透传、参数映射）
入站 OpenAI   ──→ 出站 Claude     system 合并、tool_calls ↔ tool_use 映射、stop_reason 对齐
入站 OpenAI   ──→ 出站 Gemini     systemInstruction 映射、functionCall 对齐
入站 Anthropic ──→ 出站 Claude    近透传（Claude Code 直连场景）
入站 Anthropic ──→ 出站 OpenAI / Gemini
```

- Tool calling 转换覆盖：tools 定义、assistant 发起的调用、tool 结果消息回传
- 错误响应统一规范化为**请求方协议的错误格式**（入站是什么协议，就返回该协议的错误 schema）

## 功能需求

### 第一阶段（MVP）

#### 1. 供应商管理

- 用户可**添加自定义模型供应商**，配置项包括：
  - 供应商名称（自定义）
  - Base URL（如 `https://api.openai.com/v1`）
  - API Key（**存入操作系统密钥环，SQLite 仅保存密钥环引用 ID，secret 本体绝不落盘**）
  - API 协议类型（下拉选择：OpenAI 兼容 / Claude Messages / Google Gemini）
- 提供**测试连接**按钮：校验 Base URL + Key 可用性（自动发现接口连通性）
- 参考：SwitchAI 的多提供商管理能力

#### 2. 模型自动发现与配置

- 根据 Base URL 和 API Key **自动调用对应协议的模型列表接口**获取模型（OpenAI `/models`、Gemini `listModels` 需剥去 `models/` 前缀等适配）
- **内置热门模型元数据快照**（取自 LiteLLM model registry，随版本发布更新）：用于自动填充默认值
- 每个模型支持配置：
  - **上下文窗口大小**（默认值：命中快照则用快照值，否则保守默认 128k）
  - **最大输出 Token**（默认值：4096）
- 两项均可手动修改，不填使用默认值

#### 3. 代理服务

对外暴露**双协议代理入口**：

| 入站协议 | 端点                                            | 典型客户端                  |
| -------- | ----------------------------------------------- | --------------------------- |
| OpenAI   | `GET /v1/models`、`POST /v1/chat/completions`   | ChatGPT Next Web、Continue、任意 OpenAI 客户端、DeepSeek harness |
| OpenAI Responses | `POST /v1/responses`（随 M6 交付）      | **Codex CLI**               |
| Anthropic| `POST /v1/messages`                             | **Claude Code**（`ANTHROPIC_BASE_URL` 直连）、其他 Anthropic 客户端 |

- 旧版 `POST /v1/completions`（text completions）**不在 MVP 范围**，列入第二阶段；Responses API 因 Codex 为第一梯队客户端已提前纳入（见上表）
- zcode 的协议线待首次实测确认后归入对应行，验收标准同步补充
- 网关层提供一个**随机生成的 API Key**（如 `sk-jai-xxxx`，存 SQLite 的 `gateway_keys` 表），用于客户端认证，本机访问也强制鉴权
- 支持**流式响应（SSE）全链路透传/转换**；转发时注入 `stream_options: { include_usage: true }` 以采集 token 用量
- 支持 **tool/function calling** 跨协议转换（见「架构核心」）
- 路由规则（MVP 版）：
  - 根据请求的 `model` 字段自动路由到对应供应商渠道
  - **允许多个渠道提供同名模型**；请求时按渠道顺序尝试，失败自动顺延下一个渠道（轻量故障转移）

#### 4. 数据持久化

- 使用 **SQLite 作为唯一事实源**，不引入 YAML/JSON 主配置的双写
- 表结构（初版）：`providers`（含密钥环引用 ID）、`models`、`gateway_keys`、`request_logs`、`tool_id_map`（tool id 兜底映射）、`meta`（设置 KV）
- 网关密钥（sk-jai-\*）：明文存储、UI 可回显（常态脱敏显示前缀 + 一键轮换）；导出/同步排除该表
- `request_logs` 默认仅记**元数据**：时间、供应商、模型、状态码、耗时、prompt/completion token 数、错误摘要；**不落 prompt/响应明文**
- 「配置透明可备份」由设置页的**导出 JSON** 实现：导出文件剔除全部敏感字段（API Key、gateway key）
- SQLite 文件位于系统应用数据目录

#### 5. 基础 UI

- 供应商列表（增删改 + 测试连接 + 状态标识）
- 模型列表（查看、编辑上下文窗口/最大输出 Token）
- 网关状态卡（运行/停止、实际监听地址与端口、当前 gateway key 一键复制）
- 基础请求日志查看（元数据级别）
- 设置页（端口、日志级别、导出配置）
- 系统托盘支持快速启停

#### 6. 网关进程

- 默认监听 `127.0.0.1:1314`
- 端口被占用时**自动顺延寻找可用端口**，并在 UI 与托盘提示实际端口
- 支持自定义监听端口

### 第二阶段（后续迭代）

#### 7. WebDAV 同步与配置导入（2025-08 复议：提前至公测前，随路线图 M7 交付）

- 通过 WebDAV 将配置同步到云端，多设备保持一致
- 配置导入：读入导出 JSON，按供应商名称+Base URL 去重合并；导入后逐个引导补录密钥环凭据
- 同步内容 = 导出 JSON（**天然不含任何 API Key / gateway key**）
- 冲突策略：last-write-wins（v1）；换设备后仅需重新录入密钥环中的 Key

> 调度说明：多设备同步是本项目的核心动机之一，故从「二期」提前到公测（v0.1 beta）之前实现。

#### 8. 高级路由

- 权重负载均衡：多供应商间按权重分发
- 策略化故障转移：健康探测驱动的主备切换（MVP 的按序顺延是它的简化版）
- 模型别名/映射：将 `gpt-4o` 映射为 `deepseek-chat`

#### 9. 用量统计

- Token 用量统计与可视化图表（数据源：MVP 已落库的 request_logs + usage）
- 请求日志查询与导出

#### 10. 更多入站协议形态

- 旧版 `POST /v1/completions`
- （OpenAI Responses API 已因 Codex 第一梯队优先级提前至 M6，不再是二期条目）

### 远期规划（暂不定里程碑）

#### MCP 和 SKILL 管理 ⚠️ 边界待界定

- **配置托管（低成本，优先考虑）**：为 Claude Code、Continue 等客户端一键写入 MCP Server 配置（参考 CC-Switch 思路）
- **MCP Hub / 编排（高成本，谨慎评估）**：JAI 自己充当 MCP 网关，将工具编排进上游请求——涉及能力协商、工具命名冲突、鉴权等，复杂度量级远超前者，需独立立项论证后再排期

## 非功能需求

| 需求           | 说明                                              |
| -------------- | -------------------------------------------------- |
| **开箱即用**   | 下载安装包即可运行，无需额外部署                  |
| **纯本地运行** | 无服务器依赖，不强制联网                          |
| **跨平台**     | Windows + macOS 统一体验                          |
| **配置透明**   | 支持一键导出 JSON 备份（自动剔除敏感字段）；敏感信息存密钥环，绝不落盘 |
| **低资源占用** | 网关进程轻量，适合个人开发机常驻                  |
| **稳定性/可用性** | ⛳ **一票否决项（2025-08 确立）**：任何功能里程碑不得以牺牲稳定为代价换进度。全局基线见路线图「稳定性工程基线」；每个里程碑的验收标准均含稳定性子集 |

### 本地安全要求

- 默认仅监听回环地址 `127.0.0.1`，不暴露局域网
- 所有代理端点**即使本机访问也强制鉴权**（`sk-jai-*`）
- 校验 `Host` / `Origin` 头，防御 DNS rebinding 攻击（参考 Ollama/LM Studio 历史 CVE）
- CORS 白名单策略，默认拒绝浏览器跨域直连
- 请求日志默认仅元数据；明文调试模式需手动开启且醒目提示
- 密钥认证失败具备基础限速

### 发布工程（交付前必须完成）

- macOS：开发者签名 + 公证（Notarization）
- Windows：代码签名证书（规避 SmartScreen 警告）
- Tauri Updater 自动更新通道

## 待决策事项

1. ~~UI 框架选型~~ ✅ 已定：React + TailwindCSS + shadcn/ui + Zustand
2. ~~配置存储格式~~ ✅ 已定：SQLite 唯一事实源 + 导出 JSON
3. ~~开源协议~~ ✅ 已定：MIT
4. 模型元数据快照的**更新机制**：仅随版本发版更新，还是支持应用内联网刷新（MVP 先随版本走，是否联网刷新待观察需求）

## 需求评审记录

- **v2 定稿要点**：入站升级为 OpenAI + Anthropic 双协议（支撑 Claude Code 直连）；tool calling 转换纳入 MVP；确定 SQLite 单一事实源 + 导出备份；轻量故障转移（多渠道按序顺延）从二期提前至 MVP；开源协议定为 MIT；MCP/SKILL 移入远期规划并写明两种实现路径的成本差异。
- **v2.1**：新增设计文档《协议 IR》《存储层设计》；网关密钥定为明文可回显（附三项缓解）；日志默认 30 天/5 万行封顶。
- **v2.2**：新增开发路线图 [docs/design/roadmap.md](docs/design/roadmap.md)——MVP 切分为 M0–M7 细粒度里程碑，含各阶段验收标准与需求覆盖矩阵；配置导入调度至二期（与 WebDAV 同批）。
- **v2.3**：确立立项动机（杂牌 token 源收敛 + 多设备同步）；锁定第一梯队客户端（Claude Code / Codex / zcode / DeepSeek harness）；Responses API 因 Codex 提前至 M6；WebDAV 同步与导入提前至公测前（M7，覆盖 v2.2 的二期决策）；路线图扩展为 M0–M9；稳定性确立为全局一票否决项。
