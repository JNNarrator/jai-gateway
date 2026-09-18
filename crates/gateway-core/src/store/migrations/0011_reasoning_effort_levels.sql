-- 0011: 推理档位（reasoning effort）值域声明 —— crates/gateway-core/src/effort.rs
--
-- 背景（2026-09-18 真机故障）：zcode 的 Responses 请求带 reasoning.effort=none，
-- JAI 跨族转换把 reasoning_effort:"none" 原样透传给基元律动（只认 low/medium/high/
-- xhigh/max）→ 上游 400 UNSUPPORTED_FIELD，客户端只看到被包装过的
-- "Provider rejected the model request."。此前无人负责「客户端参数值 ∈ 上游值域」。
--
-- 语义：NULL/空串 = 未声明 ⇒ 不干预、原样透传（向后兼容：上游自己认就认）；
--       值为**声明序**逗号串（例 'low,medium,high,xhigh,max'），首项 = 自定义档位的默认档。
-- 两级：models 覆盖 providers（模型级未声明时继承供应商级，见 store::route_candidates 的
--       COALESCE）；同一供应商下不同模型档位不同时按模型行声明。
--
-- 可空列 ADD COLUMN 在 SQLite 是 O(1) 元数据操作，不锁表、不重写行，升级平滑。
ALTER TABLE models ADD COLUMN reasoning_effort_levels TEXT;
ALTER TABLE providers ADD COLUMN reasoning_effort_levels TEXT;
