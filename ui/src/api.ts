// Tauri invoke 封装：M1 全部命令的类型化入口。
import { invoke } from "@tauri-apps/api/core";
import type {
  GwStatus,
  GatewayKeyInfo,
  ImportReport,
  LogRowView,
  McpServerRow,
  McpTool,
  ModelRow,
  ProviderDto,
  SettingsDto,
  SkillRow,
  UsageStatRow,
  WebDavConfigDto,
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
    weight?: number;
    extraHeaders?: string | null;
    apiKey: string;
  }) => invoke<ProviderDto>("provider_create", { input }),
  providerUpdate: (input: {
    id: string;
    name?: string;
    baseUrl?: string;
    priority?: number;
    weight?: number;
    extraHeaders?: string | null;
    apiKey?: string;
  }) => invoke<void>("provider_update", { input }),
  providerDelete: (id: string) =>
    invoke<void>("provider_delete", { id }),
  providerSetEnabled: (id: string, enabled: boolean) =>
    invoke<void>("provider_set_enabled", { id, enabled }),
  providerTest: (id: string) => invoke<string>("provider_test", { id }),
  providerTestDraft: (input: {
    baseUrl: string;
    family: string;
    apiKey: string;
  }) =>
    invoke<{ ok: boolean; count: number; modelNames: string[] }>(
      "provider_test_draft",
      { input }
    ),
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
  modelSetAlias: (modelId: string, upstreamModelId: string | null) =>
    invoke<void>("model_set_alias", { input: { modelId, upstreamModelId } }),
  modelToggle: (modelId: string, enabled: boolean) =>
    invoke<void>("model_toggle", { modelId, enabled }),

  // 网关密钥
  gatewayKeyInfo: () => invoke<GatewayKeyInfo | null>("gateway_key_info"),
  gatewayKeyReveal: () => invoke<GatewayKeyInfo>("gateway_key_reveal"),
  gatewayKeyRegenerate: () =>
    invoke<GatewayKeyInfo>("gateway_key_regenerate"),

  // 日志 / 导出 / 设置 / WebDAV
  logsRecent: (limit = 100) => invoke<LogRowView[]>("logs_recent", { limit }),
  statsUsage: (days = 7) => invoke<UsageStatRow[]>("stats_usage", { days }),
  exportConfigJson: () => invoke<string>("export_config_json"),
  exportConfigToFile: () => invoke<string>("export_config_to_file"),
  revealInFolder: (path: string) => invoke<void>("reveal_in_folder", { path }),
  configImport: (text: string, strict?: boolean) =>
    invoke<ImportReport>("config_import", { text, strict: strict ?? false }),
  webdavConfigGet: () => invoke<WebDavConfigDto | null>("webdav_config_get"),
  webdavConfigSet: (input: {
    url: string;
    username: string;
    directory: string;
    password?: string | null;
  }) => invoke<void>("webdav_config_set", { input }),
  webdavTest: (input: {
    url: string;
    username: string;
    password?: string | null;
  }) => invoke<string>("webdav_test", { input }),
  webdavPreview: () =>
    invoke<{
      remoteProviders: number;
      remoteModels: number;
      localProviders: number;
      localModels: number;
      willOverwrite: boolean;
      message: string;
    }>("webdav_preview"),
  webdavPush: () => invoke<void>("webdav_push"),
  webdavPull: () => invoke<ImportReport>("webdav_pull"),

  // MCP 管理
  mcpList: () => invoke<McpServerRow[]>("mcp_list"),
  mcpCreate: (input: {
    name: string;
    kind: string;
    command?: string | null;
    args?: string | null;
    url?: string | null;
  }) => invoke<McpServerRow>("mcp_create", { input }),
  mcpUpdate: (input: {
    id: string;
    name: string;
    kind: string;
    command?: string | null;
    args?: string | null;
    url?: string | null;
  }) => invoke<void>("mcp_update", { input }),
  mcpSetEnabled: (id: string, enabled: boolean) =>
    invoke<void>("mcp_set_enabled", { id, enabled }),
  mcpDelete: (id: string) => invoke<void>("mcp_delete", { id }),
  mcpToolsList: (id: string) => invoke<McpTool[]>("mcp_tools_list", { id }),
  mcpToolsCall: (id: string, name: string, args: unknown) =>
    invoke<unknown>("mcp_tools_call", { id, name, arguments: args }),

  // Skill 管理
  skillList: () => invoke<SkillRow[]>("skill_list"),
  skillCreate: (input: {
    name: string;
    description: string;
    content: string;
  }) => invoke<SkillRow>("skill_create", { input }),
  skillUpdate: (input: {
    id: string;
    name: string;
    description: string;
    content: string;
  }) => invoke<void>("skill_update", { input }),
  skillSetEnabled: (id: string, enabled: boolean) =>
    invoke<void>("skill_set_enabled", { id, enabled }),
  skillDelete: (id: string) => invoke<void>("skill_delete", { id }),
  skillImportZip: (data: number[]) =>
    invoke<number>("skill_import_zip", { data }),
  vaultStorageKind: () => invoke<string>("vault_storage_kind"),

  corsAllowGet: () => invoke<string[]>("cors_allow_get"),
  corsAllowSet: (list: string[]) => invoke<void>("cors_allow_set", { list }),
  settingsGet: () => invoke<SettingsDto>("settings_get"),
  settingsSetPort: (port: number) =>
    invoke<number>("settings_set_port", { port }),
  settingsSetLogsEnabled: (enabled: boolean) =>
    invoke<boolean>("settings_set_logs_enabled", { enabled }),
  settingsSetRetention: (days: number, rowCap: number) =>
    invoke<void>("settings_set_retention", { days, rowCap }),
  readEnvVar: (name: string) => invoke<string>("read_env_var", { name }),
  portInUse: (port: number) => invoke<boolean>("port_in_use", { port }),
  families: () => invoke<string[]>("families"),
};
