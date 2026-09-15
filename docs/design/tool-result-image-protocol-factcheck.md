# 工具结果能否携带图片 — 四协议事实核查

核查日期：本次会话内（所有结论均基于下方列出的**当时抓取到的原文**）
方法：优先官方文档正文；官方文档站点不可达时，改用**官方 OpenAPI/OpenAPI-spec 文件、官方 proto 定义（googleapis）、官方 SDK 类型定义**。

## 结论表

| 协议 | 能否在工具结果内携带图片 | 精确结构形状（JSON 示例） | 置信度 | 官方文档/规范 URL |
|---|---|---|---|---|
| **Anthropic Messages API** | **是**（`tool_result.content` 可为 content block 数组，其中 `{"type":"image","source":{...}}` 合法） | ```json\n{"role":"user","content":[\n  {"type":"tool_result","tool_use_id":"toolu_01A09q90qw90lq917835lq9","content":[\n    {"type":"text","text":"15 degrees"},\n    {"type":"image","source":{"type":"base64","media_type":"image/jpeg","data":"/9j/4AAQSkZJRg..."}}\n  ]}\n]}\n``` | **高** | https://platform.claude.com/docs/en/agents-and-tools/tool-use/handle-tool-calls （"Handling results from client tools" → Accordion "Example of tool result with images"）<br>类型定义：https://github.com/anthropics/anthropic-sdk-typescript/blob/main/src/resources/messages/messages.ts （`ToolResultBlockParam`） |
| **OpenAI Chat Completions API** | **否**（`role:"tool"` 的 `content` 只允许 string 或 `text` part 数组，**image part 被规范明确排除**） | ```json\n// 允许：\n{"role":"tool","tool_call_id":"call_abc","content":"15 degrees"}\n{"role":"tool","tool_call_id":"call_abc","content":[{"type":"text","text":"15 degrees"}]}\n// 不允许：content 数组中出现 {"type":"image_url", ...}\n``` | **高** | https://github.com/openai/openai-openapi/blob/master/openapi.yaml （`components.schemas.ChatCompletionRequestToolMessage` / `ChatCompletionRequestToolMessageContentPart`，spec version 2.3.0）<br>SDK：https://github.com/openai/openai-node/blob/master/src/resources/chat/completions/completions.ts （`ChatCompletionToolMessageParam`） |
| **OpenAI Responses API** | **是**（`function_call_output.output` 可为数组，元素为 `input_text` / `input_image` / `input_file`） | ```json\n{"type":"function_call_output","call_id":"call_abc","output":[\n  {"type":"input_text","text":"screenshot captured"},\n  {"type":"input_image","image_url":"data:image/png;base64,iVBORw0KGgo...","detail":"auto"}\n]}\n```\n也支持 `{"type":"input_image","file_id":"file-123"}`；`image_url` 为「完全限定 URL 或 data URL 形式的 base64」。 | **高** | https://github.com/openai/openai-openapi/blob/master/openapi.yaml （`FunctionCallOutputItemParam` → `InputTextContentParam \| InputImageContentParamAutoParam \| InputFileContentParam`；读取侧 `FunctionCallOutputItemResource`/`FunctionCallOutputResource`）<br>SDK：https://github.com/openai/openai-node/blob/master/src/resources/responses/responses.ts （`ResponseFunctionCallOutputItemList`） |
| **Google Gemini API (`generateContent`)** | **是（仅 v1beta；且仅 `inlineData`）** — `FunctionResponse.parts` 存在，其中 `inlineData` 可携带图片字节 | ```json\n{"contents":[{"role":"user","parts":[\n  {"functionResponse":{\n    "name":"get_screenshot",\n    "response":{"output":"captured"},\n    "parts":[{"inlineData":{"mimeType":"image/png","data":"iVBORw0KGgo..."}}]\n  }}\n]}]}\n``` | **中**（proto + 两套官方 SDK 一致；但 v1（GA）表面无法确认，且官方文档站本次不可达） | https://github.com/googleapis/googleapis/blob/master/google/ai/generativelanguage/v1beta/content.proto （`message FunctionResponse` 中 `repeated FunctionResponsePart parts = 8`）<br>https://github.com/googleapis/googleapis/blob/master/google/cloud/aiplatform/v1beta1/tool.proto （Vertex v1beta1 同字段，且 `FunctionResponsePart` 另有 `file_data`） |

---

## 逐条原文证据

### 1. Anthropic Messages API —— 是

官方文档正文（tool_result 的 `content` 字段说明，原文）：

> * `content` (optional): The result of the tool, as a string (for example, `"content": "15 degrees"`), a list of nested content blocks (for example, `"content": [{"type": "text", "text": "15 degrees"}]`), or a list of document blocks (for example, `"content": [{"type": "document", "source": {"type": "text", "media_type": "text/plain", "data": "15 degrees"}}]`). **These content blocks can use the `text`, `image`, `document`, or `search_result` types.**

同页折叠示例标题即 `Example of tool result with images`，内容为 `tool_result.content` 数组内 `{"type":"image","source":{"type":"base64","media_type":"image/jpeg","data":"/9j/4AAQSkZJRg..."}}`。

官方 SDK 类型（`src/resources/messages/messages.ts`，v0.125.0）：

```ts
export interface ToolResultBlockParam {
  tool_use_id: string;
  type: 'tool_result';
  cache_control?: CacheControlEphemeral | null;
  content?:
    | string
    | Array<
        | TextBlockParam
        | ImageBlockParam
        | SearchResultBlockParam
        | DocumentBlockParam
        | ToolReferenceBlockParam
        | BrowserStateBlockParam
      >;
  is_error?: boolean;
  toolset_name?: string | null;
}

export interface ImageBlockParam {
  source: Base64ImageSource | URLImageSource | FileImageSource;
  type: 'image';
  cache_control?: CacheControlEphemeral | null;
  transformations?: ImageTransformationsParam | null;
}

export interface Base64ImageSource {
  data: string;
  media_type: 'image/jpeg' | 'image/png' | 'image/gif' | 'image/webp';
  type: 'base64';
}
```

补充（用于容量估算）：Vision 文档明确「images nested inside `tool_result` content」计入单请求图片数量与尺寸限制；computer use / browser use 工具集的 `tool_result` image 超限会直接 400，不会被 API 自动缩放。

### 2. OpenAI Chat Completions API —— 否

官方 OpenAPI spec（`openapi.yaml`, `components.schemas.ChatCompletionRequestToolMessage`）原文：

```yaml
content:
  oneOf:
    - type: string
      description: The contents of the tool message.
      title: Text content
    - type: array
      description: An array of content parts with a defined type. For tool messages,
        only type `text` is supported.
      title: Array of content parts
      items:
        $ref: "#/components/schemas/ChatCompletionRequestToolMessageContentPart"
      minItems: 1

ChatCompletionRequestToolMessageContentPart:
  oneOf:
    - $ref: "#/components/schemas/ChatCompletionRequestMessageContentPartText"
```

对照 user 消息（同一 spec）：

```yaml
ChatCompletionRequestUserMessageContentPart:
  oneOf:
    - ...MessageContentPartText
    - ...MessageContentPartImage      # type: image_url
    - ...MessageContentPartAudio
    - ...MessageContentPartFile
```

官方 Node SDK 类型同义：`ChatCompletionToolMessageParam.content: string | Array<ChatCompletionContentPartText>`（`ChatCompletionContentPartText` 只有 `{type:'text', text:string}`）。
**结论：Chat Completions 的 tool 消息是纯文本，无任何原生图片表达。** 降级必须走「紧接着的一条 user 消息」。

### 3. OpenAI Responses API —— 是

官方 OpenAPI spec（`FunctionCallOutputItemParam`）原文：

```yaml
output:
  oneOf:
    - type: string
      maxLength: 10485760
      description: A JSON string of the output of the function tool call.
    - items:
        oneOf:
          - $ref: "#/components/schemas/InputTextContentParam"
          - $ref: "#/components/schemas/InputImageContentParamAutoParam"
          - $ref: "#/components/schemas/InputFileContentParam"
        description: A piece of message content, such as text, an image, or a file.
      type: array
      description: An array of content outputs (text, image, file) for the function
        tool call.
  description: Text, image, or file output of the function tool call.
```

图片元素形状（`InputImageContentParamAutoParam`）：

```yaml
type:            # enum: [input_image], required
image_url:       # string, maxLength 20971520, format: uri
                 # "A fully qualified URL or base64 encoded image in a data URL."
file_id:         # string（Files API file id）
detail:          # high | low | auto | original
```

读取侧（`FunctionCallOutputItemResource.output` → `FunctionCallOutputResource`）同样是 `string | Array<InputContentResource>`，其中 `InputContentResource` = `InputContentResourceInputText | InputContentResourceInputImage`（discriminator `input_text` / `input_image`）。
官方 Node SDK 类型同义：`FunctionCallOutput.output: string | ResponseFunctionCallOutputItemList`，`ResponseFunctionCallOutputItem = ResponseInputTextContent | ResponseInputImageContent | ResponseInputFileContent`；注释原文为 "Text, image, or file output of the function tool call."。

**注意**：这是**schema 层**允许。官方是否有「某模型实际忽略 function_call_output 内的图片」的限制说明，本次无法核实（见「不确定项」）。

### 4. Google Gemini API (`generateContent`) —— 是（v1beta，inlineData）

官方 proto（`google/ai/generativelanguage/v1beta/content.proto`，即 Gemini API 的权威 API 定义）原文：

```proto
message FunctionResponse {
  // Optional. The id of the function call this response is for. ...
  string id = 3 [(google.api.field_behavior) = OPTIONAL];

  // Required. The name of the function to call. ...
  string name = 1 [(google.api.field_behavior) = REQUIRED];

  // Required. The function response in JSON object format.
  google.protobuf.Struct response = 2 [(google.api.field_behavior) = REQUIRED];

  // Optional. Ordered `Parts` that constitute a function response. Parts may
  // have different IANA MIME types.
  repeated FunctionResponsePart parts = 8 [(google.api.field_behavior) = OPTIONAL];

  bool will_continue = 4 [...];
  optional Scheduling scheduling = 5 [...];
}

// A datatype containing media that is part of a `FunctionResponse` message.
message FunctionResponsePart {
  oneof data {
    // Inline media bytes.
    FunctionResponseBlob inline_data = 1;
  }
}

message FunctionResponseBlob {
  string mime_type = 1;   // image/png, image/jpeg, ...
  bytes data = 2;         // proto3 JSON: base64 string
}
```

- **引入版本/时间**：googleapis 提交历史显示 `FunctionResponsePart` 于 **2025-10-15** 通过 *"feat: add support for FunctionResponsePart / feat: add support for raw media bytes for function response"* 加入 v1beta `content.proto`（commit `fd84be8a…`）。`id` 字段更早，2025-04-28 加入。
- **官方 SDK 一致**：
  - JS（`googleapis/js-genai` v2.22.0, `src/types.ts`）：`class FunctionResponse { parts?: FunctionResponsePart[]; response?: Record<string, unknown>; ... }`，`FunctionResponsePart { inlineData?: FunctionResponseBlob; fileData?: FunctionResponseFileData }`，并提供 `createFunctionResponsePartFromBase64(data, mimeType)` / `createFunctionResponsePartFromUri(uri, mimeType)`；`createPartFromFunctionResponse(id, name, response, parts)`。
  - Python（`googleapis/python-genai` v2.23.0, `google/genai/types.py`）：`FunctionResponse.parts: Optional[list[FunctionResponsePart]]`，docstring 原文 "Optional. Ordered `Parts` that constitute a function response. Parts may have different IANA MIME types."；`FunctionResponsePart` 有 `inline_data` / `file_data`，后者 docstring 明确 **"URI based data. This field is not supported in Gemini API."**（即 Gemini API 侧只用 `inlineData`，`fileData` 属 Vertex）。
- **REST JSON 形状**：proto3 JSON 映射为 camelCase —— `functionResponse` → `parts` → `inlineData` → `{ mimeType, data(base64) }`（JS SDK 即以此形状直接发 REST 请求，可佐证）。
- **Vertex 侧（对照）**：`google/cloud/aiplatform/v1beta1/tool.proto` 中 `FunctionResponse.parts` 同样存在，且 `FunctionResponsePart` 额外支持 `file_data`（URI 形式）。

⚠️ **`response` 字段本身仍是 `google.protobuf.Struct`（JSON 对象），不能放图片**；图片只能通过并列的 `parts` 字段传。

---

## 降级方案：把图片提升为「紧随其后的一条 user 消息中的图片」

四个协议**都有原生表达**（这是可行降级路径的事实基础）：

| 协议 | 原生表达 | JSON 形状 | 置信度 |
|---|---|---|---|
| Anthropic | user 消息 `content` 数组中的 `image` block（三种 source：base64 / URL / Files API `file_id`） | ```json\n{"role":"user","content":[\n {\"type\":\"image\",\"source\":{\"type\":\"base64\",\"media_type\":\"image/jpeg\",\"data\":\"...\"}},\n {\"type\":\"text\",\"text\":\"Describe this image.\"}\n]}\n``` | 高（文档明列三种 source 类型；Bedrock/Google Cloud 上仅 base64） |
| OpenAI Chat Completions | user 消息 `content` part 数组中的 `image_url` | ```json\n{"role":"user","content":[\n {\"type\":\"text\",\"text\":\"see image\"},\n {\"type\":\"image_url\",\"image_url\":{\"url\":\"data:image/png;base64,...\"}}\n]}\n``` | 高（spec：user content parts = text/image/audio/file） |
| OpenAI Responses | 新增一个 `message`（role `user`）输入项，`content` 为 `input_text` + `input_image` | ```json\n{\"type\":\"message\",\"role\":\"user\",\"content\":[\n {\"type\":\"input_text\",\"text\":\"see image\"},\n {\"type\":\"input_image\",\"image_url\":\"data:image/png;base64,...\"}\n]}\n``` | 高 |
| Gemini | 追加一个 `contents[]` 条目，`role: "user"`，`parts[].inlineData`（或 `fileData`） | ```json\n{\"role\":\"user\",\"parts\":[{\"inlineData\":{\"mimeType\":\"image/png\",\"data\":\"...\"}},{\"text\":\"see image\"}]}\n``` | 高（v1 与 v1beta 的 `Part.inline_data` 均存在） |

**实施注意（会影响降级代码是否被拒）**：
- **Anthropic**：带 `tool_result` 的那条 user 消息里，`tool_result` 块必须**排在最前**，文本必须在**所有 tool_result 之后**；若该轮还有未回填的 server tool，该消息**只能**包含 `tool_result`（其后追加文本会提前结束该轮，甚至 400）。因此「tool_result 之后紧跟同一条消息内的 image block」在类型上合法但**未被文档明确背书**，最稳妥是**另起一条 user 消息**放图片（本环境未能验证 Anthropic 是否允许连续两条 user 消息——见不确定项）。
- **OpenAI Responses / Chat**：每个 `tool_call` 必须被一条对应 `tool`/`function_call_output` 消息应答（Chat 侧 `role:"tool"` 的 `content` 只能放文本占位，如 `"image attached in the next message"`），图片放**其后的 user 消息**。
- **Gemini**：`functionResponse` 之后追加 role `user` 的 content 携带 `inlineData` 即可。

---

## 不确定项（无法从官方来源确认）

1. **Gemini 官方文档原文缺失（网络不可达）**
   - 尝试：`https://ai.google.dev/api/generate-content`、`https://ai.google.dev/api/caching`、`https://ai.google.dev/gemini-api/docs/function-calling`、`https://cloud.google.com/vertex-ai/generative-ai/docs/model-reference/function-calling`、`https://docs.cloud.google.com/...` → 全部 **连接超时（curl exit 28 / HTTP 000）**；`generativelanguage.googleapis.com/$discovery/rest?version=v1beta`（官方 Discovery 文档，可给出 `parts` 的 REST 名称）同样超时；`github.com/googleapis/discovery-artifact-manager` 下无 generativelanguage 的 discovery 文件（404）。
   - 因此**没有**「文档正文」级别的引用，只有 proto + 两套官方 SDK 的**三方一致**证据。若你要求文档级引用，需在能访问 ai.google.dev 的环境重抓 `#FunctionResponse` 锚点。

2. **Gemini API 的 v1（GA）是否支持 `FunctionResponse.parts`：无法确认，建议按 v1beta 实现**
   - 依据：googleapis master 的 `google/ai/generativelanguage/v1` 目录（`content.proto`、`generative_service.proto` 等，`BUILD.bazel` srcs 已核对）**完全没有** `FunctionResponse` / `FunctionCall` / `Tool` 定义，`v1/content.proto` 只有 `Content/Part{text,inline_data}/Blob/VideoMetadata`。这与「v1 实际可做 function calling」的常识不符，可能是该快照未同步，但我**无法**从官方来源证明 v1 是否接受 `functionResponse.parts`。
   - 建议：协议转换时把 Gemini 侧固定打到 **v1beta**（必要时 Vertex `v1beta1`），不要假设 v1 可用。

3. **OpenAI 官方文档正文（403 Cloudflare）**
   - 尝试：`https://platform.openai.com/docs/api-reference/chat/create`、`https://platform.openai.com/docs/guides/function-calling`、`https://developers.openai.com/api/docs/guides/function-calling`、`.../llms.txt`、`.../llms-full.txt` → **HTTP 403**（Cloudflare 拦截）。
   - 因此引用的是**官方 OpenAPI spec 文件**（`openai/openai-openapi`，version 2.3.0）与**官方 SDK 类型**（`openai/openai-node` 7.15.0）。两者一致；但「guide 页面里的文字说明」未直接取证。

4. **Anthropic API reference 页面无法抓取（区域限制）**
   - 尝试：`https://platform.claude.com/docs/en/api/messages/create.md` → 返回区域不可用页面（HTML，434 KB，与 `docs.anthropic.com/*`、`platform.claude.com/en/api/messages` 同一结果），`/llms.txt` 与 `/llms-full.txt` 可达。
   - 因此引用的是 tool-use **指南页**（handle-tool-calls）正文 + 官方 SDK 类型；**未能**核对 API reference 页里 `ToolResultBlockParam` 的逐字段说明（例如是否对 `tool_result` 内的图片 source 类型有额外限制）。

5. **非 base64 图片源能否出现在 `tool_result` 内：未确认**
   - 类型层 `ImageBlockParam.source` 允许 `base64 | url | file`（Files API `file_id`）；文档示例只用 base64；本次**未找到**任何官方正文明确允许或禁止 URL/`file_id` 源出现在 `tool_result` 内。
   - 建议实现：`tool_result` 内一律用 **base64**；URL/`file_id` 仅在你实测通过后启用。

6. **schema 允许 ≠ 模型实际利用（OpenAI / Gemini）**
   - Responses 的 `function_call_output.output` 数组含 `input_image`、Gemini 的 `functionResponse.parts[].inlineData` 都在**请求 schema** 层成立；本次**未找到**官方关于「模型一定读取该图片」「哪些模型支持」的说明。Anthropic 侧相反，文档明确以 computer use 截图为例说明模型会读 tool_result 内的图片。
   - 建议：把「模型是否真的看到图」作为运行时验证项，不要仅凭 schema 通过就认为语义生效。

7. **Anthropic 是否允许「连续两条 user 消息」**
   - 在 `llms-full.txt`（34 MB，含全部英文文档）中检索 `alternat`、`consecutive`、`same role`、`two user messages`、`user messages in a row`，**未找到**「角色必须交替」或「允许连续同角色」的明确条文（仅 server-tool `pause_turn` 示例注释提到 "maintain alternating roles"）。
   - 影响：降级方案若采用「tool_result 消息之后另起一条 user 消息放图」，其合法性未能证实。可先实测，或改在同一条 user 消息的 tool_result 之后附 image block（类型合法，但文档只保证了 text 可以放后面）。

---

## 取证环境与版本（供复现）

| 来源 | 版本/标识 | 抓取方式 |
|---|---|---|
| Anthropic 文档全文 | `https://platform.claude.com/llms-full.txt`（34,581,554 B） | curl（200） |
| Anthropic SDK 类型 | `anthropic-sdk-typescript` v0.125.0，`src/resources/messages/messages.ts` | raw.githubusercontent（200） |
| OpenAI 规范 | `openai/openai-openapi` `openapi.yaml`，info.version **2.3.0**（3,562,936 B） | raw.githubusercontent（200，断点续传） |
| OpenAI SDK 类型 | `openai/openai-node` v7.15.0（`chat/completions/completions.ts`、`responses/responses.ts`） | raw.githubusercontent（200） |
| Gemini proto | `googleapis/googleapis` master：`google/ai/generativelanguage/v1beta/content.proto`、`.../v1/content.proto`、`google/cloud/aiplatform/v1beta1/tool.proto` | raw.githubusercontent（200） |
| Gemini SDK | `googleapis/js-genai` v2.22.0 `src/types.ts`；`googleapis/python-genai` v2.23.0 `google/genai/types.py` | raw.githubusercontent（200） |

不可达来源（均已尝试）：`docs.anthropic.com` / `platform.claude.com` 的普通页面与 `.md` 页面（区域不可用）、`platform.openai.com` / `developers.openai.com`（403 Cloudflare）、`ai.google.dev` / `cloud.google.com` / `docs.cloud.google.com` / `generativelanguage.googleapis.com`（连接超时）、`r.jina.ai` 代理（000）。另：本会话的 `web_search` 工具不可用（缺少 DEEPSEEK_API_KEY），故全部依靠直接抓取。
