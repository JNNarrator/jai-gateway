# JAI 统一 MCP 代理 + Skill 投递 — 设计文档

日期：2026-09-03
状态：已获用户批准（先出文档、提交推送后开发）

## 背景与目标

### 用户需求

dsh-tui 驱动 dsh 时，要让 agent 使用 JAI 里配置好的 MCP 工具和 Skill，**dsh 只注册 jai-registry 一个入口**：

1. 发现并**调用**所有已配置的 MCP 工具（网关显式代理转发）
2. 发现并**加载**所有已配置的 Skill（按需投递全文）

### 为什么不是「恢复旧方案」

旧方案（commit d03d5fb，后 d2993f5 删除约 870 行）是**网关强制注入 + 自动执行循环**：网关把 MCP 工具偷偷塞进每次请求（`MAX_MCP_TOOL_ROUNDS=8` 自动往返），会污染 agent 行为——模型不知道工具被塞进来、中间步骤被网关吞掉、dsh 会话历史里出现「幽灵工具调用」。删除是正确的。

新方案与旧方案的本质区别：

| | 旧方案（已删） | 新方案 |
|---|---|---|
| 工具进入 agent 视野 | 网关**强制注入**请求工具定义 | dsh 只注册 jai-registry，registry **作为一个 MCP server 暴露**全部工具 |
| 谁决定调用 | 上游模型被动收到 | dsh 的模型**主动选择**（工具就在它工具列表里） |
| 谁执行 | 网关偷偷代跑循环 | 网关**显式转发**（dsh 调 `tools/call` → 网关转发到真实 server） |
| 会话污染 | 有（幽灵工具） | 无（工具调用在 dsh 侧可见、可控） |

**底线**：不做隐式注入、不替 agent 跑工具循环——工具/技能的选择权始终在 agent，网关只做显式转发/投递。

## 现状事实（探索取证）

- `registry.rs`（564 行）是只读台账：5 个工具 `list_mcp_servers` / `get_mcp_server_detail` / `get_tool_schemas` / `list_skills` / `get_skill_detail`，`dispatch_tool` 无转发能力，`tools/call` 只返回文本。
- `mcp.rs`（311 行）已有完整客户端原语：`list_tools(server)`（stdio/http）、`call_tool(server, name, args)`——**代理转发可完全复用**，stdio 走 spawn 子进程 + 换行分隔 JSON-RPC，http/sse 走 POST。
- 表结构：`mcp_servers(id,name,kind,command,args,url,enabled,…)`、`skills(id,name,description,content,enabled,…)`；`McpServerRow` 无代理开关列。
- **dsh 侧 skill 机制**（会话日志取证）：system prompt 注入 `<available_skills>` 目录块（名+描述），再给模型一个 `skill` 工具按名加载全文——是「目录 + 按名加载」的拉取模式。
- **dsh-mcp-client 只支持 tools**（lib/index.js 仅 import `ListToolsResultSchema` / `ToolListChangedNotificationSchema`）——MCP resources/prompts **到不了 agent**，skill 不能走 resources 路线。
- MCP 工具与 Skill 本质不同：工具是**可执行函数**（调用→拿结果），Skill 是**文本指令**（读进上下文→照做）。skill 需要的是「按需投递」而非「代理执行」。

## 已拍板决策

- 采用**两级命名空间 + 动态工具**方案（见设计）。
- MCP 工具命名 `<server_name>__<tool_name>`，按**第一个双下划线**分割。
- Skill 命名 `skill__<skill_name>`，独立前缀避免与真实 MCP 工具重名。
- 网关只做显式转发，不注入 system、不代跑工具循环。
- MCP 代理执行需要 `mcp_servers.proxy_allowed` 开关（默认关），权限边界显式化。
- 动态工具列表 30s TTL 缓存。
- 有状态/长时工具（terminal 会话类）第一版默认不开放，由用户逐 server 开启。

## 设计

### 1. 架构

```
dsh (只注册 jai-registry)
  │  MCP protocol（mcp__jai-registry__ 前缀由 dsh 自动加）
  ▼
jai-registry (gateway /mcp)
  ├─ 静态只读工具（保留）：list_mcp_servers / get_mcp_server_detail /
  │    get_tool_schemas / list_skills / get_skill_detail
  ├─ 动态代理工具：mcp__jai-registry__<server>__<tool>   ← MCP 工具转发
  └─ 动态技能工具：mcp__jai-registry__skill__<name>       ← Skill 全文投递
  ▼
真实 MCP server（stdio/http，复用现有 mcp.rs 客户端）
```

### 2. 表结构变更（migration 0007）

```sql
ALTER TABLE mcp_servers ADD COLUMN proxy_allowed INTEGER NOT NULL DEFAULT 0;
```

`proxy_allowed=1` 的 server 才进入动态工具列表、才可被转发调用。

### 3. `tools/list` → 动态聚合

现有静态数组**追加**动态生成的工具：

- **动态 MCP 工具**：遍历 `mcp_servers(enabled=1 AND proxy_allowed=1)` → `mcp::list_tools(server)`（已有能力）→ 每条映射 `{ name: "<server>__<tool>", description: "[proxy: <server>] 原描述", inputSchema: 原文 }`。
- **动态 Skill 工具**：遍历 `skills(enabled=1)` → `{ name: "skill__<name>", description: skills.description, inputSchema: {"type":"object"} }`。
- **30s TTL 缓存**：动态部分缓存，避免每次 tools/list 都 spawn 子进程。
- **失败容错**：单个 server list_tools 失败 → 跳过该 server，不阻塞整体。

### 4. `tools/call` → 转发分派

`dispatch_tool` 增加分支，保留现有 5 个只读工具：

```
name = "<server>__<tool>" 且 server 存在且 proxy_allowed=1
    → mcp::call_tool(server, tool, arguments)（已有能力）
    → 成功：{ content:[{type:"text", text:...}], isError:false }
    → 失败：isError:true + 错误信息（含来源 server 名）

name = "skill__<name>" 且 skill 存在且 enabled
    → 返回 skills.content 全文（复用 tool_skill_detail 读取逻辑）
    → 超长截断策略：32KB 上限 + 注明「已截断」

其他 → 现有只读工具 / 未知工具报错
```

### 5. 元数据标注

- 动态工具 `description` 前缀 `[proxy: <server>]`——agent 与审计都清楚来源。
- `tools/call` 转发结果可选附 `source: "jai-gateway-proxy"`。

## 明确不做的事（边界）

| 边界 | 处理 |
|---|---|
| 有状态/长时工具（terminal 会话类） | 第一版默认不开放：`proxy_allowed` 逐 server 开启，文档提示有状态工具建议客户端直连 |
| 网关替 agent 跑工具循环 | 不做。agent 调一次 `tools/call` → 网关转发一次 → 返回，agent 自己决定下一步 |
| Skill 注入 system | 不做。保持「agent 按名调用 → 网关投递全文」拉取模式（上轮删除的教训） |
| MCP resources/prompts | 不做。已验证 dsh-mcp-client 只支持 tools |
| 同族请求字节直通 | 不动。registry 是独立 /mcp 端点，与 /v1/* 代理链路完全隔离 |

## 实施里程碑

### M1：MCP 工具代理（核心价值）

- 表结构：`mcp_servers` 加 `proxy_allowed`（migration 0007）
- `registry.rs`：动态 tools/list + tools/call 转发 + TTL 缓存 + 来源标注
- 单测：
  - 工具命名解析（`server__tool` 拆分、server 名含下划线）
  - 动态列表聚合（proxy_allowed 过滤、失败跳过、TTL 命中）
  - 转发成功 / 失败 / isError
- 端到端验证：dsh-tui 注册 jai-registry → 调 `netcatty-external__get_environment` → 返回真实结果

### M2：Skill-as-Tool 投递

- `registry.rs`：动态 `skill__<name>` 工具 + 全文投递 + 截断策略
- 单测 + 端到端：dsh agent 加载 skill 全文并遵循

### M3：加固

- UI「MCP」页加「允许代理执行」开关（per-server）
- 调用日志/审计：proxy 调用落 request_logs 或独立表
- 进程复用优化（stdio server 冷启动 50-100ms，如需再优化）

## 风险与对策

| 风险 | 对策 |
|---|---|
| stdio server 每次调用 spawn 进程（冷启动开销） | M1 接受（一次调用一次进程，简单可靠）；M3 若实测超标再做连接池 |
| 动态工具列表膨胀（多 server × 多工具） | `proxy_allowed` 默认关 + 工具列表注明来源 + 必要时按 server 分组折叠 |
| 工具重名（两个 server 都有 get_environment） | 两级命名空间天然隔离（`serverA__get_environment` ≠ `serverB__get_environment`） |
| 代理执行越权风险 | `proxy_allowed` 显式开关 + 来源标注 + 文档写明权限语义 |

## 验收标准

1. dsh-tui 只注册 jai-registry，`tools/list` 能看到已开启代理的 server 的全部工具（带 `[proxy: …]` 前缀）。
2. dsh agent 调用 `<server>__<tool>` → 网关转发 → 真实结果回传（isError 语义正确）。
3. dsh agent 调用 `skill__<name>` → 拿到 Skill 全文并遵循。
4. 未开启 `proxy_allowed` 的 server 工具不可见、不可调用。
5. `cargo test -p gateway-core` 全绿、clippy 零警告。
