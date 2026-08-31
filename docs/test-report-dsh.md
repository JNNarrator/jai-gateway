# dsh 真机联调测试报告

> 测试目标：以本机 DeepSeek Harness（dsh）为第一梯队客户端，验证 JAI 网关已有功能在真实模型链路上的可用性。
> 测试日期：2026-08-30
> 环境：Windows + dsh `0.1.1-rc.2` + JAI 独立网关（`examples/jai_standalone`）

## 0. 第二轮：UI 2.0 阶段 0 后全流程回归（2026-08-31）

> 环境：Windows + dsh `0.1.1-rc.2`（本机 `~/.dsh` profile + settings.yaml provider 接入）
> + JAI 桌面应用（debug 构建，网关 127.0.0.1:1314）
> 上游：onemodel（openai_responses，14 模型）、jiyuanlvdong（openai_compat，20 模型），
> 出站经 Clash（127.0.0.1:7890）。

| # | 场景 | 结果 | 说明 |
|---|------|------|------|
| 1 | `GET /healthz` | ✅ | `{"ok":true}` |
| 2 | `GET /v1/models` | ✅ | 34 个模型，按供应商命名空间 |
| 3 | Chat 链路：dsh → 网关 → jiyuanlvdong `glm-5`（流式） | ✅ | usage 8238/24 落库 |
| 4 | Responses 链路：dsh → 网关 → onemodel `deepseek-v4-pro`（流式） | ✅ | usage 8369/63 落库 |
| 5 | MCP 工具循环：dsh → 网关自动合并 MCP 工具 → 自动执行 echo | ✅ | 回复 `Echo: MCP-FLOW-OK` |
| 6 | Skill 注入：暗号探针 | ✅ | 模型精确回答 `JAI-SKILL-OK-2026` |
| 7 | 日志/用量落库 | ✅ | `request_logs` 含 openai/responses 两族，502 错误行亦有记录 |

### 本轮发现并修复

**MCP stdio 客户端把通知行误当响应（commit 26e7add）**

- 现象：`server-everything` 等 MCP server 在收到 `notifications/initialized`
  后会先输出 `notifications/message` 日志通知帧，「列出工具」报
  「MCP 响应缺少 result」。
- 根因：`gateway-core::mcp::run_stdio_jsonrpc` 写出请求后只 `read_line`
  一次即视为响应；无 `id` 的通知帧被解析为响应体。
- 修复：循环读取，跳过无 `id` 的通知行与非 JSON 噪音行，只认响应帧。
- 回归：`cargo test -p gateway-core` 104 个单测全绿；dsh 真机工具循环通过。

### 环境备注

- jai.exe（reqwest）不读 Windows 系统代理；访问需代理的上游时需以
  `HTTPS_PROXY=http://127.0.0.1:7890` 启动，否则 502 `all_providers_failed`。
- dsh 通过 `~/.dsh/settings.yaml` 增加 `jai`（openai-completions）/`jai-responses`
  （openai-responses）两个 provider（baseURL `http://127.0.0.1:1314/v1`，
  apiKeyEnv `JAI_GATEWAY_KEY`）接入网关，默认模型保持 `deepseek-official` 不变。

## 1. 测试拓扑（第一轮，2026-08-30）

```
dsh (headless)
  │  OpenAI Chat / OpenAI Responses
  ▼
JAI 网关 (127.0.0.1)
  │  路由 / 转换 / MCP / Skill / 别名 / 故障转移
  ▼
真实上游：
  - onemodel（openai-responses，deepseek-v4-pro）
  - jiyuanlvdong（openai-completions，glm-5 / kimi-k2.6）
```

## 2. 测试结果

| # | 功能 | dsh 场景 | 结果 | 说明 |
|---|------|----------|------|------|
| 1 | 网关健康检查 | `GET /healthz` | ✅ 通过 | 返回 `{"ok":true}` |
| 2 | 模型列表 | `GET /v1/models` | ✅ 通过 | 返回 3 个可用模型 |
| 3 | OpenAI Chat 直通 | dsh → JAI → jiyuanlvdong `glm-5` | ✅ 通过 | dsh 正常输出 |
| 4 | OpenAI Responses 直通 | dsh → JAI → onemodel `deepseek-v4-pro` | ✅ 通过 | dsh 正常输出 |
| 5 | 多渠道故障转移 | `glm-5` 一级 broken → 二级 jiyuanlvdong | ✅ 通过 | 日志命中 `p-jiyuanlvdong` |
| 6 | 模型别名/映射 | dsh 请求 `alias-test` → 上游 `glm-5` | ✅ 通过 | 路由与上游均正常 |
| 7 | MCP 工具自动合并/执行 | dsh 要求调用 `jai_echo` | ✅ 通过 | JAI 自动执行并回填 |
| 8 | Skill 自动注入 | dsh 询问暗号 | ✅ 通过 | 模型回答 `JAI-SKILL-OK` |
| 9 | 日志落库 | 请求后查询 `request_logs` | ✅ 通过 | 含 `openai` / `responses` 两族 |
| 10 | 用量统计落库 | `usage_input / usage_output` | ✅ 通过 | 多条请求均有 usage |

## 3. 测试过程中发现并修复的问题

### Responses + MCP 合成 SSE 不兼容 dsh

- 现象：启用 MCP 后，dsh 走 `/v1/responses` 报
  `Cannot read properties of undefined (reading 'startsWith')`。
- 根因：MCP 自动循环会把上游切成非流式，再把最终响应合成为 SSE；
  旧合成器只输出 `data:` 帧，缺少 `event:` 行、`output_text.done`、
  `content_part.done`、完整 `output_item.done` 和 `[DONE]`，与真实
  Responses SSE 形状不一致，导致 dsh/pi-ai 解析失败。
- 修复：新增 `codec::responses::render_response_sse()`，按真实 Responses SSE
  形状输出完整事件序列；代理侧 Responses 入站合成流式时使用该函数。
- 回归：新增 `render_response_sse_has_event_lines_and_done` 单测；
  `cargo test -p gateway-core` 全绿，`clippy -D warnings` 通过。

## 4. 日志/用量证据

`request_logs` 抽样（本次联调）：

| inbound_family | model | http_status | is_stream | usage_input | usage_output | provider |
|---|---|---|---|---|---|---|
| openai | glm-5 | 200 | 1 | 8246 | 25 | p-jiyuanlvdong |
| responses | deepseek-v4-pro | 200 | 1 | 7134 | 409 | p-onemodel |

## 5. 尚未覆盖/需要人工验收

- dsh 真实工具循环（非 MCP 自带的 dsh 工具）在跨族转换下的完整多轮验证
- dsh TUI 交互模式（本次使用 headless）
- WebDAV 真机同步
- 48h 常驻稳定性
- 发布签名/公证安装包

## 6. 结论

dsh 在 Chat Completions 与 Responses 两条主链路上均可通过 JAI 正常使用；
MCP、Skill、别名映射、故障转移在真实 dsh 场景下验证通过。发现并修复了
Responses+MCP 合成 SSE 兼容性问题，dsh 链路现已恢复。
