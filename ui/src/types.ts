// JAI UI 类型定义 —— 与 src-tauri 命令 DTO 对齐（camelCase）。

export interface GwStatus {
  running: boolean;
  port: number;
  restarts: number;
}

export interface HealthDownProvider {
  name: string;
  message: string;
}

export interface HealthSummary {
  checkedAtMs?: number | null;
  down: HealthDownProvider[];
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

/** 模型模态：输入/输出共用同一枚举，与后端 gateway_core::modality 逐字对齐。 */
export type Modality = "text" | "image" | "audio" | "video";

export interface ModelRow {
  id: string;
  providerId: string;
  modelName: string;
  upstreamModelId?: string | null;
  contextWindow?: number | null;
  maxOutputTokens: number;
  enabled: boolean;
  /** 派生视图（后端由 inputModalities 推出，兼容旧口径）：null=未知 */
  supportsMultimodal: boolean | null;
  /** 输入模态集合：null=未知/未标注 */
  inputModalities: Modality[] | null;
  /** 输出模态集合：null=未知/未标注 */
  outputModalities: Modality[] | null;
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
  autoPullEnabled: boolean;
  password?: string | null;
}

export interface ProxyConfigDto {
  enabled: boolean;
  url: string;
  bypass: string[];
}

export interface WebDavAutoPushStatus {
  atMs: number;
  ok: boolean;
  message: string;
}

export interface WebDavSnapshotInfo {
  exists: boolean;
  atMs?: number | null;
  chars: number;
}

export interface PushDiffDetailDto {
  remoteExists: boolean;
  /** [名称, base_url] 对 */
  remoteOnlyProviders: string[][];
  /** [providerId, modelName] 对 */
  remoteOnlyModels: string[][];
  localOnlyProviders: string[][];
  localOnlyModels: string[][];
  blocks: boolean;
}

export interface WebDavBackupItem {
  name: string;
  href: string;
  size?: number | null;
  ts?: number | null;
  isCurrent: boolean;
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
  /** 是否允许 jai-registry 代理执行（动态工具 + tools/call 转发） */
  proxyAllowed: boolean;
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
