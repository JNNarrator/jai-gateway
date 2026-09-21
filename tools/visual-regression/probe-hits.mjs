// 「有效命中区」探针 —— **命中区判据的唯一归属**。
//
// 为什么只有它能判：审计（audit.mjs）用 getBoundingClientRect 量视觉盒子，
// **看不到** `::after` 伪元素 / `<label>` 包裹造成的热区外扩，于是长期给出
// 「42 处命中区不达标」而 probe-hits 复核后大多合格 —— 两处结论互相矛盾，
// 谁也不能作为门禁。现在 audit 只负责能用 rect 可靠测量的事（对比度/字号/截断/溢出/弹窗几何），
// 命中区一律由本探针用 elementFromPoint 逐点探测**有效**命中区来判定。
//
// 两个测量坑（都踩过，勿回退）：
//   ① 判据只认「元素本身 / 其后代 / 包裹它的 label」。**不能**把祖先算命中，
//      否则父 td/span 都算命中，整行都算可达，数值虚高；
//   ② 从中心对称采样，有效尺寸 = 2×可达半径（写成 rect + 2×reach 会把尺寸算大）。
//
// 覆盖：9 个页面 + 逐页打开的代表性弹窗，全量扫描交互控件；
// 并断言「本轮清单点名的控件类型」确实出现在扫描结果里（覆盖缺口也算失败），
// 避免哪天选择器失效、探针静默只测到 3 个元素却报「全绿」。
//
// 用法（仓库根目录，需先起 vite:5173）：
//   node tools/visual-regression/probe-hits.mjs --size=1180x800
//   node tools/visual-regression/probe-hits.mjs --size=900x600
// 产出：.vr/probe-hits-<size>.json；命中区不达标或有覆盖缺口 → 退出码 1。
import fs from "node:fs";
import path from "node:path";
import { launchBrowser, parseArgv, parseSize, installMockSrc, VR_DIR, DEFAULT_URL, TABS, TAB_LABEL } from "./_env.mjs";
const { fixtures } = await import(new URL("./fixtures.mjs", import.meta.url).href);

const argv = parseArgv();
const [VW, VH] = parseSize(argv.size, "1180x800");
/** WCAG 2.2 AA 目标尺寸；也是本项目 docs 里写定的「交互控件 ≥24×24」 */
const MIN_TARGET = 24;
const REACH_CAP = 44; // 探测半径上限（px）；超过就认为足够大，无需继续

const browser = await launchBrowser();
const ctx = await browser.newContext({ viewport: { width: VW, height: VH }, locale: "zh-CN" });
await ctx.addInitScript({
  content: `window.__JAI_FIX__=${JSON.stringify(fixtures)}; window.__VR_VW__=${VW}; window.__VR_VH__=${VH};`,
});
await ctx.addInitScript({ content: `(${installMockSrc()})();` });
await ctx.addInitScript({ content: `try{localStorage.setItem('jai-sidebar-collapsed','0');}catch(e){}` });
const page = await ctx.newPage();
const errors = [];
page.on("console", (m) => { if (m.type() === "error") errors.push("console: " + m.text().slice(0, 160)); });
page.on("pageerror", (e) => errors.push("pageerror: " + String(e.message).slice(0, 160)));
await page.goto(argv.url || DEFAULT_URL, { waitUntil: "domcontentloaded" });
await page.waitForTimeout(1200);

// ── 页面内扫描：只对「视觉盒子已小于 minTarget」的候选做逐点探测（其余必然达标，省时间） ──
// 注意：Playwright 的 page.evaluate 只允许传**一个**参数，故这里收一个对象。
const SWEEP = function ({ minTarget, reachCap, scopeSel }) {
  const root = document.querySelector(scopeSel || "main") || document.body;
  const INTER = [
    "button", "a[href]", "input", "select", "textarea",
    "[role=button]", "[role=menuitem]", "[role=switch]", "[role=checkbox]",
    "[role=combobox]", "[role=radio]", "[role=tab]",
  ].join(",");
  const vis = (el) => {
    const cs = getComputedStyle(el);
    if (cs.visibility === "hidden" || cs.display === "none" || +cs.opacity === 0) return false;
    const r = el.getBoundingClientRect();
    return r.width >= 1 && r.height >= 1;
  };
  const pathOf = (el) => {
    const parts = [];
    let n = el;
    for (let i = 0; i < 4 && n && n.tagName !== "BODY"; i++) {
      let s = n.tagName.toLowerCase();
      const dl = n.getAttribute("data-slot");
      if (dl) s += `[${dl}]`;
      else {
        const c = (typeof n.className === "string" ? n.className : "").split(/\s+/).filter(Boolean)[0];
        if (c) s += "." + c.slice(0, 22);
      }
      parts.unshift(s);
      n = n.parentElement;
    }
    return parts.join(">");
  };
  const out = [];
  for (const el of root.querySelectorAll(INTER)) {
    // 跳过无障碍「视觉隐藏代理」（Radix Select 的 1×1 hidden select 等）：
    // 它们不是可点元素，属历史已知假阳性。
    if (el.closest('[aria-hidden="true"]')) continue;
    if (!vis(el)) continue;
    let r = el.getBoundingClientRect();
    const info = {
      tag: el.tagName.toLowerCase(),
      role: el.getAttribute("role") || "",
      type: el.getAttribute("type") || "",
      slot: el.getAttribute("data-slot") || "",
      name: (el.getAttribute("aria-label") || el.textContent || el.getAttribute("title") || el.getAttribute("placeholder") || "").trim().replace(/\s+/g, " ").slice(0, 44),
      title: (el.getAttribute("title") || "").slice(0, 60),
      path: pathOf(el),
      rect: { w: Math.round(r.width), h: Math.round(r.height) },
    };
    if (r.width >= minTarget && r.height >= minTarget) {
      out.push({ ...info, effective: { w: Math.round(r.width), h: Math.round(r.height) }, measured: false });
      continue;
    }
    // 两种「量不了」的情况，都必须先滚到视口中央再量（这也是用户真实会做的动作）：
    //   ① 元素在视口外（页面/表格已滚动）→ elementFromPoint 返回 null；
    //   ② 元素被**某个可滚动祖先裁掉**（典型：抽屉正文 `overflow-y-auto` 的底部，
    //      元素 rect 仍在视口内、但已经被祖先裁掉，该点的 elementFromPoint 落到
    //      叠在上面的 footer 上）→ 会量出「有效 0×0」的假失败。
    //      只判视口（旧写法）漏掉了第 ② 种，实测在模型页抽屉的「启用」开关上稳定误报。
    const clippedBy = (node) => {
      let n = node.parentElement;
      while (n && n !== document.body) {
        const cs = getComputedStyle(n);
        if (/(auto|scroll)/.test(cs.overflowY) || /(auto|scroll)/.test(cs.overflowX)) {
          const a = n.getBoundingClientRect();
          if (r.top < a.top - 1 || r.bottom > a.bottom + 1 || r.left < a.left - 1 || r.right > a.right + 1) return true;
        }
        n = n.parentElement;
      }
      return false;
    };
    const outsideViewport = () => r.top < 0 || r.bottom > innerHeight || r.left < 0 || r.right > innerWidth;
    if (outsideViewport() || clippedBy(el)) {
      el.scrollIntoView({ block: "center", inline: "center" });
      const r2 = el.getBoundingClientRect();
      r = r2;
      if (outsideViewport() || clippedBy(el)) {
        // 滚了也进不去（被祖先彻底裁掉/隐藏）→ 命中区无从测量，交折叠线类探针负责，不计入判定
        out.push({ ...info, effective: null, measured: false, offscreen: true });
        continue;
      }
      info.rect = { w: Math.round(r2.width), h: Math.round(r2.height) };
      info.scrolledIntoView = true;
    }
    const cx = r.left + r.width / 2, cy = r.top + r.height / 2;
    const owns = (t) => !!t && (t === el || el.contains(t) || (t.tagName === "LABEL" && t.contains(el)));
    const reach = (dx, dy) => {
      const x = Math.min(Math.max(cx + dx, 1), innerWidth - 1);
      const y = Math.min(Math.max(cy + dy, 1), innerHeight - 1);
      return owns(document.elementFromPoint(x, y));
    };
    let maxX = 0, maxY = 0;
    for (let d = 1; d <= reachCap; d++) { if (reach(d, 0) && reach(-d, 0)) maxX = d; else break; }
    for (let d = 1; d <= reachCap; d++) { if (reach(0, d) && reach(0, -d)) maxY = d; else break; }
    // 采样自中心 → 有效尺寸 = 2×可达半径（对称探测）
    const eff = { w: Math.min(2 * maxX, reachCap * 2), h: Math.min(2 * maxY, reachCap * 2) };
    out.push({ ...info, effective: eff, reachX: maxX, reachY: maxY, measured: true });
  }
  return out;
};

// ── 控件类型分类（用于「覆盖缺口」断言；判据写在 Node 侧，便于审阅） ──
const KINDS = {
  switch: (c) => c.role === "switch",
  checkbox: (c) => c.tag === "input" && c.type === "checkbox",
  inlineCopy: (c) => c.tag === "button" && c.name.startsWith("复制"),
  externalLink: (c) => /^https?:/.test(c.title) || (c.tag === "a" && /^https?:/.test(c.title)),
  effortLevels: (c) => c.name.startsWith("推理档位值域"),
  maxTools: (c) => c.name.startsWith("工具声明数上限"),
  dropdownTrigger: (c) => c.slot === "dropdown-menu-trigger",
  dialogClose: (c) => c.slot === "dialog-close",
  select: (c) => c.role === "combobox",
  textInput: (c) => c.tag === "input" || c.tag === "textarea",
};

/** 本轮清单点名要求「必须被覆盖到」的类型；缺任何一个 → 覆盖缺口 → 失败 */
const REQUIRED_KINDS = ["switch", "checkbox", "inlineCopy", "externalLink", "effortLevels", "dropdownTrigger"];
// 逐页扫描（含弹窗：每个页面点开前若干个「添加/编辑/导入」类入口再扫一遍）
const all = [];
const scan = async (scope, label) => {
  // 直接传函数本身：Playwright 会序列化它并在页面里调用（比 new Function 更稳，
  // 不依赖页面 CSP 允许 eval）。参数按顺序注入。
  const list = await page.evaluate(SWEEP, { minTarget: MIN_TARGET, reachCap: REACH_CAP, scopeSel: scope });
  for (const c of list) all.push({ ...c, where: label });
  return list;
};
const TRIGGER = /添加|新增|新建|编辑|导入|导出|设置|高级|详情|恢复|备份|清理|别名|限额|模态|工具|连接|重新生成|显示|删除|重命名/;
for (const tab of TABS) {
  await page.goto(argv.url || DEFAULT_URL, { waitUntil: "domcontentloaded" });
  await page.waitForTimeout(700);
  await page.click(`aside nav button[aria-label="${TAB_LABEL[tab]}"]`, { timeout: 4000 }).catch(() => {});
  await page.waitForTimeout(500);
  const n = (await scan("main", `tab:${tab}`)).length;
  const seen = new Set();
  for (let iter = 0; iter < 8; iter++) {
    const cand = await page.evaluate((re) => {
      const rx = new RegExp(re);
      const scope = document.querySelectorAll("main button, main [role=button]");
      return Array.from(scope).map((el, i) => {
        el.setAttribute("data-vr-hit", String(i));
        return { i, text: (el.getAttribute("aria-label") || el.textContent || "").trim().replace(/\s+/g, " ").slice(0, 40) };
      }).filter((x) => x.text && rx.test(x.text));
    }, TRIGGER.source);
    const next = cand.find((x) => !seen.has(x.text));
    if (!next) break;
    seen.add(next.text);
    const loc = page.locator(`main [data-vr-hit="${next.i}"]`).first();
    if (!(await loc.isVisible().catch(() => false))) continue;
    await loc.scrollIntoViewIfNeeded().catch(() => {});
    if (!(await loc.click({ timeout: 2000 }).then(() => true).catch(() => false))) continue;
    await page.waitForTimeout(400);
    if (await page.locator('[role="dialog"],[role="alertdialog"]').count()) {
      await scan('[role="dialog"],[role="alertdialog"]', `dlg:${tab}:${next.text}`);
      for (let k = 0; k < 3; k++) {
        if (!(await page.locator('[role="dialog"],[role="alertdialog"],[role="menu"],[role="listbox"]').count())) break;
        await page.keyboard.press("Escape");
        await page.waitForTimeout(200);
      }
      await page.goto(argv.url || DEFAULT_URL, { waitUntil: "domcontentloaded" });
      await page.waitForTimeout(600);
      await page.click(`aside nav button[aria-label="${TAB_LABEL[tab]}"]`, { timeout: 3000 }).catch(() => {});
      await page.waitForTimeout(400);
    }
  }
  console.log(`  [${tab}] 扫描 ${n} 个控件`);
}

// ── 判定 ──
const fails = all.filter((c) => c.effective && (c.effective.w < MIN_TARGET || c.effective.h < MIN_TARGET));
const coveredKinds = Object.fromEntries(
  Object.entries(KINDS).map(([k, f]) => [k, all.filter(f).length]),
);
const missingKinds = REQUIRED_KINDS.filter((k) => coveredKinds[k] === 0);

console.log(`\n# probe-hits ${VW}x${VH} —— 有效命中区判据（唯一归属）`);
console.log(`扫描控件 ${all.length} 个；视觉盒子已达标直接判合格 ${all.filter((c) => !c.measured && !c.offscreen).length} 个，逐点实测 ${all.filter((c) => c.measured).length} 个，滚入视口仍不可测（不计入判定）${all.filter((c) => c.offscreen).length} 个`);
console.log(`\n## 命中区 < ${MIN_TARGET}×${MIN_TARGET}  共 ${fails.length} 处`);
for (const c of fails.slice(0, 40)) {
  console.log(`  rect ${c.rect.w}×${c.rect.h} → 有效 ${c.effective.w}×${c.effective.h}  [${c.where}] "${c.name}"  ${c.path}`);
}
if (fails.length > 40) console.log(`  …另有 ${fails.length - 40} 处`);
console.log(`\n## 类型覆盖（缺一即失败，防探针静默失效）`);
for (const k of Object.keys(KINDS)) {
  const need = REQUIRED_KINDS.includes(k);
  console.log(`  ${coveredKinds[k] > 0 ? "✓" : need ? "★缺口" : "·"} ${k}: ${coveredKinds[k]}${need ? "（必需）" : ""}`);
}
if (errors.length) console.log(`\n## 控制台报错\n  ${[...new Set(errors)].slice(0, 5).join("\n  ")}`);

const out = { vw: VW, vh: VH, minTarget: MIN_TARGET, scanned: all.length, fails, coveredKinds, missingKinds, errors: [...new Set(errors)], controls: all };
const outFile = path.join(VR_DIR, `probe-hits-${VW}x${VH}.json`);
fs.writeFileSync(outFile, JSON.stringify(out, null, 2));
console.log(`\n→ ${outFile}`);

const bad = fails.length > 0 || missingKinds.length > 0 || errors.length > 0;
console.log(bad ? `\n✗ 不通过：命中区不达标 ${fails.length} 处；覆盖缺口 ${missingKinds.join(",") || "无"}；控制台报错 ${errors.length}` : "\n✓ 通过：命中区全部达标、类型覆盖完整、无控制台报错");
await browser.close();
process.exit(bad ? 1 : 0);
