# 多模态（输入/输出模态集合）支持方案 — 模型能力标注与配套

> 状态：**已实施**（2026-09-11）。本文自「0009 单一 vision 布尔」升级为
> **0010 输入/输出模态集合**的定稿口径，并记录派生规则与兼容边界。
> 设计原则贯穿全文：**只增不删、未知即 NULL、不臆断**。

---

## 0. 背景与现状核验（已读代码）

| 层 | 现状 | 关键位置 |
| --- | --- | --- |
| 协议 IR | `Block::Image` 已定义（media_type / data_base64 / url），openai 解码支持 `image_url`/base64，编码支持还原 | `codec/ir.rs`、`codec/openai.rs` |
| 能力声明 | `Capabilities` 六面（parameters/tools/tool_choice/response_formats/reasoning/streaming），无 image 维度；多模态是**模型级**差异，不入族级表 | `codec/capability.rs` |
| 模型存储 | `models` 表新增 `input_modalities` / `output_modalities`（TEXT 逗号串，NULL=未知）；0009 的 `supports_multimodal` 保留可读、降为派生回落 | `store/migrations/0010_model_modalities.sql` |
| 模态枚举 | `Modality = text \| image \| audio \| video`，规范序编码、大小写归一、非法 token 忽略 | `modality.rs` |
| 模型发现 | `discover` 解析 Gemini `inputModalities`、OpenRouter `architecture`、openai_compat 旧 vision 键 | `discover.rs` |
| 出站 `/v1/models` | `models_list` 追加 `inputModalities` / `outputModalities`（null=未知） | `server/proxy.rs` |
| Tauri 命令 | `model_set_modalities`（取代 `model_set_multimodal`）；`model_list` 透出三字段 | `src-tauri/src/main.rs` |
| UI | `ModelsPage` 模态列：入/出双维多选 + 清除回未知 | `ui/src/pages/ModelsPage.tsx` |

**核心判断**：图片**内容链路**早已打通（跨族转换都能处理 Image 块）；本方案补的是
「**这个模型到底吃什么、吐什么**」这一**模型级能力元数据**，让客户端与网关不必猜。

---

## 1. 字段与语义

| 字段 | 形态 | 语义 |
| --- | --- | --- |
| `input_modalities` | TEXT，规范序逗号串 | 输入模态集合；`NULL` = 未知/未标注 |
| `output_modalities` | TEXT，规范序逗号串 | 输出模态集合；`NULL` = 未知/未标注 |
| `supports_multimodal` | INTEGER 0/1/NULL（0009 旧列） | **只读回落源**：无集合时才生效 |

派生规则（唯一真相源在 `modality::derive_supports_multimodal`）：

```
input_modalities 有值  → supportsMultimodal = 是否含 image
input_modalities = NULL → supportsMultimodal = 旧 0009 列（老数据可读）
两者皆无                → NULL（未知）
```

**为什么集合一旦存在就压过旧列**：否则 UI 清除标注后，旧列的 `1` 会变成
「幽灵 true」。因此 `model_set_modalities` 在写集合的同时把旧列置 `NULL`。

---

## 2. 全链路改动清单（已落地）

| # | 层 | 文件 | 改动 |
| --- | --- | --- | --- |
| 1 | 模块 | `modality.rs` | 枚举 + `encode`/`parse`/`parse_opt`/`from_json_array`/`supports_image`/`derive_supports_multimodal` |
| 2 | DB | `store/migrations/0010_model_modalities.sql` + `migrations.rs` | 两个可空 TEXT 列，注册 `0010_model_modalities` |
| 3 | Store | `store/mod.rs` | `ModelRow` 增两字段；`MODEL_COLS`/`row_to_model`/`model_upsert` 改造；`model_set_modalities` |
| 4 | 发现 | `discover.rs`、`src-tauri/src/main.rs` | `DiscoveredModel` 带两集合；解析规则见 §3 |
| 5 | 出站 | `server/proxy.rs:models_list` | SELECT 增列；JSON 追加 `inputModalities`/`outputModalities` |
| 6 | 同步 | `store/import.rs`（导出随 `ModelRow` 自动携带） | 读集合；缺省回落旧布尔 |
| 7 | 命令/UI | `src-tauri/src/main.rs`、`ui/src/{api,types}.ts`、`ModelsPage.tsx` | `model_set_modalities` + 入/出双维编辑器 |
| 8 | 测试 | `crates/gateway-core/tests/modalities.rs` | 9 例：enum_/store_/discover_/proxy_/sync_ 五组 |

数据流：

```
上游 /models 模态字段 ──发现(discover)──▶ input/output_modalities
                                             │
              UI 标注 ──model_set_modalities─▶ ▲（用户优先，可清除回 NULL）
                                             ▼
                 GET /v1/models 出站 ──▶ inputModalities/outputModalities + 派生布尔
```

---

## 3. 上游各家族模态字段解析（字段名不统一）

| 家族 | 端点 | 字段 | 解析要点 |
| --- | --- | --- | --- |
| openai_compat / openai_responses | `GET {base}/models` | OpenRouter 风格 `architecture.input_modalities`/`output_modalities`；扁平 `input_modalities`/`inputModalities`；旧 `supports_vision`/`multimodal`/`vision` | 按可信度递降取第一个可解析者；布尔 `true` ⇒ 文本+图像，`false` ⇒ 仅文本 |
| anthropic | `GET {base}/v1/models` | 官方无模态标志 | 一律 `NULL`（未知），不臆断 |
| gemini | `GET {base}/v1beta/models` | `inputModalities` / `outputModalities`（大写数组） | 逐 token 归一（`TEXT`/`IMAGE`/`AUDIO`/`VIDEO`） |

**规则**：取到就映射；**完全取不到 → `NULL`（未知）**；不做基于模型名的启发式
猜想（`gpt-4o`、`qwen-vl` 之类一律不猜——猜错比未知更贵）。

---

## 4. 兼容性与数据语义

- `NULL`：未知（默认大多如此）。
- 逗号串：明确标注（发现解析或用户手动标注）；空串按未知处理。
- 出站 `/v1/models`：`inputModalities` / `outputModalities` 为**只增字段**，
  `supportsMultimodal` 与 `contextWindow` 语义未变——旧客户端（dsh 等）不受影响。
- 老快照导入：只有 `supportsMultimodal` 的导出文件仍可导入，按
  `true ⇒ 文本+图像`、`false ⇒ 仅文本` 回填集合。
- 迁移 `0010` 为可空列 `ADD COLUMN`，SQLite 下是 O(1) 元数据操作，**不锁表、
  不重写行**；0009 列**不 DROP**，老库升级零损失。

---

## 5. 开放问题（不阻塞已落地部分）

1. **能力声明层**：`Capabilities` 是族级静态表，多模态是模型级。是否引入
   `ModelCapabilities` 承载模型级能力？建议后续单独立项。
2. **是否按能力拦截含图/含音请求**：本方案**当前不做拦截**——默认透传给上游，
   行为策略（4xx 拒绝 / 降级提示 / 告警）留给策略层另立章程。
3. **启发式定标**：维持不采纳（见 §3 规则）。
4. UX 加分项：拉取模型后提示「发现 N 个多模态模型」，可选。

---

## 6. 影响面小结

- **Schema**：1 个新迁移（两个可空列，向后兼容）。
- **Rust**：新模块 `modality` + store/discover/proxy/import + 1 个 Tauri 命令。
- **UI**：模态列（入/出双维多选 + 清除回未知）。
- **不触**：codec 图片转换、能力规划决策、路由。
- **风险**：低（纯新增可空字段 + 出站只增字段）。
- **验收**：`cargo test -p gateway-core --test modalities`（9 例）+ 既有
  `multimodal_image`（5 例，图片链路回归护栏）。
