// 校验多模态方案文档覆盖到「0010 模态集合」口径：五层 + 兼容 + 开放问题 + 派生规则。
// 判据只收紧不放宽：旧 key 一条不删，新口径逐条加。零壳直跑。
import fs from "node:fs";
const P = "/Users/jiangnan/Documents/workspace/JAI/docs/design/multimodal-support.md";
if (!fs.existsSync(P)) {
  console.error("FAIL: 未找到方案文档 " + P);
  process.exit(1);
}
const s = fs.readFileSync(P, "utf8");
// 0009 旧口径（保留——老字段仍在文档中作为回落源出现）
const legacy = [
  "supports_multimodal", "0009", "models_list", "ModelsPage", "discover",
  "inputModalities", "openai_compat", "anthropic", "NULL", "开放问题", "兼容", "只增",
];
// 0010 新口径（本单增量：字段名 / 迁移号 / 出站键 / 上游来源 / 命令 / 派生规则）
const modalities = [
  "input_modalities", "output_modalities", "0010", "outputModalities",
  "architecture", "model_set_modalities", "派生", "幽灵 true",
];
const missing = [...legacy, ...modalities].filter((k) => !s.includes(k));
if (missing.length) {
  console.error("FAIL: 缺关键内容 " + missing.join(","));
  process.exit(1);
}
console.log(
  `OK doc_multimodal_exists=1 五层覆盖=1 旧口径=${legacy.length} 模态集合口径=${modalities.length}`
);
