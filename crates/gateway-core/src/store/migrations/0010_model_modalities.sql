-- 0010: 模型级「输入/输出模态集合」—— docs/design/multimodal-support.md
-- 语义：NULL = 未知/未标注；值为规范序逗号串，取值 text|image|audio|video
--       （例：'text,image' = 文本+图像输入）。空串按未知处理（读取侧 parse 归一）。
-- 与 0009 的关系：0009 的 supports_multimodal 保留**只读**，降为派生回落来源
--       （新真相源是下面两列）；不 DROP 列，老库升级零损失。
-- 可空列 ADD COLUMN 在 SQLite 是 O(1) 元数据操作，不锁表、不重写行，升级平滑。
ALTER TABLE models ADD COLUMN input_modalities TEXT;
ALTER TABLE models ADD COLUMN output_modalities TEXT;
