-- 0004_advanced_routing — 高级路由：供应商权重（加权负载均衡）
-- 说明：priority 仍决定主备大序；同 priority 内按 weight 加权随机打散。
ALTER TABLE providers ADD COLUMN weight INTEGER NOT NULL DEFAULT 1;
