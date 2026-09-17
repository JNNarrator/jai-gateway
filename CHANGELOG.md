# Changelog

All notable changes to this project will be documented in this file.

## [Unreleased]

### Fixed
- **Responses 出站丢弃「消息级」图片**（bug 7，多模态链路最后一块静默丢失）：
  - 现象：`codec/responses.rs::encode_request` 的 user 消息分支对 `Block::Image` 直接跳过
    （注释称「Responses 上游 v1 不支持图片内联转换」），图片被无声吃掉；**只有图片没有文本**
    的用户消息更严重——连 `message` 都不产出，整轮图消失。同族的「工具结果内嵌图片」此前已修，
    只剩这条路径没接上。
  - 修复：按**块序**把图片渲染为 `input_image` 内容项，复用已有的 `render_input_image()`
    （url 优先，base64 包回带真实 media_type 的 data URL）。纯文本消息的产出形状**逐字节不变**。
  - 风险评估：`input_image` 已在 `function_call_output.output` 上生产使用并通过跨族实测，
    此处是同 schema 类型、同族协议，不新增风险面；能力声明侧本就视「user 消息带图」为原生支持，
    本次是让 encoder 与已声明能力面对齐。
  - 回归：`tests/multimodal_image.rs` 新增 3 例；临时恢复丢弃逻辑时含图用例必红
    （`text + image` 只产出 1 个内容项、仅图片时「应产出 message」panic）。
- **`mcp_pool` 集成测试负载敏感 + 失败级联**（bug 6，长期污染 `scripts/regression.sh` 门禁）：
  - 根因一：`initialize` 握手复用了 `JAI_MCP_POOL_CALL_TIMEOUT_MS`（测试压到 100ms），
    机器有负载时「spawn 假 server + 握手」超标 → 误报 `MCP initialize 超时`。
    现单列 `JAI_MCP_POOL_INIT_TIMEOUT_MS`（默认与调用超时一致，**生产行为不变**）。
  - 根因二：测试的 `remove_var` 写在断言**之后**，panic 即跳过清理 → 环境变量残留
    → 后续用例连带超时（表现为「每轮失败的用例集合都不同」）。现改 `EnvGuard` RAII，
    `Drop` 时还原，panic 与提前 return 都能收回。
  - 回归：新增 `init_handshake_uses_separate_budget`（调用超时压到 1ms，断言报错不含
    `initialize`），旧实现下必红；原复现命令 `JAI_MCP_POOL_CALL_TIMEOUT_MS=30` 下
    `MCP initialize 超时` 出现次数由 5/7 → **0**；6 路 CPU 负载连跑 15 轮 **0 失败**。
- **暗色主题下 toast 不可读**（近白字 + 浅绿底，对比度 1.01）:
  - 根因：`main.tsx` 把 `<Toaster>` 挂在 `<ThemeProvider>` **外面**，sonner 里的 `useTheme()`
    取不到主题、永远退回 `system`，`richColors` 于是给浅色调色板（成功底色浅绿）；
    而 `toastOptions.style` 又把文字色写死为 `var(--foreground)`（暗色下近白）→ 两者叠加不可读。
  - 修复：`<Toaster>` 移入 `<ThemeProvider>` 内部；暗色下实测对比度 1.01 → **16.73**。

### Changed
- **UI 可读性与可达性整改**（视觉回归审计复核，`docs/bug和优化清单.md` §3 第 4/8/9/10 条）：
  - 对比度：浅色主题 4 个根因逐条修掉（日志表 muted 小字 `--muted-foreground` 0.552 → 0.52、
    日志 `errorKind` 列 `text-red-600` → `text-red-700`、主按钮 hover 由「冲淡」改「加深」：
    新增 `--primary-hover` token 取代 shadcn 默认的 `hover:bg-primary/90`，把原 4.27 提到 6.16）。
    **双主题不达标项均清零**（审计分组 浅色 0 组 / 暗色 0 组）。
  - 命中区：模型页「复制模型名」16→**30×30**、技能页批量勾选框 16→**26×26**、
    供应商页「官网」链接 40×16→**54×26**（视觉尺寸不变，靠 `::after` / `<label>` 外扩，均达 WCAG 2.2 AA）。
  - 截断：MCP 注册路径/URL 与供应商 base URL 补 `title`（hover 可读全量），审计截断项 3 → 0。
  - 主操作可达：网关页新增吸顶操作条（`启动/停止` + `复制 MCP 配置`），移除会被折叠线切掉
    的浮动按钮；网关页折叠线下控件 1 → **0**。
  - 模型表列降级：`<1024px` 隐藏「模态（入/出）」列（该信息降级为「模型名」列内的单字 badge，
    完整集合放 `title`），并把三个行内输入窄窗收窄 → **最小窗口 900×600 下不再横向溢出**
    （表宽 804 → 674，容器 724）；1180×800 下 7 列与编辑器完整保留。
  - 顶部渐隐遮罩：内容滚过顶部时不再是硬边。踩坑两点（已写进代码注释）：
    ① `sticky` 方案贴不到真正的裁切边（滚动容器 padding 也在可滚动区内，内容被裁切的是
    `main` 边框盒顶边，sticky 恒定低 24px）→ 改「包裹 `main` + 绝对定位覆盖层」，偏移 0；
    ② 必须显式 `pointer-events-none`，否则 24px 透明带吞掉顶部点击（同历史 P0-3 那类遮挡）。
    包裹层不加 `z-index`，吸顶条（z-10）仍盖住遮罩（z-5）；`absolute` 不占布局高度。
- 视觉回归探针加固（`tools/visual-regression/`）：
  - `audit.mjs` 主题改为**页面加载前**写入 `localStorage.theme`（原先加载后才加 `.dark` 类，
    next-themes 已初始化完毕，造成「应用暗色 + 组件库浅色」错配，使 toast 报出假结论）；
  - `audit.mjs` 的 `truncated` 检测**原为死代码**（只初始化、从不 push，字段恒空）→ 补上实现；
  - 新增 `probe-hits.mjs`（有效命中区）、`probe-toast2.mjs`（toast 主题与对比度）、
    `probe-sticky.mjs`（吸顶条是否真的常驻）、`probe-fade.mjs`（顶部渐隐贴边/不吞点击）、
    `probe-models-cols.mjs`（模型表列可见性与表宽）。

## [0.2.3] - 2026-09-17

### Fixed
- **技能工具名可能违反 MCP 规范**（严格客户端会拒绝整份 `tools/list`，连累全部代理工具）：
  - 现象：技能名只校验非空，`代码 评审/甲` 这类名字会被原样拼成工具名 `skill__代码 评审/甲`，
    违反 MCP 名契约 `[A-Za-z0-9_-]{1,64}`。dsh 会自行 sanitize 后映射回原名（不致命），
    但严格校验的客户端可能整份拒绝 —— 本机实测一次广告 72 个工具，其中 67 个是代理工具。
  - 修复：工具名走**确定性编码映射**——原名合法且不占用时保持 `skill__<原名>`（向后兼容），
    否则编码为 `skill__<sanitized>_<hash6>`；真实技能名仍在工具 description 里；
    反解时先查映射表、再回退原样名字（兼容历史会话与手工调用）。
  - 回归：`tests/skill_lifecycle.rs::advertised_tool_names_are_spec_legal` 断言所有工具名合法；
    **临时回退修复前逻辑时该测试必红**（报错与生产症状逐字一致：
    `工具名违反 MCP 契约: "skill__代码 评审/甲"`）。
- **`get_skill_detail` 可绕过 `enabled` 闸门**：投递（`skill__*`）拒绝未启用技能，台账却照返全文，
  语义自相矛盾。现在两者口径一致：未启用即拒绝，并指引到「技能」页启用。
- **台账类工具把失败拉平成成功**（`isError=false` + 错误埋在内层 JSON）：`get_skill_detail` 未找到、
  `get_mcp_server_detail` / `get_tool_schemas` 的未找到/未启用/上游超时，现在一律 `isError=true`，
  与 v0.2.0「失败不得被拉平成成功」的修复哲学统一（只看 `isError` 的客户端不再把失败当成功）。

### Added
- **启动时按需回收磁盘**（bug 清单 14 的永久解法）：删除大行后 SQLite 文件不会自动缩小，
  本机实测出现过「895 MB 库里 894 MB 全是空洞」，只能人工 `VACUUM` 收尾。现在启动时按
  空闲页占比判定并自动回收：默认占比 ≥ 60% 且库 ≥ 32 MB 才触发，仅告警不阻塞启动；
  `JAI_VACUUM_ON_START=0` 可关，`JAI_VACUUM_FREELIST_RATIO` / `JAI_VACUUM_MIN_MB` 可调。
  实测效果：894.6 MB → 0.7 MB。
- `tests/skill_lifecycle.rs`：技能全生命周期端到端回归（6 条，真实 HTTP + 断言 `isError` 真值），
  覆盖工具名合法性、向后兼容、逐字节无损投递、未启用拒绝、32KB UTF-8 边界截断、台账错误语义。

## [0.2.2] - 2026-09-16

### Fixed
- **WebDAV 本地快照自引用递归：单行涨到 595 MB，且每次推送翻一倍**（磁盘/WAL/远端备份链无界增长）：
  - 现象：`jai.db` 938 MB、WAL 629 MB；`dbstat` 显示 `meta` 表占 **596 MB**，其中单行
    `webdav_last_snapshot` = **624,552,032 字节**（内含自己 **19 层**）。远端同源：`/jai-config.json`
    = 42.06 MB，时间戳备份链逐轮翻倍 `0.01→0.02→0.03→0.05→0.08→0.12→0.21→0.38→0.72→1.38→2.69→
    5.32→21.07 MB`，相邻间隔约 30 分钟（= `webdav_auto_push_interval_min`）。
  - 根因：`build_export_json` 用 `SELECT key,value FROM meta` **全量导出 meta**，而
    `webdav_last_snapshot` 就是「上一版导出物」本身；`push_now()` 的顺序是「构建导出 → 存为快照 →
    PUT 远端」→ 每次推送把上一版快照嵌进新快照。**导入侧一直有白名单过滤该 key，导出侧漏了**。
  - 影响：本地磁盘与 WAL 无界增长，每次推送体量翻倍（595 MB → 下次约 1.19 GB），远端备份链同步膨胀；
    删行后 DB 文件不会自动缩小（实测 298 MB 空闲页）。不会跨设备传染（导入白名单挡住）。
  - 修复：① 导出侧 `WHERE key <> ?1`（`sync::snapshot_meta_key()` 单一常量）排除自引用 key；
    ② `snapshot_put` 体积硬上限（默认 4 MB，`JAI_SNAPSHOT_MAX_BYTES` 可覆盖）——超限跳过写入并 WARN，
    **不返回 Err**（快照只是回退手段，不得阻塞配置推送）；③ 启动自愈 `heal_oversized_snapshot`：
    存量 > 2 MB 的快照用当前配置重建（失败则删 key）；④ `try_pull` 体积护栏（默认 32 MB，
    `JAI_REMOTE_CONFIG_MAX_BYTES` 可覆盖），远端被撑大时给出指向根因条目的错误而非读进内存；
    ⑤ 新增 `store::meta_delete`。
  - 回归：`export_size_stable_across_push_cycles` **在修复前实测必红**——8 轮「构建导出 → 存快照」
    体积为 `1206→2628→4334→6604→10002→15656→25822→45012` 字节（逐轮翻倍），修复后恒定 1206 字节；
    另含 `export_excludes_self_snapshot_key`、`snapshot_put_refuses_oversized_text_without_blocking_push`、
    `heal_rebuilds_oversized_snapshot`、`oversized_remote_error_is_actionable` 与集成测试
    `m7_import_webdav::pull_rejects_oversized_remote_config`。
- **存量被撑大的快照自动修复**：升级后首次启动即重建（无需手工清库），日志打印旧体积。

## [0.2.1] - 2026-09-16

### Fixed
- **工具参数护栏把长会话判死锁**（agent 客户端每轮报 `turn error`、会话永久 400）：
  - 现象：dsh 等客户端在会话进行到中后段后，**每一轮**都是
    `OpenAI API error (400): {"code":null,"message":"工具参数累计超过 262144 字节上限（护栏）"}`，
    新开会话才恢复——会话本身没有异常输入，纯属「聊得够久就必坏」。
  - 根因：`codec/ir.rs::validate_guards` 的 `tool_args_bytes` 在**整请求所有消息之间累加**，
    而 agent 客户端**每轮全量重放历史**：历史里 tool_use 参数合计一旦越过 256KB，该会话此后每一轮
    必然超限。同文件 `blocks ≤ 64` 早已因相同理由改为「按单条消息、不跨消息累计」，**args 侧漏改**
    （触发量级并不夸张：几次把整份文件塞进 `write` 参数 + 大命令 + 历史重放即可越过 256KB）。
  - 修复：`validate_guards` 的工具参数累加**移入消息循环内**，改为按单条消息校验（跨消息不再累计）；
    常量更名 `MAX_TOTAL_TOOL_ARGS_BYTES` → `MAX_TOOL_ARGS_BYTES_PER_MESSAGE`（名字即语义）；
    单条消息内多块累计超限仍拦，整请求体大小由上游自身限制兜底。
  - 报错可定位可操作：`单条消息工具参数累计超过 262144 字节上限（护栏）：第 K 条消息 M 字节；
    请缩小单次工具调用参数，或新开会话丢弃历史中的超大参数`（blocks 超限同理带消息序号与块数）。
  - 回归：`codec::ir::tests::guards_tool_args_per_message_not_request`——
    4 条各 ~200KB 的历史消息（合计远超上限）必须放行；单条消息内累计超限仍拦并指明消息序号。
- **MCP 管理页两个 per-server 开关没有任何说明**：`启用` 是裸开关（界面上只有 aria-label，肉眼无字），
  `代理执行` 只有一个 6px 的「代理」小字，看不出各自作用与依赖关系。现在：两个开关都有可见文字标签；
  列表上方一行常驻解释（启用 = 网关是否连接它；代理执行 = 是否让 Agent 调它的工具，且需同时「启用」）；
  悬停标签显示细节（含权限语义与「最小权限/默认关闭」提示）。配套探针
  `tools/visual-regression/mcp-switches.mjs` 把「有标签 / 有解释 / hover 有详情 / 点文字能切换 /
  无横向溢出且右侧按钮不被挤出」固化成断言（1180×800 与 900×600 双尺寸通过）。

## [0.2.0] - 2026-09-16

### Changed
- **MCP 代理转发（`/mcp`）语义收紧**，两处行为变化：
  - **结果原样透传**：`content` 逐块保留（含 `image` / `resource_link`）、`isError` 原样冒泡、
    `structuredContent` 保留，来源降为非标准 `source` 字段。**上游工具级失败不再被当作成功**。
  - **独立等待预算** `JAI_MCP_PROXY_CALL_TIMEOUT_MS`（默认 **55s**，低于常见 MCP 客户端的 60s
    硬中止）：接近/超过客户端预算的长时调用改为网关提前返回工具级错误（含「改用
    `terminal_start` + `terminal_poll`」指引），而不是让客户端报 `-32001` 并连带作废同批调用。
  - 迁移提示：有状态/长时工具（终端会话类）建议把客户端 `toolCallTimeoutMs` 调到网关预算之上。
    详细语义与预算表见 README「MCP 元数据服务 / 代理转发的两条语义」。

### Fixed
- **`/mcp` 代理转发的工具级失败被拉平成成功**（客户端把失败当成功）：
  - 根因：`registry.rs` 的 `tools/call` 成功分支把上游结果包成 `{source, result}` 再 `to_string()`
    塞进单个 text 块，且外层 `"isError": false` **硬编码** —— 上游 MCP 的 `isError: true`
    （如 `Error: Session … is not in the current scope.`）被覆盖；dsh 侧按外层判定
    （`dsh-mcp-client`：`if (result.isError === true) throw …`），于是失败被当成功交给模型。
  - 修复：代理路径**原样透传**上游结果（`content` 逐块保留 + `isError` 冒泡 + `structuredContent`），
    来源标注改为非标准 `source` 字段；静态台账/技能工具仍走「JSON 文本」。顺带救回 `image` /
    `resource_link` 等块（此前被字符串化后客户端的图片投影永不触发）。
- **`/mcp` 代理转发与客户端超时预算不匹配**（dsh 侧 `MCP error -32001: Request timed out`）：
  - 根因：三方预算错位——dsh 客户端 60s 硬中止、网关 120s、Netcatty 长时工具（`terminal.execute`
    `policy.longRunning`）60s 操作超时 + 5s RPC 缓冲 = 65s。网关比客户端更能等 → 实测网关等满
    60.007s 才拿到上游结果、客户端 60.000s 已放弃，**差 7ms 输掉竞速**；agent 只看到无信息量的
    客户端超时，同批并行的其它工具调用被一起作废。
  - 修复：代理转发加独立预算 `JAI_MCP_PROXY_CALL_TIMEOUT_MS`（默认 55s，**低于**常见客户端预算），
    超时返回**工具级错误** + 可执行指引（长命令改走 `terminal_start` + `terminal_poll`）。
- **`request_logs.tool_calls` 恒 0**（该列此前写死，排查工具问题时误导判断）：
  - 修复：`emit_log` / `emit_log_with` 增加 `tool_calls` 参数并落真值——跨族转换按 IR 计数
    （非流式数 `Block::ToolUse`，流式按 `ToolCallStart` 的 id 去重，容忍 Gemini 恒 0 index），
    同族直通非流式按入站线形状数响应体；`LogRowView` 同步暴露该字段。
    直通**流式**不采集（字节直通不解析 SSE 语义），已在代码注释中标明。
  - 单测 2 例（三种入站线形状 + IR 块计数）、集成断言 2 例（非流式 1 次 / 流式并行 2 次）。

### Added
- **dsh 接入生成器**（`scripts/setup_dsh.mjs`）：把 JAI 结构化合并进 dsh 的
  `$DSH_HOME/settings.yaml`（`llm-pi-ai.providers.<route>`，`api: openai-responses`
  / `openai-completions` 可切），`models` 取自 JAI 实时 `/v1/models`（含 `contextWindow`
  与 `input` 模态），网关 Key 写入 `$DSH_HOME/.env`（0600，经 `apiKeyEnv` 引用）；
  支持 `--dry-run` 打 diff（key 脱敏、不写盘）、改动前时间戳备份、幂等，且保留用户既有的
  其他 route、注释与自加字段；落盘前过 dsh 自身运行时 schema 校验。配套离线自测
  `scripts/setup_dsh_test.mjs`（9 项断言）。**未做 dsh 真机 boot 验证**（需短暂退出 dsh）。

### Fixed
- **流式 usage 丢失 → 客户端上下文占比恒 0**（dsh-tui 状态栏 `ctx 0/128k 0.0%`，仅部分供应商复现）：
  - 根因：`codec/openai.rs` 判定 usage 帧时只认「`choices` 为空数组」这一种形状。scnet（超算）的末帧是
    `{"choices":[{"index":0,"delta":{}}],"usage":{...}}`（choices 非空）→ 整帧被丢弃 → 出站
    `response.completed.usage` 恒 0 → dsh/pi-ai 的 `tokens.input` 恒 0；tokenrhythm（基元律动）末帧为
    `"choices":[] + usage`，故一直正常，表现为「换供应商就不统计」
  - 修复：usage 采集与 choices 形状解耦（带非 null `usage` 的帧一律进 IR，同帧带正文也不吞）；
    `convert_streaming_response` 把 Finish 挂起到上游 `[DONE]`/EOF 再合并下发，避免「先发零值收尾帧、
    再补一帧造成重复 completed/message_stop」；落库时零值 Finish 不再覆盖真实 usage
  - 顺带修：`emit_log` 把 `route_mode` 硬编码为 `"passthrough"`，跨族转换请求也被记成直通
    （本次排查即被该字段误导）→ 新增 `RouteMode` 枚举 + `emit_log_with`，转换路径 20 处调用点改标 `converted`
  - 回归：单测 4 例 + 集成用例 `responses_to_openai_stream_usage_split_frames`（按抓包真实帧形状跑完整链路，
    断言 completed 唯一、`input_tokens 259132`、日志 `route_mode=converted` 与 usage 落库）；两处 A/B 反证会失败
- **工具结果内嵌图片的跨族丢图**（agent 客户端截图类工具的真实形态：图片在
  `tool_result` / `function_call_output` 里，而非用户消息）：
  - 入站解码：Anthropic `tool_result.content` 内容块数组中的 `image` 块不再被只取文本字段
    的逻辑丢弃；OpenAI `role=tool` 消息 `content` 为内容数组时不再整条退化成空串；
    Responses `function_call_output.output` 为内容项数组（含 `input_image`）时不再被抹成空串
  - 出站编码：**四族各按自身规范承载**——Anthropic `tool_result.content` 与 Responses
    `function_call_output.output` 按内容块数组原生承载；Gemini 走 v1beta 的
    `functionResponse.parts[].inlineData`（`response` 是 JSON 对象装不下图）；**OpenAI chat
    是唯一装不下的**（规范原文「For tool messages, only type `text` is supported」），
    故降级**提升为紧随其后的一条 user 消息**并在 tool 消息文本里留一行说明
  - 降级可见：新增能力面 `Capabilities::tool_result_images`（anthropic / openai_responses /
    gemini = true，openai_compat = false），装不下时规划层产出 `CapabilityWarn`
    进结构化日志（UI 日志页可见），不再静默
  - 顺带修一处 data URL 误判：Responses 入站 `input_image` 的 `data:` URL 不再被整串塞进
    `url` 字段（会被 Anthropic/Gemini 上游拒），改为拆出真实 `media_type` + 载荷
  - 新增共享工具 `codec/image.rs`（data URL 解析口径统一 + 工具结果图片探测）
  - 回归测试：`crates/gateway-core/tests/multimodal_image.rs` 由 5 例扩至 15 例；
    新增 9 例均经改动前源码验证会失败（非空转）
  - 协议依据来自四家官方 OpenAPI spec / proto / SDK 类型核查（Anthropic tool-use 文档示例、
    OpenAI Chat 规范原文、Responses `FunctionCallOutputItemParam`、Gemini v1beta
    `FunctionResponse.parts`），核查报告留档 `docs/design/tool-result-image-protocol-factcheck.md`；
    **未验证项**：Gemini v1(GA) 是否支持 `parts` 无法确认（故依赖出站固定打 v1beta）、
    `tool_result` 内非 base64 图源未获文档背书

### Notes
- **仍存**：Responses **出站**的消息级图片（`Block::Image` 在 user 消息里）仍按 "v1 不支持
  图片内联转换" 丢弃，见 `codec/responses.rs` —— 同类静默丢失，本次未动（超出本次范围）。
  注意：Responses 官方 schema 的 `input_image` 在 message 里是合法的，此注释疑似过时，
  但改动会影响既有成功请求，需单独评估。

## [0.1.9] - 2026-09-11

### Added
- **图片跨族链路**：OpenAI / Anthropic / Gemini 三族 `image` 内容块互转（data URL ↔ base64 source
  ↔ inlineData），media_type 与载荷不失真；5 例链路测试 `multimodal_image`
- **模型输入/输出模态集合**（迁移 `0010_model_modalities`，取代 0009 单一 vision 布尔）：
  `models` 表新增 `input_modalities` / `output_modalities`（可空 TEXT，规范序
  `text,image,audio,video` 逗号串，`NULL` = 未知）；旧 `supports_multimodal` 列保留可读、
  降为**派生回落源**（输入含 image → true；输入为 NULL 时才回落旧列；清除标注时旧列一并置
  NULL，杜绝「幽灵 true」）
- **上游模态发现**：Gemini `inputModalities` / `outputModalities`、OpenRouter
  `architecture.input_modalities`、openai 系 `supports_vision` / `multimodal` / `vision`
  按可信度递降解析；完全取不到一律 `NULL`（未知），不做模型名启发式臆断
- **出站 `GET /v1/models` 追加 `inputModalities` / `outputModalities`**（只增字段，
  `contextWindow` / `supportsMultimodal` 语义不变，旧客户端不受影响）
- **UI 模型页模态编辑器**：入/出双维多选即时生效 + 「清除标注（未知）」；
  命令 `model_set_modalities` 取代 `model_set_multimodal`；模态随配置导出/导入同步，
  老快照（仅 `supportsMultimodal` 布尔）导入时自动回填为等价模态集合
- **验收**：`tests/modalities.rs` 9 例（enum_/store_/discover_/proxy_/sync_）+ 老库 v9→v10
  真升级验证

## [0.1.8] - 2026-09-05

### Added
- **日志详情与复现命令**（UX-T1）：点击日志行弹出详情（入站协议/路由/供应商/模型/耗时/token/错误），
  一键「复制为 cURL」生成可复现命令（请求体为占位模板，隐私基线不变——仍不落内容）；新增供应商筛选
- **Ctrl+K 命令面板**（UX-T2）：全局唤起——页面跳转 / 快捷操作（复制网关端点、WebDAV 立即推送/拉取）/
  搜索供应商与模型
- **健康自检横幅**（UX-T3）：网关页显示最近一轮健康检查摘要（时间 + 不可用供应商名单，
  全绿仅小徽章不打扰）；新 IPC `health_summary`
- **WebDAV 推送冲突可视 diff**（UX-T5）：推送前差异预警升级为明细弹窗——远端独有
  （将被覆盖丢失）与本地独有（将新增到远端）逐条列出后再确认；新 IPC `webdav_push_diff`

## [0.1.7] - 2026-09-05

### Added
- **能力声明 + 兼容性规划层**（`gateway-core::codec::capability`，借鉴 GodeX bridge）：协议族级
  六面能力表（参数/工具/tool_choice/响应格式/reasoning/流式 usage）+ 四级决策
  （supported/degraded/ignored/rejected），跨族转换先规划后编码，能力面拒绝统一收敛于此
  （错误码 `response_format_not_supported` / `tools_limit_exceeded` / `tool_choice_not_supported`）
- **json_schema 结构化输出降级执行**（决策翻转，见 docs/design/protocol-ir.md §3/§10）：
  openai 系上游原生外传 `response_format`（含 Responses `text.format` 形状还原）；
  anthropic/gemini 上游降级为「提示词指令注入 + 输出后 JSON 校验」（strict 时校验失败
  返回 502 `structured_output_validation_failed`）；连降级都无法表达时才 400
- **reasoning effort 完整闭环**：入站建模进 `SampleParams.reasoning_effort`（chat 的
  `reasoning_effort`、Responses 的 `reasoning.effort`）；出站 Native 档原样透传、
  anthropic 映射 `thinking` 开/关、无能力族忽略并 WARN
- **工具声明上限护栏**：工具数超目标族上限（默认 128）→ 400 `tools_limit_exceeded`
- **Codex 扩展工具折叠与还原**（§10 扩展工具降级矩阵）：shell / apply_patch / custom /
  local_shell 声明与调用折叠为 function 工具（固定名或原名 + function input_schema），
  Codex 客户端可经 JAI 接任意普通模型；回程按工具身份映射还原原始 item
  （`shell_call` / `apply_patch_call` / `custom_tool_call` / `local_shell_call`，
  含流式 item 类型与增量事件名还原）
- **能力告警进结构化日志**：跨族能力面降级 / Lenient 丢弃 / 流式丢帧从控制台
  `eprintln` 升级为 `emit_log`（error_kind `CapabilityWarn` / `SseParseWarn`），
  UI 日志页可见，不再仅落终端
- **协议标准字段静默忽略**：Responses 协议标准字段（`store` / `parallel_tool_calls` /
  `stream_options` / `metadata` / `user` / `include` / `previous_response_id`）转换丢弃
  属预期（如 `store` 请求服务端存储会话，转发网关不代管），不再刷 `CapabilityWarn`——
  告警只保留给真正的降级与陌生字段，避免 dsh 等客户端常规请求淹没日志
- **WebDAV 远端备份管理**（`webdav_backups_list` / `webdav_backup_restore` /
  `webdav_backup_delete`）：同步页新增「远端备份（WebDAV 目录）」区——PROPFIND 列出
  同目录 `jai-config.<时间戳>.json` 备份（时间/大小），可恢复指定版本到本地
  （与拉取同回声抑制，并把自动拉取基线对齐远端当前版本，防止恢复结果立刻被拉回去）、
  可删除（仅时间戳备份名，当前配置与无关文件拒绝；404 幂等）
- **远端路径透明化**：目录字段按 URL 语义百分号编码（空格/中文不再拼出非法地址），
  同步页新增「远端配置文件地址」实时预览 + 一键复制
- **自动同步失败系统通知**：自动推送/拉取失败时发系统通知（错误摘要 140 字符截断）
- **网关出站代理配置（D8）**：设置页新增「网络代理」卡片——HTTP(S)/SOCKS5 代理地址
  （可含 `user:pass@` 认证）+ 绕过列表（每行 host 或 `.suffix`，`*` 全过）+「测试连接」；
  上游模型 / 健康检查 / WebDAV 同步统一经代理出站，**保存后重启网关生效**（与端口约定一致）；
  关闭时代理行为与默认完全一致（零回归）

### Changed
- **推送前差异预警**：手动推送若远端有本机没有的供应商/模型，首次调用返回可读提示并在
  前端弹「仍然推送」确认框——防止多设备场景无意覆盖掉另一台设备刚加的内容；
  `webdav_push` 新增 `force` 参数（true 跳过预警）
- **远端备份保留策略**：`BACKUP_KEEP=10`，备份解析/清理只认 `jai-config.<digits>.json`
  形态（防误删，当前配置永不列入）

### Added
- **WebDAV 从本地快照恢复**：同步页新增「本地推送前快照」卡片与「从快照恢复」按钮
  （`webdav_snapshot_info` / `webdav_snapshot_restore`），推送前快照（`webdav_last_snapshot`）
  终于有回退入口；恢复后如开启自动推送，防抖会将其同步回远端
- **WebDAV 自动拉取 / 双向同步**：同步页新增「自动拉取」开关——与自动推送共用间隔，
  按导出 `exportedAt` 时间戳 last-write-wins（远端非空且比上次成功同步更新才导入；
  空远端不拉取，防远端空配置清空本机）；新增「上次自动拉取」状态显示

### Changed
- **供应商官网字段**：供应商表单新增「官网」（可空），列表卡片一键跳转系统浏览器
  （tauri-plugin-opener）
- **WebDAV 密码回显**：同步页密码框回显已保存密码（明文入库后与网关 Key 同级展示语义）

### Changed
- **密钥迁入 SQLite，钥匙串退场**（migration `0006_secrets_in_db`）：供应商凭据明文存
  `providers.api_key`（与网关 Key / MCP env 同级安全模型，安全性依赖数据目录文件权限）。
  启动时一次性迁移钥匙串存量（`jai/provider/{id}`、`jai/webdav`）→ 入库 → 删除钥匙串项 →
  置 `keyring_migrated` 标记位；迁移改为后台执行且不持 DB 锁，授权弹框不再阻塞启动。
  此后转发、导入导出、启动**零钥匙串访问**；`vault_storage_kind` 命令、文件降级存储
  （vault_fallback.json）与 UI 存储类型显示全部移除
- **WebDAV 同步协议扩展（jai-export/v1）**：导出 providers 携带 `api_key`/`website`，
  顶层新增 `gateway_key`（当前 active），meta 全量导出（WebDAV 密码随行）；
  导入时远端 `api_key` 非空才覆盖本地、新建供应商直接带凭据（导入后立即可路由）、
  `gateway_key` 与本地不同才轮换（吊销旧 key 保留审计）、meta 按白名单应用
  （仅 WebDAV 连接配置与密码）
- 网关 Key 手动重新生成后触发自动推送防抖通知（配置 WebDAV 即自动更新远端）

### Fixed
- **WebDAV 备份被空配置覆盖（数据丢失）**：远端 `jai-config.json` 此前被直接 PUT 覆盖——另一台
  设备（空/新装配置 + 自动推送）会把远端完整备份覆盖成空文件（2026-09 实测事件）。现在：
  1. **推送前留存远端旧版**——每次覆盖前先把远端现有配置复制为同目录 `jai-config.<时间戳>.json`
     （备份失败即中止推送，不盲覆盖），远端备份永不丢失；
  2. **自动推送护栏**——本地为 0 供应商/0 模型而远端有内容时，自动推送跳过并把原因写入
     「上次自动推送」状态（同步页可见），改由用户手动推送（手动推送不受限）
- **流式转换无终止标记上游流的缓冲护栏**（roadmap 稳定性 finding 修复）：转换路径 SSE 行缓冲
  加双重护栏——①单行超限（>1MiB 无换行刷流）立即断开并落日志，防无界内存；
  ②持续收字节但长时间（90s，`JAI_SSE_LINE_HOLD_SECS` 可覆盖）无完整行时断开，
  防「零字节挂起」拖死下游；passthrough 不受影响（逐块转发无缓冲）
- **日志「输入/输出」token 数恒空**：修复流式 usage 抽取器两处缺陷——
  `"usage":null` 后 32 字节窗口误命中下一 SSE 行触发错误收集且线索被丢弃
  （glm-5.3-flash 中转流 17 个 null + 末尾 usage 帧实测恒空）；现在 `null` 显式跳过、
  关键字与值跨 feed 分割时悬挂判定（64 字节上限保护），回归测试覆盖整段/逐字节喂入。
  转换路径流式结束时透传 IR 累计 usage 落日志（此前硬编码 None）

### Added
- **MCP 导入自动识别三种格式**（`mcp_import` 命令，原 `mcp_import_from_json` 更名）：
  1. `{"mcpServers": {...}}` JSON（Claude Code / Claude Desktop，另兼容无包装的裸对象）
  2. Codex CLI 命令行：`codex mcp add <名称> --env K=V -- "命令" [参数...]`
     （`claude mcp add` 同构兼容；无 `--` 时名称后第一个位置参数为命令/URL）
  3. Codex `config.toml` 片段：`[mcp_servers.<名称>]`（支持 `command`/`args`/`env`
     与 `url`/`transport`），TOML→JSON 后复用同一条目解析
- **应用内更新**：设置页「软件更新」卡片——打开时静默检查 + 手动检查更新、
  进度条下载安装（tauri-plugin-updater，minisign 签名校验）、完成后一键重启
  （tauri-plugin-process）；更新源为 GitHub Releases 的 `latest.json`
- `/mcp` 元数据 MCP Server（Streamable HTTP，与网关共用端口与鉴权）：
  把网关登记的 MCP Server / Skill 台账以 MCP 协议暴露给 Agent——五个只读工具
  （`list_mcp_servers` / `get_mcp_server_detail` / `get_tool_schemas` /
  `list_skills` / `get_skill_detail`），不注入对话链路、不代执行工具；
  env 仅回键名不回值。网关页提供 `mcpServers` 接入配置一键复制（复制时自动填入真实密钥）

### Changed
- **MCP / Skill 不再注入对话链路**：删除网关侧 MCP 工具自动合并与自动执行循环、
  Skill 注入 system 逻辑；同族请求恢复纯字节直通。工具执行统一由客户端侧完成，
  消除网关代执行导致的对话链路污染。管理页面保留（连接测试/工具查看/导入导出）

### Fixed
- 修复启动崩溃（v b8503f1 引入）：Tauri `setup` 闭包不在 tokio runtime 上下文，
  `spawn_autopush` / `spawn_health_check` 裸 `tokio::spawn` 启动即 panic；
  改用 `tauri::async_runtime::spawn`

### Added
- UI 2.0 界面升级（阶段 0–6，spec 见 `docs/superpowers/specs/2026-08-31-ui-framework-upgrade-design.md`）
  - 基建：Tailwind 3→4、shadcn/ui 基座、蓝紫明暗双主题（next-themes）、sonner 通知、
    可折叠侧边栏导航、App.tsx 拆分 pages/ + components/
  - 全部 9 页迁移语义化组件（Card/Button/Dialog/Switch/Select/Table/Tooltip），
    移除过渡期 legacy 样式；列表加载态 Skeleton
  - 供应商/MCP/技能表单 Dialog 化：react-hook-form + zod，校验错误就地展示；
    window.prompt/confirm 全部替换为 Dialog/ConfirmDialog
  - 统计页 recharts 堆叠柱状图（输入/输出分色 + 单日明细 Tooltip）
  - 阶段 6 平台视觉：自绘标题栏（Windows/Linux 自绘窗口三键，macOS overlay 保留红绿灯）、
    窗口毛玻璃特效（mica/acrylic/vibrancy，不支持平台实色回退）
- 全新品牌 logo（J + AI 星火，蓝紫渐变）与应用图标全套（ico/icns/PNG/Square）、UI favicon
- M6: OpenAI Responses API 入站（`POST /v1/responses`，Codex 原生线）
  - Responses ↔ IR 编解码、SSE 事件流、错误形状
- M7: 配置导入 + WebDAV 同步
  - `jai-export/v1` 导入、按名称+Base URL 去重 uplift、缺失密钥报告
  - WebDAV 手动推/拉、推送前快照、last-write-wins；UI「同步」页
- M8: 收尾加固
  - 全矩阵回归脚本 `scripts/regression.sh`
  - 500 随机 body 解码器 fuzz 简表
- M9: 发布工程
  - CHANGELOG、发布检查单、tag 触发 CI（签名/公证需真实 secrets）
- MCP Server 管理：`mcp_servers` 表 + CRUD IPC + UI「MCP」页；支持 tools/list、tools/call 客户端调用（stdio/http/sse）
- MCP 工具自动合并 + 自动执行循环：网关把启用 MCP 工具注入请求工具定义；上游发起 MCP 工具调用时自动执行并回填结果继续生成
- 高级路由：模型别名/映射（upstream_model_id）、同优先级权重负载均衡、基于最近成功/失败的健康感知排序
- 用量统计：新增「统计」页，展示近 7/30/90 天请求数与 Token 用量柱状图
- 旧版 `POST /v1/completions`：支持 openai_compat 渠道字节级直通
- MCP 工具列表缓存：自动合并路径增加 30s TTL 缓存，避免每个请求都去 MCP Server 拉取工具
- 发布门禁脚本 `scripts/release_check.sh`：一键检查工作区/版本/CHANGELOG/tag 并跑全量回归
- dsh 真机联调：修复 Responses+MCP 合成 SSE 与真实 API 形状不一致的问题（event 行、output_text.done、content_part.done、[DONE]）
- Responses 流式转换也补齐真实 SSE 事件行与 done 事件，保证 dsh/zcode 经 JAI 与直连体验一致
- 新增 `docs/test-report-dsh.md` dsh 真机测试报告
- README 明确：优先支持国产 Agent（dsh / zcode）
- MCP 配置托管：新增「复制客户端配置」，导出标准 `mcpServers` JSON，供 Claude Code / Continue 等 Agent 加载
- Skill 导出：新增「复制技能包」，导出启用技能 Markdown 文本，供 Agent 加载/查看
- zcode 接入指南：`docs/zcode接入.md`，按本机 zcode 配置确认 Anthropic 协议线
- 技能（skill）管理：`skills` 表 + CRUD IPC + UI「技能」页；跨族转换请求自动注入启用技能到 system

### Changed
- README/roadmap 同步至 M9 + MCP/Skill 基础管理
- README 更新：国产 Agent 优先支持说明、新 logo

### Fixed
- MCP stdio 客户端把服务端通知行误当响应帧，导致「列出工具/工具调用」报
  「MCP 响应缺少 result」；现跳过无 id 的通知帧与非 JSON 噪音行
  （dsh 真机回归：server-everything 13 工具列出、echo 工具循环端到端通过）
- 流式转换首字节含完整 SSE 流时未消费行缓冲的问题
- Anthropic SSE 渲染缺失 `content_block_stop`、交错 tool_calls 顺序问题

### Added
- **统一 MCP 代理（M1）**：`/mcp` registry 动态聚合可代理 Server 的全部工具为
  `<server>__<tool>` 命名（按首个双下划线分割，server 名可含下划线），dsh 只注册
  `jai-registry` 一个入口即可发现并调用所有已配置 MCP 工具；网关显式转发
  `tools/call` 到真实 Server（复用 mcp.rs 客户端），`description` 标注
  `[proxy: <server>]`，30s TTL 缓存动态工具列表，单个 Server 拉取失败跳过不阻塞
- **Skill 投递（M2）**：registry 动态生成 `skill__<name>` 工具，agent 按名调用即投递
  Skill 全文（32KB 截断并注明），保持「目录 + 按名加载」拉取模式，不注入 system、
  不代跑工具循环
- **MCP 代理权限边界**：`mcp_servers.proxy_allowed` 开关（默认关，migration 0007），
  未开启的 Server 工具不可见、不可调用；UI「MCP」页每行新增「代理」Switch

### Changed
- **MCP stdio 进程池**：`run_stdio_jsonrpc` 重构为「懒初始化 + 进程复用」——
  每个 `(cmd,args,env)` 内容指纹维护一个可复用进程（env 值只哈希不落池防密钥泄漏），
  首次调用 spawn+initialize、后续 tools/list、tools/call 复用（JSON-RPC id 递增）；
  空闲超时回收（默认 30s，另挂 30s 后台清扫，Server 停用后不留孤儿进程）、
  进程崩溃/无响应自动重建、每次调用超时保护（默认 120s，旧实现无限挂起）；
  同连接请求锁串行化，不同 Server 独立连接。冷启动从每次 ~198ms 降到仅首次/回收后
- **代理转发审计**：独立表 `proxy_call_logs`（migration 0008），每次代理调用记录
  `{ts, server, tool, kind, status, duration_ms, error}`，失败也记、异步落库，
  与 `request_logs` 的 `route_mode` CHECK / usage 聚合完全隔离

### Fixed
- `/v1/models` 暴露 `contextWindow`、usage 输出 cache 细分（prompt_tokens_details
  cached_tokens）、解析 `choices:[]` 的 usage 末帧，修复 ctx 占用指标
- **WebDAV「测试连接」误判**：原用 OPTIONS 探测，DUFS 等服务器匿名放行
  （实测免认证 200），凭据无效也误报「连接成功」；改用带凭据的
  `PROPFIND Depth:0` 真实校验（sync::probe），401/403 → 「认证失败：用户名或
  密码不正确」，404 → 「路径不存在」，拉取/推送错误同步分类提示
  （401/403 认证失败、拉取 404 远端尚无配置文件、推送 404 目录不存在）