-- 0013: 网关密钥的白/黑名单（D9-T6b）。
--
-- 背景（缺口④）：多密钥（D9-T6a）之后「一把密钥发给一个客户端」是常态，但每把
-- 密钥能碰到的**渠道 / 模型**完全一样 —— 于是「给 CI 流水线的那把只许用便宜模型」
-- 「给某人的那把不许碰某家上游」这类事只能靠客户端自觉。本迁移让密钥自带规则。
--
-- 语义（收敛口径，简单可解释 —— 实现见 store::keyrules::KeyRules）：
--   deny 命中  → 不可用（**优先于 allow**）
--   allow 非空 → 只允许列表内的
--   两者都为空 → 不限制（向后兼容：没配规则的密钥行为与改造前**完全一致**）
--
-- 两条轴相互独立、同时生效：
--   渠道规则用 provider_id（渠道有稳定 UUID）；模型规则用 model_name ——
--   「同一个模型名挂在多个渠道上」是常态，规则要能一次覆盖全部渠道。
--   一个候选必须两条轴都通过才可用。
--
-- 外键取舍：
--   key_provider_rules.provider_id → providers(id) ON DELETE CASCADE：
--     删渠道时把它的规则行一起带走，否则会攒下一堆悬空行。注意 provider_delete
--     是**硬删**（DELETE FROM providers），没有 CASCADE 这里会直接报外键冲突。
--   key_id **刻意不设外键**（与 request_logs.provider_id 同一取舍）：密钥只有软删
--     （revoked_at），行永远在；而规则行悬空本身无害（该 key_id 不再被查到）。
--   model_name 也刻意不设外键：它对应的不是主键（models 的主键是 id）。
CREATE TABLE key_provider_rules (
  key_id      TEXT NOT NULL,
  provider_id TEXT NOT NULL REFERENCES providers(id) ON DELETE CASCADE,
  mode        TEXT NOT NULL CHECK (mode IN ('allow','deny')),
  PRIMARY KEY (key_id, provider_id)
);

CREATE TABLE key_model_rules (
  key_id     TEXT NOT NULL,
  model_name TEXT NOT NULL,
  mode       TEXT NOT NULL CHECK (mode IN ('allow','deny')),
  PRIMARY KEY (key_id, model_name)
);