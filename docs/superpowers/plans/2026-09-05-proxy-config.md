# 下一迭代：网关代理配置化（Proxy Configuration, D8）

> 提案日期：2026-09-05 · 前置：v0.1.6 + Sync Reliability v2（已交付）
>
> 一句话目标：网关出站（上游模型转发 / 健康检查 / WebDAV 同步）统一支持可配置 HTTP(S)/SOCKS5 代理
> 与绕过列表，设置页可视化配置——国内网络访问 OpenAI/Anthropic 等上游不再依赖系统代理碰运气。

---

## 背景与动机

- 当前 `reqwest` 关闭 default-features（`rustls-tls`/`stream`/`json`），网关出站**没有任何代理配置能力**；
  上游不可达时用户只能改系统/环境变量，且无法按供应商区分。
- 上一迭代计划中已把「代理配置化」列入非目标（另立迭代）——本次正式立项。
- 项目已有成熟约定可复用：端口变更走「持久化 + 重启网关生效 + UI 提示」（`settings_set_port`），
  代理配置采用同一套约定即可，无需热重建 client（避免动 `GatewayCtx`/`AppCore` 结构引发 dsh 链路风险）。

## 迭代范围（4 项任务）

### T1 代理配置模型与校验（gateway-core 新模块 `netcfg`）
- `ProxyConfig { enabled: bool, url: String, bypass: Vec<String> }`，meta 键
  `proxy_enabled` / `proxy_url` / `proxy_bypass`（逗号分隔）。
- 纯函数：
  - `parse_proxy_url(&str) -> Result<reqwest::Proxy, String>`——仅接受 `http://` / `https://` /
    `socks5://` scheme 且含 host:port；URL 内 `user:pass@` 原生透传（代理认证）。
  - `build_client(proxy_cfg: Option<&ProxyConfig>) -> reqwest::Client`——启用时
    `Proxy::all(url).no_proxy(绕过列表)`，关闭时与现状完全一致（不设置任何 Proxy，行为零变化）。
- 单测：URL 校验矩阵、bypass 语义（精确 host / `.suffix` / `*`）、meta roundtrip。
- 前置：workspace `reqwest` 增加 `socks` feature（支持 socks5；纯增量特性）。

### T2 接线（生效时机 = 重启网关，与端口一致）
- `GatewayCtx::new`（gateway-core）：构造 http client 时读 meta → `netcfg::build_client`
  （上游转发 / 健康检查统一生效）。
- `AppCore`（main.rs）：构造 http client 同一逻辑（WebDAV 同步 / 测试连接统一生效）。
- 新 IPC：
  - `proxy_get` / `proxy_set`（读写 meta，`proxy_set` 校验非法值并回读）
  - `proxy_test`（用候选配置经代理探测 `https://www.gstatic.com/generate_204`，
    返回「连接成功/失败原因」，不落库——与 `webdav_test` 同风格）

### T3 设置页 UI
- 设置页新增「网络代理」卡片：启用开关、代理地址（`http://host:port`）、绕过列表
  （每行一个 host 或 `.suffix`，`#` 注释）、「测试连接」按钮、「保存后重启网关生效」提示
  （复用端口卡片的既有提示文案与模式）。
- `api.ts` / `types.ts` 接线。

### T4 集成测试与回归
- 集成测试（gateway-core）：本地 mock HTTP 代理（记录目标 host + 返回固定响应）+ mock 上游
  ——配置代理后经网关请求上游，断言**请求确实走了代理**（代理侧计数+1）且响应正确；
  bypass 命中条目直连上游（代理计数不变）。
- 全量门禁：fmt / clippy `-D warnings` / `cargo test --workspace` / tsc + vite build。
- **铁律**：代理默认关闭，不改变任何现有请求行为；dsh / zcode 链路（含 Responses 流式）回归通过。

## 验收标准

- T1 单测覆盖校验与 bypass 全矩阵；T4 端到端证明「走了代理 / 绕过直连」两条路径。
- 设置页 UI 无头验证（开关/输入/测试按钮渲染 + 交互），截图留档。
- 全量门禁零错误，代理关闭时现有测试零回归（行为不变是硬验收）。

## 风险与决策点（开工前拍板）

1. **socks5 支持**：默认加 `socks` feature 一并支持（成本极低）。
2. **生效时机**：默认「重启网关生效」（与端口一致）；若体验要求可后续做 ArcSwap 热重建（另立小迭代）。
3. **代理认证**：走 URL 内 `user:pass@`（reqwest 原生），UI 不做独立密码字段（本期）。
4. **探测 URL**：默认 `https://www.gstatic.com/generate_204`（国内可达性一般但 204 语义最准；
   失败提示里附完整错误，用户可自行判断）。

## 非目标（另立迭代）

- 用量成本估算 / 请求配额限流
- 发布工程收尾（CI secrets / 首 tag）
- MCP Hub / 高级编排（远期）
- 代理热重建（ArcSwap，体验优化时再做）

## 交付物清单（预估）

| 任务 | 主要文件 | 工作量 |
| --- | --- | --- |
| T1 | `crates/gateway-core/src/netcfg.rs`（新）、Cargo.toml(+socks)、netcfg 单测 | 中（0.5d） |
| T2 | `proxy.rs`(GatewayCtx::new)、main.rs(IPC×3 + AppCore.http)、注册 | 中（0.5d） |
| T3 | SettingsPage.tsx、api.ts、types.ts | 中（0.5d） |
| T4 | `tests/m10_proxy.rs`（新，mock 代理 + 端到端） | 中（0.75d） |
