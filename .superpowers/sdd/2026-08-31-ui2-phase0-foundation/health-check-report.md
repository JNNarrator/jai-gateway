# 供应商定时健康检查 + 失败提醒 — 实施报告

日期：2026-09-01 · 分支 main · 基线 869b8af · 需求：优化点 7「供应商里面的模型要定时测试，如果不通了要有提醒机制」

## 改动文件

- `src-tauri/Cargo.toml`：新增 `tauri-plugin-notification = "2"`（Cargo.lock 随之新增其传递依赖：mac-notification-sys / notify-rust / zbus / tauri-winrt-notification 等 39 项，均为该插件跨平台后端，正常）
- `src-tauri/src/main.rs`：探测核心提取 + 健康检查循环 + 通知 + 插件注册
- `src-tauri/capabilities/default.json`：`permissions` 追加 `notification:default`
- 未触碰 `ui/src/api.ts`、`ui/src/types.ts`（前端零改动，tsc 确认无波及）

## 探测复用点说明

现有「测试连接」命令 `provider_test` 的探测链路为：`fetch_provider`（读库）→ `load_secret_or_none`（密钥环，内部已 spawn_blocking）→ `discover_models`（gateway-core 自由函数，按协议族拉模型列表，内部 HTTP 超时 20s；HTTP 200 即连通，0 个模型也算通）。

本次把「读凭据 → 模型发现」两步提取为共享函数 `probe_provider(core: &AppCore, row: &ProviderRow) -> Result<usize, String>`（main.rs），`provider_test` 改为调用它（行为不变），定时健康检查复用同一函数——不通过 State 调命令，探测逻辑单一来源。

## 健康检查循环（spawn_health_check / health_round）

- 仿照 `spawn_autopush` 模式：setup 阶段 `tokio::spawn` 常驻循环；间隔为代码常量 `HEALTH_CHECK_INTERVAL = 600s`（本期不做 UI 配置）；启动后立即跑第一轮
- 每轮：`store::provider_list` 拉全量 → 仅探测 `enabled=1` → **顺序**逐个探测
- 单点防护：
  - 每个 provider 的探测包在独立 `tokio::spawn` 里，panic 只表现为 `JoinError`（记为该 provider 失败），不拖垮循环，继续下一个
  - 探测整体包 `tokio::time::timeout(30s)`（常量 `HEALTH_PROBE_TIMEOUT`）；超时记为失败，孤儿任务受 `discover_models` 内部 20s HTTP 超时约束自行结束
  - DB/密钥环操作走 `spawn_blocking`，不阻塞 runtime
- 结果落库：复用 gateway-core store 层 `provider_mark_ok` / `provider_mark_err`（proxy.rs 真实流量用的同一套函数），写 `last_ok_at / last_err_at / last_err_msg`，即供应商列表健康徽章的数据源
- 探测不改动 `enabled`，不参与网关路由决策（路由读 enabled/优先级，健康字段仅展示）

## 通知触发逻辑（tauri-plugin-notification）

- 插件注册：`tauri::Builder .plugin(tauri_plugin_notification::init())`（注意：与 updater 的 `Builder::new().build()` 写法不同，notification 导出的是 `init()`）
- 权限：`capabilities/default.json` 追加 `notification:default`
- 状态判定以**库为准**：`last_err_at` 非空即处于失败态（`provider_mark_ok` 会清空该列、`provider_mark_err` 会写入；真实流量的 mark 也落在同一列，因此健康检查与实际业务视角一致）
- 仅状态跃迁时通知，不逐轮打扰：
  - ok → fail：通知「供应商『X』连接失败」，正文为错误摘要（截断至 140 字符；DB 内另由 `provider_mark_err` 截断至 300）
  - fail → ok：通知「供应商『X』已恢复」
  - 首轮（应用启动后第一轮）只记录落库，一律不通知；后续每轮开始前未见过该 provider 的（新增/重启用）也天然以库内既有状态为基准，无虚假跃迁
- 通知发送失败只 `eprintln!`，不影响探测流程；日志前缀统一 `[health]`（与 `[autopush]` 风格一致）

## 验证证据

```
cargo build -p jai
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 10.43s   → 零错误

cargo clippy -p jai -p gateway-core 2>&1 | grep -vE "deprecated in favor" | grep -cE '^warning: |^error: '
0                                    （clippy 退出码 0；未新增任何 #[allow]）

cargo test -p gateway-core 2>&1 | grep "test result"
ok. 95 passed; 0 failed   ok. 1 passed   ok. 4 passed   ok. 5 passed   ok. 7 passed
ok. 5 passed; 0 failed    ok. 6 passed   ok. 4 passed   ok. 3 passed   ok. 1 passed
ok. 1 passed; 0 failed    ok. 2 passed   ok. 0 passed                    → 全部 ok，0 failed

pnpm --dir ui exec tsc --noEmit
（无输出，退出码 0）→ 前端零错误
```

## 说明与边界

- 本会话改动不部署（48h 观察期中），仅本地验证
- 首轮立即执行：启动后即有最新徽章数据；因首轮不通知，不会开机弹窗
- 超时探测的孤儿任务与真实流量的 mark 写库之间存在秒级竞态（如探测成功覆盖刚被流量标记的错误），与 proxy 侧 mark 行为一致，可接受
- macOS 通知权限由系统在首次发送时授予；desktop 端 `init()` 即所需配置，无额外 info.plist 改动

## 提交

- commit：见下方（`git log -1` 输出）
- 提交后 `git status`：仅剩观察期日志 `.superpowers/observe48h.log` 的既有修改与未跟踪 `.playwright-mcp/`（本任务未触碰）
