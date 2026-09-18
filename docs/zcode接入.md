# zcode 接入 JAI 指南

> 目标：zcode 通过 JAI 访问上游模型时，体验与直连一致。

## 原理

zcode 支持自定义 Provider，当前本机 zcode 配置使用 `kind: "anthropic"` + `baseURL` 的形态。
因此让 zcode 走 JAI 的最简方式：

- zcode Provider → `http://127.0.0.1:<JAI端口>`（Anthropic 协议）
- API Key → JAI 网关 Key（`sk-jai-*`）
- 模型 → JAI 中已启用且对应该 Provider 的模型名

JAI 收到 Anthropic `/v1/messages` 后会按模型路由到真实上游（OpenAI 兼容 / Anthropic / Gemini / Responses 均可），
跨协议转换由 JAI 完成。

## 在 zcode 中添加 JAI Provider

1. 打开 zcode 的模型/Provider 设置，添加自定义 Provider：
   - 名称：`JAI`
   - 类型/协议：`anthropic`
   - Base URL：`http://127.0.0.1:1314`
   - API Key：JAI 网关页显示的 `sk-jai-xxxx`
2. 添加要使用的模型，模型名与 JAI「模型」页一致。
3. 选择该 Provider 与模型后开始对话。

## 模型名怎么填（多供应商必须区分时的唯一注意点）

JAI 支持 **`供应商名/模型名`** 限定键（`server/proxy.rs` 的 `split_once('/')`），
所以「好几个供应商都有同名模型、必须区分」时，直接写限定名即可：

- ✅ `基元律动/deepseek-flash` —— 斜杠前是 JAI「供应商」页的名称，必须**逐字一致**；
- ❌ `基元律动\deepseek-flash` —— **反斜杠不是分隔符**，会被当成一个字面模型名 →
  404「模型 X 不存在或其渠道未启用」（2026-09-18 实测踩过）；
- ✅ 裸模型名 `deepseek-flash` 也支持，此时按各渠道 `priority/weight` 路由（多供应商同名时才需要限定名）。

斜杠前那段**只用于 JAI 内部选渠道**，发给上游前会被剥掉（`rewrite_body_model`），
不会污染上游模型名，也不会因为上游不认识「供应商名」而失败。

## 推理档位（reasoning effort）：供应商/模型级值域声明

**背景（2026-09-18 真机故障）**：zcode 的 Responses 请求会带 `reasoning:{"effort":"none"}`
（其 provider 未声明推理能力时按「无推理」发 `none`）。JAI 默认**原样透传** `reasoning_effort`，
而上游「基元律动」只认 `low/medium/high/xhigh/max`：

```
400 {"code":"UNSUPPORTED_FIELD",
     "message":"DeepSeek reasoning_effort 只支持 low、medium、high、xhigh、max",
     "data":{"field":"reasoning_effort"}}
```

zcode 把上游 400 统一包装成 **`Provider rejected the model request.`** —— 看到这句，
去 JAI **日志页**看该行的 `error_summary`（那里是上游原文），不要从模型名上找原因。

### 声明方式（JAI 0.2.6+）

- **供应商页** → 卡片上的「档位 …」→ 填声明序逗号串，如 `low,medium,high,xhigh,max`
  （预设里第一条就是基元律动对 DeepSeek 的实测值域）；
- **模型页** → 模型名旁的「档位?」芯片 → 模型级覆盖供应商级（同一供应商下不同模型档位不同时用）；
- **留空/清除 = 未声明 ⇒ 原样透传**（老供应商行为完全不变）。

### 归一规则（客户端值 → 上游值）

| 客户端值 | 声明值域含该值 | 声明值域不含 | 说明 |
|---|---|---|---|
| `high` 等域内值 | 原样透传 | — | 不擅自改写客户端意图 |
| `none` / `off` / `disabled` | 原样透传 | **丢弃该参数** | 上游无法表达「关掉推理」→ 不传即上游默认；不会硬塞一个注定 400 的值 |
| `minimal` / `xhigh` 等 | 原样透传 | **收敛到域内最近档** | 低于下限 → 取最低档；高于上限 → 取最高档 |

每次丢弃/改写都会在日志页留一条 `CapabilityWarn`（状态 200），说明「为什么这个参数没发给上游」。
同族直通（客户端协议 = 上游协议）与跨族转换两条路径都会归一。

## 让 zcode 加载 JAI 托管的 MCP

JAI「MCP」页点击 **复制客户端配置**，会得到标准 `mcpServers` JSON。
zcode / Claude Code / Continue 等支持标准 MCP 配置的 Agent 可将其写入自己的 MCP 配置文件，
加载后由 Agent 自行决定何时调用这些 MCP 工具。

## 让 zcode 看到 JAI 托管的 Skill

JAI「技能」页点击 **复制技能包**，会把启用中的技能导出为 Markdown。
可将该文本放入 zcode 的 System Prompt / 项目说明 / 技能目录，Agent 即可看到这些技能内容并自行调度。

## 排查清单（连接失败时按序看）

1. **JAI 日志页**看该请求的 `http_status` / `error_kind` / `error_summary`：
   - `404 InvalidRequest`「模型 X 不存在或其渠道未启用」→ 模型名写法（反斜杠 / 供应商名不一致 / 模型未启用）；
   - `400 InvalidRequest` + `[convert] 上游名：{...}` → **上游拒绝的参数**（如本次的 `reasoning_effort` 值域），按上文声明档位。
   - `403/502 UpstreamAuth` → 上游 API Key 失效（供应商页「测试连接」可复现）。
2. **zcode 侧**：`~/.zcode/v2/provider_config.json` 里该 provider 的 `api.type`（`openai-responses` /
   `openai-chat-completions` / anthropic）决定走 JAI 的哪条入站线，`baseUrl` 是否指向
   `http://127.0.0.1:1314/v1`、`apiKey` 是否与 JAI 网关页一致。
3. **zcode 的模型元数据**：`config.json` 里该 provider 的模型若没有 `reasoning` 字段，
   zcode 会按「无推理」发 `effort=none`（这就是本次触发条件）；补上档位声明或让 JAI 侧声明值域都可解。

## 备注

- **✅ 2026-09-01 真机实测通过**：zcode 真实会话经 JAI 收到正常回复，实测走 **OpenAI Responses 入站**（`/v1/responses`，非本指南初稿预想的 Anthropic 线），流式/非流式均 200。以实测为准：zcode Provider 的配置形态决定协议线，两条线（Responses / Anthropic）网关均已支持并经真实流量验证。
- **✅ 2026-09-18 真机实测通过（推理档位值域）**：上游直连复验 —— 不带 `reasoning_effort` → 200，
  带 `reasoning_effort:"low"` → 200；即修复后 JAI 会发出的两种形态（丢弃 / 收敛）上游都接受。
- 如 zcode 后续版本调整协议形态，接入方式同步更新本指南。
