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
   - 协议（zcode 内部叫 `api.type`）：三选一，见下表
   - Base URL：按所选协议填 —— **三条线规则不一样**，见下表
   - API Key：JAI 网关页显示的 `sk-jai-xxxx`
2. 添加要使用的模型，模型名与 JAI「模型」页一致。
3. 选择该 Provider 与模型后开始对话。

### Base URL 怎么填（三条线规则不一样）

zcode 用 AI SDK 起请求，**不同协议线的 URL 拼法不同**（`adapters/src/model/model-execution.ts`）：

| `api.type` | zcode 的拼法 | 填给 JAI 的 Base URL | 最终打到 |
| --- | --- | --- | --- |
| `anthropic-messages` | 缺 `/v1` 自动补，再追加 `/messages` | `http://127.0.0.1:1314` | `/v1/messages` |
| `openai-responses` | **原样**追加 `/responses` | `http://127.0.0.1:1314/v1` | `/v1/responses` |
| `openai-chat-completions` | **原样**追加 `/chat/completions` | `http://127.0.0.1:1314/v1` | `/v1/chat/completions` |

- `anthropic-messages` 线填 `http://127.0.0.1:1314` 或 `http://127.0.0.1:1314/v1` **都对**
  （zcode 会归一：已以 `/v1` 结尾就不动）；
- 另两条线**必须显式带 `/v1`**，否则会打到 `http://127.0.0.1:1314/responses` → JAI 无此路由 → 404。

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

### 两条线各自发的形态（zcode 源码口径）

zcode 的推理档位不是硬编码字段，而是**按模型规则表做 JSON merge-patch** 注入请求体
（`config/provider/zcode-builtin.json` 的 `reasoningLevel.map` + `packages/model-option-map`）。
所以不同协议线注入的字段名不同，JAI 三条都认：

| 协议线 | zcode 注入 | JAI 入站读取 |
| --- | --- | --- |
| `openai-responses` | `reasoning:{"effort": …}`（关闭时 `"none"`） | → `reasoning_effort` |
| `openai-chat-completions` | `reasoning_effort`（部分模型另带 `thinking` / `enable_thinking`） | → `reasoning_effort` |
| `anthropic-messages` | `thinking:{"type":"adaptive"}` + `output_config:{"effort": …}`；关闭时 `thinking:{"type":"disabled"}` | → `reasoning_effort` |

**2026-09-22 修复前**：`output_config` 在 JAI 里**没有任何引用**（连「未建模已丢弃」的
`CapabilityWarn` 都不产生）→ 走 Anthropic 线的跨族转换时，用户选的档位**静默丢失**。
现在 `output_config.effort` 会读成 `reasoning_effort`，`thinking.type:"disabled"` 归一为 `none`；
只有 `enabled` / `adaptive` 而不带档位时**不臆断**成具体档位（保持不干预）。

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

## 工具声明数（tools）超限

**背景（2026-09-18 真机故障）**：zcode 这类 agent 会声明大量工具（内置 + MCP，实测 140 个），
JAI 早期按 **128 硬拦**，于是整个 turn 直接失败：

```
Turn execution failed
provider_code=tools_limit_exceeded  reason=invalid_request  status=400
工具声明数 140 超过上游上限 128
```

而**上游实测 140 / 300 个工具都返回 200** —— 这个 128 是**网关自己发明的限制**（能力对齐表的
经验值），并非任何上游的真实约束，等于误杀了上游完全能跑的配置。

**现在（0.2.7+）**：

- **未声明 = 不拦**：放行给上游，上游真报错时其错误原文照常回给客户端（链路不再被网关截断）；
- **声明了就严格执行**：供应商页卡片「工具上限 …」或模型页「上限?」芯片填一个数字（如 128），
  超限即 400 `tools_limit_exceeded`，文案给出实际个数与上限；
- 模型级覆盖供应商级；清除声明 = 回到「不拦」。跨族转换与同族直通两条路径语义一致。

想知道某家上游到底卡多少，直接拿一个多工具请求试它即可（网关会如实回传上游错误）。

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
2. **zcode 侧**：`~/.zcode/v2/provider_config.json` 里该 provider 的 `api.type` 三选一 ——
   `anthropic-messages` / `openai-chat-completions` / `openai-responses`。
   注意枚举值就是 **`anthropic-messages`**，不是 `anthropic`：`anthropic` 是 zcode 内部由
   `api.type` 映射出的 AI SDK provider kind（`toAiSdkProviderConfig`），两者别混。
   它决定走 JAI 的哪条入站线；`baseUrl` 按上面「Base URL 怎么填」的表核对；`apiKey` 与 JAI 网关页一致。
3. **zcode 的模型元数据**：`config.json` 里该 provider 的模型若没有 `reasoning` 字段，
   zcode 会按「无推理」发 `effort=none`（这就是本次触发条件）；补上档位声明或让 JAI 侧声明值域都可解。

## 备注

- **✅ 2026-09-01 真机实测通过**：zcode 真实会话经 JAI 收到正常回复，实测走 **OpenAI Responses 入站**（`/v1/responses`，非本指南初稿预想的 Anthropic 线），流式/非流式均 200。以实测为准：zcode Provider 的配置形态决定协议线，两条线（Responses / Anthropic）网关均已支持并经真实流量验证。
- **✅ 2026-09-18 真机实测通过（推理档位值域）**：上游直连复验 —— 不带 `reasoning_effort` → 200，
  带 `reasoning_effort:"low"` → 200；即修复后 JAI 会发出的两种形态（丢弃 / 收敛）上游都接受。
- **✅ 2026-09-22 源码对账修复（zcode 开源后）**：结合 `zai-org/ZCode` 源码修掉三处适配缺陷 ——
  ① **多段 `system` → Anthropic 上游**不再被判 400（按 IR 契约 `\n\n` 合并，与 openai /
  responses / gemini 三个 encoder 对齐）。注意这条的触发面**不在 zcode 的 Anthropic 线本身**：
  该 encoder 只在**上游族是 anthropic** 时被调用，而 Anthropic 入站 × Anthropic 上游是同族
  直通（不过 encoder）；真正会踩的是「**别的入站族** × Anthropic 上游」—— 例如客户端在
  chat-completions 线发了两条 `system` 消息；
  ② 入站 **`thinking` 内容块**不再丢弃（跨族到 thinking 上游要靠这段文本重建
  `reasoning_content`，丢掉会让上游在后续轮次校验 400）；
  ③ **`output_config.effort` / `thinking.type`** 接入推理档位归一（见上文「两条线各自发的形态」）。
- 如 zcode 后续版本调整协议形态，接入方式同步更新本指南。
