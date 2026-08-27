// Tauri invoke 封装：M1 全部命令的类型化入口。
import { invoke } from "@tauri-apps/api/core";
import type {
  GwStatus,
  GatewayKeyInfo,
  LogRowView,
  ModelRow,
  ProviderDto,
  SettingsDto,
} from "./types";

export const api = {
  // 网关启停
  status: () => invoke<GwStatus>("gateway_status"),
  start: () => invoke<GwStatus>("gateway_start"),
  stop: () => invoke<GwStatus>("gateway_stop"),

  // 供应商
  providerList: () => invoke<ProviderDto[]>("provider_list"),
  providerCreate: (input: {
    name: string;
    baseUrl: string;
    family: string;
    priority?: number;
    extraHeaders?: string | null;
    apiKey: string;
  }) => invoke<ProviderDto>("provider_create", { input }),
  providerUpdate: (input: {
    id: string;
    name?: string;
    baseUrl?: string;
    priority?: number;
    extraHeaders?: string | null;
    apiKey?: string;
  }) => invoke<void>("provider_update", { input }),
  providerDelete: (id: string) =>
    invoke<void>("provider_delete", { id }),
  providerSetEnabled: (id: string, enabled: boolean) =>
    invoke<void>("provider_set_enabled", { id, enabled }),
  providerTest: (id: string) => invoke<string>("provider_test", { id }),
  providerDiscoverModels: (id: string) =>
    invoke<[number, number]>("provider_discover_models", { id }),

  // 模型
  modelList: (providerId: string) =>
    invoke<ModelRow[]>("model_list", { providerId }),
  modelSetLimits: (input: {
    modelId: string;
    contextWindow?: number | null;
    maxOutputTokens: number;
  }) => invoke<void>("model_set_limits", { input }),
  modelToggle: (modelId: string, enabled: boolean) =>
    invoke<void>("model_toggle", { modelId, enabled }),

  // 网关密钥
  gatewayKeyInfo: () => invoke<GatewayKeyInfo | null>("gateway_key_info"),
  gatewayKeyReveal: () => invoke<GatewayKeyInfo>("gateway_key_reveal"),
  gatewayKeyRegenerate: () =>
    invoke<GatewayKeyInfo>("gateway_key_regenerate"),

  // 日志 / 导出 / 设置
  logsRecent: (limit = 100) => invoke<LogRowView[]>("logs_recent", { limit }),
  exportConfigJson: () => invoke<string>("export_config_json"),
  corsAllowGet: () => invoke<string[]>("cors_allow_get"),
  corsAllowSet: (list: string[]) => invoke<void>("cors_allow_set", { list }),
  settingsGet: () => invoke<SettingsDto>("settings_get"),
  settingsSetPort: (port: number) =>
    invoke<number>("settings_set_port", { port }),
  settingsSetLogsEnabled: (enabled: boolean) =>
    invoke<boolean>("settings_set_logs_enabled", { enabled }),
  families: () => invoke<string[]>("families"),
};
