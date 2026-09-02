# 密钥入库 + WebDAV 同步增强 + 日志 token 修复 — 设计文档

日期：2026-09-02
状态：已获用户批准（含三项补充：彻底消灭钥匙串访问、网关 key 非必要不变尽量取自 WebDAV、供应商官网字段）

## 背景与目标

用户提出三个需求：

1. **放弃系统钥匙串**：每次访问触发 macOS 钥匙串授权弹框（dev/release 二进制签名交替导致 ACL 不匹配）。密钥全部改存 SQLite，并随 WebDAV 配置同步。要求**彻底消灭对系统钥匙串的运行时访问**。
2. **日志「输入/输出」列为空**：排查是否 bug。
3. **网关 API key 随 WebDAV 固定**：换机器拉取即用，客户端配置不用改；未配置 WebDAV 时行为不变；手动重新生成后自动更新远端。

补充需求：**供应商配置新增「官网」字段（可空）**，方便查看供应商情况。

## 现状事实（探索取证）

- 钥匙串里只有两类密钥：供应商 API key（`jai/provider/{uuid}`，vault.rs:157-159）、WebDAV 密码（`jai/webdav`，sync.rs:14）。网关 key 已在 `gateway_keys` 表明文（mod.rs:448），MCP env 已在 `mcp_servers.env` 明文。
- 供应商 key 读写点：main.rs:224/293（写）、proxy.rs:853/1276（转发读）、main.rs:1843（健康检查）。
- 导出（store/export.rs:13-57）不含任何密钥，单测 `export_leaks_no_secrets`（export.rs:104-121）明确断言不含 `sk-jai`；导入（store/import.rs:25-208）只处理 providers/models，**meta 被解析但完全忽略**（import.rs:37-46）。
- 导入的新供应商无凭据即参与路由，转发必失败（proxy.rs:855-876）。
- 日志「输入/输出」= `usage_input/usage_output`（token 数），非 body。转换路径+流式恒空（proxy.rs:1867-1879 直接传 None）。
- **日志 bug 实测取证**（2026-09-02）：glm-5.3-flash 流式响应（tokenrhythm 中转）含 17 个 `"usage":null` + 末尾完整 usage 对象（prompt 13 / completion 16），JAI UsageScanner（codec/openai.rs:80-190）未能记录。9月1日 deepseek（响应无 null 字段）记录正常。根因：①`"usage"` 关键字后 32 字节窗口内无 `{` 时线索被丢弃且不跨 feed 保持；②`"usage":null` 后 32 字节内命中下一 SSE 行的 `{` 会误触发对象收集。
- meta 表无 `gateway_port`（网关走 bind_with_fallback，当前实际监听 localhost:1314）。
- 用户 WebDAV：`http://jn_file.88933.vip`（HTTPS 已实测可用）。

## 已拍板决策

- 同步根地址切换为 **https**（用户确认）。
- 网关 key 同步策略：**拉取覆盖**（用户确认）——启动时本地无 key 才生成；拉取时远端 key 非空且与本地不同才覆盖本地（吊销旧 key）；其余情况 key 保持不变；导出/推送总是携带当前 active key。
- 密钥明文入库与明文同步被用户接受（与现有网关 key/MCP env 安全模型一致）。

## 设计

### 1. 密钥迁入 SQLite + 钥匙串退场

- migration 0006（一个 migration 做完）：
  - `providers` 加 `api_key TEXT`（明文，可空）、`website TEXT`（可空）
  - meta 写入 `webdav_password`（由迁移逻辑填）
- **存量迁移**（Rust 侧一次性，`migrate_keyring_secrets`）：
  - meta 标记位 `keyring_migrated`；未标记时用 keyring crate 逐个读 `jai/provider/{id}` 与 `jai/webdav` → 写入 providers.api_key / meta.webdav_password → 删除钥匙串项 → 置标记
  - 这是**运行时对钥匙串的唯一一次访问**（可能弹最后一次授权框）；此后启动、转发、导入导出均零钥匙串访问
  - 代码集中在一个函数，注释注明「下个版本可随 keyring crate 一并删除」
- 代码删减：`vault.rs` 的读写封装、文件降级（vault_fallback.json）、`vault_storage_kind` 命令与 UI 存储类型显示全部移除；转发路径改为从 provider 行直接读 `api_key`（route_candidates SQL 替换 keyring_ref 为 api_key）
- provider_create/update/delete 改为写 DB 列；`has_key` DTO 语义不变
- UI 供应商表单新增「官网」输入框（可空）；供应商列表提供点击跳转系统浏览器（用 tauri-plugin-opener；若项目尚未集成该插件则新增依赖）

### 2. WebDAV 同步协议（jai-export/v1 扩展）

- 导出：providers 每项带 `api_key`（snake_case，与现有字段风格一致）、`website`；JSON 顶层新增 `gateway_key`（当前 active）；meta 含 `webdav_password` 后自然随 meta 导出
- 导入（apply_import）：
  - providers：远端 `api_key` 非空才覆盖本地对应供应商（远端空不清空本地）；新建供应商直接带 key/website——导入后立即可路由，消除「待录入密钥」状态
  - `gateway_key` 非空且与本地 active 不同 → 以远端 key 执行 rotate（吊销本地旧 key）；相同则跳过
  - meta 白名单应用：`webdav_url/username/directory/auto_push_enabled/auto_push_interval_min/webdav_password`；其余 meta（端口等）不导入
- 触发：`gateway_key_regenerate` 后调用既有自动推送防抖通知（配置了 WebDAV 即自动更新远端）；未配置 WebDAV 时一切行为不变
- 导出 note 文案更新；`export_leaks_no_secrets` 单测按新契约反转改写

### 3. 日志 token 数修复

- UsageScanner（codec/openai.rs）状态机修复：
  - `"usage"` 关键字后紧跟 `null`（窗口内可判定）→ 显式跳过该关键字继续扫描
  - 窗口内既无 `{` 也无 `null` 且缓冲耗尽 → 保持「等待 `{`」的跨 feed 悬挂状态，下个 feed 从关键字后继续判定（不再丢弃线索）
  - 悬挂等待需有上限保护（如 64 字节内仍无判定则放弃该关键字），避免病态输入撑大缓冲
- 转换路径流式（convert_streaming_response）：结束时透传 IR 累计的 usage，替换硬编码 None
- 回归测试：用真实 glm SSE 字节流（17 个 `"usage":null` + 末尾 usage 帧）整段喂入断言取出 (13,16)；按 1 字节粒度逐块喂入断言同样成功；转换流式路径 usage 断言

### 4. 配置切换与收尾

- 用户实例迁移时将 `webdav_url` 改写为 `https://jn_file.88933.vip`
- 测试：迁移单测（keyring 存量 → DB → 标记位幂等）、导出/导入含密钥往返测试、gateway key 拉取覆盖/幂等测试、scanner 回归、clippy `-p jai --bins` 与 workspace 零告警
- 全量回归：`cargo test`（workspace）+ 前端 `pnpm build` 类型检查

## 风险与安全声明

- 明文密钥的安全性依赖 SQLite 文件权限与 WebDAV 服务器访问控制（与网关 key、MCP env 现状同级）
- 两台机器同时开自动推送仍可能互相覆盖（last-write-wins，现状不变，不在本次范围）
- 首启迁移读钥匙串可能弹最后一次授权框，迁移后永不弹

## 非目标（YAGNI）

- 不做密钥加密存储（DB 级加密）
- 不做 WebDAV 多机冲突检测/合并
- 不做启动时自动拉取（保持拉取覆盖的手动/既有触发路径）
- 不迁移其余 meta（端口、CORS 等）
