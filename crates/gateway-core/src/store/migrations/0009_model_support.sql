-- 0009: 模型级多模态（vision）能力标注 —— docs/design/multimodal-support.md §2-1
-- 语义：NULL = 未知/未标注（区别于 0 = 明确不支持，未知即保守）；
--       1 = 支持图像输入；0 = 明确不支持。
-- 可空列 ADD COLUMN 在 SQLite 是 O(1) 元数据操作，不锁表、不重写行，升级平滑。
ALTER TABLE models ADD COLUMN supports_multimodal INTEGER;
