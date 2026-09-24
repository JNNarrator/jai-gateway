# Changelog

All notable changes to this project will be documented in this file.

## [Unreleased]

## [0.4.2] - 2026-09-23

### Changed

- **按钮反馈的落点全局整改**（用户反馈「按钮的 tips……有些反馈都在最顶部，窗口上下有滚动条。
  还得滚到上面或者下面去看提示」）。先量后改，结论是**不是 toast 的错**：`[data-sonner-toaster]`
  实测 `position: fixed`、单条 rect `723→776`（视口 800）、无 `transform` 祖先 —— toast 永远在视口里。
  真问题是 5 个页面把页级 `msg/err` 直接渲染在 `PageHeader` 之后，而触发它的按钮常在列表下方；
  右下角的 toast 又在另一个对角，视线来回跳。（`ProvidersPage` 早就把反馈放在产生它的卡片里，
  是对的，只是没推广开。）

  规则收敛成两条，按**触发按钮的层级**决定落点：

  - **行级按钮**（列表某一行的「列出工具」等）→ 反馈落在**该行内**；
  - **页级 / 卡片级按钮** → 反馈**吸顶**（`sticky top-0`），跟着滚动条走；
  - 错误（`role="alert"`）一律**不自动消失**、可手动关；成功 / 确认类 2.4s 自动收。

  实测（1180×800，整改前后各测一次；MCP 夹具扩到 10 个 server 让列表真的滚动）：

  | 场景 | 整改前 | 整改后 |
  |---|---|---|
  | MCP 页点**最后一行**的「列出工具」 | 点击时已滚 404px，反馈在**页面顶部（非吸顶）**→ **需滚回 272px** | 落在**该行内**，不用滚 |
  | 同步页点「推送」（WebDAV 操作条） | 点击时已滚 458px，反馈在**页面顶部** → **需滚回 366px** | 反馈与操作条合并**吸顶**，不用滚 |

  改动：
  - 新增 `ui/src/components/common/PageFeedback.tsx`：`FeedbackLine`（行内与吸顶条共用，带关闭按钮）、
    `StickyFeedback`（只给页面上没有别的吸顶条的页面）、`useFeedback()`（页级 key `__page__`，
    行级 key = 行 id；错误无 TTL，成功 2.4s）。**每个页面只保留一个 `sticky top-0` 区域** ——
    两个 `top-0` 的 sticky 会叠、DOM 靠后的压掉前面那个。为此同步页的 WebDAV 操作条从卡片内
    **上移**到页面顶部与反馈条合并（实测：滚到最底 915px 时操作条仍在视口内，且页面上
    `position: sticky` 的元素**恰好 1 个**），并把 **WebDAV 卡移到最前** —— 操作条作用的就是
    这张卡的字段，操作条上移后它必须紧邻被作用的对象，否则首屏看到的是「保存配置」孤零零挂在
    一张无关的卡上面。
  - `ui/src/lib/toast.ts`：错误 toast 改 `duration: Infinity` + `closeButton: true` ——
    上游报错原文往往是一大段 JSON/HTML，2.4s 看不完，等用户去查问题时它已经没了。
    `ui/src/index.css` 单独把 `[data-close-button]` 恢复成 `pointer-events: auto`
    （`Toaster` 整体是 `none`，用来防「toast 吞掉底下的点击」）。
  - 5 个页面（网关 / 同步 / MCP / 技能 / 供应商）的 `msg/err` 全部改为就近或吸顶。
  - 新增探针 `tools/visual-regression/probe-feedback.mjs` + `gate.mjs` 判据 5 条，且 5 处
    **分别改坏实测必红**（见 `docs/bug和优化清单.md` §6.5）。

## [0.4.1] - 2026-09-23

### Changed

- **网关页（首页）排版整改**（用户反馈「按钮样式排版混乱不堪都看不懂啥意思了」）。先按真实
  窗口量了一遍，问题不是「感觉乱」而是可量化的：
  - 内容盒恒 **672px**（`max-w-2xl`），而 1180 窗口下内容区有 **1004px** → 白扔 1/3 宽度；
  - 每把密钥行 **4 个按钮必然换行成两行**（实测行高 52/58，按钮掉到第二行、左边界与元数据
    对不齐）；
  - 同屏 **3 档按钮高度**（28/32/36）混用；
  - 「客户端接入」一张卡里 **25 个按钮**（密钥行 4×3 + 新建组 3 + 复制图标 5 + 输入框）；
  - **5 个实心红按钮**语义完全不同：停止（可恢复）/ 吊销×3（局部不可逆）/ 轮换全部密钥
    （把所有客户端一起踢下线）—— 全同一种红等于没有分级；
  - 文案缺宾语：「规则」是谁的规则、「复制」复制什么、「复制最新一把」指哪一把。

  改动：
  - **拆成两张卡**：「接入信息」（Base URL / API Key / 完整请求地址 —— 客户端要填什么）与
    「密钥管理」（列表 / 新建 / 轮换 —— 多把密钥怎么管）。此前两者挤在同一张「客户端接入」
    卡里，正是分不清「拿去用」与「管起来」的根因。
  - 页面宽度 `max-w-2xl` → `max-w-4xl`（与 MCP / 模型页统一），1180 下内容宽 672 → **896**。
  - **密钥行 4 个按钮 → 1 个状态 chip + 2 个按钮**：前缀本身可点（就地显示/隐藏全文，替掉
    「显示全文」按钮）；「规则」按钮改成**状态 chip**（未配「不限」/ 已配「已限制」，点开配
    规则）—— 既回答「这把密钥能用到什么」又比第 4 个按钮信息量大；「复制」保留；「吊销」
    收进「⋯」菜单（低频 + 危险动作不该各占一个行内按钮）。行高不再换行。
  - 「最后使用」改**相对时间**（「42 分钟前用过」），创建时间移进 hover 提示（低频信息让位）。
  - **危险动作统一为 outline + 红字**（`DANGER_OUTLINE`），**实心红只保留在二次确认框里**；
    「轮换全部密钥」移到底部分隔线后的**危险区**并配一行后果说明，不再和「新建密钥」并排。
  - 删掉「复制最新一把」（语义模糊）：接入信息区的 API Key 字段直接显示**最新那一把**并带
    复制/显示全文，与 Base URL 同构。复制动作一律以最新一把为准，不再复用「用户上次点开过
    哪一把」——那会让复制到的密钥与界面上标着的那把不一致。
  - 吸顶条「复制完整地址」→「复制接入信息」（与「复制 MCP 配置」同一命名口径）。
  - `ConfirmDialog` 的危险按钮改走设计系统 `variant="destructive"`，不再手写一份色值 ——
    改一处两边都变，且带上 `data-variant` 之后「实心红只出现在二次确认里」这条**可被门禁断言**。

  已知取舍：拆卡后页面变高（1671 → 1891px）。这是「分开两件事」的代价，主操作都在吸顶条上
  随时可达。

## [0.4.0] - 2026-09-23

### Added

- **网关密钥白/黑名单**（D9-T6b）：多密钥（D9-T6a）之后「一把密钥发给一个客户端」是
  常态，但每把密钥能碰到的**渠道 / 模型**完全一样 —— 发给 CI 流水线的那把同样能调最贵的
  模型。现在每把密钥可以自带规则，在「网关」页对应密钥行的「规则」里配：
  - 语义（简单可解释）：`拒绝`命中 → 该渠道 / 模型不可用（**优先于允许**）；一旦有
    `允许`项 → 未列出的一律不可用；全部「默认」 → 不限制（**与改造前完全一致**）。
    一个候选必须渠道轴与模型轴都通过才可用。
  - 迁移 `0013_key_rules.sql`：`key_provider_rules` / `key_model_rules`（主键都是
    `(key_id, 条目)`）。渠道规则按 `provider_id`（外键 + `ON DELETE CASCADE`，删渠道
    时规则行一起走）；模型规则按 `model_name` —— 同名模型常挂在多个渠道上，规则要能
    一次覆盖全部渠道。同一项**不可能既允许又拒绝**：写库前把冲突项从 allow 侧摘掉
    （否则会撞 UNIQUE 约束，「deny 优先」就变成了「保存失败」）。
  - 过滤点：`dispatch` 拿到候选后按规则 `retain`，**候选全被过滤 → 403
    `model_not_allowed`**（不是 404 `model_not_found` —— 后者会让用户以为「模型没了」，
    而实际上换把密钥就能用；错误文案直接点名是密钥规则导致）。位置刻意排在两个 404
    之后：「东西本来就没有」与「你有，但不给你用」是两回事。
  - `/v1/models` 同样按密钥过滤：否则客户端会看到一堆自己调不通的模型，选到了必然 403。
  - 鉴权结果注入请求扩展（此前 `security_mw` 把它直接丢弃 —— 这正是本功能的前置缺失）。
  - 规则缓存 5s TTL，且**与桌面端共享同一份**：保存后立刻失效，不必等 TTL 才生效；
    查库失败按「不限制」处理（路由本身也要查库，把规则查询失败升级成全拒只会让所有
    客户端一起 403）。
  - 前端：密钥行上新增「规则」按钮 + 三态（允许 / 拒绝 / 默认）编辑弹窗，带
    「当前密钥可访问的模型」**实时预览**（未保存的草稿也跟着变）；无可访问模型时当场
    提示「保存后调任何模型都会返回 403」（这是本功能最危险的误操作）。列表上给配过
    规则的密钥加「已限制」徽标。
  - 新 IPC：`gateway_key_rules_get` / `gateway_key_rules_set` / `gateway_key_rules_options`。
  - **已知局限**：规则不随 WebDAV 同步（导出仍只带最新一把密钥），多台机器需各自配一次。
- **网关多密钥**（D9-T6a）：此前「密钥」实质只有一把 —— `gw_key_active` 只取最新一条，
  鉴权也只比那一把。于是「给 A 客户端一把、给 B 客户端一把，A 泄露了只吊销 A」做不到：
  只能轮换，而那会把所有人一起踢掉。现在每把密钥独立存在、可单独吊销：
  - store：新增 `gw_keys_active`（全部未吊销）/ `gw_key_create`（**不吊销旧的**）/
    `gw_key_revoke`（按 id 软删，返回是否真的改动了行；重复吊销不改时间戳）。
    `gw_key_active` 保留「最新一把」语义（导出 / 导入比对 / 首次自举在用）。
    排序补 `rowid DESC` —— `created_at` 只到毫秒，同毫秒创建时顺序不确定会让列表「跳」。
    label 在 store 层归一（去空白，空白串视同未填）。
  - 鉴权：`security::authenticate` 从「取唯一活跃 key」改为「取全部活跃 key → 逐个
    `ct_eq`」。**不做短路优化**（不「先比 prefix 再比全文」——prefix 是 14 字符明文，
    用它筛选会让「哪些 prefix 存在」可被时序区分）；密钥数量是个位数，全量比对成本可忽略。
  - 新 IPC：`gateway_key_list`（**不含全文**）/ `gateway_key_create`（返回带全文，
    这是唯一一次能拿到全文的时机）/ `gateway_key_revoke(id)`；
    `gateway_key_reveal` 增加可选 `id`（省略时取最新一把）。
    `GatewayKeyInfo` 增加 `id` / `revokedAt`。
  - WebDAV 导入语义收窄：本地只有 ≤1 把时保持原轮换语义；本地**已有多把**时**只补不删**
    —— 本机配多把是刻意的，远端快照没资格清掉它们。已知局限：导出仍只带最新一把密钥。
  - 前端「网关」页：单卡片改为密钥列表（前缀 / 备注 / 创建时间 / 最后使用 +
    显示全文 / 复制 / 吊销），吊销走二次确认；新增「新建密钥」（可填备注）与
    「复制最新一把」；原「轮换密钥」改为「轮换全部密钥」并在确认框里写清
    「这是把所有密钥一起换掉，只想换某个客户端请用新建 + 单独吊销」。
- **渠道草稿端点探测**（D9-T1）：在「新建/编辑供应商」弹窗里，除现有「测试连接」（只打
  `/models`）之外新增「端点探测」——对每个候选端点发一次**真实的最小推理请求**
  （`max_tokens = 1` + `stream: false`），逐端点给出「通过 / 失败 + 原因 + 延迟」。
  为什么需要它：能列模型 ≠ 能推理。中转站常见三种假绿 —— `/models` 有数据但
  `/chat/completions` 404；网关前置的 HTML 拦截页返回 200；key 只能读模型不能推理。
  用户此前只有保存完、在客户端里真发一次请求才会发现，那时排查成本已经很高。
  新增 `crates/gateway-core/src/probe.rs`：
  - 端点与 payload 按 family 派生（`openai_compat` → `/chat/completions`、
    `openai_responses` → `/responses`、`anthropic` → `/v1/messages` +
    `anthropic-version`、`gemini` → `/v1beta/models/{model}:generateContent`），
    鉴权头照抄转发链路的口径（Gemini 走 `x-goog-api-key`，避免 key 进 URL/日志）。
  - **2xx 不等于通过**：必须能把响应解析成该协议的预期最小结构（`choices` /
    `content` / `output|status` / `candidates`），否则判 `protocol` 失败 —— 这条是
    防「假绿」的关键（拦截页也是 200）。200 + `error` 对象的中转站也会被识别。
  - 失败分类：`authentication` / `model` / `endpoint_unsupported` / `request` /
    `rate_limit` / `overloaded` / `timeout` / `network` / `protocol` / `url_blocked` /
    `no_model`。404 按正文分流（「model not found」→ `model`，nginx 的
    `<html>404 Not Found</html>` → `endpoint_unsupported`，避免把用户引错方向）。
  - 结论 message **脱敏**（抹掉明文 key 与 `sk-*` / `AIza*` / `Bearer *` 形态）并
    按字符截断到 300；`cost_possible` 明示「这次探测真的调用了模型」。
  - 附加信息性探测：`openai_compat` 渠道额外探一次 `/responses`，回答「这个中转站
    能不能接 Codex」。该行带 `informational: true`，**不进「通过」门禁**，UI 灰显。
  - 新增 `crates/gateway-core/src/netguard.rs`（SSRF 校验）：**发请求之前**校验
    `base_url`。拦截非 http(s)、URL 内嵌用户名/密码、IPv4 私网与保留段
    （`0/8`、`10/8`、`100.64/10`、`127/8`、`169.254/16`、`172.16/12`、`192.0.2/24`、
    `192.168/16`、`198.18/15`、`224/4`、`240/4`）、IPv6（`::`、`::1`、`fc00::/7`、
    `fe80::/10`、`ff00::/8`）、**IPv4-mapped IPv6**（`::ffff:127.0.0.1` —— 最常见的
    绕过手法），以及 `localhost` / `*.localhost` / `*.local` / `*.internal` 主机名。
    `allow_loopback = true` 只放行 loopback（Ollama / LM Studio / vLLM），
    **link-local 与云元数据端点（`169.254.169.254`）任何情况下都不放行**。
    判定用 `url.host()` 而非 `host_str()`，于是十进制（`http://2130706433/`）、
    十六进制（`http://0x7f.0.0.1/`）、八进制、省略段（`http://127.1/`）这些
    **字面量混淆写法**也被归一化后照样拦住。已知局限：不做 DNS 解析后的复查
    （「域名解析到 127.0.0.1」不在拦截范围内，理由见模块头）。
  - 接线：`provider_create` / `provider_update` 保存时做一次结构性校验
    （**允许 loopback** —— 保存动作本身不发请求，且本机部署是正当用法；私网与云元数据
    一律拒绝）；刻意**不做**热路径每请求校验（否则「已存渠道突然被拦」会变成可用性事故）。
  - 可选门禁（meta KV `require_probe_pass`，**默认 `false` = 只展示不拦截**）：
    开启后保存渠道必须带上一次「刚刚探测通过」的指纹。receipt 存在**进程内**、
    TTL 30 分钟、重启即失效（刻意不落库：半年前测过的渠道不该被信任）；
    判据是「至少一个非信息性端点真的通过 + 无失败」，全是 Skipped 不算。
    `provider_update` 只在动了 `base_url` 或密钥时才校验（否则改个显示名也会被挡住）。
  - 新增 IPC `provider_probe_draft`（复用 `core.http`，自动继承出站代理与
    10s connect timeout）。编辑态表单里没重输 key 时传 `providerId`，后端用库里存的
    key 探测 —— 否则会以未鉴权身份打上游，得到假失败。
  前端：`ProviderDialog` 新增「端点探测」按钮（在「测试连接」右侧）+ 探测区块
  （探测用模型输入 + `datalist` 候选 + 「允许访问本机地址」开关 + 结果面板）；
  新增 `components/common/ProbePanel.tsx` 展示逐端点结论行，失败行 hover 显示完整
  message，信息性行灰显并标注「（信息性）」。探测是弹窗内临时状态，
  **不改表单、不影响 `useDirtyGuard`**。顺手修掉 `ui/src/types.ts` 的 `Family`
  类型漏了 `openai_responses`（后端与 providers 表 CHECK 都是四个值）。
- **迁移前自动备份 DB**（D9-T5）：此前 `Db::open` 用裸 `?`，迁移失败 → `setup` 返回 Err
  → `main.rs` 的 `.expect("error while running jai")` panic 退出。用户只看到一个闪退的
  图标，**既没有提示、也没有回滚点**。
  现在 `store::migrate` 在应用**任何**迁移之前把当前库快照一份到
  `<数据目录>/backups/jai.db.<unix_ms>.bak`，滚动保留最近 `MIGRATION_BACKUP_KEEP = 3` 份。
  三条同时满足才备份（避免产生垃圾文件）：① 文件库（`:memory:` 跳过）；② `user_version > 0`
  （全新库没有数据可丢）；③ `user_version < MIGRATIONS.len()`（没有待应用迁移就不备份，
  否则每次启动都会备份一次）。
  用 **`VACUUM INTO`** 而不是 `fs::copy` —— WAL 模式下直接拷文件可能漏掉尚未 checkpoint
  的数据；`VACUUM INTO` 产出的是一致快照，且不要求停机。
  备份**失败不阻止迁移**（决策 D4）：磁盘满 / 权限这类原因导致应用打不开，比「没有备份」
  更糟；迁移本身仍有逐条事务保护（版本号与 schema 同事务提交）。
  桌面壳侧：迁移失败不再 panic，改为发系统通知并带上「迁移前备份在 <路径>」
  （新增 `store::latest_migration_backup`，因为此时 `Db::open` 已返回 Err，拿不到
  `migrate` 的返回值）。顺带把 `notify_health` 更名为 `notify_user`（它已不只服务健康检查）。
  `StoreError` 增加 `Io` 变体。
  注：原方案打算复用 `sync::backup_evict_candidates`，但那个纯函数硬编码识别
  `jai-config.<ts>.json`（远端配置备份的命名），与本地 DB 快照的
  `jai.db.<ts>.bak` 形态不同，因此另写了一个同样只认时间戳的本地清理。
  测试：新增 `crates/gateway-core/tests/m13_migration_backup.rs`（9 用例，全绿）——
  内存库/全新库/已最新 → 不备份；落后一版 → 备份且**内容是迁移前状态**
  （断言备份的 `user_version` = 11）；清理保留最新 3 份；备份目录不可写时迁移照常完成；
  无关文件不被误认成备份。
- **单实例保护**（D9-T4）：此前启动路径**没有任何单实例机制**，双击两次图标（或从
  Dock / 开始菜单再点一次）会起第二个进程。网关端口被占会自动顺延，于是两个网关各监听
  一个端口 —— 用户看到两个窗口、两个网关，不知道客户端该连哪个；两个实例还会同时写
  同一个 SQLite 数据目录。
  接入官方 `tauri-plugin-single-instance = "2"`（v2.4.5）：第二个实例**不启动**，改为把
  已有窗口显示并置前（符合桌面用户预期）。插件放在插件链**首位** —— 它的单实例判定发生在
  Tauri runtime 初始化阶段、早于 `setup()`，所以第二个实例不会执行 `Db::open`，不碰数据目录。
  顺带把托盘菜单「显示主窗口」里内联的 `show()` + `set_focus()` 抽成
  `fn show_main_window(&AppHandle)`，与单实例回调共用（不复制两份）。
  该插件无 JS API（无 `permissions/` 目录），因此不需要在 `capabilities/default.json` 登记。
  ⚠️ **真机验收待做**：需要在 macOS / Windows 上实际双击两次验证（见
  `docs/superpowers/plans/2026-09-22-channel-draft-probe-and-gap-fixes.md` §T4.4）。

- **上游 `Retry-After` 现在真正用于网关内部退避**（D9-T2）：此前该头只被**回传给客户端**，
  网关自己切换候选时不等 —— 上游 429/503 明确说了「X 秒后再来」，我们却立刻去撞下一个
  候选，把限流窗口二次打满（实测某上游 502 占全量 9.78%）。
  新增 `router::{parse_retry_after, backoff_delay_ms, kind_waits_for_backoff}`：
  `delta-seconds`（含小数）+ `HTTP-date` 两种形式都解析（用 `httpdate`，不手写日期解析），
  单次等待封顶 `RETRY_AFTER_CAP_MS = 5s`、叠加 ±20% 抖动、一次请求累计封顶
  `MAX_TOTAL_BACKOFF_MS = 10s`。`Attempt::Failed` 增加 `retry_after_ms` 字段，
  直通与转换两条路径都填。
  三个刻意的边界：
  1. **只在上游明确给出 `Retry-After` 时等待** —— 上游没说就不等。500/503 往往是瞬时
     故障，而下一个候选是**另一个上游**，等它对这个上游毫无意义，只会让客户端白等。
     指数退避（`backoff_delay_ms` 的 `None` 分支）保留给 T3 的分组重试，那时同组内会
     回到同一个上游，等待才有意义。
  2. **只对限速/过载类失败等待**（`RateLimit` / `Overloaded`）—— 401/403 与 400 等多久
     都不会变好。
  3. **只在后面确实还有候选可试时才等**，否则只是让客户端多等一次超时。
  `retry-after-ms`（非标准但精确）优先于标准 `Retry-After`。

### Changed

- **重试模型升级：候选分组 + 双档预算 + 跨组规则**（D9-T3）。此前 `dispatch` 是
  「单轮把候选遍历一遍即止」：没有预算概念，也没有「哪些失败允许跨协议族重试」的语义。
  三处具体问题：① 候选多时会把上游全部打一遍（限流窗口被二次打满）；
  ② 401/403 这类**渠道凭据错**会继续往别的协议族上撞 —— 一旦那个协议恰好成功，
  权限/语义错误就被静默掩盖成「能用」；③ `router::first_byte_verdict` 是**死代码**
  （只有自己的单测），它想表达的「响应头已到、首字节未到时的失败可以 failover」
  从未接线 —— 每个上游都会碰到的偶发断流被直接变成客户端可见的 502。
  新增（`router`，纯函数可单测）：`FailureClass` 六类（`CallerTerminal` /
  `ChannelAuthTerminal` / `EndpointUnsupported` / `Retryable` / `UpstreamProtocolError` /
  `CommittedStreamError`）、`Failure{class, kind}`、`classify_failure`、
  `GroupTier` + `group_candidates`（把 `proxy.rs` 里隐式的 `family` 排序显式化）、
  `RetryBudget` + `retry_budget_from_meta`、`FlowStep` + `AttemptFlow` 状态机。
  `dispatch` 改为「原生组 → 转换组」两组遍历，组内顺序**沿用改造前的排序结果**
  （所以分组本身不改变现状行为），失败后由 `AttemptFlow::next(class, group_exhausted)`
  决定「组内换 / 跨组 / 停」：
  - 预算默认「组内 3 / 全局 6」，可由 meta `retry_max_per_group` / `retry_max_total`
    覆盖；设成 `(1, 1)` 等价于**关掉重试**（= 改造前行为）。
  - 跨组规则：401/403 **只在本组内换候选，绝不跨组**；429/5xx/404/408 与
    405/501（端点不支持）允许降到转换组。`classify_status` 相应把 405/501 从
    「其它 4xx → Stop」里摘出来判为 `Failover{ProviderOther}`。
  - **非幂等请求不放宽**：Responses 入站带 `store:true` / `background:true`
    （会产生服务端持久化副作用）时预算强制压到 `(1, 1)` —— 重发等于提交两次。
  - `group_exhausted` 必须参与判定：否则「同组只有 1 个候选」时 `per_group_used`
    永远小于上限，401 会顺着候选列表跨进转换组，绕过上面那条硬约束
    （有专门的单测 `attempt_flow_treats_group_exhausted_as_budget_exhausted` 守着）。
  - 首字节失败落地：`streaming_response` / `convert_streaming_response` 的三个首字节
    分支（流错误 / 空流 / 首字节超时）从 `Attempt::Delivered(错误响应)` 改为
    `Attempt::Failed` —— 边界严格是「`tx.send(first_chunk)` 之前」，之后一律按
    现状断流 + 落日志。同时把预构造的错误响应放进 `Attempt::Failed::fallback`：
    真的没有候选可试时**原样回它**，否则 `upstream_stream_error` /
    `upstream_empty_stream` / `upstream_first_byte_timeout` 这些专属诊断码会退化成
    笼统的 `all_providers_failed`，排障信息反而变少。死代码 `first_byte_verdict` 删除。
  **兼容约束**：落库 `request_logs.error_kind` 的字符串**逐字不变**（仍由
  `classify_status` 的 kind 携带，`Failure::as_kind()` 原样透出），有单测
  `classify_failure_kind_matches_classify_status_verbatim` 对照守着 —— 否则会静默
  改掉前端日志页与统计的语义。退避判定从 `kind_waits_for_backoff` 换成
  `FailureClass::waits_for_backoff`（只有 `Retryable` 值得等，语义不变）。
  已知遗留（**不是**本次引入）：转换路径的三个首字节分支本来就没有 `emit_log`，
  现在仍然没有 —— 跨组成功时那条候选的失败不会留下日志行。

### Fixed

- **转换路径不再丢退避头**（D9-T2）：`try_converted_candidate` 此前只取错误 body、不收集
  错误响应头，跨族失败时 `Retry-After` 直接丢失。现收集 `FORWARD_ERROR_HEADERS` 用于解析
  退避时长；**但刻意不把它放进 `UpstreamError`** —— 跨族全失败仍按入站方言合成 502
  （由 `m4_conversion.rs::converted_upstream_429_renders_oai_error` 守着：上游是 Anthropic
  形状的错误体，直接回传给 OpenAI 入站客户端会违反协议），只是把 `Retry-After` 头附在
  502 上，让客户端仍有退避依据。

### Tests

- `tools/visual-regression/probe-keys.mjs` 重写并补**排版硬指标**（都是可量化的回归闸门）：
  密钥行不换行（判据用「直接子元素的行中心是否一致」—— 行高会被 h-8 按钮 + py-2 抬到 50px，
  与旧版换行时的 52/58 几乎同高，区分不出来；也不能看 `top`，`items-center` 下 16px 文本与
  32px 按钮本来就 top 不同）、行内按钮 ≤2、页面宽度吃满窗口、按钮高度档位 ≤2、页面主体
  **无实心红按钮**、二次确认里**必须**有实心红、「⋯」菜单含「显示全文 / 吊销」。五条判据
  逐条实测「改坏就会红」（改回 `max-w-2xl`、把吊销加回行内实心红、删掉菜单里的吊销项、
  让 chip 只留图标不写状态）。
- 探针健壮性：菜单项缺失时**不再硬点**（硬点会超时崩溃 → 退出码非 0 → 门禁只能报泛化的
  「探针可运行」，真正该报的那条判据永远没机会说话）。改为记录缺项 + Esc 关菜单，让判据
  去指出具体缺了什么。
- 修 `mcp-switches.mjs` / `sync-intervals.mjs`：两者的极简 mock 未知命令返回 `null`，而网关页
  自 D9-T6a 起会调 `gateway_key_list` —— `setKeys(null)` 在渲染时抛错、整棵 React 树被卸载，
  探针切页时表现为「等侧边栏按钮超时」。**这是 T6a 留下的真回归**（这两个探针不在 gate.mjs
  里，所以当时没被拦住）。已补上缺失的命令并写明这类极简 mock 的维护要求。
- 新增 `crates/gateway-core/tests/m13_key_rules.rs`（7 用例，端到端走完整 HTTP 链路）：
  被 `deny` 的渠道上游**零请求**（不是「试了再失败」）；候选全被过滤 → 403
  `model_not_allowed`（含错误文案要点名是密钥规则）；模型 `deny` 在所有渠道上一起生效；
  `allow` 非空即白名单；`/v1/models` 按密钥过滤；无规则密钥行为与改造前一致；
  保存后 invalidate 缓存立刻生效（同时把 5s TTL 的「不失效就走缓存」语义钉住）。
- `store::keyrules` 新增 4 单测：语义矩阵（deny 命中 / allow 命中 / allow 未命中 /
  都为空 / deny 优先）、读写往返与**覆盖式**语义、allow 与 deny 冲突时按 deny 落地、
  删渠道级联删规则行。
- UI 门禁新增探针 `tools/visual-regression/probe-rules.mjs` + `gate.mjs` 判据：
  候选清单渲染、回填已保存的规则、预览随草稿实时变（2→4→2→0→1）、无可用模型时的
  403 后果提示、提交的四个数组与三态**逐字**一致、保存后关闭并可读回、「已限制」徽标
  只出现在配过规则的行上。判据已逐条实测「改坏就会红」。
- 新增 `crates/gateway-core/tests/m13_multi_key.rs`（7 用例）：创建 3 把后**三把都能鉴权**
  （新建不踢旧）；吊销其中 1 把后该密钥 401、其余两把 200（**端到端走完整中间件**，
  只测函数会漏掉接线回归）；轮换后新密钥可用且旧的全失效；全吊销 → 一律失败；
  吊销幂等且不改时间戳；空白 label 归一为 NULL；`gw_key_active` 仍返回最新一把
  （吊销最新那把后回退到上一把而不是 None）。
- `store::import` 新增 1 个单测：本地已有多把密钥时，WebDAV 拉取**只补不删**
  （不产生任何吊销），且重复拉取幂等。
- UI 门禁新增探针 `tools/visual-regression/probe-keys.mjs` + `gate.mjs` 判据：
  列表渲染全部密钥、显示/隐藏全文、吊销**先二次确认且确认前不动数据**、
  新建插到最前且当场显示全文、全吊销后有空态文案。判据已实测「改坏就会红」。
- 新增 `crates/gateway-core/tests/m13_probe.rs`（11 用例）：200 + 合法 body → `Passed`；
  **200 + HTML 拦截页 → `Failed{Protocol}`**（防「假绿」，这条最重要）；401 →
  `Failed{Authentication}`；404 + 模型语义 → `Failed{Model}`；慢于 `per_probe_timeout`
  → `Failed{Timeout}`；连接拒绝 → `Failed{Network}`；云元数据地址 → `Failed{UrlBlocked}`
  且**一个请求都不发**（断言 mock 命中数为 0、延迟为 0）；`127.0.0.1` 默认拦、显式勾选后
  放行；未填模型 → `Skipped{NoModel}`；未知协议族 → `Failed{Protocol}`；结论里不含明文 key。
- `netguard` 内联 14 个单测：全地址族矩阵（含 IPv4-mapped IPv6 与**字面量混淆写法**
  十进制/十六进制/八进制/省略段）、`allow_loopback` 只放行 loopback、link-local 与
  云元数据任何开关都不放行、非法 scheme / 缺 host / 内嵌用户名密码 / 本机主机名。
- `probe` 内联 22 个单测：四族 payload 矩阵（`stream:false` + 1 token + 鉴权拼法）、
  失败分类矩阵、404 的 model/endpoint 分流、200 + error 对象、预期结构矩阵、
  脱敏与按字符截断、指纹稳定性与不可逆（改 key/模型/URL/族/开关/超时都变）、
  receipt 的通过/失败/全跳过/信息性失败/过期/覆盖。
- UI 门禁新增探针 `tools/visual-regression/probe-endpoint.mjs` + `gate.mjs` 判据：
  逐端点结论行渲染完整、信息性行**看得出**灰显（opacity 区分）、截断摘要自带 `title`、
  被拦截地址的文案与零延迟、探测不改表单脏状态。判据已实测「改坏就会红」。
- 新增 `crates/gateway-core/tests/m13_retry_model.rs`（9 用例，全绿）——每个用例都按
  「mock 被请求了几次」断言，而不是只看状态码：
  401 同族全失败 → 转换族**零请求**；429 与 405 → 跨组并成功；组内预算 3（4 个同族候选
  只试 3 个，第 4 个零请求）；总预算 6（meta 放宽组内预算到 10 → 4+2 恰好 6 次，后面的
  转换族候选零请求）；Responses + `store:true` 只尝试 1 次（对照 `store:false` 走满 3 次）；
  首字节失败（上游 200 + `text/event-stream` 但流立即结束）→ 切到下一个候选并拿到内容；
  首字节失败且无候选可试 → 保留 `upstream_empty_stream` 专属诊断码；meta `(1,1)` →
  只尝试第一个候选。
- `router` 内联新增 11 个单测：`AttemptFlow::next` 真值表（6 类 × 组内预算状态）、
  「候选走完 == 预算耗尽」的跨组判定、跨组只发生一次且进组后组内计数归零、总预算封顶、
  非幂等压到 (1,1)、`retry_budget_from_meta` 的默认/非法/覆盖/(1,1)、`group_candidates`
  保序切分、`classify_failure` 的状态码 → 分类矩阵、`from_kind` 还原、`degradable` /
  `waits_for_backoff` 矩阵、以及 **kind 字符串与 `classify_status` 逐字一致**的兼容护栏。

- 新增 `crates/gateway-core/tests/m13_retry_after.rs`（4 用例）：`Retry-After: 1` 真的等 ~1s；
  `Retry-After: 3600` 被 CAP 截断在 5s；`401 + Retry-After: 30` **不等**；转换路径全失败
  仍是 502 且带 `Retry-After` 头。时间断言用「A 首个请求到 B 首个请求的间隔」而非墙钟。
- `router` 内联新增 7 个单测：delta-seconds / 小数 / 垃圾值 / `inf` / `NaN` / 超大值饱和
  （不 panic）/ HTTP-date（含已过期）/ CAP 截断 / jitter 落在 ±20% 区间 / `Retry-After: 0`
  不被抖动放大。
- 新增依赖 `httpdate = "1"`（已在 `Cargo.lock` 里作为传递依赖存在，不引入新编译单元）。

- **集成测试不再用固定 `sleep` 等异步日志落库**（v0.3.1 发版时撞上）：tag `v0.3.1` 的
  `CI`（main push）在 **windows-latest** 上红在 `m3_anthropic.rs:244`
  （`logs_recent(...).find(...).unwrap()` 拿到 `None`），而同一份代码 30 分钟前在 Windows
  上是绿的、同轮 macOS job 也绿 ⇒ 与产品无关的时序抖动。根因：日志落库是异步的
  （后台线程 + 批量写入），测试写死 `sleep(700ms)` 后立刻查库，慢机器/高负载下不够。
  全仓共 5 处同一写法（`m2_failover` / `m3_anthropic` / `m4_conversion` /
  `m6_responses_inbound` / `output_truncation_diagnostic`）。现新增
  `tests/common/mod.rs::logs_settled(db, limit, timeout)`：有界轮询到「行数连续两次相同」
  即认为本批写完（正常约 100ms 返回，比原来更快），**超时不 panic**（把当前快照交给调用方，
  让真正的断言判断对错；只有一条都没有才报错 —— 那说明日志管道没启动）。5 处全部替换；
  m6 用例耗时由约 2s → 0.71s。注：`v0.3.1` tag 指向的提交仍带这处抖动（纯测试代码，
  产物不受影响），修复落在 tag 之后的提交。

## [0.3.1] - 2026-09-22

### Fixed

- **WebDAV 404 不再一律说成「目标目录不存在」**（真机 2026-09-22）：推送失败时提示
  `WebDAV 推送失败 HTTP 404（目标目录不存在，请先在远端创建该目录）: <!DOCTYPE html> …`，
  后面还跟着一整页 nginx 的 HTML 404 页。按提示去远端建目录没有任何改善（那边本来就有目录、
  远端 `jai-config.json` 一直在），而错误正文把 `<style>` 之类的可读噪声灌进 UI。
  **根因**：`sync.rs` 把所有 404 都翻译成「目录不存在」，但实测那台服务器是 **DUFS**，
  对 `PUT` 到不存在的目录返回 **201 自动建目录** ⇒ 404 在该服务器上不可能指「目录不存在」。
  独立复现（curl）显示事发两分钟内 `GET`/`PROPFIND`/`OPTIONS /` **全是** nginx 自己的
  HTML 404（该 vhost 没路由到后端，同机另一个站点同时 502），即请求根本没到 WebDAV 处理器；
  约两分钟后自愈。**对照实测**：同一台 DUFS 对「文件不存在」的 404 是
  `content-type: text/plain` + 正文 `Not Found`，与网页 404 完全不同 —— 分类因此可判。
  **处置**：404 先看正文再下结论。新增 `looks_like_web_page`（网页 vs WebDAV 的
  XML/纯文本错误；剥 UTF-8 BOM，只看开头 1KB）、`http_hint`（状态码 + 正文 → 提示）、
  `brief_body`（折叠空白 + 截断 200 字符）。正文是网页 ⇒「该地址当前不是可用的 WebDAV
  端点（服务未就绪，或根地址/路径不对；稍后重试）」；正文为空或 XML/纯文本 ⇒ 保留原措辞。
  同一口径覆盖：推送主文件、推送前留存备份、连接测试、备份列表/读取/删除、显式拉取
  （`pull` 把「端点不可用」与「远端还没配置」分开说）。
  - **刻意不 fail-closed**（对抗性审查提出的坑，已用反向控制钉住）：`try_pull` 对「网页 404」
    仍返回 `Ok(None)`。把 404 正文直接判成 `Err` 会炸掉正常流程 —— `push` 的第 1 步
    （留存远端旧版）与 `webdav_push` 的差异预警都用它的 `?`，一旦报错**首次推送永远建不出
    远端文件**（建文件正是推送要做的事）。而 HTML 404 并不等于端点坏了：nginx 的
    dav_module 就用自带 HTML 页回答「文件不存在」，反向代理加一行 `error_page 404 /404.html;`
    也会把所有 404 正文改写成网页。所以分类只用来选措辞，控制流与修复前一致。
  - 测试：单测 3 个 + 集成 8 个（含 5 个反向控制：空正文 404 仍说「目录不存在」、原
    「路径不存在」「远端尚无配置文件」措辞不变、`try_pull` 的 `Ok(None)`、幂等删除的
    `Ok(())`）。变异验证：把 `looks_like_web_page` 改成恒 `false` ⇒ **8 个用例变红**
    （2 单测 + 6 集成）、5 个反向控制仍绿；把 `try_pull` 改回 fail-closed ⇒
    `try_pull_html_404_is_not_fail_closed` 变红。
- **WebDAV 真机验收用例入库**（`crates/gateway-core/tests/webdav_live_e2e.rs`，`#[ignore]`）：
  发布检查单 §4 一直要求「每次发版有 WebDAV 真机验收」，但此前只能手点 UI，没有可复现的
  记录。现在是一条命令（`JAI_DAV_LIVE_URL/USER/PASS/DIR` + `-- --ignored --nocapture`），
  在**隔离目录**里跑完整链路：连接测试 → 推送 → 拉取逐字节比对 → 覆盖前留存时间戳备份 →
  备份列表/读取/删除（含幂等）→ 第二台机器 `apply_import` 落库（供应商 / 上游密钥 / 模型 /
  网关 Key）→ 空目录 404 语义 → 自动清理。**v0.3.1 实测通过**（2026-09-22，DUFS 真机）：
  ```
  [1/8] 连接测试：连接成功          [2/8] 推送成功（1010 字节）
  [3/8] 拉取一致（1010 字节）        [4/8] 覆盖前留存备份：jai-config.1790069393632.json
  [5/8] 备份内容 = 覆盖前的旧版      [6/8] 备份删除 + 幂等删除均成功
  [7/8] 第二台机器导入：供应商 / 上游密钥 / 模型 / 网关 Key 全部到位
  [8/8] 空目录拉取提示：远端尚无配置文件（`text/plain` 404，与网页 404 区分正确）
  [cleanup] 已删除隔离目录 …         真机验收通过
  ```
  顺带记录两条真机行为（mock 测不出）：**DUFS 对不存在的目录会自动建目录**（首次推送即
  成功建出 `jai-e2e-*/`）；**`apply_import` 会为新供应商重新生成本地 id**（文件里的 id 只
  用于模型映射），所以「数据是否到位」必须按名字/列表核对，不能拿导出物里的 id 查。

## [0.3.0] - 2026-09-22

### Fixed
- **「零可见输出的截断轮」被记成干净的 200**（转换路径 + 直通路径都补上）：上游以
  `finish_reason:"length"` 收尾、整轮只有 reasoning 增量而**正文为空、无工具调用**时，
  收尾日志把 `error_kind` / `error_summary` 写死 `None` ⇒ 日志页与统计里它与一次正常 200
  完全无法区分。而客户端（PI-Desktop 的 agent 循环）会把这轮判为 silent turn、追加
  `<no_output_recovery>` 提示重跑一次，仍静默则报 `EMPTY_MODEL_RESPONSE` —— 用户只看到一句
  无从下手的「模型没有产生任何输出」。**真机 2026-09-22**：基元律动/deepseek-flash，460k 上下文，
  四条这样的轮次（completion_tokens 16/96/165/250）全被记成成功，排查只能靠手工比对
  `request_logs` 才发现。现新增 `empty_truncation_diagnostic`：`stop_reason ∈
  {max_tokens, safety}` 且整轮零可见输出（正文或工具调用；**推理不算可见**）时落
  `error_kind = "OutputTruncatedEmpty"` + 可操作摘要（带本轮生效的输出预算与实际产出 token）。
  - **转换路径**：`convert_streaming_response` 逐事件跟踪可见输出；「上游回整包 JSON →
    补成 SSE」的 `deliver_synthesized_stream` 同口径。
  - **直通路径**：新增 `VisibleOutputProbe` —— 字节级转发没有 IR，但可以按 `data:` 行切分后
    复用各族已有的 `parse_stream_event` 判定「这一轮有没有产出用户看得见的东西」。刻意不用
    关键字扫描（分不清 `"content":""` 与 `"content":"x"`，也分不清 `"tool_calls":null` 与
    真正的工具调用），且**只读不写**：客户端收到的字节与上游发出的逐字节一致（有用例钉住）。
  - **HTTP 状态码与线上形状都不变**（截断不是错误，见 `responses.rs::finish_status` 的注释：
    标成错误会把一次截断放大成重试风暴）；**有正文 / 有工具调用的截断一律不标**，避免噪音。
    两个方向都有反向用例，并已用「探针瞎了」「诊断关掉」两种变异验证过不是空洞通过。
- **直通流式的 `request_logs.tool_calls` 此前恒 0**：`streaming_response` 落库时写死 `0`
  （注释理由是「字节直通不解析 SSE 语义」），于是「模型到底有没有发起工具调用」在直通行上
  看不出来，只能去翻转换路径的行。现由 `PassthroughStreamProbe` 按 IR 口径（工具调用 id
  **去重**）计数，与非流式直通的 `count_tool_calls_in_body` 对齐；`client disconnected` /
  上游断流 / 空闲超时几条收尾日志也一并填上探针的实时计数。探针因此必须**全程**解析
  （不能像只判可见输出那样见到正文就提前收工 —— 工具调用可能出现在正文之后），
  每帧一次小对象 JSON 解析，与转换路径同量级；病态输入（无换行的超限巨块）放弃观测，
  该列可能偏低（已写进 `LogRowView::tool_calls` 的文档）。
- **「被预算掐短」单独一档**（P2）：有可见输出、但**确实撞上预算上限**
  （`output_tokens >= max_output_tokens`）时落 `error_kind = "OutputBudgetClipped"`
  —— 用户拿到的是被掐短的答案，不像零输出那样让客户端整轮报废，所以与
  `OutputTruncatedEmpty` 分档（同一条日志行、同一份判定函数 `truncation_diagnostic`，
  不新增行类型、不动响应头、不改状态码）。
  **刻意要求「撞上预算」而不是「只要 `length` + 有正文」**：后者会把客户端**故意**设小预算
  的场景也标成异常（`max_tokens=100` 拿到 100 token 正文是照做，不是故障），而 agent 客户端
  在窗口边缘会把预算压到几十 token ⇒ 每一轮都命中、日志被噪音淹没。真机频率佐证：全库
  7995 行里 `stop_reason=max_tokens` 只有 11 行（0.14%）。已知保守缺口（宁可漏判）：
  客户端未声明预算时不触发；上游在声明预算**之前**就自行截断时不触发。
- **零可见输出的截断「不编进 Responses 协议」（P3 决策记录，未实现）**：这种轮次对 agent
  客户端等于丢一轮，但**刻意只走日志、不走协议**。理由：① 对现有客户端零收益（PI-Desktop
  的 Responses 解析器只读 `status` / `incomplete_details` / `output` / `usage`，加字段不改变
  用户看到的东西，要兑现必须同时改客户端，是跨仓库协同）；② 客户端用已有信息就能解决
  （它已经有 `stopReason:"length"` + 空正文，可在本地给出「预算被推理耗尽」）；
  ③ 改响应体形状有实测过的风险（`openai@6.x` 只在 `response.completed` 上累积快照是实测结论，
  未知字段态度未验证；`metadata` 是用户传入原样回显的字段，网关写会冲突；`status` /
  `incomplete_details.reason` 的组合绝不能动，否则复现「一次截断被放大成重试风暴」）。
  详见 `codec::responses` 里 `TERMINAL_EVENT` 上方的决策记录。
- **刻意不接管 `models.max_output_tokens`**（决策记录）：这一列是模型元数据，不是网关要替
  客户端执行的策略。曾试过两条路，都撤掉了：
  - 在 `/v1/models` 里发布成 `maxOutputTokens` —— 客户端会直接采纳为自己的输出上限
    （PI-Desktop 就读走），而输出预算是客户端上下文规划的一部分；网关替它决定等于从远端
    插手 agent 循环，本次真机故障（预算被压到几十个 token → 整轮耗在推理上 → 正文一个字
    不出）正是这条链路的产物。
  - 请求侧缺省时按该列兜底 —— 会让任何省略 `max_output_tokens` 的客户端被静默封顶，同样是
    把网关的判断塞进客户端（对 Anthropic 上游那类「缺字段即 400」的场景，正确做法是让
    上游的错误如实冒泡，而不是网关替客户端编一个值）。
  两条都不做：网关只做转发与诊断，不发明预算。`/v1/models` 仍只发布原本就有的
  `contextWindow`（语义未变）。诊断摘要里的输出预算取自**客户端自己声明的值**
  （`PeekRequest` 只读，不改写）。
- **多段 `system` → Anthropic 上游会被整体判 400**：`anthropic::encode_request` 对
  `req.system.len() > 1` 直接 `Err` → `proxy.rs` 把它变成 400「system 段数为 N，跨族转换仅
  支持单条合并 system」。**触发拓扑**：该 encoder 只在**上游族是 anthropic** 时被调用，
  而 Anthropic 入站 × Anthropic 上游是同族直通（不过 encoder），所以真正伤到的是
  「**别的入站族** × Anthropic 上游」—— `openai::decode_request` 对每条 `role=system`
  消息（以及 system 的多段 content 数组）各 push 一段 `system`，于是「客户端发了两条
  system」就把上游完全能跑的请求在网关侧判死。现按 IR 契约（`ir.rs` 的
  `CanonicalRequest.system` 注释）改为 `\n\n` 合并，与 openai / responses / gemini 三个
  encoder 对齐。
- **Anthropic 入站的 `thinking` 内容块被丢弃**：`decode_request` 对 `type:"thinking"` 只打
  WARN 后丢块 —— 而 `openai.rs` / `responses.rs` 两个入站族早已产出 `Block::Thinking`
  （正是 `responses.rs` 里 76cb5093 实测「跨族出站缺 `reasoning_content` → thinking 上游 400」
  那次修复）。三个入站族口径现已一致；`redacted_thinking` 的 `data` 原样存入 signature 位。
- **`output_config` 在代码库里没有任何引用**：zcode 的 `anthropic-messages` 内置规则表以
  `{"thinking":{"type":"adaptive"},"output_config":{"effort":"high"}}` 的形态注入档位，
  而该字段连「未建模已丢弃」的 `CapabilityWarn` 都不产生 → 跨族时用户选的档位**静默丢失**。
  现 `output_config.effort` 读成 `reasoning_effort`，`thinking.type:"disabled"` 归一为 `none`；
  `enabled` / `adaptive` 不带档位时**不臆断**成具体档位（保持不干预）。
  值域归一仍只在出站族为 Native（openai 系）时生效，直通 / 跨族两条路径语义一致。
  （源码对账依据：`zai-org/ZCode` 的 `packages/model-option-map` + `config/provider/zcode-builtin.json`）

### 第二轮：PI-Desktop 源码对账（2026-09-22）

- **跨族 + Responses 上游 + 流式 = 静默空轮**：`convert_streaming_response` 的
  `openai_responses` 分支**没有解析器**，把每个 `data:` 行直接 `continue` 丢弃 ⇒
  `pending_finish` 永不置位、`render_frame` 永不调用，客户端只拿到
  「HTTP 200 + text/event-stream + **零帧**」（agent 客户端等于丢一轮且不报错）。
  非流式路径反而是好的（`convert_plain_response` 有 `responses::parse_response`），所以症状极隐蔽。
  现补 `responses::parse_stream_event`（按官方 SSE 形状解析成 IR StreamEvent，`stop_reason` /
  usage 与 `parse_response` 同一套口径）。
- **上游忽略 `stream: true` 时客户端拿到空流**：分发原先只看 `req.stream`，
  于是「客户端要流式、上游回整包 JSON」的请求被喂进 SSE 解析器。现改为**按上游实际
  content-type 决定转换器**（`ct_is_sse`），并把 `req.stream` 传给
  `convert_plain_response(as_stream)`：整包解析后用**同一套渲染器**补成入站 SSE，
  收尾口径（Anthropic `message_stop` / 其余 `[DONE]`）与流式路径的自然结束分支一致。
- **上游错误响应头被全部丢弃（`Retry-After` 退避提示丢失）**：全渠道失败与确定性错误的收尾
  只重建 status + content-type + body（`grep -rn retry-after` 在整个 crate 里 0 命中）⇒
  客户端只能按自己的指数退避重试，与上游限速窗口错拍（对已限速的上游**重试放大**）。
  现按白名单回传 `retry-after` / `retry-after-ms` / `x-request-id`。
- **客户端历史里的推理此前完全没读**：`openai::decode_request` 的 assistant 分支不看
  `reasoning_content`（该区 `grep reasoning` 0 命中）⇒ 跨族到 thinking 上游时历史推理丢失。
  现读入为 `Block::Thinking`，并把**实际命中的线上字段名**记进 `signature`。
- **只认一种推理字段名**：线上有三种拼写（`reasoning_content` / `reasoning_text` /
  `reasoning`，与 PI-Desktop 的 `COMPLETIONS_REASONING_SIGNATURES` 同一份清单）。
  编码侧现在**原样回传命中的那个名字** —— 换名字会被严格中继当成「没有回传推理」而 400。
  `signature` 做白名单校验：Anthropic 入站填的是加密签名串，不会被误当成字段名。
- **显式空串推理被丢弃**：客户端**显式发过** `reasoning_content: ""` 时保留「字段存在」
  这一事实（官方 DeepSeek 接受 `""`），但不发明模型没产生过的内容（客户端没发过就仍不发）。
- **`role: "developer"` 在两条 OpenAI 系入站都被处理错**：
  chat-completions 侧落到 `other` 分支被**整条丢弃**（客户端用 developer 传 system prompt 时
  指令静默消失，只剩一条 CapabilityWarn）；responses 侧被折成 **User 消息**（指令语义降级成
  用户轮次）。现两者都归一到 system 段 / `instructions`。

### 第三轮：推理回放兼容 —— **自适应**方案（2026-09-22）

**问题**：DeepSeek 系 thinking 模型要求**每个回放的 assistant 消息都带推理字段**，否则 400；
官方 `deepseek.com` 接受空串 `""`，但部分第三方中继**拒绝空串、要求非空值**（PI-Desktop
`#296` 记录的 OpenCode 系）。而客户端**本来就会**裁掉历史推理：PI-Desktop 只保留最近 3 轮
思考（`MAX_RETAINED_REASONING_TURNS = 3`）、跨模型回放主动删、上下文压缩后、纯工具调用轮、
模型那轮确实没思考。于是跨族转换（入站 Anthropic/Responses × 上游 `openai_compat`）出站时
很多 assistant 轮次**没有推理字段** → 严格中继 400。

**为什么不做「渠道开关」**：上一轮把这条留成了「需要 0013 迁移 + 渠道级显式开关」的产品决策。
但上游可能很多、且随时新增，靠人配配不完、靠猜模型名会漏（PI-Desktop 自己的注释就承认
「聚合器和自定义网关匹配不上」）。改成**三层、逐层收紧的自适应**，**零迁移、零签名变更**：

1. **推断**（`codec::replay::infer_from_channel`，零额外往返）：渠道的模型名 / base_url /
   供应商名带 `deepseek` 字样时，直接按「需要非空回放」处理。保守，不外推「thinking 模型都算」。
2. **学习**（`Registry` + 上游真实报错）：没猜中的上游，第一次被它 400 后**识别报错 → 记下该
   渠道 → 用兼容原地重试同一请求**，客户端只看到成功。**只有重试成功才保留学习结果**。
3. **遗忘 / 抑制**：已开启的渠道若再次被同类错误拒绝 ⇒ 这个开关对它无效 → 记 `Some(false)`，
   之后不再干预 —— 既避免反复塞占位把本来能跑的配置改坏，也**不抖动**
   （只在未学习时判断一次，落到 `Some(_)` 后不再改变）。

标记是**供应商级**的（兼容性是中转端点的属性），落库用现成的 `meta` KV 表
（`store::meta_get/meta_set/meta_delete`）⇒ **不需要新迁移**。规划层通过 `req.extensions`
（`__jai_reasoning_replay`，与既有 `__jai_validate_output` / `__jai_tool_identities` 同一套机制）
告诉 `openai` 编码器，所以四个 `encode_request` 的签名**一个都没改**。

- **跨族转换路径**：`openai::encode_request` 在该轮历史确无推理时补
  `reasoning_content: "[reasoning not retained for this turn]"`（占位符默认关闭，
  只有推断或学习命中才出现 —— 网关不发明模型没产生过的内容）。
- **同族直通路径**：字节转发不经编码器，所以在原始 JSON 上做手术
  （`codec::replay::inject_placeholders`）：只给**三个已知拼写都没有**的 assistant 消息补
  `reasoning_content`，客户端带过的推理与 user / tool 消息一字不动；解析失败或一条都不缺则
  保持原字节。

**回归证明**：`tests/reasoning_replay_passthrough.rs`（直通路径）+ `tests/m5_anthropic_inbound.rs`
的 `adaptive_reasoning_replay_learns_and_retries_transparently`（跨族路径）都验证「**旧行为
把上游 400 原样交给客户端 / 新行为学习后原地重试，客户端只看到 200**」；另有 8 个单测覆盖
推断边界、报错识别的窄匹配、三态持久化与「不抖动」。日志对此有 `CapabilityWarn`
（「上游要求回放推理（HTTP 400）→ 已记下该渠道…正在重试同一请求」）。

**已知边界**：占位符是**中性的英文串**，不是协议常量；上游若因其它原因（非「缺推理字段」）
报 4xx，`is_replay_rejection` 的窄匹配不会误判，网关不干预。

### Docs
- `docs/zcode接入.md`：`api.type` 枚举值更正为 `anthropic-messages`（`anthropic` 是 zcode 内部
  映射出的 AI SDK provider kind，两者不同）；新增「Base URL 怎么填」——三条协议线的 URL 拼法不同
  （`anthropic-messages` 自动补 `/v1`，另两条**必须显式带 `/v1`**，否则打到 `/responses` 直接 404）；
  新增「两条线各自发的形态」对照表。
- `docs/design/protocol-ir.md`：§10 补「入站方向」档位映射表；§11-3 由「thinking/signature 跨族
  仅占位存储不转换」改为「已实现」（文档此前落后于 openai / responses 侧代码）；
  §11 新增「Responses 上游流式」条目（由「暂不支持」改为已支持 + 补流兜底）；
  §11-8「严格中继要求非空推理回放」由「未做（需渠道级开关 + 0013 迁移）」改为「已实现（自适应方案）」，
  并指向新增的 `codec::replay` 模块。

### 最小窗口尺寸：macOS 上的限制从未生效（2026-09-22）

- **macOS 上「最小窗口 900×600」一直是个空承诺**：`tauri.macos.conf.json` 用 `app.windows`
  覆盖窗口配置，而 Tauri 的平台配置合并走 **JSON Merge Patch（RFC 7396）—— 数组是整体替换，
  不是按下标逐字段合并**。于是基础配置里的 `label/title/width/height/minWidth/minHeight`
  在 macOS 上被**全部丢弃**：窗口退回 Tauri 默认 **800×600**（而非设计值 1180×800），
  且 `minWidth/minHeight` 变成 `None` ⇒ **macOS 上没有任何最小尺寸限制，窗口能被拖到极小、UI 错乱**。
  实测（`tauri_utils::config::parse::read_from(Target::MacOS)`，改动前）：
  `app.windows = [{ decorations, hiddenTitle, titleBarStyle, transparent, windowEffects }]`。
  一直没被发现，是因为双尺寸验收是在**浏览器探针**里按视口跑的（验证「900×600 时 UI 正常」），
  **从来不是**「真实窗口拖不到 900×600 以下」。
- 处置：macOS 平台配置**显式补齐**被数组替换吃掉的 6 个键（值取基础配置设计值
  1180×800 / 最小 900×600）；平台专有的 `decorations`/`titleBarStyle`/`hiddenTitle`/
  `transparent`/`windowEffects` 保持不变。
- **下限 900×600 是量出来的，不是估的**（`audit.mjs` 的 `truncated` = 被截断且无 `title` 兜底的文本）：
  固定高 600 只改宽度 —— 1180/900 为 **0** 处，890 为 7、880 为 9、860 为 16、820 为 18；
  `hScroll` 全程 0（表格是流式的，不会「崩」，但会开始**静默截断**长 URL 与技能描述）。
  ⇒ **900 是「零信息丢失」的硬边界且余量为 0px**。高度方向宽松（900 宽下 600/560/520/480 的
  `truncated`/`hScroll`/弹窗越界/弹窗控件滚不到**全为 0**，弹窗自带限高 + 内滚），
  600 是**可用性下限**，故不改这个数。
- **新增零依赖门禁 `scripts/tauri_window_check.mjs`**（已接进 `gate.mjs` 的静态规范段，
  即 `release_check.sh` 第 6 步自动覆盖）：① 平台配置的窗口对象必须重新声明基础配置里的
  **每一个**键（以后谁在基础配置加窗口键、忘了同步平台文件，门禁立刻红，而不是在某个平台静默丢）；
  ② 解析后的 `minWidth/minHeight` ≥ UI 验收尺寸 —— **单一来源**，从 `gate.mjs` 的 `--sizes`
  默认值取最小一组（改验收尺寸只需改一处），且**正则失配直接报错**（不允许静默变绿）。
  负控制已做：回退成改动前的 macOS 配置 → 红（报出漏掉的 6 个键 + `minWidth=undefined`）；
  键齐全但填 `700×480` → 红（低于验收下限）；恢复 → 绿。
- 顺带修掉第一版门禁里的一个优先级 bug（`!(k in X || {})` 恒为 false 会让判据**静默失效**）。

## [0.2.13] - 2026-09-21

### Added
- **UI 规范门禁**：`node tools/visual-regression/gate.mjs` 一条命令在 **1180×800 与 900×600
  双尺寸 × light/dark 双主题**下验收 UI 规范，**单一退出码**；判据集中在该文件里
  （对比度 AA / 字号 ≥11px / 截断有 title 兜底 / 无横向溢出 / 弹窗几何 /
  弹窗 footer 不与 toast 占位带重叠 / 主操作首屏可达 / 行内控件不被折叠线切半 /
  吸顶表头真的吸顶 / 表格不溢出容器 / 有效命中区 ≥24×24）。
  新增零依赖静态检查 `scripts/ui_lint.sh`（字号 / 图标按钮可访问名 / 列表 key / 命中区手法）；
  已接进 `scripts/release_check.sh` 第 6 步。探针的 Playwright/Chrome 定位统一到
  `tools/visual-regression/_env.mjs`（可用 `PLAYWRIGHT_PATH`/`CHROME_PATH` 覆盖），
  15 个探针不再有写死的绝对路径；`ui` 新增 devDependency `playwright-core`（只驱动系统 Chrome）。

### Changed
- **模型页改为「只读表格 + 右侧详情抽屉」**：行内可交互控件 **194 → 68**，表头吸顶，
  单个模型的全部编辑项在抽屉里**一次保存**（与「添加/编辑」弹窗同一套 footer / 校验 / 保存反馈）。
  900×600 下表格横向溢出由 +8px 变为 **-50px**（有余量）。
- 弹窗最大高度改为 `calc(100dvh - 8rem)`，使弹窗底边恒在 toast 占位带上方
  （900×600 下此前 toast 会视觉压住弹窗主按钮 2.4 秒；默认窗口行为不变）。

### Fixed
- **改表单不保存就切页/关窗会静默丢失**：新增脏状态 guard（`ui/src/lib/dirty.ts` +
  `components/common/UnsavedGuard.tsx`），供应商 / MCP / 技能 / 设置四处表单接入
  （弹窗内与页内表单均覆盖），并拦 `beforeunload`。
- **日志页轮询不随可见性暂停**：`document.hidden` 时停轮询，恢复可见立即刷新一次
  （不误开用户手动关掉的开关）。
- **命中区「已修」项实测不达标**：模型页「复制模型名」有效命中区只有 **6×30**、
  供应商页「官网」**54×18** —— 伪元素热区被 DOM 在后、paint 在上的相邻小芯片抢走。
  改用**真实盒子** + 负 margin 抵消布局影响，并以 `z-10` 赢下重叠区。
- 4 处 `text-[10px]`（推理档位 / 工具声明数上限编辑器）改 `text-[11px]`，由 `ui_lint` 守住。
- **日志页与模型页的吸顶表头此前从未真正生效**（确实声明了 `sticky top-0`，实际照旧滚走）：
  根因是 `Table` 基座自带的 `div[data-slot=table-container].overflow-x-auto` 成了 thead
  的最近可滚动祖先且自身无纵向溢出；现把 `max-h + overflow:auto` 加到**那一层**。
- 探针自身的三处「假绿」：`fold.mjs` 的「顶部操作区」选择器永不匹配（恒为「—」，死字段）；
  `clippedRows` 45/102 步把滚动容器误报成 `html`；`gate.mjs` 会在探针**崩溃**时读到上次的
  旧 JSON 并报「全部通过」（现改为跑前删旧结果 + 探针退出码非 0 直接判失败）。
- `gate.mjs` 自己创建 `<repo>/.vr/tmp`：此前只有 `.vr/tmp` 恰好存在时才能跑，
  全新克隆/CI 上第一次运行会 `ENOENT: mkdtemp '…/.vr/tmp/playwright-artifacts-…'`。
- **截断（`max_output_tokens`）被误报成「可重试错误」，把一次截断放大成 10 次重试风暴**（bug 清单 11）。
  Responses 转换路径的收尾帧只把 `response.status` 写成 `"incomplete"`，事件名恒为
  `response.completed`，且**从不输出 `incomplete_details`**（全仓库零命中）。只读 `status` 的
  严格客户端（PI-Desktop 的 Responses 适配器）因此把「incomplete 且无 reason」映射成
  `stopReason:"error"` + `retriable:true`，**重发同一 prompt 最多 10 次**
  （其 `PROVIDER_RETRY_MAX_RETRIES = 10`）。
  - 实测证据：客户端日志 `agent.turn.failed … "Response incomplete without a provider reason",
    retriable:true, retryAttempt:10`；JAI 日志里同一个 `usage_input` 连续出现 3–10 次、
    每次 `usage_output` 跑满上限（8192）、每次约 40s。
  - 后果：每次重试都让模型重新生成一遍（该渠道在长上下文 agentic 场景下会退化复读），
    客户端又把重试结果拼进同一条消息 → 会话里同一句话被拼 2/4/…/176 遍（份数恒为偶数），
    而 thinking 块与工具参数从不重复。
  - 修法：新增 `finish_status()` 统一口径 —— `MaxTokens → ("incomplete","max_output_tokens")`、
    `SafetyBlock → ("incomplete","content_filter")`，并给 `incomplete` 补上 `incomplete_details`
    （此前该字段全仓库零命中）；非流式 `render_response` 与合成流 `render_response_sse` 同口径。
    OpenAI 语义里截断**不是错误**：`status:"incomplete"` + `incomplete_details.reason`，
    客户端据此判 `stopReason:"length"` 而不再重试。
  - **终局事件名保持 `response.completed`（刻意不跟 OpenAI 的 `response.incomplete`）**：
    实测 `openai@6.x` 的 `ResponseStream` 只在 `response.completed` 上累积快照，
    `response.incomplete` 落进 `default:` 被忽略 → 快照停在 `status:"in_progress"` 且**丢 usage**
    （截断响应反而记不到用量）。判「截断」靠 `status` + `incomplete_details`，事件名是客户端的
    分支依据 —— 等目标客户端都验证过再切规范名。
  - 责任边界：上游**确实**在复读（该中转 usage 经核验准确：同一句话重复 810 遍 ≈ 8900 tokens
    ≈ 上限 8192），JAI 仍是 1:1 转发、不制造重复（delta 逐帧对应上游 `delta.content`；
    缓冲破坏性 drain；无中途重试/重放）。本次修的是「放大器」这一侧。

- **`request_logs.stop_reason` 恒为 NULL**（bug 清单 12）。该列在 schema 与 INSERT 里都有，
  但 `emit_log_with` 里写死 `None`，6434 行全空 —— 排查「模型为什么反复重发」时看不到
  `max_tokens` 截断（本次只能靠 `usage_output` 顶满上限反推）。
  现按 IR 口径统一采集（`end_turn`/`max_tokens`/`tool_use`/`safety`/`other`）：
  转换流式取 `Finish`（与 `pending_finish` 同裁决）、转换非流式取 `CanonicalResponse.stop_reason`、
  直通非流式按响应体形状取、直通流式用 `StopReasonProbe` 关键字级扫描
  （与「直通流式 `tool_calls` 恒 0」同源约束：字节直通不解析 SSE 语义，只读不写）。
  探针的两个坑（对抗性审查发现，各配回归测试）：① Responses 终局帧把**整个 response
  （含全部正文）**嵌在同一帧里，截断响应正文 30KB+ 会把帧首的 `status`/`incomplete_details`
  挤出固定窗口 → 改为**逐块扫描**（末尾窗口只作跨块兜底）；② 同帧 `output[*].status`
  在截断时反而是 `completed`，「取最后一次 `status`」会把截断静默记成 `end_turn` →
  `incomplete_details.reason` 优先（值域白名单）+ `status` 仅兜底。上游挂死/中途断流的行
  也带上已见到的结束原因。
  日志页 CSV 导出新增「结束原因」列，JSON 导出与 `logs_recent` 自动带上 `stopReason`。
  已知偏离（留作独立变更）：`output[*].status` 截断时仍为 `completed`（OpenAI 会镜像为
  `incomplete`）—— 现有客户端一直看到 `completed`，改它需单独验证。

### Tests
- Responses 收尾口径单测 4 个（截断 → `status:"incomplete"` + `incomplete_details.reason`、
  `SafetyBlock` → `content_filter`、正常结束不带 `incomplete_details`、非流式同口径）。
- M6 集成测试 3 个（流式截断/非流式截断/正常结束）：既断言客户端拿到的
  `incomplete_details.reason`（并反向断言**不得**出现 `response.incomplete`，把事件名兼容
  决策钉在测试里），也断言 `request_logs.stop_reason` 落 `max_tokens` / `end_turn`。
- 代理侧单测 5 个：结束原因词表映射、直通非流式取体、直通流式逐块扫描（含正文里转义
  同类文本不得误命中）、巨型终局帧不被窗口截断、`reason` 优先于 item `status`
  + 末尾窗口有界性测试。

## [0.2.12] - 2026-09-21

### Fixed
- **「自动拉取没生效」的真因：变更唤醒把定时器重置，自动拉取被饿死**（bug 清单 20）。
  `spawn_autopush` 每轮循环都**新建** `tokio::time::sleep(wait)`，`rx.changed()` 一到就在
  `select!` 里丢弃它并重建 —— 即「任何一次变更唤醒都重置定时倒计时」。而 `notify_change()`
  在 **15 处**调用（供应商增删改、模型改别名/限额/模态、MCP、技能、网关 Key 重生成、导入、
  快照恢复……），全是日常编辑动作。只要用户**编辑得比间隔勤**（间隔 30/60 分钟时很常见，
  360 分钟几乎必然），定时 tick 永远等不到，自动拉取再也不会跑；自动推送的定时分支同样失效。
  - 附带纠正原条目的判断：原文点名的 `let wait = push_interval.or(pull_interval)`
    当时**不是活跃缺陷** —— 全项目只有一个间隔字段，两边恒等，`or` 取哪个都一样。
    它是「一旦把两个间隔拆开就会静默踩中」的潜在隐患，本次一并消除。
  - 修法：调度抽成纯函数 `plan_sync` / `next_wake` + `ActionClock`
    （记录「上次实际执行时刻」与「启用起点」）。到期点 = `anchor + interval`，
    **只有真正执行过才往后推**，变更唤醒与配置重读都不再重置它 —— 饿死路径从结构上消失。
    定时唤醒点取**最近的一个到期点**，同时修掉旧代码「开了自动推送就每 tick 都推」。
  - 时钟用**单调时钟** `Instant`；挂钟毫秒（`at_ms`）只用于界面展示，不参与判定 ——
    系统时间被 NTP 校正或手改，不该让同步饿死或瞬间风暴。
  - 顺带修掉改调度时**自己引入**的两个风险（自查发现，各配单测）：
    ① **拿不到锁时的空转**：改成「睡到到期点」后，若动作已到期而手动推/拉正持锁，
       `next_wake` 返回 `Some(ZERO)` → `sleep(0)` 立即返回 → 又拿不到锁 → `continue`，
       变成空转 + 刷屏（旧实现每轮固定睡满一个间隔，没有这个问题）。
       新增 `AUTOSYNC_LOCK_BACKOFF`（5s）显式退避。
    ② **配置改动生效滞后**：`webdav_config_set` 原先不通知调度循环（它改的是调度参数本身），
       若睡满一个长间隔（推送 6 小时），用户刚把「拉取间隔」改成 30 分钟也要等最长 6 小时才生效。
       新增**独立的**配置变更信号 `AutopushHub::cfg_tx` + `notify_config_change()`，
       调度循环 `select!` 多一路 `Wake::Config`：只回循环顶部重读配置，**不推送、不走防抖**
       （复用 `tx` 会变成「改个间隔就顺带推一次远端」）。
       另加 `AUTOSYNC_MAX_SLEEP`（60s）作纯安全网 —— 只是「醒来重新评估配置」，
       **不会提前执行动作**（是否该跑由 `plan_sync` 按绝对到期点判定，与睡眠时长无关）。
  - 对抗性代码审查（`code-reviewer` 子代理）后另补一个真缺陷：
    **瞬时读库失败被当成「用户关掉了」**。`current_autosync_intervals` 原先把
    「读配置失败」与「读到了但未启用」都返回 `(None, None)`，调用方据此
    `sync_enabled(false, _)` → **清空锚点** → 恢复后重新计时，
    把下一次执行整体推迟一个完整间隔（最长 6 小时）—— 正是本次要消灭的症状，
    只是触发源换成了偶发 IO 错误（SQLite busy / 磁盘抖动 / `spawn_blocking` join 失败）。
    改为返回 `Option<...>`：`None` = 读失败 → **保持时钟不动**，睡 `AUTOSYNC_CONFIG_POLL` 后重试；
    `Some((None, None))` = 确定的「未启用」。

### Changed
- **自动推送与自动拉取各自拥有独立间隔**（bug 清单 20）。
  此前两者共用一个 `auto_push_interval_min`，「推送 6 小时 / 拉取 30 分钟」这种组合
  **根本无法配置**。现拆为 `auto_push_interval_min` 与 `auto_pull_interval_min`
  （`normalized_interval()` 相应更名 `normalized_push_interval()`，新增 `normalized_pull_interval()`），
  同步页从「一个共用间隔」改为**推送 / 拉取各一个**选择器，文案明确两者相互独立。
  - 新 meta 键 `webdav_auto_pull_interval_min` 已加入 `MACHINE_LOCAL_META_KEYS`
    （本机调度偏好，导出剔除 + 导入忽略）：漏加就是 bug 19 的同类问题（A 机间隔静默改写 B 机）。
    常量注释里补了「新增键务必加进本数组」的纪律。
  - 兼容：旧配置无该键时按 serde 缺省回落到 60 分钟（与推送间隔同默认值），
    且 `Default` 改为手写，保证「缺键反序列化」与 `WebDavConfig::default()` 取值一致。
  - 语义：两个间隔都从**上次成功执行完成**起算（固定延迟，避免慢同步背靠背连跑）；
    刚启用或关掉再打开都重新等一个完整间隔（避免打开瞬间立刻同步一次）；
    缩短间隔**即时生效**（按上次执行时刻重算）。

### Added
- 调度器单测 `mod autosync_schedule_tests`（12 条，**可控时钟**：全部用「基准时刻 + 偏移」
  构造 now，不依赖真实时间流逝，不会 flaky）。覆盖：拉取不被推送长间隔绑死
  （360/30 → 唤醒点取 30）、推送不被拉取短 tick 拽跑（30…330 分钟只拉不推，第 360 分钟才推）、
  **变更唤醒不推迟到期点**（第 m 分钟剩余时长须为 `30-m` —— 饿死回归）、关掉再打开重新计时、
  改间隔按上次执行重算、刚启用不立刻同步。
- `sync.rs` 新增 2 条单测：拉取间隔独立归一化（不受推送间隔影响）、
  缺键反序列化与 `Default` 取值一致。
- `m7_import_webdav.rs` 的 bug 19 守门人测试扩到新键：B 机推送间隔 360 / 拉取间隔 30，
  A 机 payload **注入** `webdav_auto_pull_interval_min=360`，断言拉取后 B 仍为 30
  （导入侧若退化即精确变红）。

### 工程 / 测试基建
- 新增视觉回归探针 `tools/visual-regression/sync-intervals.mjs`：把「两个间隔各自独立」
  固化成断言 —— 两个选择器分别存在且纵向分离、初始值各来自自己的字段（fixture 刻意取
  推送 30 / 拉取 360）、**改拉取只写 `autoPullIntervalMin` 且不碰推送**（反向同理，
  比对 IPC 参数而非只看界面）、文案不再声称共用、无横向溢出、选择器完整可见。
  1180×800 与 900×600 双尺寸全绿。
- **修掉视觉回归探针里两个「让断言静默失效」的坑**（bug 清单 28）：
  ① 三个断言「无控制台报错」的探针（`mcp-switches` / `gateway-endpoints` / `sync-intervals`）
  **长期恒红**——`@tauri-apps/api` 的 `_unlisten` 需要 Tauri 注入的
  `window.__TAURI_EVENT_PLUGIN_INTERNALS__`，而 mock 只装了 `__TAURI_INTERNALS__`；
  `TitleBar` 的 `onResized` 清理在每个页面都抛 pageerror。四个 mock 已补齐该对象，
  三者修后全绿（`gateway-endpoints` 17/17）。
  ② `audit.mjs` / `deep*.mjs` / `fold.mjs` / `probe-*.mjs` 共 **12 个**探针读的是
  `.vr/run.mjs`（未跟踪的本地镜像）而非仓库内 `run.mjs`——改源文件不同步镜像时，
  探针会静默沿用旧 mock（本次实测：改完 audit 的 pageerror 依旧）。已同步镜像，
  并把纪律写进 `tools/visual-regression/README.md`。

## [0.2.11] - 2026-09-20

### Fixed
- **流式 `response.completed.response.output` 恒为 `[]`** —— 只读最终对象的客户端会拿到一个**空回合**。
  `RenderState::new_response` 把 `output` 写死成空数组，真实内容只存在于增量事件里；
  同一份内容在非流式路径 `render_response` 里是完整的，**两条路径行为不一致**。
  靠 delta 解析的客户端（Reasonix / dsh）无碍，但严格实现（只读最终对象）会丢整轮。
  - 物证（2026-09-20 抓包实测，Reasonix 一次真实请求）：同一帧流里
    **448 个 `response.output_text.delta` 带着完整文本**，而收尾帧是 `"output":[]`。
  - 修法：`RenderState` 新增 `completed_items: Vec<Value>`，在三处 `output_item.done`
    （reasoning / message / function_call）发出时同步累积；收尾改用新的
    `completed_response(status)` 回填 `output`。`response.created` / `response.in_progress`
    仍走 `new_response`（此时确实还没有 output）。
  - **顺序语义**：累积顺序 = `output_item.done` 的**收尾顺序**。因为 `output_index` 只在收尾时
    递增，收尾顺序必然等于递增的 `output_index` 顺序 —— 与客户端实际观察到的事件序一致。
    注意「文本 + 工具调用同轮」时 `msg_started` 有意跨工具保持打开（为的是不重发
    `output_item.added`），所以文本 item 的 index 在收尾时才落定，`output` 里呈现收尾顺序
    而非「先文本后工具」的语义顺序；这是既有特性，本次**未改**，测试也不锁该顺序。
  - `Ev::Start` 新增清空累积，避免上一轮的 item 串进本轮。
  - 验证：新增 5 个单测（纯文本 / 推理+工具调用（抓包真实形状）/ 三类混合 / 反证「没有
    `output_item.done` 就不该凭空多出 item」/ 反证「跨轮不泄漏」）。反证实测：把收尾帧改回
    `new_response` 后，4 个用例精确变红、对照用例仍绿。
    全量回归 **366 通过 / 0 失败**（原 361，+5）。
  - 附注：既有测试只断言收尾帧「存在」（`contains("response.completed")`），**从没检查过 `output`**
    —— 这正是该洞长期未被发现的原因。

### Changed
- **限定名 `供应商/模型` 的语义从「只用这家」改为「优先这家」**，恢复故障转移。
  旧实现是 `candidates.retain(|c| c.provider_name == …)` —— 等于把故障转移**彻底关掉**：
  只要客户端用的是 JAI `/v1/models` 推荐的限定名（Reasonix 就是这么选的），
  这家上游一抖动就**没有任何退路**（实测该上游 502 占 9.78%，当天 20%），
  与 v0.2.10 加的同渠道重试是同一问题的两个层面。
  - 新语义：指定供应商**优先**，其余同模型候选保留为**后备**。
  - **健康优先于指定**：指定渠道已知不健康时不抢健康备渠道的位置（否则每次都要先撞一次已知失败）。
    四档顺序：健康+指定 → 健康+其它 → 不健康+指定 → 不健康+其它。
    为此把 `router::is_healthy` 由私有改为 `pub`（`proxy.rs` 复用同一判定，不另写一套）。
  - **保留一条硬约束**：指定供应商**一个候选都没命中时仍然 404**。否则供应商名打错会
    静默换家出答案，比报错难排查得多。
  - 排序用**稳定排序**叠加在既有「同族优先 + 健康/优先级/权重」序之后，
    故每档内部顺序不变；`route_candidates` 精确匹配 `model_name`、逐候选按各自
    `upstream_model_id` 改写模型名，回退到别家时模型名依然正确。
  - 验证：新增 `crates/gateway-core/tests/m12_qualified_provider_fallback.rs`（4 用例）。
    夹具里**指定供应商的 priority 刻意更差**（200 vs 100）—— 若「优先」没生效，
    请求必然落到另一家，用例立刻变红。覆盖：
    ① 指定渠道 500 → 回退到备渠道成功；② 同一夹具再发一次 → 指定渠道已被标记不健康，
    健康逻辑接管，**指定渠道不再被尝试**；③ 两家都 200 → 用指定那家（优先压过更优 priority），
    备渠道一次都没被碰；④ 供应商名对不上 → 404；⑤ 裸模型名行为不变（仍按 priority）。
    反证实测：把实现退回 `retain` 后，用例 ①③⑤ 精确变红。
  - 附注：仓库**没有**任何既有测试依赖旧的 `retain` 行为（已 grep 确认），故回归风险低。

### Added
- 新增诊断器 `crates/gateway-core/tests/diag_responses_conversion.rs`（默认 `#[ignore]`，不进回归）：
  把 Responses 入站 → chat/completions 出站的转换结果**逐条打印**（role / content /
  `reasoning_content` / `tool_calls` / `tool_call_id`），并自动检查 5 件事：最后一条是否为
  用户真正的问题、system 是否来自 `instructions`、推理是否混进 `content`、工具调用与结果是否配对、
  有无多余插入消息。客户端报「答非所问 / 上下文被插了东西」时先用它自查，不必改任何线上配置：
  `cargo test --test diag_responses_conversion -- --ignored --nocapture`，
  或 `JAI_DIAG_PAYLOAD=/tmp/real.json` 喂真实抓到的请求体。

## [0.2.10] - 2026-09-20

### Fixed
- **上游「连接类」失败改为同渠道重试一次**（`UPSTREAM_CONNECT_RETRY = 1`）。
  原先逐渠道尝试时，每个候选**只发一次**请求；候选列表里若只有一家供应商，
  等于**既无下一渠道可切、也没有任何重试**，一次上游抖动就让整个回合失败。
  - 实测依据：本机 `基元律动`（tokenrhythm.studio）502 占全量 **9.78%**
    （2026-09-20 当天 20%），错误为 `上游连接失败: error sending request for url (…)`；
    当时唯一救回来的是客户端自己的重试（Reasonix 2s 后同体重试即成功）。
    该模型在 `route_candidates` 中精确匹配 `model_name` 只命中 1 个渠道，
    且客户端用的是限定名 `基元律动/deepseek-flash`（JAI 会按供应商过滤候选），
    两条路都指向「单渠道、零重试」。
  - 重试范围**刻意收窄**：只重试 `send()` 阶段的连接类失败（建连/握手失败、
    **响应头到达前连接被重置**）。判据排除 `is_timeout()`（等待预算已花掉，重试等于翻倍）
    与 `is_builder()`（请求构造失败，重试必然再失败）；其余一律视为连接类 ——
    **有意不收窄到 `is_connect()`**：上游偶发在发响应头前 RST，reqwest 常把它归到
    request/body，只看 `is_connect()` 会漏掉真实场景。
  - 安全性：走到该分支意味着响应头都还没到、下游一个字节未收，重试不会造成重复输出；
    HTTP 层失败（含 5xx）**不重试** —— 链路已通，重试只会放大上游压力。
  - 两条发送点都已覆盖：直通 `try_candidate` 与转换 `try_converted_candidate`。
    顺带把两处请求组装收敛为闭包（reqwest 的 `RequestBuilder` 一次性，重试必须重建），
    并去掉转换路径里 `(url, (out, body))` 的多余嵌套与只为消警告的 `drop(url)`。
  - 验证：新增 `crates/gateway-core/tests/m11_connect_retry.rs`（3 用例）——
    黑洞上游（accept 后立刻断开）+ **建连计数**证明「同渠道重试恰好 1 次」（直通/转换各一条），
    外加反证「HTTP 500 不重试」（请求计数 == 1）。
    反证实测：把 `UPSTREAM_CONNECT_RETRY` 置 0 后两条重试用例精确变红、5xx 用例仍绿。
    全量回归 **357 通过 / 0 失败**（原 354，+3），`fmt --check`、`clippy -D warnings`、
    `tsc --noEmit`、`vite build` 全绿。
  - 附注（未改）：`route_candidates` + 限定名过滤导致的「限定名关掉故障转移」是另一个
    独立问题，本次**按用户要求不动**，仅登记。

## [0.2.9] - 2026-09-20

### Added
- **网关页「客户端接入」补「完整请求地址」与一键复制**：原先只提供 Base URL
  （`http://127.0.0.1:<端口>/v1`），而部分客户端把配置项当**精确请求地址**用、不会再补路径
  （典型：Reasonix 桌面版的「API 地址」即 `request_url`，其文档写明
  *“Reasonix does not append or rewrite its path”*）。把 Base URL 填进这类客户端，请求会打到
  `/v1` 上，JAI 无该路由 → **404**；而客户端报错只说
  `Request endpoint not found (HTTP 404). Check the API format and request address.`
  （该文案**不是** JAI 产生的，是 Reasonix 自己的模板），很难定位到「少填了后缀」。
  - 新增「完整请求地址」区块：Chat Completions / Anthropic Messages / Responses /
    模型列表 / MCP 元数据 共 5 条真实端点，每条独立复制按钮；端口跟随实际监听端口，
    被占用顺延时自动生效（非写死 1314）；
  - 顶部常驻操作条新增「复制完整地址」：一键复制接入清单（Base URL + API Key + 全部完整地址）；
  - **Base URL 复制字段保留**（两个功能并存），只填 Base URL 的客户端（OpenAI SDK、dsh 等）不受影响；
  - 卡片文案点明「精确请求地址（不会再补路径）」与填错会 404 的后果。
  - 验证：新增探针 `tools/visual-regression/gateway-endpoints.mjs`（17 项断言，含点复制后
    **读剪贴板逐条比对**，以及「每条都是完整端点而非裸 `/v1`」这个本次踩坑形态的防退化断言），
    1180×800 与 900×600 双尺寸 17/17 全绿；`tsc --noEmit` 与 `vite build` 通过；
    `fold.mjs` 复核网关页 `pageHScroll 0 / mainHScroll 0 / clippedTextCount 0`。
  - 附注：JAI 的 `/v1/responses` 本身可用（带 tools / streaming / reasoning 的 Agent 载荷实测 200，
    SSE 事件链完整），**本次未改 JAI 服务端**；同批发现 3 个缺口已登记待评估
    （流式 `response.completed.output` 恒空、无 `GET`/`DELETE /v1/responses/{id}`、
    `web_search` 工具静默忽略）。

## [0.2.8] - 2026-09-18

### Fixed
- **推理内容用「非 modeled 事件」下发，客户端拿不到 → 下轮上游 400
  `The reasoning_content in the thinking mode must be passed back to the API.`**（bug 26）：
  zcode 报 `Provider rejected the model request.`（JAI 只是把上游 tokenrhythm 的 400 原文透传）。
  - 根因：JAI 用 `response.reasoning_text.delta` 下发思考内容，而该事件**不在**严格客户端的
    modeled chunk 列表里（`@ai-sdk/openai` 只认 `response.reasoning_summary_part.added/done`
    与 `response.reasoning_summary_text.delta`）→ 推理文本被客户端**静默丢弃** → 客户端回传历史
    时无法带上 reasoning → thinking 模式的上游（deepseek/LiteLLM）拒绝。
  - 修复：改用 modeled 的 `reasoning_summary_part.added` + `reasoning_summary_text.delta`
    （收尾补 `reasoning_summary_part.done`），并让 reasoning item 的 `summary` 带上文本
    （流式 `output_item.done` 与非流式 body 都改；`content` 保留兼容既有客户端）。
  - 验证：用真实 `ai`+`@ai-sdk/openai` 回放真实流 → `reasoning-delta: 243`、推理文本 1011 字符
    被客户端捕获（修前为 0）；新增链路测试
    `reasoning_summary_item_becomes_reasoning_content_on_assistant_message` 断言
    客户端回传的 reasoning item 会被合并成上游 assistant 消息的 `reasoning_content` + `tool_calls`。
  - 附注：把用户失败的请求体原样重放两次都 200 → 该 400 属上游**非确定性**（部分后端实例强制校验），
    因此「客户端能拿到并回传 reasoning」是唯一可靠解法。
- **Responses 流式渲染 item id/状态错乱：zcode 每轮都报「Model request failed.」**（bug 25）：
  zcode 每次请求都已收到 `200 + text/event-stream`，却在流里拿到 error chunk，报
  `reason=unknown`（无 HTTP 状态），而 JAI 侧 `request_logs` 记的是 `200 + tool_calls=N`
  —— 看起来像上游抖动，实为自己渲染的流违反协议契约。客户端 cause 链给出铁证：
  `UnknownError: text part msg_resp_jai_942 not found`（errorPhase=stream）。
  - 「文本 item 已开」的标志被 `ToolCallStart` 复用 → **只调用工具、没有文本**的回复
    收尾时，`Finish` 会为一个从未登记的 `msg_*` 发 `output_text.done`/`content_part.done`
    → 客户端查不到该 text part → 整轮 turn 失败（错误恰好发生在流尾，与线上时间戳一致）；
  - `ToolCallArgsDelta`/`ToolCallEnd` 写死 `fc_{index}_pending`，与 `ToolCallStart`
    登记的 `fc_{index}_{call_id}` 不一致 → 参数增量引用不存在的 item；
  - 修复：状态拆为 `msg_started` + `active_tool_item_id`；文本增量按 `msg_started` 补发
    item/part 登记；工具增量/结束帧复用登记 id（`call_id` 从 id 反解）；纯工具调用不再发
    `msg_*` 收尾帧。回归 3 条单测（含反证：恢复旧行为即变红）+ 真机逐流 id 一致性校验。
  - **同族第三处**：工具调用**永远没有终结帧**。openai 族上游不产生「工具调用结束」事件
    （`openai.rs`：`Ev::ToolCallEnd => None`），而 `Finish` 只关 reasoning/文本 item，
    于是每条工具流都是 `output_item.added → …delta… → response.completed`，严格客户端只认
    `output_item.done` 终结工具调用 → 工具行永远关不掉，报
    `fault.runtime.toolLifecycleIncomplete`「Tool call ended without a terminal event.」；
    且终结 item 早期还传空 `name`/`arguments`。修复：`close_tool_call()` 在 `ToolCallEnd`、
    **`Finish`**、以及新 item 开始前补发带完整参数的 `function_call_arguments.done` +
    完整 `output_item.done`，并推进 `output_index` 防撞号。回归 2 条单测 + 真机逐流校验。
  - **第四处（真凶）**：`response.output_item.done` 的 `status` 是**严格客户端必填字段**
    （AI SDK zod schema：`z.enum(["in_progress","completed","incomplete"])`，无 `.nullish()`），
    而 JAI 从不发 `status` → 整帧校验失败被**静默丢弃** → SDK 永不产生 `tool-input-end`/`tool-call`
    （`chunkCounts` 只有 start/delta），工具行永远关不掉。已补 `status`
    （added=`in_progress`、done=`completed`），并对齐其它 item 形状：`custom_tool_call.input`
    转字符串、`apply_patch_call.operation` 保留对象、`shell_call` 带 `status`。
    新增 `scripts/verify_responses_with_ai_sdk.mjs`（用真实 `ai`+`@ai-sdk/openai` 回放 JAI 的流），
    修复后实测 `tool-call: 2`、name/input 正确、无 TypeValidationError。

- **`m4_conversion` 的 flood 护栏回归随机失败，会打红 CI**（bug 25 修复过程暴露）：
  `cargo test --workspace` 偶发 `conversion_disconnects_on_newline_flood_upstream`
  失败（实测 8 轮 4 红），但单跑该文件恒绿——典型的负载敏感 flake。
  - **现象会误导方向**：断言期望「已断开」护栏文案，实际拿到的是
    `stream aborted by upstream: ...` —— 看起来像上游流中断或护栏失效。
  - **根因（分诊取证，非推测）**：mock handler **不消费请求体** → hyper 在
    `poll_drain_or_close_read` 走 `_ => self.close_read()`（`hyper-1.11.0/src/proto/h1/conn.rs:849`）
    → 服务端 close 时接收缓冲区仍有客户端发来的未读字节 → 内核发 **RST 而非 FIN**
    → **客户端接收缓冲区里已到达但应用尚未读走的数据被一并丢弃**。
  - **证据链**（三步排除法，每步都是实测）：
    1. mock 侧显式打点确认 1056774 字节**全部写完**（排除「上游没发完」）；
    2. 网关侧读到的 `buf_len` 每次都不同（实测 685204 / 751158 / 908054 / 924126 / 997438），
       而 `content_length` 声明值一直是正确的 1056774 —— 排除「网关护栏逻辑错」，
       实为乱序/部分丢失；错误链给出
       `error decoding response body <- ... unexpected EOF during chunk size line`；
    3. **单变量验证**：只让 handler 消费请求体（`_req_body: axum::body::Bytes`）
       → 连跑 6 轮全绿（修前 8 轮 4 红）。
  - 负载相关性也由此解释：机器越忙 → 客户端读得越慢 → 接收缓冲区积压越多 → 被 RST 丢得越多。
  - **生产代码零改动**：真实上游会读请求体，这是**测试夹具缺陷**。两个 mock 都补
    `_req_body`，根因写进 `m4_conversion.rs` 注释防复发。
  - 另修 `clippy -D warnings` 门禁失败：bug 25 的 4 个回归测试里 `feed` 闭包无需
    `mut` 绑定（`unused_mut`）——该批改动当初未过 clippy 就提交了，发布门禁首次运行即暴露。


## [0.2.7] - 2026-09-18

### Fixed
- **网关自己发明的工具数上限（128）把上游能跑的请求拦成 400**（bug 24）：zcode 经 JAI 声明
  140 个工具 → `400 tools_limit_exceeded 工具声明数 140 超过上游上限 128`，整个 turn 失败；
  而**上游实测 140 / 300 个工具都返回 200** —— 128 是跨族能力对齐表里的经验值，并非任何上游的
  真实约束，等于误杀。与 bug 21 同源：网关不该**发明**上游限制，只能**执行上游声明的**限制。
  - 四族 `max_tools` 一律 `None`（删除 `DEFAULT_MAX_TOOLS`）⇒ 未声明即**不拦**，放行由上游裁决
    （上游真报错时其错误原文照常回给客户端）；
  - 新增 `providers/models.max_tools` **渠道声明**（迁移 0012，模型级覆盖供应商级；`NULL`/0 = 未声明），
    声明了超限仍 400，文案含实际个数与上限并提示可调整；
  - 规划层新增 `ChannelPolicy`（0011 effort + 0012 max_tools 统一为「渠道覆盖」），
    `plan_compatibility_with(req, caps, Option<&ChannelPolicy>)`；
  - **同族直通路径同样生效**（`capability::body_tool_count`），避免「声明了上限却被直通绕过」。
  - 回归：`max_tools_only_enforced_when_channel_declares_it`、`body_tool_count_reads_tools_array`；
    m9 新增 `undeclared_tool_cap_lets_140_tools_through`（bug 复刻：140 个完整透传）、
    `declared_tool_cap_applies_on_passthrough_path_too`，原 `too_many_tools_rejected` 改为
    `declared_tool_cap_rejects_overflow`（声明才拦）。
  - UI：供应商卡片「工具上限 …」+ 模型表「≤N / 上限?」芯片（留空 = 不拦）。

- **发布后 updater 通道仍推上一版**（v0.2.6 发布时实测）：`release.yml` 建草稿时硬编码
  `-F prerelease=true`，而 updater 端点取 `/releases/latest/download/latest.json`，
  **GitHub 的 latest 不含 prerelease** → 草稿转正式后 feed 仍返回上一版（实测返回 0.2.5），
  全程无报错，表现为「新版本发了但没人收到更新」。已改为 `-F prerelease=false`
  （草稿态本已由 `draft=true` 表达），并在 `docs/design/release.md` §5 新增发布后校验步骤
  （含 `gh release edit vX.Y.Z --prerelease=false --latest` 兜底）。见 bug 23。

## [0.2.6] - 2026-09-18

### Fixed
- **zcode 经 JAI 测试连接恒失败：「Provider rejected the model request.」**（真机 2026-09-18）：
  - 现象：zcode 自定义 Provider（`openai-responses` → `http://127.0.0.1:1314/v1`）选
    `基元律动/deepseek-flash`，测试连接与真实会话全部失败；报错文案是 zcode 把上游
    400/422 统一包装后的产物，看起来像「模型名不对」，**实际与模型名无关**。
  - 根因：zcode 发 `reasoning:{"effort":"none"}`（其 provider 未声明推理能力时按「无推理」发 none），
    而 JAI 的族级能力表把 `openai_compat` 的 reasoning 定为 `EffortMode::Native` ⇒
    **客户端值原样透传**；上游基元律动只认 `low/medium/high/xhigh/max` →
    400 `UNSUPPORTED_FIELD`（`request_logs` 里同一文案累计 53 次）。「客户端参数值 ∈ 上游值域」
    这一步此前无人负责：族级能力只回答「这一族支不支持原生 effort」，管不了「这家上游认哪些写法」。
  - 修复（迁移 0011 + `effort.rs`）：新增**供应商级/模型级「推理档位值域」声明**
    （`providers/models.reasoning_effort_levels`，模型级覆盖供应商级，`NULL`/空 = 未声明）。
    规划层 `plan_reasoning` 按值域归位：域内值原样透传；关闭语义（`none`/`off`/`disabled`）
    在域内无对应档位时**丢弃该参数**（不传 = 上游默认，而不是硬塞一个注定 400 的值）；
    其余未命中档位**收敛到域内最近档**（低于下限取最低、高于上限取最高）。
    `CompatibilityPlan::resolve` 首次真正改写 `params.reasoning_effort`（此前只收 WARN 不改写），
    各 encoder 零改动。**同族直通路径**同步归一（顶层 `reasoning_effort` 与嵌套 `reasoning.effort`
    两种形态），未声明值域的渠道整段短路、请求体逐字节不变。
  - 向后兼容：未声明值域的供应商行为不变（`m9_12` 断言 `none` 仍原样透传）。
  - 回归：`tests/m9_capability.rs` 新增 6 条（`m9_7` 复刻本次故障：`none` 应被丢弃且不回 400、
    `m9_8` minimal→low、`m9_9` 域内透传、`m9_10` 模型级覆盖供应商级、`m9_11` 直通路径归一、
    `m9_12` 未声明不干预）；`effort.rs` 13 条单测覆盖值域解析/归一/空壳 `reasoning` 摘除。
  - 真机复验：上游直连不带 `reasoning_effort` → 200、带 `"low"` → 200（即修复后 JAI 的两种出站形态）。
  - UI：供应商卡片「档位 …」+ 模型表「档位?」芯片（就地编辑、预设值域、清除即未声明，不新增表格列
    以免窄窗横向溢出回归）；每次丢弃/改写留一条 `CapabilityWarn` 结构化日志。
  - 文档：`docs/zcode接入.md` 补「模型名怎么填（必须正斜杠）」与「推理档位声明」两节，
    并把排查入口改为先看 JAI 日志页的 `error_summary`（不再从模型名上找原因）。

- **导入配置时 `openai_responses` 供应商被判为「未知协议族」**（顺手修）：`store/import.rs` 的
  family 白名单漏了 `openai_responses`（0003 起已是合法族，`providers.family` 的 CHECK 也允许它），
  导致该族供应商无法随 WebDAV / 导出配置同步到另一台机器。已补齐白名单。

## [0.2.5] - 2026-09-17

### Fixed
- **换台电脑后同步「从来没成功过」**（bug 19）：新机器第一次拉取会把自己刚打开的
  「自动拉取」关掉，此后再也收不到对方机器的更新。
  - 根因：`webdav_auto_*` 三个键（自动推送开关/间隔、自动拉取开关）被当作**共享配置**随
    meta 同步，而 `apply_import` 对白名单键是**无条件覆盖**。但它们是**本机调度偏好**：
    A 机出厂默认 `auto_pull_enabled=0` 就这样"旅行"到 B 机，把 B 机用户刚打开的开关
    在第一次拉取时自己关掉——一个自我否定的闭环。对称地还会把 B 机刻意关掉的自动推送翻回开，
    让新机器反过来覆盖远端。
  - 修复：三个键定为**本机调度偏好，双向不参与同步**。单一事实源
    `sync::MACHINE_LOCAL_META_KEYS`；导出侧剔除（顺带让「只升级一台机器」也安全），
    导入侧白名单 7 → 4 并保留纵深防御判断。
  - 回归：`tests/m7_import_webdav.rs` 新增 4 条（`pull_must_not_clobber_local_auto_switches`
    修复前必红、`export_omits_machine_local_switches`、`second_machine_pull_lands_data_and_keys`
    等），13 条全绿。排查中另验证并排除了 5 个假设（凭据不同步 / 不热加载 / 尾斜杠 / 远端数据 /
    认证协议），详见 `docs/bug和优化清单.md` 第 19 条。
  - 升级须知：需两台都升级；新机器仍须**先手工填一次 WebDAV 地址/账号/密码**（拉取的前提无法自举）。

## [0.2.4] - 2026-09-17

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