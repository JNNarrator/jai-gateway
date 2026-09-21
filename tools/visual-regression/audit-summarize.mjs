// 汇总 audit JSON：跨步骤去重，输出对比度/字号/命中区/截断问题的可执行清单
// node tools/visual-regression/audit-summarize.mjs .vr/audit-dark-1180x800.json
import fs from "node:fs";
const file = process.argv[2];
const j = JSON.parse(fs.readFileSync(file, "utf8"));
const byKey = new Map();
const dlgByTitle = new Map();
for (const s of j.steps) {
  const p = s.ext;
  if (!p) continue;
  const tab = (s.label.match(/^tab:(\w+)/) || [])[1] || (s.label.match(/:(\w+):/) || [])[1] || "?";
  for (const c of p.contrast || []) {
    const k = `${c.path}|${c.size}|${c.text.slice(0, 10)}`;
    const cur = byKey.get(k);
    if (!cur || c.worst < cur.worst) byKey.set(k, { ...c, tab, label: s.label });
  }
  for (const c of p.tiny || []) {
    const k = `T|${c.path}|${c.size}`;
    if (!byKey.has(k)) byKey.set(k, { kind: "tiny", ...c, tab });
  }
  // smallTargets 已从 audit 移除（命中区判据唯一归属 probe-hits.mjs，见 audit.mjs 内注释）
  for (const c of p.clippedRows || []) {
    const k = `C|${c.container}|${c.child}`;
    if (!byKey.has(k)) byKey.set(k, { kind: "clipRow", ...c, tab });
  }
  for (const c of p.hScroll || []) {
    const k = `H|${c.path}`;
    if (!byKey.has(k)) byKey.set(k, { kind: "hscroll", ...c, tab });
  }
  for (const d of p.dialogs || []) {
    const key = d.title || d.slot;
    const cur = dlgByTitle.get(key);
    if (!cur || d.h > cur.h) dlgByTitle.set(key, { ...d, label: s.label, tab });
  }
  if ((p.toasts || []).length) byKey.set("TOAST", { kind: "toast", rects: p.toasts.map((t) => t.rect), z: p.toasts[0].z, label: s.label });
}

const contrast = [...byKey.values()].filter((x) => !x.kind && typeof x.worst === "number").sort((a, b) => a.worst - b.worst);
const tiny = [...byKey.values()].filter((x) => x.kind === "tiny");
const clip = [...byKey.values()].filter((x) => x.kind === "clipRow");
const hs = [...byKey.values()].filter((x) => x.kind === "hscroll");

console.log(`# ${file}  视口 ${j.vw}x${j.vh} 主题 ${j.theme}  步数 ${j.steps.length}`);
console.log(`\n## 1. 对比度不达标（worst = 白底/黑底两种桌面透出下的最差值；need=AA）  共 ${contrast.length} 类\n`);
for (const c of contrast.slice(0, 18)) {
  console.log(`  ${String(c.worst).padStart(5)} (需${c.need}) 白底${c.onWhite}/黑底${c.onBlack} ${c.size}px w${c.weight} [${c.tab}] "${c.text}"  ${c.path}${c.inDlg ? " (弹窗内)" : ""}`);
}
console.log(`\n## 2. 字号 < 11px  共 ${tiny.length} 类`);
for (const c of tiny.slice(0, 12)) console.log(`  ${c.size}px [${c.tab}] "${c.text}" ${c.path}`);
console.log(`\n## 3. 命中区 —— 已移交 probe-hits.mjs（本汇总不再给判定，避免两处结论互相矛盾）`);
console.log(`\n## 4. 容器底部/折叠线半行截断  共 ${clip.length} 类`);
for (const c of clip.slice(0, 10)) console.log(`  切掉${c.cutBy}px (行高${c.childH}) [${c.tab}] "${c.text}" 容器${c.container}`);
console.log(`\n## 5. 横向溢出  共 ${hs.length} 类`);
for (const c of hs.slice(0, 10)) console.log(`  +${c.extra}px [${c.tab}] ${c.path} "${c.text}"`);
console.log(`\n## 6. toast`);
const t = byKey.get("TOAST");
if (t) console.log("  ", JSON.stringify(t.rects), "z=", t.z, "首次:", t.label);
console.log(`\n## 7. 弹窗最大高度（视口 ${j.vh}）`);
for (const [k, d] of [...dlgByTitle].sort((a, b) => b[1].h - a[1].h).slice(0, 16)) {
  console.log(`  ${String(d.h).padStart(4)}px 超上${d.overTop} 超下${d.overBottom} overflowY=${d.overflowY} maxH=${d.maxHeight} 自滚=${d.selfScrollable} 不可达${d.unreachable} [${d.tab}] 「${k}」${d.primaryButtons?.length ? " 主按钮:" + d.primaryButtons.map((p) => `${p.text}@${p.top}-${p.bottom}${p.visible ? "" : "★"}`).join(",") : ""}`);
}
if (j.errors?.length) console.log("\n## 8. JS 错误\n  " + j.errors.join("\n  "));
