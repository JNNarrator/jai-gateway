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

- UI「MCP」页加「允许代理执行」开关（per-server） `[完成]`
  - 前端：`McpServerRow` 加 `proxyAllowed` 字段（serde camelCase）、`api.mcpSetProxyAllowed` 封装、McpPage 每行加「代理」Switch（照抄 enabled toggle 模式）
- 审计日志 `[完成]`
  - 独立表 `proxy_call_logs`（migration 0008）+ `store::proxy_call_log`，与 `request_logs` 隔离（后者 `route_mode` 有 CHECK、会被 usage 聚合污染）
  - `proxy_call_tool` 每次调用记录 `{ts, server_name, tool_name, kind, status(ok/error), duration_ms, error}`，失败也记，异步落库
- 进程复用优化 `[完成]`
  - 实测（netcatty-external，6 次）：平均 **~198ms/次**（181-274ms），冷启动 274ms。超出设计预期 50-100ms，故按「实测超标才做」推进连接池
  - 方案：重构 `run_stdio_jsonrpc`（mcp.rs:127）为「懒初始化 + 进程复用」。每个 `(cmd,args,env)` 键维护一个可复用 stdio 进程：首次调用 spawn+initialize，后续 tools/call 复用已初始化进程；JSON-RPC id 递增；超时回收 + 崩溃/无响应重建；单进程顺序执行用锁串行化（不同 server 独立连接）
  - 落地（mcp.rs：`StdioPool` 全局池，键 = cmd/args/排序 env 的内容指纹，env 值只参与哈希不落池防密钥泄漏；`ConnState` 状态机 Uninit→Ready→Dead）
    - 懒初始化：首次调用 spawn + initialize 握手（initialize id=1，业务请求 id 从 2 递增）
    - 复用：后续 tools/list、tools/call 直接走已初始化进程，每次调用刷新 last_used
    - 空闲回收：获取时 `last_used` 超过 `JAI_MCP_POOL_IDLE_MS`（默认 30s）→ kill + 同调用内重建
    - 崩溃重建：获取时 `try_wait` 发现进程已退出 → 同调用内重建；请求中写失败/EOF → kill + 废弃（下次调用重建）；每次调用超时 `JAI_MCP_POOL_CALL_TIMEOUT_MS`（默认 120s，旧实现无限挂起）→ kill + 废弃
    - 串行化：每连接一个 tokio Mutex，同 server 请求顺序执行；不同 server 键不同、独立连接互不阻塞
    - 孤儿防护：`ReadyConn::drop` 兜底 `start_kill`，调用方中途取消（如 registry 10s 外层超时）也不会留孤儿进程
  - 测试：`tests/mcp_pool.rs` 7 项集成测试（假 server 二进制 `mcp_fake_server`，经 `CARGO_BIN_EXE_*` 引用）——进程复用（pid/count 状态延续）、崩溃重建（crash / die_after_response 两路径）、超时废弃后自动恢复、空闲回收重建、env 参与连接键（不同 env 独立进程、同 env 共享、env JSON 键序无关）、8 路并发串行化不串帧
  - 效果：进程内所有 stdio 工具调用（tools/list + tools/call 全入口）共享进程，冷启动开销从「每次 ~198ms」降到「仅首次 + 空闲超时后」

## 风险与对策

| 风险 | 对策 |
|---|---|
| stdio server 每次调用 spawn 进程（冷启动开销） | 已实测 ~198ms 超标，M3 连接池已落地（懒初始化+复用+超时回收+崩溃重建，见上），冷启动仅剩首次/回收后 |
| 动态工具列表膨胀（多 server × 多工具） | `proxy_allowed` 默认关 + 工具列表注明来源 + 必要时按 server 分组折叠 |
| 工具重名（两个 server 都有 get_environment） | 两级命名空间天然隔离（`serverA__get_environment` ≠ `serverB__get_environment`） |
| 代理执行越权风险 | `proxy_allowed` 显式开关 + 来源标注 + 文档写明权限语义 |

## 验收标准

1. dsh-tui 只注册 jai-registry，`tools/list` 能看到已开启代理的 server 的全部工具（带 `[proxy: …]` 前缀）。
2. dsh agent 调用 `<server>__<tool>` → 网关转发 → 真实结果回传（isError 语义正确）。
3. dsh agent 调用 `skill__<name>` → 拿到 Skill 全文并遵循。
4. 未开启 `proxy_allowed` 的 server 工具不可见、不可调用。
5. `cargo test -p gateway-core` 全绿、clippy 零警告。
