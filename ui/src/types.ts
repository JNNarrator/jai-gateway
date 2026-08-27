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
  extraHeaders?: string | null;
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
