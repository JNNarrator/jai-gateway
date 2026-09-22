<p align="center">
  <img src="ui/public/jai-logo.svg" width="88" alt="JAI logo" />
</p>

# JAI — 桌面 AI API 网关

> 开箱即用的本地 AI API 网关：把官方与第三方中转的杂牌 token 来源，收敛成一个稳定的本机入口（`127.0.0.1:1314`），并让多设备（macOS / Windows）配置随 WebDAV 保持同步。

- **当前版本**：v0.3.0（2026-09-22，见 [CHANGELOG.md](CHANGELOG.md)）
- **进度**：M0–M9 里程碑工程侧全部完成 · UI 2.0（阶段 0–6）已落地 · MCP 统一代理（`server__tool` 转发 + `skill__` 投递）· WebDAV 多设备同步加固（独立推/拉间隔 / 远端备份管理 / 冲突 diff）
- **近期加固**：出站代理配置化 · 多模态（图片跨族转换 + 模型输入/输出模态集合）· UX 增强（日志复现 / 命令面板 / 健康横幅）· UI 规范门禁化（静态 lint + 双尺寸双主题探针）

> 已知缺口：macOS 目前只发布 `aarch64` 产物，Intel Mac 无安装包也无法自动更新；签名/公证与真机验收需在发布主机执行（见 [docs/design/release.md](docs/design/release.md)）。

---

## 界面预览

| 供应商管理 | 网关接入 |
| --- | --- |
| ![供应商管理](assets/screenshots/providers.png) | ![网关接入](assets/screenshots/gateway.png) |
| **用量统计** | **请求日志** |
| ![用量统计](assets/screenshots/stats.png) | ![请求日志](assets/screenshots/logs.png) |

## 项目架构

JAI 是 Tauri 2 本地应用，分三层：**React 前端 → Tauri 桌面壳 → Rust 网关核心库（gateway-core）**。

```
┌──────────────────────────────────────────────────────────────────┐
│ UI — React 19 + TypeScript + TailwindCSS 4 + shadcn/ui           │
│ Providers · Models · Gateway · Logs · Stats · MCP ·              │
│ Skills · Sync · Settings（共 9 页）                              │
└─────────────────────────────────┬────────────────────────────────┘
                                  │ Tauri IPC（73 个命令）
┌─────────────────────────────────▼────────────────────────────────┐
│ 桌面壳 src-tauri — Tauri 2                                       │
│ 网关监督循环（看门狗 + 自动重启）· 系统托盘常驻                  │
│ WebDAV 自动同步（变更防抖 + 推/拉各自独立定时）· 供应商健康检查   │
└─────────────────────────────────┬────────────────────────────────┘
                                  │ 进程内调用
┌─────────────────────────────────▼────────────────────────────────┐
│ gateway-core — Rust 网关核心库                                   │
│ server      Axum 网关：入站协议线 / 安全中间件 / 代理            │
│ codec       协议中间表示（IR）+ 各协议适配器 + 能力表            │
│ router      多渠道路由：优先级故障转移 / 负载均衡                │
│ store       SQLite 唯一事实源（供应商/模型/凭据/日志/快照）      │
│ discover    上游模型自动发现 · sync WebDAV 拉取推送              │
│ netcfg      出站网络：HTTP(S)/SOCKS5 代理 + 绕过列表             │
│ modality    模型输入/输出模态集合 · image 图片跨族转换           │
│ effort      推理档位（reasoning effort）值域声明与归一           │
│ mcp         MCP Server 台账 · skills 技能管理 · vault 存量迁移   │
└─────────────────────────────────┬────────────────────────────────┘
                                  │ HTTP 出站（reqwest）
            ┌───────────────┬─────┼─────────┬─────────────┐
            ▼               ▼               ▼             ▼
    openai_compat   openai_responses    anthropic      gemini
```

### 协议双轨制（架构核心）

网关的协议转换层按「**同族直通，跨族才转换**」设计（详细规格见 [docs/design/protocol-ir.md](docs/design/protocol-ir.md)）：

- **同族直通**：入站协议族与出站供应商同族时走**字节级直通**——只改写上游 URL 与鉴权头，body 原样转发。零损耗，缓存控制、citations 等未建模特性原样保留。
- **跨族转换**：解码 → 统一中间表示（IR）→ 编码。IR 先归一三大结构性差异：system 提示词位置、tool 结果载体、stop reason 枚举，使任意客户端协议都能组合任意上游模型（含 tool calling）。
- **能力声明 + 兼容性规划**（[protocol-ir §10](docs/design/protocol-ir.md)）：每个出站协议族声明六面能力表（参数/工具/tool_choice/响应格式/reasoning/流式 usage），跨族转换前先规划——原生支持直传、可降级执行、无法表达才 400 拒绝；降级与 Lenient 丢弃进结构化日志（`CapabilityWarn`，UI 日志页可见）。
- 日志采集在直通路径上做旁路轻量扫描（抓 usage 数字与结束原因），不做语义级解析。

### 桌面壳关键机制

- **网关监督循环**：独立任务常驻，异常退出自动重启（带重启计数），端口被占时自动顺延
- **托盘常驻**：关闭窗口只隐藏到托盘，网关保持运行；真正退出走托盘菜单
- **WebDAV 自动同步**：配置变更防抖合并 + 定时推送，**推送与拉取各自独立间隔**（互不绑定）；
  手动推/拉与其互斥，推送前留存本地快照
- **供应商健康检查**：每 10 分钟一轮探测全部启用供应商，状态跃迁时发系统通知
- **SQLite 迁移 + 异步日志管道**：启动即应用迁移，失败即中止启动（早拦截）；日志经有界队列异步落库，不影响请求主路径

## 技术栈

| 层 | 选型 |
| --- | --- |
| 桌面框架 | Tauri 2.0（macOS / Windows） |
| 后端 | Rust · Axum（HTTP 网关）· Reqwest（出站）· rusqlite（SQLite） |
| 前端 | React 19 + TypeScript + TailwindCSS 4 + shadcn/ui + recharts + react-hook-form/zod |
| 存储 | SQLite（供应商 / 模型 / 凭据 / 网关密钥 / 请求日志 / meta） |
| 同步 | WebDAV（jai-export/v1 导出协议，last-write-wins） |
| 更新 | GitHub Releases + tauri-plugin-updater（minisign 签名校验） |

## 核心特性

- 多供应商管理：OpenAI 兼容 / OpenAI Responses / Anthropic / Gemini 四族渠道；凭据明文存本地 SQLite（与网关 Key 同级安全模型，安全性依赖数据目录文件权限；钥匙串仅保留一次性存量迁移）
- **配置随 WebDAV 同步**：供应商 API Key、网关 Key、WebDAV 密码随导出同步，换机器拉取即用（客户端零改动）；手动重新生成网关 Key 自动更新远端；推送前自动留存远端上一版时间戳备份 + 本地快照，自动推送带「空配置不覆盖远端」护栏（last-write-wins）；远端目录可按时间戳列出 / 恢复 / 删除 `jai-config.<时间戳>.json` 备份（恢复后自动对齐拉取基线，避免被立刻拉回）
- 对外暴露统一网关入口（`127.0.0.1:1314`），支持多条入站协议线：
  - OpenAI `POST /v1/chat/completions` 与旧版 `POST /v1/completions`
  - OpenAI Responses API `POST /v1/responses`
  - Anthropic `POST /v1/messages`（Claude Code 直连）与 `POST /v1/messages/count_tokens`（粗估）
  - 模型列表 `GET /v1/models`、健康检查 `GET /healthz`
  - MCP 元数据服务 `POST /mcp`（Streamable HTTP）：把网关登记的 MCP Server / Skill 台账以 MCP 协议暴露给 Agent——五个只读工具（`list_mcp_servers` / `get_mcp_server_detail` / `get_tool_schemas` / `list_skills` / `get_skill_detail`，env 仅回键名不回值）；开启「代理执行」开关（`proxy_allowed`）的 Server 其工具以 `<server>__<tool>` 动态暴露、`tools/call` 显式转发给真实 Server 执行，启用技能以 `skill__<name>` 按名投递全文（32KB 截断并注明）——选择权始终在 Agent，网关只做发现与转发，不注入对话链路
- 同名模型多渠道路由：按优先级自动故障转移 + 健康感知排序 + 同优先级权重负载均衡；支持 `供应商名/模型名` 限定 ID（多供应商同名时精确指定，发给上游前剥掉）
- 模型别名/映射：每个模型可配置发给上游的真实模型 ID
- 跨协议转换（含 tool calling）：让任意客户端组合任意上游模型
  - **结构化输出降级执行**：客户端请求 `response_format(json_schema)` 时——openai 系上游原生外传；anthropic/gemini 上游降级为「提示词指令注入 + 输出 JSON 校验」（strict 校验失败返回 502）
  - **reasoning effort 闭环**：入站 `reasoning_effort` / `reasoning.effort` 建模后按目标族映射——Native 透传、Anthropic 转 `thinking` 开/关、无能力族忽略并 WARN；供应商/模型级可声明值域，超出即归一，避免上游 400
  - **Codex 扩展工具折叠还原**（[protocol-ir §10](docs/design/protocol-ir.md)）：`shell` / `apply_patch` / `custom` / `local_shell` 声明与调用折叠为 function 工具（Codex 客户端可接任意普通模型），回程按工具身份映射还原原始 item（含流式 item 类型与增量事件名）
  - **截断语义对齐**：`max_tokens` 截断输出 `status:"incomplete"` + `incomplete_details.reason`（不谎报为可重试错误），并统一落库 `request_logs.stop_reason`
- 上游模型自动发现：从供应商 `/models` 拉取模型并入库，自动填上下文窗口/最大输出缺省值；输入/输出模态按上游可信度递降解析（`inputModalities` / `architecture.input_modalities` / `supports_vision` 等），取不到即为未知，不做模型名臆断
- **出站网络代理**（设置页「网络代理」）：HTTP(S) / SOCKS5（可含 `user:pass@` 认证）+ 绕过列表 +「测试连接」；上游模型发现、健康检查与 WebDAV 同步统一经代理出站，**保存后重启网关生效**；关闭时代理行为与默认完全一致
- MCP / 技能（Skill）管理：网关内登记 MCP Server 与技能（连通检查、工具查看与调用测试、按 Server 的「代理执行」开关、ZIP 批量导入、Markdown 导出）；MCP 配置导入自动识别三种格式——`{"mcpServers":{...}}` JSON、`codex mcp add` 命令行、Codex `[mcp_servers.*]` TOML 片段
- 应用内更新：设置页一键检查 / 下载 / 安装 GitHub Releases 最新版本（minisign 签名校验，重启生效）
- 请求日志（仅元数据，含结束原因）与用量统计可视化（recharts 堆叠柱状图，近 7/30/90 天；可配保留天数/行数上限）
- **可观测与排障**：日志详情（入站协议 / 路由 / 供应商 / 模型 / 耗时 / token / 结束原因 / 错误）与一键「复制为 cURL」复现（请求体为占位模板，仍不落内容）、`Ctrl+K` 全局命令面板、网关页健康自检横幅、WebDAV 推送差异明细弹窗（远端独有 / 本地独有逐条列出）
- **UI 2.0**：明暗双主题（跟随系统）、可折叠侧边栏、shadcn/ui 组件体系、表单校验就地展示（react-hook-form + zod）、自绘标题栏 + 窗口毛玻璃特效
- **交互防丢失**：供应商 / MCP / 技能 / 设置四处表单接入脏状态 guard（弹窗内与页内均覆盖），切页、关窗与 `beforeunload` 均拦截；日志页轮询随可见性暂停 / 恢复
- **模型页交互**：只读表格 + 右侧详情抽屉（表头吸顶，单个模型的编辑项一次保存），行内可交互控件大幅收敛
- 安全基线：强制鉴权（常量时间比对）、Host/Origin 校验、CORS 默认拒绝、鉴权失败限速、推送前快照

## 重点支持客户端

JAI 当前专注适配两个国产 Agent，协议直通与跨族转换对其透明可用：

- **DeepSeek Harness（dsh）**：第一优先客户端，Chat Completions 与 Responses 两条协议线均作为适配目标——选择 `openai-completions`（Chat）或 `openai-responses`（Responses）协议线接入。真机联调记录见 [docs/test-report-dsh.md](docs/test-report-dsh.md)。
- **zcode**：**已真机实测通过**（2026-09-01）——实测走 **OpenAI Responses 入站**（`/v1/responses`），流式/非流式均 200；Anthropic 线（`kind: anthropic` + baseURL）同样可用。两条线网关均已支持并经真实流量验证，接入与推理档位排障见 [docs/zcode接入.md](docs/zcode接入.md)。

接入配置：baseURL `http://127.0.0.1:1314/v1`，API Key 用网关 Key（`sk-jai-*`，见应用「网关」页，同页提供各客户端内置接入示例）。

### dsh 一键接入（脚本）

```bash
node scripts/setup_dsh.mjs --dry-run   # 先看将发生的 diff（不写盘、key 脱敏）
node scripts/setup_dsh.mjs             # 合并进 $DSH_HOME/settings.yaml + 写 $DSH_HOME/.env(0600)
```

把 JAI 作为一条 `llm-pi-ai` provider route 写进 dsh 配置（默认 route 名 `jai`、`api: openai-responses`，
可用 `--api openai-completions` 切 Chat 线），`models` 取自 JAI 实时 `/v1/models`，网关 Key 写进
`$DSH_HOME/.env`（权限 0600，由 `apiKeyEnv` 引用）。改动前自动打时间戳备份，**不改动其他 route、
注释与用户自加字段**，重复执行幂等。合并结果会先过 dsh 自己的运行时 schema 校验
（`@deepseek-ai/dsh-llm-pi-ai.Config`）再落盘。

> ⚠️ 执行前请先退出正在运行的 dsh：dsh 运行时会整份重写 `settings.yaml`，可能覆盖合并结果。
> 自测：`node scripts/setup_dsh_test.mjs`（离线、自带假网关，9 项断言）。

> 💡 Windows 提示：JAI（reqwest）不读 Windows 系统代理。需代理访问上游时，优先在设置页「网络代理」填写代理地址（保存后重启网关生效）；也可用环境变量 `HTTPS_PROXY=http://127.0.0.1:7890` 启动，否则对需代理的上游返回 502 `all_providers_failed`。

其他客户端（Claude Code、Codex、Continue 等）经协议直通仍可使用，但不在专属适配范围内。

## 使用 · 快速接入

### 任意 OpenAI 兼容客户端

```bash
OPENAI_API_BASE=http://127.0.0.1:1314/v1
OPENAI_API_KEY=sk-jai-xxxx
```

同名模型可配置多个渠道，按优先级自动故障转移（一级 5xx/429/超时 → 顺延下一渠道）。

### Claude Code（Anthropic 线）

```bash
ANTHROPIC_BASE_URL=http://127.0.0.1:1314
ANTHROPIC_AUTH_TOKEN=sk-jai-xxxx            # 或 ANTHROPIC_API_KEY=sk-jai-xxxx
claude
```

要求：供应商页添加一个 **Anthropic** 协议族渠道并录入上游 Key，模型列表存在 Claude 型号（如 `claude-sonnet-4-5`）且启用。多轮对话、工具调用、prompt caching 走字节级直通；`count_tokens` 由网关粗估返回，避免客户端降级。

### MCP 元数据服务（/mcp）

在任意支持 MCP 的客户端（dsh / Claude Code / Continue 等）的 `mcpServers` 配置中加入（应用「网关」页有同款配置，**点「复制配置」自动填入真实密钥**）：

```json
{
  "mcpServers": {
    "jai-registry": {
      "type": "http",
      "url": "http://127.0.0.1:1314/mcp",
      "headers": { "Authorization": "Bearer <网关密钥>" }
    }
  }
}
```

接入后 Agent 可查询网关登记的 MCP Server / Skill 台账（五个只读工具）。开启「代理执行」的 Server，其工具会以 `<server>__<tool>` 出现在工具列表中、由 Agent 主动调用并经网关转发到真实 Server；启用技能可用 `skill__<name>` 拉取全文。网关只做发现与转发，不注入对话链路。

#### 代理转发的两条语义（排障必读）

1. **结果原样透传**：`tools/call` 转发路径把上游 MCP 结果**按原语义**交回客户端——`content` 逐块保留（含 `image` / `resource_link`）、`isError` 原样冒泡、`structuredContent` 保留，另附一个非标准 `source` 字段（`jai-gateway-proxy/<server>`）便于审计。
   因此「上游工具失败」在客户端就是 `isError: true`，不会被网关压成成功；静态台账/技能工具仍返回 JSON 文本。
2. **等待预算**：单次代理调用有独立预算 `JAI_MCP_PROXY_CALL_TIMEOUT_MS`（默认 **55000**，即 55s）。
   超时后网关**主动放弃等待**，返回工具级错误（`isError: true`）并提示长命令改用 `terminal_start` + `terminal_poll`。
   默认值刻意**略低于常见 MCP 客户端的单次调用预算**（dsh 客户端默认 60s 硬中止，错误码 `-32001`）：
   网关若比客户端更能等，agent 只会拿到无信息量的客户端超时，同批并行的其它调用还会一起作废。

| 环节 | 预算 | 调参入口 |
| --- | --- | --- |
| MCP 客户端（以 dsh 为例） | 60s 硬中止（`-32001`） | dsh 侧 `toolCallTimeoutMs` |
| 网关代理转发 | 55s | `JAI_MCP_PROXY_CALL_TIMEOUT_MS` |
| 网关 stdio 连接池单次交换 | 120s | `JAI_MCP_POOL_CALL_TIMEOUT_MS` |
| 连接池初始化握手 | 120s | `JAI_MCP_POOL_INIT_TIMEOUT_MS` |
| 连接池空闲回收 | 30s | `JAI_MCP_POOL_IDLE_MS` |

有状态/长时工具（终端会话类）建议把客户端预算调到网关之上，再用「先启动后轮询」的异步模式；`tools/list` 的动态部分有 30s TTL 缓存，单个 Server 拉取失败只跳过该 Server。

## 开发

### 目录结构

```
JAI/
├── crates/gateway-core/    # 网关核心库（协议/路由/存储/同步/MCP/Skill）
│   ├── src/server/         # Axum 网关：入站协议线 / 安全中间件 / 代理
│   ├── src/codec/          # 协议 IR + 四族适配器 + 能力表
│   ├── src/store/          # SQLite 迁移 / CRUD / 日志 / 导入导出 / 快照
│   └── examples/           # jai_standalone（无桌面壳的独立网关，供真机联调）
├── src-tauri/              # Tauri 桌面壳（73 个 IPC 命令 / 托盘 / 网关监督）
├── ui/                     # React 前端（9 页 + shadcn 组件 + api/types 数据层）
├── scripts/                # 开发 / 门禁 / 客户端接入脚本
├── tools/visual-regression # 双尺寸 × 双主题 UI 探针与门禁
├── assets/screenshots/     # README 截图
└── docs/                   # 需求与设计文档（见下方索引）
```

### 本地开发启动

```bash
pnpm --dir ui install   # 前端依赖（本仓库唯一 package.json 在 ui/，无根工作区）
bash scripts/dev.sh     # 一键启动：Vite + Tauri 桌面壳
```

- Vite 固定监听 `http://127.0.0.1:5173`
- 网关默认监听 `http://127.0.0.1:1314`（被占自动顺延，UI 显示实际端口）
- 健康检查：`curl http://127.0.0.1:1314/healthz`
- 只跑网关（无桌面壳）：`cargo run -p gateway-core --example jai_standalone`（真机联调用的临时独立网关：读 `ONEMODEL_API_KEY` / `JIYUANLVDONG_API_KEY` 建渠道，数据目录 `JAI_STANDALONE_DATA_DIR`，端口默认 13140）

### 质量门禁

| 命令 | 内容 |
| --- | --- |
| `bash scripts/regression.sh` | `cargo fmt --check` + `clippy -D warnings` + `cargo test --workspace` + 前端 `tsc --noEmit && vite build` |
| `bash scripts/ui_lint.sh` | UI 静态规范：字号 ≥11px、图标按钮有可访问名、列表 key 不用下标、命中区不靠伪元素外扩 |
| `node tools/visual-regression/gate.mjs` | UI 探针（需 vite:5173）：1180×800 / 900×600 × 明暗双主题，判据含对比度 AA、字号 ≥11px、无横向溢出、弹窗几何、主操作首屏可达、有效命中区 ≥24×24、**最小窗口尺寸在各平台都生效**，单一退出码 |
| `node scripts/tauri_window_check.mjs` | 最小窗口尺寸门禁（零依赖）：平台配置合并（RFC 7396）数组整体替换会静默丢掉基础配置的 `minWidth/minHeight`（macOS 曾因此没有最小尺寸限制），故要求平台窗口补齐基础配置的每个键，且解析后最小尺寸 ≥ UI 验收尺寸（900×600） |
| `bash scripts/release_check.sh` | 发布前 7 步门禁：工作区干净 / 版本 / CHANGELOG / tag / 全量回归 / UI 门禁 |
| `node scripts/setup_dsh_test.mjs` | dsh 接入脚本自测（离线、自带假网关，9 项断言） |
| `bash scripts/observe48h.sh` | 常驻观察采样（配合 `scripts/jai_supervisor.sh` 拉活） |

CI（[.github/workflows/ci.yml](.github/workflows/ci.yml)）：macOS + Windows 跑 Rust 三件套，Ubuntu 跑前端类型检查与构建；发布由 tag `v*` 触发（[release.yml](.github/workflows/release.yml)）。

## 文档索引

| 文档 | 内容 |
| --- | --- |
| [《JAI — 桌面 AI API 网关.md》](./JAI%20—%20桌面%20AI%20API%20网关.md) | 需求全量定义与评审记录 |
| [docs/design/protocol-ir.md](docs/design/protocol-ir.md) | 协议中间表示、逐字段映射总表、行为规范 |
| [docs/design/storage-schema.md](docs/design/storage-schema.md) | SQLite 表结构、凭据管理、日志策略 |
| [docs/design/multimodal-support.md](docs/design/multimodal-support.md) | 图片跨族转换链路与模型输入/输出模态集合 |
| [docs/design/roadmap.md](docs/design/roadmap.md) | M0–M9 里程碑路线图与实施进度快照 |
| [docs/design/release.md](docs/design/release.md) | 签名 / 公证 / 更新通道 / 发布检查单 |
| [docs/design/antivirus.md](docs/design/antivirus.md) | 杀软误报排查流程 |
| [docs/zcode接入.md](docs/zcode接入.md) | zcode 接入与推理档位值域声明排障 |
| [docs/test-report-dsh.md](docs/test-report-dsh.md) | dsh 真机联调测试报告 |
| [docs/bug和优化清单.md](docs/bug和优化清单.md) | 唯一问题追踪入口（bug / 优化） |
| [docs/ui优化.md](docs/ui优化.md) | UI/UX 优化清单 |
| [docs/视觉回归整改plan.md](docs/视觉回归整改plan.md) | 视觉回归整改（动态点击遍历 + 几何测量） |
| [docs/superpowers/specs/2026-08-31-ui-framework-upgrade-design.md](docs/superpowers/specs/2026-08-31-ui-framework-upgrade-design.md) | UI 2.0 设计 spec（阶段 0–6） |
| [docs/superpowers/specs/2026-09-02-secrets-sqlite-sync-design.md](docs/superpowers/specs/2026-09-02-secrets-sqlite-sync-design.md) | 凭据入库 + 随 WebDAV 同步设计 |
| [docs/superpowers/specs/2026-09-03-mcp-proxy-skill-delivery-design.md](docs/superpowers/specs/2026-09-03-mcp-proxy-skill-delivery-design.md) | MCP 统一代理 + Skill 投递设计 |

## License

[MIT](./LICENSE)
