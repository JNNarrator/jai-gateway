# dsh 真机联调测试报告

> 测试目标：以本机 DeepSeek Harness（dsh）为第一梯队客户端，验证 JAI 网关已有功能在真实模型链路上的可用性。
> 测试日期：2026-08-30
> 环境：Windows + dsh `0.1.1-rc.2` + JAI 独立网关（`examples/jai_standalone`）

## 1. 测试拓扑

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
