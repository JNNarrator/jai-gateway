-- 0012: 工具声明数上限（max_tools）改为「按渠道声明」—— crates/gateway-core/src/codec/capability.rs
--
-- 背景（2026-09-18 真机故障）：zcode 声明 140 个工具，JAI 的跨族能力表把 max_tools 硬编码为
-- 128（protocol-ir §能力对齐表当年取的经验值），plan_tools 超限直接 Rejected →
-- 400 tools_limit_exceeded，而**上游实测 140 / 300 个工具都返回 200**：
-- 这个默认值是 JAI 自己发明的，误杀了上游完全能跑的配置。
--
-- 新语义：
--   NULL / 0 = 未声明 ⇒ **不拦**（放行，由上游裁决；上游真有限制时其错误原文会照常回给客户端）
--   N > 0    = 该渠道上限，超限即 400 `tools_limit_exceeded`（文案给出数字与调整指引）
-- 两级：models 覆盖 providers（模型级未声明时继承供应商级，见 store::route_candidates 的
--       COALESCE）。「谁有上限」从此是可配的事实，而不是网关的臆测。
--
-- 可空列 ADD COLUMN 在 SQLite 是 O(1) 元数据操作，不锁表、不重写行，升级平滑。
ALTER TABLE providers ADD COLUMN max_tools INTEGER;
ALTER TABLE models ADD COLUMN max_tools INTEGER;
