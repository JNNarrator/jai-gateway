// 用**真实 AI SDK**（zcode 内部就是 `ai` + `@ai-sdk/openai`）回放 JAI 发出的 Responses SSE，
// 验证流能否被严格客户端正确解析成 tool-call / text。
//
// 为什么需要它：JAI 的 Responses 流必须满足 AI SDK 的 **zod 逐事件 schema**，
// 缺字段/形状不对会让整帧校验失败并被静默丢弃（线上表现为「工具行永远关不掉」、
// 「Model request failed.」这类看起来与上游无关的怪错）。纯 Rust 单测只能断言“我们发了什么”，
// 这个脚本断言“严格客户端能不能收下”，两者互补。
//
// 用法（需要联网装依赖，故不进 CI）：
//   mkdir -p /tmp/sdktest && cd /tmp/sdktest
//   npm i ai @ai-sdk/openai zod
//   cp <repo>/scripts/verify_responses_with_ai_sdk.mjs .
//   # 先抓一条真实流（示例）：
//   #   curl -sN -o /tmp/stream.sse -H "Authorization: Bearer $JAI_KEY" \
//   #     -H 'content-type: application/json' -d @body.json \
//   #     http://127.0.0.1:1314/v1/responses
//   node verify_responses_with_ai_sdk.mjs /tmp/stream.sse
//
// 期望：无 `_TypeValidationError`，含 `tool-input-end` + `tool-call`（有工具调用时），
// 且 toolCalls 的 name/input 与模型输出一致。
import fs from 'node:fs';
import { z } from 'zod';
import { streamText } from 'ai';
import { createOpenAI } from '@ai-sdk/openai';

const file = process.argv[2];
if (!file) {
  console.error('用法: node verify_responses_with_ai_sdk.mjs <responses-sse 文件>');
  process.exit(2);
}
const sse = fs.readFileSync(file, 'utf8');

// 用固定响应替换真实网络：把捕获到的 SSE 原样喂给 SDK 解析
const provider = createOpenAI({
  apiKey: 'test-key',
  baseURL: 'http://127.0.0.1:1314/v1',
  fetch: async () =>
    new Response(sse, {
      status: 200,
      headers: { 'content-type': 'text/event-stream', 'x-jai-mode': 'converted' },
    }),
});

const result = streamText({
  model: provider.responses('model'),
  prompt: 'x',
  // 声明要足够宽：未声明的工具名会让 SDK 报 tool-error（那是测试台的问题，不是 JAI 的）
  tools: {
    Bash: { inputSchema: z.object({ command: z.string() }).loose() },
    Read: { inputSchema: z.object({ file_path: z.string() }).loose() },
    TodoRead: { inputSchema: z.object({}).loose() },
    Write: { inputSchema: z.object({}).loose() },
    Edit: { inputSchema: z.object({}).loose() },
  },
});

const counts = {};
const toolCalls = [];
const errors = [];
let reasoningText = '';
for await (const part of result.fullStream) {
  counts[part.type] = (counts[part.type] || 0) + 1;
  if (part.type === 'tool-call') {
    toolCalls.push({ id: part.toolCallId, name: part.toolName, input: part.input });
  }
  if (part.type === 'reasoning-delta') reasoningText += part.text ?? part.delta ?? '';
  if (part.type === 'error') errors.push(String(part.error?.message ?? part.error));
}

console.log('文件:', file);
console.log('chunkCounts:', JSON.stringify(counts));
console.log('toolCalls:', JSON.stringify(toolCalls));
console.log('推理文本长度:', reasoningText.length, '| 开头:', JSON.stringify(reasoningText.slice(0, 60)));

let failed = false;
if (errors.length) {
  console.log('errors:', errors);
  failed = true;
}
// 有推理内容时，客户端必须真的拿到文本（否则下轮回传缺 reasoning_content → 上游 400）
if (sse.includes('reasoning_summary_text.delta') && reasoningText.length === 0) {
  console.log('✗ 流里有推理但客户端没拿到文本（事件名/形状不被 SDK 识别）');
  failed = true;
}
if (failed) process.exit(1);
console.log('OK：严格客户端（AI SDK）解析通过');
