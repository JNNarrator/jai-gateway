# 存储层设计：SQLite Schema 与密钥管理 — v1

> 状态：已评审定稿（2025-08）。本文档是数据层（`gateway-core::store`）的实现对照权威。
> 关联文档：《JAI — 桌面 AI API 网关.md》§数据持久化、[protocol-ir.md](protocol-ir.md)（Usage/StopReason/错误枚举的来源）。

## 1. 设计原则

1. SQLite 为唯一事实源；**密钥实体不入库**——上游供应商 Key 在系统密钥环，DB 只存引用地址
2. 代理热路径**零阻塞**：日志写入全程异步攒批，宁可丢一条统计不拖慢一次转发
3. 表结构直接服务二期图表（token / 耗时 / 错误率维度在 v1 就埋好）
4. 不引入 ORM 与迁移框架，保持桌面工具轻量底色

## 2. ER 总览

```
providers 1───N models            （ON DELETE CASCADE）
    │
    │ keyring_ref ───────────→  OS Keyring（密钥实体所在；DB 无法逆向取回）
    │
request_logs   无外键 —— 日志寿命 > 配置寿命
gateway_keys   独立表（v1 仅明文可回显方案，见 §6）
tool_id_map    独立表，TTL 滚动清理
meta           设置 KV
```

## 3. DDL

```sql
-- 连接初始化执行：
-- PRAGMA journal_mode = WAL; PRAGMA foreign_keys = ON; PRAGMA synchronous = NORMAL;

CREATE TABLE meta (
  key   TEXT PRIMARY KEY,
  value TEXT NOT NULL                        -- JSON 编码值
);

CREATE TABLE providers (
  id            TEXT PRIMARY KEY,            -- UUIDv7 字符串（时间有序）
  name          TEXT NOT NULL,
  base_url      TEXT NOT NULL,
  family        TEXT NOT NULL CHECK (family IN ('openai_compat','anthropic','gemini')),
  enabled       INTEGER NOT NULL DEFAULT 1,
  priority      INTEGER NOT NULL DEFAULT 100,-- 同名模型渠道路由顺序，小者先试
  extra_headers TEXT,                        -- JSON；OpenRouter 类站点扩展头透传
  keyring_ref   TEXT NOT NULL UNIQUE,        -- 形如 jai/provider/{uuid}
  last_ok_at    INTEGER,                     -- 健康徽标数据源（重启后仍在）
  last_err_at   INTEGER,
  last_err_msg  TEXT,                        -- 截断 300 字符
  created_at    INTEGER NOT NULL,
  updated_at    INTEGER NOT NULL
);

CREATE TABLE models (
  id                TEXT PRIMARY KEY,
  provider_id       TEXT NOT NULL REFERENCES providers(id) ON DELETE CASCADE,
  model_name        TEXT NOT NULL,           -- 路由键 = 客户端请求中的 model
  upstream_model_id TEXT,                    -- 上游真实 id；NULL ⇒ 同名（二期别名映射预留位）
  context_window    INTEGER,                 -- NULL ⇒ 快照值或保守默认 128k
  max_output_tokens INTEGER NOT NULL DEFAULT 4096,
  enabled           INTEGER NOT NULL DEFAULT 1,
  UNIQUE (provider_id, model_name)
);
CREATE INDEX idx_models_route ON models(model_name, enabled);
CREATE INDEX idx_models_prov  ON models(provider_id);

CREATE TABLE gateway_keys (
  id           TEXT PRIMARY KEY,
  key          TEXT NOT NULL UNIQUE,         -- 明文存储（2025-08 已拍板，理由与缓解见 §6）
  prefix       TEXT NOT NULL,                -- 如 sk-jai-aB12cd → UI 常态仅显示前缀
  label        TEXT,
  created_at   INTEGER NOT NULL,
  revoked_at   INTEGER,                      -- 非空即吊销；认证查询排除
  last_used_at INTEGER
);

CREATE TABLE request_logs (
  id                INTEGER PRIMARY KEY,     -- rowid 自增，优于 UUID
  ts                INTEGER NOT NULL,        -- unix ms
  inbound_family    TEXT NOT NULL,           -- 'openai' | 'anthropic'
  route_mode        TEXT NOT NULL CHECK (route_mode IN ('passthrough','converted')),
  model_name        TEXT NOT NULL,
  provider_id       TEXT,                    -- 刻意不设外键，允许悬空
  upstream_model_id TEXT,                    -- 多渠道顺延时记录最终命中的渠道模型 id
  http_status       INTEGER NOT NULL,        -- 返回给客户端的状态码
  stop_reason       TEXT,                    -- IR StopReason 枚举名
  usage_input       INTEGER,
  usage_output      INTEGER,
  usage_cache_read  INTEGER,                 -- 直通旁路扫描同样能取到这四个数
  usage_cache_write INTEGER,
  duration_ms       INTEGER NOT NULL,
  is_stream         INTEGER NOT NULL DEFAULT 0,
  tool_calls        INTEGER NOT NULL DEFAULT 0,  -- assistant 发起的调用次数
  error_kind        TEXT,                    -- IR 错误 kind 枚举名（协议 IR 文档 §6）
  error_summary     TEXT                     -- 截断 300 字符；永不含 prompt/响应明文
);
CREATE INDEX idx_logs_ts       ON request_logs(ts DESC);
CREATE INDEX idx_logs_model_ts ON request_logs(model_name, ts DESC);

CREATE TABLE tool_id_map (                   -- 协议 IR §5-B 超长 id 的兜底映射
  outbound_id  TEXT PRIMARY KEY,
  canonical_id TEXT NOT NULL,
  created_at   INTEGER NOT NULL,
  expires_at   INTEGER NOT NULL              -- 7 天滚动过期
);
```

### 路由查询（多渠道按序顺延即一条 SQL）

```sql
SELECT p.id, p.base_url, p.family, p.extra_headers, p.keyring_ref,
       m.upstream_model_id, m.max_output_tokens, m.context_window
FROM models m JOIN providers p ON p.id = m.provider_id
WHERE m.model_name = ?1 AND m.enabled = 1 AND p.enabled = 1
ORDER BY p.priority ASC, m.rowid ASC;
```

执行器按结果序逐渠道尝试，失败换下一行；全部失败则返回最后一个错误。`GET /v1/models` 对上述结果按 `model_name` 去重输出。

## 4. 密钥环集成（上游供应商 Key）

| 环节 | 行为 |
| --- | --- |
| service / account | `service = "JAI"`，`account = "provider/{provider_uuid}"` |
| 新建供应商 | 生成随机 32 字节 → `keyring.set` → INSERT 行；INSERT 失败则回滚删除条目 |
| 更新 Key | 原 ref 原地覆盖写（ref 与 provider UUID 绑定不变） |
| 删除供应商 | DELETE 行（CASCADE models）→ 尽力 `keyring.delete` |
| 读失败降级 | 转发时按 `ProviderOther` 错误返回 + 供应商标记不健康（写 `last_err_*`），UI 提示重开设置确认凭据 |
| 启动探测 | 首次启动做 set/get/delete 三连探测，密钥环不可用的环境在**添加供应商之前**即拦截 |

已知限制：keyring crate 的全量枚举各平台支持不一，无法可靠做「孤儿凭据清扫」；接受以命名约定（`jai/provider/*`）作为人工清理依据。

## 5. 写路径与常驻任务

```
Axum handler ──(mpsc::channel)──→ logger 任务 ──≥64 行或 ≥500ms 攒批──→ 批量 INSERT
                                      │
UI 读取 ── tokio_rusqlite 只读调用 ──→ 单一连接（规模小，无需连接池）

保活 timer（每日一次）:
  - request_logs: 删除 ts < now-30d，且总量超 50_000 行时裁剪最旧
  - tool_id_map : 删除 expires_at < now
  - 两者完成后 PRAGMA incremental_vacuum（可选）
```

- 保留策略默认值（2025-08 拍板）：**开启记录，30 天且 5 万行封顶**，设置页可整体关闭记录开关。
- 批处理通道有界（如 1024）：日志洪峰时丢最旧事件并计数告警，绝不反压 HTTP 层。

## 6. 网关密钥（sk-jai-*）：明文方案的定案与缓解

已拍板：**明文存储、UI 可回显**。动机是多设备迁移便利，优先于「SQLite 零秘密」洁癖。

必须落实的三项缓解：

1. **常态脱敏**：列表页只展示前缀（`sk-jai-aB12…`）；全文仅在生成时弹窗一次性展示 + 手动点「显示」时可见；
2. **一键轮换**：UI 提供 regenerate 按钮（吊销旧 key 插入新行，而非 UPDATE，保留审计痕迹）；
3. **导出/同步继续排除** gateway_keys 全表内容。

远期可选加固（记入未决项，MVP 不做）：SQLCipher 整库加密或对该列做 OS 级加密（DPAPI/Keychain 包裹）作为设置页可选开关。

## 7. 迁移机制

- 迁移以嵌入式 SQL 数组表达：`const MIGRATIONS: &[(&str /*name*/, &str /*sql*/)]`，SQL 经 `include_str!` 内联；
- 应用侧用 `PRAGMA user_version` 记录当前版本，逐版本事务内执行并推进版本号；
- 不引入 refinery/diesel_migrations 等外部依赖；版本数到达两位数再评估是否迁移工具化。

## 8. 导出 JSON 映射（「配置透明」的落地）

| 来源 | 是否导出 | 说明 |
| --- | --- | --- |
| providers 除 `keyring_ref` 外全部字段 | ✅ | `extra_headers` 一并导出 |
| providers.keyring_ref | ❌ | 引用地址无跨机意义 |
| models 全部字段 | ✅ | |
| gateway_keys 全表 | ❌ | 见 §6-3 |
| request_logs / tool_id_map | ❌ | 属运行数据非配置 |
| meta 设置 KV | ✅ | WebDAV 同步复用同一构建器 |

导入：按 provider 名称+base_url 去重合并（upsert），导入后所有供应商处于「待录入密钥」状态（无 keyring 条目），UI 逐个引导补录。
> 调度注记（2025-08）：导入已提前至公测前，随路线图 **M7** 交付（与 WebDAV 同批）；本节先固化其语义，M1–M6 阶段仅交付导出。

## 9. 未决 / 远期项

| # | 事项 | 说明 |
| --- | --- | --- |
| 1 | 整库或敏感列加密（SQLCipher / DPAPI 包裹列） | 明文网关密钥决策的远期对冲，设置页可选开关形态 |
| 2 | request_logs 分区按月归档导出 CSV | 二期用量统计需要时再做 |
| 3 | tool_id_map 内存 LRU 前置层 | 当前直查 SQLite 足够，命中率为零成本项 |
