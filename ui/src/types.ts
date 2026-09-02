// JAI UI 类型定义 —— 与 src-tauri 命令 DTO 对齐（camelCase）。

export interface GwStatus {
  running: boolean;
  port: number;
  restarts: number;
}

export type Family = "openai_compat" | "anthropic" | "gemini";

export interface ProviderDto {
  id: string;
  name: string;
  baseUrl: string;
  family: Family;
  enabled: boolean;
  priority: number;
  weight: number;
  extraHeaders?: string | null;
  website?: string | null;
  lastOkAt?: number | null;
  lastErrAt?: number | null;
  lastErrMsg?: string | null;
  hasKey: boolean;
}

export interface ModelRow {
  id: string;
  providerId: string;
  modelName: string;
  upstreamModelId?: string | null;
  contextWindow?: number | null;
  maxOutputTokens: number;
  enabled: boolean;
}

export interface GatewayKeyInfo {
  prefix: string;
  label?: string | null;
  createdAt: number;
  lastUsedAt?: number | null;
  key: string;
}

export interface UsageStatRow {
  day: number;
  requests: number;
  inputTokens: number;
  outputTokens: number;
  cacheReadTokens: number;
}

export interface LogRowView {
  id: number;
  ts: number;
  inboundFamily: string;
  routeMode: string;
  modelName: string;
  providerId?: string | null;
  httpStatus: number;
  durationMs: number;
  isStream: boolean;
  usageInput?: number | null;
  usageOutput?: number | null;
  errorKind?: string | null;
  errorSummary?: string | null;
}

export interface SettingsDto {
  preferredPort: number;
  logsEnabled: boolean;
  retentionDays: number;
  logRowCap: number;
}

export interface WebDavConfigDto {
  url: string;
  username: string;
  directory: string;
  autoPushEnabled: boolean;
  autoPushIntervalMin: number;
  password?: string | null;
}

export interface WebDavAutoPushStatus {
  atMs: number;
  ok: boolean;
  message: string;
}

export interface ImportReport {
  providersImported: number;
  providersSkippedDuplicate: number;
  modelsImported: number;
  missingKeys: string[];
  invalidProviders: string[];
}

export interface McpTool {
  name: string;
  description?: string | null;
  inputSchema: unknown;
}

export interface McpServerRow {
  id: string;
  name: string;
  kind: "stdio" | "sse" | "http";
  command?: string | null;
  args?: string | null;
  url?: string | null;
  env?: string | null;
  enabled: boolean;
  createdAt: number;
  updatedAt: number;
}

export interface SkillRow {
  id: string;
  name: string;
  description: string;
  content: string;
  enabled: boolean;
  createdAt: number;
  updatedAt: number;
}
