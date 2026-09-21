// UI 静态规范检查（零依赖，只读源码文本）。
//
// 存在意义：本项目的 UI 规范此前**只写在文档里**（docs/ui优化.md、docs/视觉回归整改plan.md），
// 所以每次加新功能都会静默回退 —— 实例：bug 21/24 引入的 0011/0012 两个编辑器
// 又写出了 4 处 `text-[10px]`（P2-10 曾把全项目清零），以及只靠伪元素外扩的命中区。
// 这个脚本把这些「可被文本判定」的规范变成一条命令，接进 release_check.sh。
//
// 覆盖：
//   1. 字号 < 11px（`text-[10px]` 之类；P2-10 的结论：最小字号 11px）
//   2. 图标按钮缺可访问名（无 aria-label / title，且内部只有图标）
//   3. 列表 key 用下标（`key={i}` / `key={index}`）—— React 列表反模式
//   4. 命中区只靠伪元素外扩的图标按钮（`after:-inset-*` 且自身盒子 < 24px）
//      —— 实测会被相邻元素抢走热区（probe-hits 逐点探测），故要求「真实盒子 ≥24px」
//
// 用法：node scripts/ui_lint.mjs        # 仓库根目录或任意目录
// 退出码：0 = 全过；1 = 有违规
import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const HERE = path.dirname(fileURLToPath(import.meta.url));
const SRC = path.join(HERE, "..", "ui", "src");

function walk(dir, out = []) {
  for (const e of fs.readdirSync(dir, { withFileTypes: true })) {
    const p = path.join(dir, e.name);
    if (e.isDirectory()) walk(p, out);
    else if (/\.(tsx?|css)$/.test(e.name)) out.push(p);
  }
  return out;
}

const files = walk(SRC);
const violations = [];
const add = (rule, file, line, text, why) =>
  violations.push({ rule, file: path.relative(SRC, file), line, text: text.trim().slice(0, 110), why });

// 1) 字号 < 11px
const TINY = /text-\[(\d+(?:\.\d+)?)px\]/g;
const TINY_STYLE = /fontSize:\s*(\d+(?:\.\d+)?)/g;
// 2) 图标按钮缺可访问名
const ICON_ONLY = /^<[A-Z][A-Za-z0-9]*\s*\/>$/;
// 3) 列表 key 用下标。只认「下标语义」的变量名（i/idx/index）——`key={k}` 这种
//    用稳定 id 作 key 的写法不该被误报（曾误报 SidebarNav 的 tab id）。
//    另：**固定条数的骨架占位**（`<Skeleton>`）用下标是正确写法（无数据身份、不会重排），
//    单独放行，避免把规则变成噪音。数据驱动列表一律不许用下标。
const BAD_KEY = /\bkey=\{(?:i|idx|index)\}/;
const SKELETON_KEY = /<Skeleton\s+key=/;
// 4) 只靠伪元素外扩的图标按钮
const PSEUDO_HIT = /after:(?:absolute\s+)?after:-inset/;

for (const f of files) {
  const lines = fs.readFileSync(f, "utf8").split("\n");
  const src = lines.join("\n");

  lines.forEach((ln, i) => {
    for (const m of ln.matchAll(TINY)) {
      if (Number(m[1]) < 11) add("字号<11px", f, i + 1, ln, `text-[${m[1]}px]：最小字号应为 11px`);
    }
    for (const m of ln.matchAll(TINY_STYLE)) {
      if (Number(m[1]) < 11) add("字号<11px", f, i + 1, ln, `fontSize:${m[1]}：最小字号应为 11px`);
    }
    if (BAD_KEY.test(ln) && !SKELETON_KEY.test(ln)) {
      add("key用下标", f, i + 1, ln, "数据驱动列表的 key 用下标会在增删/排序时错位，改用稳定 id（骨架占位除外）");
    }
  });

  // 2) 逐个 <button ...> 取开标签 + 内部内容，判断「只有图标且无可访问名」
  const btnRe = /<button\b([^>]*?)(\/>|>)([\s\S]*?)<\/button>/g;
  for (const m of src.matchAll(btnRe)) {
    const attrs = m[1];
    const inner = m[3] || "";
    const lineNo = src.slice(0, m.index).split("\n").length;
    const hasName = /aria-label\s*=|title\s*=/.test(attrs);
    const textOnly = inner
      .replace(/\{[^}]*\}/g, "")
      .replace(/<[^>]+>/g, "")
      .replace(/\s+/g, " ")
      .trim();
    const hasIcon = /<[A-Z][A-Za-z0-9]*\b/.test(inner);
    if (!hasName && hasIcon && !textOnly) {
      add("图标按钮无可访问名", f, lineNo, m[0].split("\n")[0], "只有图标且没有 aria-label/title，读屏与 hover 都拿不到名字");
    }
    // 4) 该按钮的 className 里只靠伪元素外扩，且没有把真实盒子做到 24px
    const cls = (attrs.match(/className=\{?["'`]([^"'`]*)["'`]/) || [])[1] || "";
    const realBox = /(?:^|\s)(?:size-6|size-7|h-6|h-7|min-h-6|py-1|py-1\.5|p-1)\b/.test(cls);
    if (PSEUDO_HIT.test(cls) && !realBox) {
      add("命中区只靠伪元素", f, lineNo, m[0].split("\n")[0], "只用 after:-inset 外扩命中区会被相邻元素抢走（实测有效命中区可低到 6×30）；把真实盒子做到 ≥24px 或加 z-10");
    }
  }
}

const byRule = {};
for (const v of violations) (byRule[v.rule] ||= []).push(v);

console.log(`# ui_lint —— UI 静态规范检查（${files.length} 个源文件）\n`);
if (!violations.length) {
  console.log("✓ 无违规");
} else {
  for (const [rule, list] of Object.entries(byRule)) {
    console.log(`## ${rule}  共 ${list.length} 处`);
    for (const v of list.slice(0, 25)) console.log(`  ${v.file}:${v.line}  ${v.why}\n      ${v.text}`);
    if (list.length > 25) console.log(`  …另有 ${list.length - 25} 处`);
    console.log();
  }
  console.log(`✗ 共 ${violations.length} 处违规`);
}
process.exit(violations.length ? 1 : 0);
