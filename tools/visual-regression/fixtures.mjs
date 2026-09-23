// 视觉回归夹具：字段形状与 src-tauri 命令 DTO / ui/src/types.ts 对齐，
// 数据取自本机真实导出（1 个供应商 + 21 个模型 + 别名）并补齐多供应商/错误态，
// 目的是让 980×640 下的界面与真实运行等价。
const NOW = Date.now();
const DAY = 86_400_000;

const P_MAIN = "01a0565e-792a-7aa3-a1cb-a4c8a689568c"; // 基元律动
const P_ANTH = "01a0565e-792a-7aa3-a1cb-a4c8a689568d"; // Anthropic 官方
const P_GEM = "01a0565e-792a-7aa3-a1cb-a4c8a689568e"; // Google Gemini（停用 + 长错误）

const MAIN_MODELS = [
  ["alias-dsflash-0000-0000-00000001", "deepseek-v4-flash", "deepseek-v4-flash-0731", 1000000],
  ["01a0565e-b0f4-7c50-961b-0dddefecc259", "deepseek-v4-flash-0731", null, 128000],
  ["01a0565e-b0f5-7f02-a7cf-3a9f4f997962", "deepseek-v4-pro-0813", null, 128000],
  ["01a0565e-b0f0-7a52-a730-6d8cd6d295df", "glm-5", null, 128000],
  ["01a0565e-b0f1-7b12-95e4-13946b9fdbb6", "glm-5.1", null, 128000],
  ["01a0565e-b0f4-7c50-961b-0db94e914dcb", "glm-5.2", null, 200000],
  ["01a0565e-b0f5-7f02-a7cf-3aaaa0c8f75d", "glm-5.3", null, 200000],
  ["01a0565e-b0f6-7e53-bf37-e83abc8fe880", "glm-5.3-flash", null, 200000],
  ["01a0565e-b0f2-77e0-8010-e94069a9478f", "kimi-k2.5", null, 256000],
  ["01a0565e-b0f2-77e0-8010-e9514a390648", "kimi-k2.6", null, 256000],
  ["01a0565e-b0f3-7de0-9815-b971ef9f8969", "kimi-k2.7-code", null, 256000],
  ["01a0565e-b0f6-7e53-bf37-e8242c4a19e8", "longcat-2.0", null, 128000],
  ["01a0565e-b0f3-7de0-9815-b950fe43a2a7", "mimo-v2.5-pro", null, 128000],
  ["01a0565e-b0f2-77e0-8010-e96c5413d38d", "minimax-m2.5", null, 204800],
  ["01a0565e-b0f1-7b12-95e4-13a771cb6bc7", "minimax-m2.7", null, 204800],
  ["01a0565e-b0f5-7f02-a7cf-3ab1d19b8d76", "qwen3.7-flash", null, 1000000],
  ["01a0565e-b0f3-7de0-9815-b96e2a193143", "qwen3.7-max", null, 1000000],
  ["01a0565e-b0f5-7f02-a7cf-3ac55fb3d189", "qwen3.8-27b", null, 262144],
  ["01a0565e-b0f4-7c50-961b-0dc8ca0bfb83", "qwen3.8-max", null, 1000000],
  ["01a0565e-b0f4-7c50-961b-0de5a86c678f", "seed-2.1-pro", null, 512000],
  ["01a0565e-b0f5-7f02-a7cf-3a88cd0fef2d", "seed-2.1-turbo", null, 512000],
];

const model = (id, providerId, modelName, upstream, ctx, i) => ({
  id,
  providerId,
  modelName,
  upstreamModelId: upstream,
  contextWindow: ctx,
  maxOutputTokens: 4096,
  enabled: i !== 11,
  supportsMultimodal: i % 3 === 0 ? true : i % 3 === 1 ? false : null,
  inputModalities: i % 4 === 0 ? ["text", "image"] : i % 4 === 1 ? ["text"] : null,
  outputModalities: i % 4 === 0 ? ["text"] : i % 4 === 2 ? ["text", "image"] : null,
});

export const fixtures = {
  // 渠道草稿端点探测（D9-T1）：四行覆盖四种展示形态 ——
  // 主探测通过 / 信息性探测失败（灰显）/ 未填模型跳过 / 地址被拦截。
  probe_report: {
    runId: "01a0565e-792a-7aa3-a1cb-a4c8a689568c",
    testedAt: NOW,
    fingerprint: "9f2c1d4b7a6e5c8f0b3a2d1e4f6a8c9b0d2e4f6a8c1b3d5e7f9a0b2c4d6e8f1a",
    results: [
      {
        endpoint: "chat_completions",
        status: "passed",
        category: null,
        message: "通过",
        latencyMs: 412,
        testedModel: "gpt-4o-mini",
        costPossible: true,
        informational: false,
      },
      {
        endpoint: "responses",
        status: "failed",
        category: "endpoint_unsupported",
        message:
          "HTTP 404：<html><head><title>404 Not Found</title></head><body><center><h1>404 Not Found</h1></center><hr><center>nginx/1.24.0</center></body></html>",
        latencyMs: 380,
        testedModel: "gpt-4o-mini",
        costPossible: true,
        informational: true,
      },
    ],
  },
  probe_report_blocked: {
    runId: "01a0565e-792a-7aa3-a1cb-a4c8a689568d",
    testedAt: NOW,
    fingerprint: "1111111111111111111111111111111111111111111111111111111111111111",
    results: [
      {
        endpoint: "chat_completions",
        status: "failed",
        category: "url_blocked",
        message: "目标 169.254.169.254 属于链路本地地址（含云元数据端点，任何情况下都不放行），已拦截",
        latencyMs: 0,
        testedModel: null,
        costPossible: false,
        informational: false,
      },
    ],
  },
  gateway_status: { running: true, port: 1314, restarts: 2 },
  health_summary: {
    checkedAtMs: NOW - 42_000,
    down: [
      {
        name: "Google Gemini",
        message:
          "403 PERMISSION_DENIED: Permission denied on resource project jai-gateway-2025 (request id: 01a0b3cd-77e2-4c11-9f0a-2b7d5e881a44), 请检查 API Key 是否已启用 Generative Language API",
      },
      { name: "Anthropic 官方", message: "401 invalid x-api-key" },
    ],
  },
  gateway_key_info: {
    id: "k-main",
    prefix: "sk-jai-pnxAR",
    label: null,
    createdAt: NOW - 30 * DAY,
    lastUsedAt: NOW - 3 * 60_000,
    revokedAt: null,
    key: "sk-jai-pnxARZHZCYBK3sCEJxOxZ8UrUCws",
  },
  // 多密钥（D9-T6a）：三把覆盖三种展示形态 —— 有备注且用过 / 无备注从未用过 /
  // 很久以前建的（时间列宽度与截断都要能扛住）。
  gateway_key_list: [
    {
      id: "k-main",
      prefix: "sk-jai-pnxAR",
      label: null,
      createdAt: NOW - 30 * DAY,
      lastUsedAt: NOW - 3 * 60_000,
      revokedAt: null,
      key: "",
    },
    {
      id: "k-laptop",
      prefix: "sk-jai-Lp7Qm",
      label: "笔记本 Claude Code",
      createdAt: NOW - 6 * DAY,
      lastUsedAt: NOW - 42 * 60_000,
      revokedAt: null,
      key: "",
    },
    {
      id: "k-ci",
      prefix: "sk-jai-Ci9Zx",
      label: "CI 流水线",
      createdAt: NOW - 2 * 3600_000,
      lastUsedAt: null,
      revokedAt: null,
      key: "",
    },
  ],
  // 密钥白/黑名单（D9-T6b）：候选清单 = 启用中的渠道 × 模型（P_GEM 已停用 ⇒ 不出现）
  key_rules_options: [
    { providerId: P_MAIN, providerName: "基元律动", modelName: "kimi-k2.7-code" },
    { providerId: P_MAIN, providerName: "基元律动", modelName: "glm-5.3" },
    { providerId: P_ANTH, providerName: "Anthropic 官方", modelName: "claude-sonnet-4-5" },
    { providerId: P_ANTH, providerName: "Anthropic 官方", modelName: "claude-opus-4-1" },
  ],
  // 三把密钥覆盖三种规则形态：不限制 / 白名单（「已限制」徽标）/ 拒绝 + 模型白名单
  key_rules: {
    "k-main": { providerAllow: [], providerDeny: [], modelAllow: [], modelDeny: [] },
    "k-laptop": { providerAllow: [P_ANTH], providerDeny: [], modelAllow: [], modelDeny: [] },
    "k-ci": {
      providerAllow: [],
      providerDeny: [P_ANTH],
      modelAllow: ["glm-5.3"],
      modelDeny: [],
    },
  },
  provider_list: [
    {
      id: P_MAIN,
      name: "基元律动",
      baseUrl: "https://tokenrhythm.studio/v1",
      family: "openai_compat",
      enabled: true,
      priority: 100,
      weight: 100,
      extraHeaders: null,
      website: "https://tokenrhythm.studio",
      lastOkAt: NOW - 90_000,
      lastErrAt: null,
      lastErrMsg: null,
      hasKey: true,
    },
    {
      id: P_ANTH,
      name: "Anthropic 官方",
      baseUrl: "https://api.anthropic.com",
      family: "anthropic",
      enabled: true,
      priority: 90,
      weight: 60,
      extraHeaders: '{"anthropic-version":"2023-06-01"}',
      website: "https://console.anthropic.com",
      lastOkAt: NOW - 3 * DAY,
      lastErrAt: NOW - 3600_000,
      lastErrMsg: "401 invalid x-api-key",
      hasKey: true,
    },
    {
      id: P_GEM,
      name: "Google Gemini",
      baseUrl: "https://generativelanguage.googleapis.com/v1beta",
      family: "gemini",
      enabled: false,
      priority: 50,
      weight: 40,
      extraHeaders: null,
      website: null,
      lastOkAt: null,
      lastErrAt: NOW - 6 * 3600_000,
      lastErrMsg:
        "403 PERMISSION_DENIED: Permission denied on resource project jai-gateway-2025 (request id: 01a0b3cd-77e2-4c11-9f0a-2b7d5e881a44)",
      hasKey: false,
    },
  ],
  model_list: {
    [P_MAIN]: MAIN_MODELS.map(([id, name, up, ctx], i) => model(id, P_MAIN, name, up, ctx, i)),
    [P_ANTH]: [
      model("01a06000-0000-7000-8000-000000000001", P_ANTH, "claude-opus-4-1", null, 200000, 0),
      model("01a06000-0000-7000-8000-000000000002", P_ANTH, "claude-sonnet-4-5", null, 200000, 1),
      model("01a06000-0000-7000-8000-000000000003", P_ANTH, "claude-haiku-4-5", null, 200000, 2),
      model("01a06000-0000-7000-8000-000000000004", P_ANTH, "claude-3-7-sonnet-20250219", null, 209000, 3),
      model("01a06000-0000-7000-8000-000000000005", P_ANTH, "claude-3-5-haiku-20241022", null, 200000, 4),
    ],
    [P_GEM]: [
      model("01a06100-0000-7000-8000-000000000001", P_GEM, "gemini-3-pro", null, 1048576, 0),
      model("01a06100-0000-7000-8000-000000000002", P_GEM, "gemini-3-flash", null, 1048576, 5),
    ],
  },
  logs_recent: Array.from({ length: 80 }, (_, i) => ({
    id: 90000 - i,
    ts: NOW - i * 7 * 60_000,
    inboundFamily: i % 5 === 0 ? "anthropic" : "openai_compat",
    routeMode: i % 7 === 0 ? "fallback" : i % 11 === 0 ? "sticky" : "round_robin",
    modelName: MAIN_MODELS[i % MAIN_MODELS.length][1],
    providerId: i % 13 === 0 ? P_ANTH : P_MAIN,
    httpStatus: i % 9 === 0 ? 429 : i % 17 === 0 ? 500 : i % 23 === 0 ? 499 : 200,
    durationMs: 180 + ((i * 137) % 9000),
    isStream: i % 3 !== 0,
    usageInput: i % 9 === 0 ? null : 1200 + i * 37,
    usageOutput: i % 9 === 0 ? null : 120 + i * 11,
    errorKind: i % 9 === 0 ? "rate_limited" : i % 17 === 0 ? "upstream_5xx" : i % 23 === 0 ? "client_cancel" : null,
    errorSummary:
      i % 9 === 0
        ? "429 Too Many Requests: upstream quota exhausted for tokenrhythm.studio, retry-after=30s（已按权重切换到下一供应商）"
        : i % 17 === 0
          ? "502 Bad Gateway: upstream connect error (connection reset by peer during TLS handshake)"
          : i % 23 === 0
            ? "客户端中断：client disconnected before first token（stream aborted at 3.1s）"
            : null,
  })),
  stats_usage: Array.from({ length: 30 }, (_, i) => ({
    day: Math.floor((NOW - i * DAY) / DAY),
    requests: 40 + ((i * 97) % 620),
    inputTokens: 120_000 + i * 8431,
    outputTokens: 30_000 + i * 2210,
    cacheReadTokens: i % 4 === 0 ? 90_000 + i * 1200 : 0,
  })),
  mcp_list: [
    {
      id: "01a07000-0000-7000-8000-000000000001",
      name: "netcatty-external",
      kind: "stdio",
      command:
        "/Applications/Netcatty.app/Contents/Resources/app.asar.unpacked/electron/cli/netcatty-external-mcp",
      args: '["--stdio","--scope","workspace","--allow-sftp","--verbose-logging=true"]',
      env: '{"NCT_HOME":"/Users/jiangnan","NCT_TRACE":"1"}',
      enabled: true,
      proxyAllowed: true,
      createdAt: NOW - 20 * DAY,
      updatedAt: NOW - 2 * DAY,
    },
    {
      id: "01a07000-0000-7000-8000-000000000002",
      name: "jai-registry",
      kind: "http",
      command: null,
      args: null,
      url: "http://127.0.0.1:1314/registry/mcp",
      env: null,
      enabled: true,
      proxyAllowed: false,
      createdAt: NOW - 12 * DAY,
      updatedAt: NOW - 5 * DAY,
    },
    {
      id: "01a07000-0000-7000-8000-000000000003",
      name: "playwright-mcp",
      kind: "stdio",
      command: "npx",
      args: '["-y","@playwright/mcp@latest","--browser","chrome"]',
      env: '{"HTTPS_PROXY":"http://127.0.0.1:7890"}',
      enabled: false,
      proxyAllowed: false,
      createdAt: NOW - 40 * DAY,
      updatedAt: NOW - 30 * DAY,
    },
  ],
  mcp_tools_list: Array.from({ length: 18 }, (_, i) => ({
    name: [
      "terminal_execute", "terminal_start", "terminal_poll", "sftp_upload", "sftp_download",
      "vault_hosts_list", "vault_hosts_create", "scripts_run", "snippets_create", "portforward_start",
      "get_mcp_server_detail", "list_mcp_servers", "get_skill_detail", "list_skills",
      "host_connect_scripts_set", "vault_notes_update", "session_close", "read_attachment",
    ][i],
    description:
      i % 3 === 0
        ? "在一个终端会话中执行短命令并等待完成；超过 60 秒的命令请改用 terminal_start 与 terminal_poll 组合。"
        : i % 3 === 1
          ? "上传本地文件到远端路径（走 SFTP/SCP 后端）。"
          : null,
    inputSchema: { type: "object", properties: { sessionId: { type: "string" } } },
  })),
  skill_list: [
    {
      id: "01a08000-0000-7000-8000-000000000001",
      name: "rust-review",
      description: "对 Rust 改动做逐条评审：所有权、错误处理、并发与 unsafe 边界。",
      content:
        "# Rust 评审清单\n\n1. 所有 public 函数是否需要 `#[must_use]`\n2. `Result` 的 `map_err` 是否携带上下文\n3. 是否存在 `unwrap()` / `expect()` 落在请求路径上\n4. `Arc<Mutex<_>>` 是否有跨 `.await` 持锁\n5. 迁移脚本是否幂等\n\n输出格式：按文件分组，每条给出 行号 / 风险 / 修法。\n",
      enabled: true,
      createdAt: NOW - 15 * DAY,
      updatedAt: NOW - 4 * DAY,
    },
    {
      id: "01a08000-0000-7000-8000-000000000002",
      name: "changelog",
      description: "从 git log 生成面向用户的中文 CHANGELOG 段落。",
      content: "读取 `git log --no-merges v<last>..HEAD`，按 feat/fix/perf 分组，每条不超过 40 字。\n",
      enabled: true,
      createdAt: NOW - 9 * DAY,
      updatedAt: NOW - 9 * DAY,
    },
    {
      id: "01a08000-0000-7000-8000-000000000003",
      name: "gateway-smoke-test",
      description: "对运行中的网关做端到端冒烟：流式、非流式、切换、限流回退各打一发。",
      content:
        "curl -sN http://127.0.0.1:1314/v1/chat/completions -H \"Authorization: Bearer $KEY\" -d '{...}' 依次校验：首字节延迟、usage 是否回传、fallback 时 model 字段变化。\n",
      enabled: false,
      createdAt: NOW - 2 * DAY,
      updatedAt: NOW - 2 * DAY,
    },
  ],
  settings_get: { preferredPort: 1314, logsEnabled: true, retentionDays: 30, logRowCap: 5000 },
  cors_allow_get: ["http://localhost:3000", "http://127.0.0.1:5173", "https://console-some-very-long-origin.internal.jai-gateway.dev"],
  proxy_get: { enabled: true, url: "http://127.0.0.1:7890", bypass: ["localhost", "127.0.0.1", "*.88933.vip"] },
  families: ["openai_compat", "anthropic", "gemini"],
  webdav_config_get: {
    url: "https://jn_file.88933.vip",
    username: "jiangnan",
    directory: "jai/config",
    autoPushEnabled: true,
    autoPushIntervalMin: 30,
    autoPullEnabled: true,
    autoPullIntervalMin: 360,
    password: null,
  },
  webdav_autopush_status: { atMs: NOW - 12 * 60_000, ok: true, message: "pushed 1 provider / 21 models" },
  webdav_autopull_status: null,
  webdav_preview: {
    remoteProviders: 2,
    remoteModels: 26,
    localProviders: 3,
    localModels: 28,
    willOverwrite: true,
    message: "远端快照早于本地，拉取会覆盖本地 3 个供应商配置",
  },
  webdav_snapshot_info: { exists: true, atMs: NOW - 40 * 60_000, chars: 33672 },
  webdav_push_diff: {
    remoteExists: true,
    remoteOnlyProviders: [["旧-DeepSeek", "https://api.deepseek.com/v1"]],
    remoteOnlyModels: [
      [P_MAIN, "deepseek-chat"],
      [P_MAIN, "deepseek-reasoner"],
    ],
    localOnlyProviders: [
      ["Anthropic 官方", "https://api.anthropic.com"],
      ["Google Gemini", "https://generativelanguage.googleapis.com/v1beta"],
    ],
    localOnlyModels: MAIN_MODELS.slice(0, 9).map((m) => [P_MAIN, m[1]]),
    blocks: true,
  },
  webdav_backups_list: Array.from({ length: 6 }, (_, i) => ({
    name: `jai-config-${new Date(NOW - i * DAY).toISOString().slice(0, 19).replace(/[:T]/g, "-")}.json`,
    href: `https://jn_file.88933.vip/jai/config/backup/${i}.json`,
    size: 30000 + i * 512,
    ts: NOW - i * DAY,
    isCurrent: i === 0,
  })),
  config_import: {
    providersImported: 2,
    providersSkippedDuplicate: 1,
    modelsImported: 21,
    missingKeys: ["Anthropic 官方"],
    invalidProviders: ["旧-DeepSeek (family 无法识别)"],
  },
};
