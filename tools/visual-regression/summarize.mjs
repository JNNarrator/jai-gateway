// 汇总 walk 探针结果：列出所有「丢失/被遮挡/弹窗溢出」的步骤与元素明细
import fs from "node:fs";

const file = process.argv[2] || ".vr/out-walk-980x640.json";
const j = JSON.parse(fs.readFileSync(file, "utf8"));
const lines = [];
let badSteps = 0;
const dlgMax = new Map();

for (const s of j.steps) {
  const p = s.probe;
  if (!p || p.probeError) continue;
  const unreach = p.unreachable || [];
  const covered = p.covered || [];
  const dlgs = (p.dialogs || []).map((d) => ({ ...d, overflow: d.overflowTop > 1 || d.overflowBottom > 1 }));
  for (const d of p.dialogs || []) {
    const key = d.title || d.slot || "?";
    const cur = dlgMax.get(key) || { h: 0, overflowTop: 0, overflowBottom: 0, unreach: 0, step: "" };
    if ((d.rect?.h || 0) > cur.h) dlgMax.set(key, { h: d.rect.h, overflowTop: d.overflowTop, overflowBottom: d.overflowBottom, unreach: d.unreachableCount, step: s.label, overflowY: d.overflowY, maxHeight: d.maxHeight, interactive: d.interactive, unreachable: d.unreachable.map((u) => u.text || u.tag) });
  }
  if (unreach.length || covered.length || dlgs.some((d) => d.overflow || d.unreachableCount > 0)) {
    badSteps++;
    lines.push(`\n### ${s.idx} ${s.label}  (shot: ${s.shot})`);
    for (const u of unreach) lines.push(`  · 视口外且滚不到: <${u.tag}> "${u.text}"  rect=${JSON.stringify(u.rect)} out=${JSON.stringify(u.out)} dlg=${u.inDialog} path=${u.path}`);
    for (const c of covered) lines.push(`  · 被遮挡点不到: <${c.tag}> "${c.text}"  ← 盖住者 <${c.hitBy.tag}> "${c.hitBy.text}" z=${c.hitBy.z} pos=${c.hitBy.pos} path=${c.path}`);
    for (const d of dlgs) if (d.overflow || d.unreachableCount) lines.push(`  · 弹窗: "${d.title}" h=${d.rect.h} top=${d.rect.y} 超出上=${d.overflowTop} 超出下=${d.overflowBottom} overflowY=${d.overflowY} maxHeight=${d.maxHeight} 自滚=${d.selfScrollable} 控件=${d.interactive} 不可达=${d.unreachableCount} [${d.unreachable.map((u) => u.text || u.tag).join(", ")}]`);
  }
}

console.log(`总步数 ${j.steps.length}，问题步 ${badSteps}，视口 ${j.vw}x${j.vh}`);
console.log("\n## 弹窗高度排行（该弹窗在遍历中出现的最大高度 / 视口 " + j.vh + "）");
for (const [k, v] of [...dlgMax].sort((a, b) => b[1].h - a[1].h).slice(0, 24)) {
  console.log(`  ${String(v.h).padStart(4)}px  超上${String(v.overflowTop).padStart(3)} 超下${String(v.overflowBottom).padStart(3)} 不可达${v.unreach}  overflowY=${v.overflowY} maxH=${v.maxHeight}  「${k}」 ← ${v.step}`);
}
console.log("\n## 问题明细");
console.log(lines.join("\n"));
