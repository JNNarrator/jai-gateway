# 下一迭代：WebDAV 同步可靠性二期（Sync Reliability v2）

> 提案日期：2026-09-05 · 前置：v0.1.6 + 已交付的「推送前远端备份 / 自动推送护栏 / 快照恢复 / 自动拉取（exportedAt last-write-wins）/ 流式缓冲护栏」
>
> 一句话目标：让多设备同步**看得见（路径透明）、丢不了（备份可管理）、可回退（远端版本恢复）、不被误覆盖（差异预警）**。

---

## 背景与动机

- 用户实测事故：远端 `jai-config.json` 被空配置覆盖后「备份找不到了」——根因已修（推送前时间戳备份 + 自动推送护栏），但**备份只落盘、不可见、不可管理**，用户仍无法主动确认"远端到底有没有、是什么版本"。
- 目录字段拼 URL 是裸字符串拼接：含空格/中文会直接拼出非法 URL；用户也看不到最终写入路径（路径漂移隐患）。
- 手动推送仍是"直接覆盖"，多设备场景下可能无意覆盖掉另一台设备刚加的内容。

## 迭代范围（5 项任务）

### T1 远端路径透明化 + 目录 URL 编码（小）
- `WebDavConfig::config_url` / `backup_url` 改为按 URL 语义拼接并**对路径段做百分号编码**（空格、中文、`#`、`?` 等），不再裸字符串拼接。
- 同步页新增只读「远端配置文件地址」，随 URL/目录输入实时预览 + 一键复制（防"备份找不到"= 路径漂移）。
- 测试：目录含空格/中文/特殊字符的 `config_url`/`backup_url` 单测。
- 影响面：`crates/gateway-core/src/sync.rs`、`ui/src/pages/SyncPage.tsx`。

### T2 远端备份列表与管理（中，核心）
- 新 IPC `webdav_backups_list`：PROPFIND（Depth:1）列出同目录 `jai-config.*.json`（含当前文件），解析时间戳与大小；备份名 `jai-config.<unix_ms>.json` 自带时间戳可解析。
- 新 IPC `webdav_backup_restore(<name>)`：GET 指定备份 → 走既有 `apply_import` 落库（与本地快照恢复同语义）。
- 新 IPC `webdav_backup_delete(<name>)`：仅允许删除 `jai-config.<digits>.json` 且非当前配置文件名（防误删）。
- UI：同步页「远端备份」区——列表（时间/大小）、「恢复此版本」、「删除」。
- **新依赖**：PROPFIND 响应是 XML，需引入 `quick-xml`（轻量、纯 Rust、无 C 依赖）；mock WebDAV 需支持 PROPFIND 返回 XML。
- 测试：mock PROPFIND + 备份列表/恢复/删除 roundtrip；改名防护单测。

### T3 推送前差异预警（中）
- 手动推送前（复用 `sync::try_pull` + `export_counts`）比对远端：若远端有**本地没有**的供应商/模型，弹确认框：
  「远端还有 N 个供应商 / M 个模型是本机没有的，推送将覆盖掉它们。仍然推送？」
- 默认**确认阻断**（可点「仍然推送」）；自动推送维持现有护栏语义不变。
- 测试：差异判定单测（本地⊂远端 / 等价 / 本地⊇远端 三态）。

### T4 远端备份保留策略（小，随 T2 交付）
- 推送成功后在远端滚动清理：同目录 `jai-config.<digits>.json` 保留最近 **10 份**（默认，可常量调整），删除最老的；绝不触碰当前配置文件与无关文件。
- 测试：清理选择逻辑单测（纯函数：给定文件列表+当前名 → 待删列表）。

### T5 自动同步失败系统通知（可选，低）
- 复用健康检查的系统通知通道：自动推送/拉取失败时发系统通知（错误摘要截断 140 字符）。
- 影响面：`src-tauri/src/main.rs`（`spawn_autopush` 失败分支挂通知）。

## 验收标准

- 每任务配测试；新增/修改单测 + 集成测试全绿。
- 全量门禁：`cargo fmt --check`、`cargo clippy --workspace --all-targets -- -D warnings`、`cargo test --workspace`、`tsc --noEmit` + `vite build` 零错误。
- **铁律不回归**：dsh / zcode 链路（含 Responses 流式）回归通过；passthrough 字节级直通不受影响。
- UI 无头验证：同步页新增区块渲染正确（真实 Chrome + Tauri 桩截图）。

## 风险与决策点（开工前需拍板）

1. **quick-xml 依赖**：备份列表需要解析 PROPFIND XML。若不想引入依赖，退化方案为「固定单份 `.bak` 轮替」（功能弱化：只保留最近一版）。→ 默认引入 quick-xml。
2. **T3 差异预警形态**：确认阻断 vs 仅提示。→ 默认确认阻断（可"仍然推送"），因为它正是用户事故场景的兜底。
3. **T4 保留份数**：默认 10 份（≈10 次推送历史），可常量调整。

## 非目标（另立迭代）

- 代理配置化（设置页 HTTP/HTTPS 代理 + reqwest 接线）
- 发布工程收尾（CI secrets / 首 tag / v0.1.0-beta）
- MCP Hub / 高级编排（远期立项）
- 用量成本估算 / 请求配额限流

## 交付物清单（预估）

| 任务 | 主要文件 | 工作量 |
| --- | --- | --- |
| T1 | sync.rs, SyncPage.tsx, sync 单测 | 小（0.5d） |
| T2 | sync.rs(备份列表/恢复/删除), main.rs(IPC), SyncPage.tsx, m7 测试扩展, Cargo.toml(+quick-xml) | 中（1.5d） |
| T3 | main.rs(webdav_push 前置差异), SyncPage.tsx(确认框), sync 单测 | 中（0.5d） |
| T4 | sync.rs(清理纯函数), 单测 | 小（随 T2） |
| T5 | main.rs(通知接线) | 小（0.25d，可选） |
