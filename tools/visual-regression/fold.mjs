// 折叠线测量：默认窗口下「打开页面就看不到的常规操作」+ 弹窗可关闭性 + 横向溢出
// node .vr/fold.mjs --size=980x640
import { createRequire } from "node:module";
import fs from "node:fs";
import path from "node:path";
const require = createRequire("/Users/jiangnan/Documents/workspace/deepseek-harness/node_modules/.pnpm/playwright@1.61.1/node_modules/playwright/");
const { chromium } = require("playwright");
const { fixtures } = await import(new URL("./fixtures.mjs", import.meta.url).href);
const argv = Object.fromEntries(process.argv.slice(2).map((s) => { const m = s.match(/^--([^=]+)(?:=(.*))?$/); return m ? [m[1], m[2] ?? true] : [s, true]; }));
const [VW, VH] = (argv.size || "980x640").split("x").map(Number);
const mockSrc = fs.readFileSync(path.resolve(".vr/run.mjs"), "utf8").match(/function installMock\(\)[\s\S]*?\n}\n/)?.[0];
const say = console.log;
const browser = await chromium.launch({ executablePath: "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome", headless: true });
const ctx = await browser.newContext({ viewport: { width: VW, height: VH }, deviceScaleFactor: 1, locale: "zh-CN" });
await ctx.addInitScript({ content: `window.__JAI_FIX__ = ${JSON.stringify(fixtures)}; window.__VR_VW__=${VW}; window.__VR_VH__=${VH};` });
await ctx.addInitScript({ content: `(${mockSrc})();` });
const page = await ctx.newPage();
const all = {};

const FOLD = (VH) => {
  const main = document.querySelector("main");
  const mr = main.getBoundingClientRect();
  const vis = (el) => { const cs = getComputedStyle(el); const r = el.getBoundingClientRect(); return r.width > 0 && r.height > 0 && cs.visibility !== "hidden" && +cs.opacity > 0; };
  const nm = (el) => (el.getAttribute("aria-label") || el.textContent || el.getAttribute("placeholder") || el.title || el.tagName).trim().replace(/\s+/g, " ").slice(0, 30);
  const ctrls = Array.from(main.querySelectorAll("button,input,textarea,select,[role=button],[role=combobox],[role=switch],[role=checkbox]")).filter(vis);
  const below = ctrls.map((el) => ({ n: nm(el), t: Math.round(el.getBoundingClientRect().top), b: Math.round(el.getBoundingClientRect().bottom) })).filter((x) => x.t >= VH);
  const partial = ctrls.map((el) => { const r = el.getBoundingClientRect(); return { el, n: nm(el), cut: Math.round(r.bottom - VH) }; }).filter((x) => x.cut > 2 && x.el.getBoundingClientRect().top < VH).map((x) => ({ n: x.n, cut: x.cut }));
  const tables = Array.from(main.querySelectorAll("table")).map((t) => { const r = t.getBoundingClientRect(); return { cols: t.querySelectorAll("thead th").length, overflowX: Math.round(r.width - main.clientWidth), tableW: Math.round(r.width), mainW: main.clientWidth }; });
  const clippedText = Array.from(main.querySelectorAll("td,span,div,p,label")).filter((el) => !el.children.length && vis(el) && el.scrollWidth > el.clientWidth + 2 && getComputedStyle(el).textOverflow === "ellipsis").map((el) => ({ n: (el.textContent || "").trim().slice(0, 28), lost: el.scrollWidth - el.clientWidth }));
  const cards = Array.from(main.querySelectorAll('[data-slot="card"]')).map((c) => { const r = c.getBoundingClientRect(); return { t: Math.round(r.top), b: Math.round(r.bottom), h: Math.round(r.height), title: (c.querySelector("h3,h2,[data-slot=card-title]")?.textContent || "").trim().slice(0, 18) }; });
  return {
    scrollH: main.scrollHeight, clientH: main.clientHeight, foldPct: Math.round((1 - main.clientHeight / main.scrollHeight) * 100),
    total: ctrls.length, belowFold: below.length, belowList: below, partialCut: partial,
    firstBelowAt: below.length ? below[0].t : null,
    headerBtns: Array.from(document.querySelectorAll("main [data-slot=page-header] button, main h1 ~ * button, main header button")).map((b) => ({ n: nm(b), vis: b.getBoundingClientRect().bottom <= VH })),
    tables, pageHScroll: document.documentElement.scrollWidth - document.documentElement.clientWidth,
    mainHScroll: main.scrollWidth - main.clientWidth,
    clippedTextCount: clippedText.length, clippedTextSample: clippedText.slice(0, 6),
    cards,
  };
};

const TABS = [["网关", "gateway"], ["同步", "sync"], ["MCP", "mcp"], ["技能", "skills"], ["供应商", "providers"], ["模型", "models"], ["统计", "stats"], ["日志", "logs"], ["设置", "settings"]];
for (const [label, key] of TABS) {
  await page.goto(argv.url || "http://127.0.0.1:5173/", { waitUntil: "domcontentloaded" });
  await page.waitForTimeout(850);
  await page.click(`aside nav button[aria-label="${label}"]`, { timeout: 4000 }).catch(() => say("navfail", label));
  await page.waitForTimeout(600);
  const f = await page.evaluate(FOLD, VH);
  all[key] = f;
  say(`\n### ${label} (${key}) 内容高 ${f.scrollH} / 可视 ${f.clientH} → 首屏只见 ${100 - f.foldPct}%  ${f.foldPct > 25 ? "▲超一屏" : ""}`);
  say(`   可交互控件 ${f.total}，其中整块在折叠线以下：${f.belowFold}${f.firstBelowAt ? `（第一个在 y=${f.firstBelowAt}）` : ""}`);
  if (f.belowList.length) say(`   需滚动才可见的操作: ${f.belowList.slice(0, 12).map((x) => `${x.n}@${x.t}`).join(" ; ")}`);
  if (f.partialCut.length) say(`   被折叠线切一半: ${f.partialCut.slice(0, 8).map((x) => `${x.n}(切${x.cut}px)`).join(" ; ")}`);
  say(`   顶部操作区: ${f.headerBtns.map((b) => `${b.n}${b.vis ? "" : "★不可见"}`).join(" ; ") || "—"}`);
  if (f.tables.length) say(`   表格: ${f.tables.map((t) => `${t.cols}列 宽${t.tableW}/容器${t.mainW} 横向溢出${t.overflowX}`).join(" ; ")}`);
  if (f.pageHScroll > 1 || f.mainHScroll > 1) say(`   ★横向溢出: html+${f.pageHScroll} main+${f.mainHScroll}`);
  if (f.clippedTextCount) say(`   省略号截断文本 ${f.clippedTextCount} 处: ${f.clippedTextSample.map((c) => `${c.n}(-${c.lost}px)`).join(" ; ")}`);
  const half = f.cards.filter((c) => c.t < VH && c.b > VH);
  if (half.length) say(`   跨折叠线的卡片: ${half.map((c) => `${c.title || "card"} ${c.t}-${c.b}`).join(" ; ")}`);
  await page.screenshot({ path: path.resolve(`.vr/shots/fold-${VW}x${VH}-${key}.png`) }).catch(() => {});
}

// 弹窗可关闭性：逐个入口 → Esc → 是否还在 → 还能不能切页
await page.goto(argv.url || "http://127.0.0.1:5173/", { waitUntil: "domcontentloaded" });
await page.waitForTimeout(850);
say("\n\n## 弹窗 Escape 可关闭性 / 关闭后能否继续导航");
const MODALS = [
  ["模型", "模态标注", "main button:has-text('模态') >> nth=0"],
  ["模型", "别名", "main button:has-text('别名') >> nth=0"],
  ["模型", "上下文窗口", "main button:has-text('上下文') >> nth=0"],
  ["技能", "编辑技能", "main button:has-text('编辑') >> nth=0"],
  ["MCP", "添加 MCP", "button:has-text('添加 MCP Server')"],
  ["供应商", "添加供应商", "button:has-text('添加供应商')"],
  ["同步", "预览变更", "button:has-text('预览变更')"],
];
for (const [tab, name, sel] of MODALS) {
  await page.goto(argv.url || "http://127.0.0.1:5173/", { waitUntil: "domcontentloaded" });
  await page.waitForTimeout(800);
  await page.click(`aside nav button[aria-label="${tab}"]`).catch(() => {});
  await page.waitForTimeout(500);
  const loc = page.locator(sel).first();
  if (!(await loc.count())) { say(`— ${tab}/${name}: 无入口`); continue; }
  await loc.click({ timeout: 2500 }).catch((e) => say(`— ${tab}/${name}: 点击失败 ${String(e.message).slice(0, 40)}`));
  await page.waitForTimeout(450);
  const opened = await page.evaluate(() => {
    const d = document.querySelector('[role="dialog"],[role="alertdialog"],[data-slot=dialog-content]');
    if (!d) return null;
    const r = d.getBoundingClientRect();
    return { role: d.getAttribute("role"), tag: d.getAttribute("data-slot"), title: (d.querySelector("h2,[role=heading]")?.textContent || "").trim().slice(0, 20), t: Math.round(r.top), b: Math.round(r.bottom), h: Math.round(r.height), vh: innerHeight, closeBtn: !!d.querySelector('[data-slot=dialog-close],[aria-label=关闭],[aria-label="Close"]') };
  });
  if (!opened) { say(`— ${tab}/${name}: 未开弹窗（可能是行内开关）`); continue; }
  await page.keyboard.press("Escape");
  await page.waitForTimeout(450);
  const afterEsc = await page.evaluate(() => !!document.querySelector('[role="dialog"],[role="alertdialog"]'));
  await page.click(`aside nav button[aria-label="设置"]`, { timeout: 2500 }).catch(() => {});
  await page.waitForTimeout(400);
  const navOk = await page.evaluate(() => (document.querySelector("main h1,main h2")?.textContent || "").trim().slice(0, 10));
  const over = Math.max(0, -opened.t) + "/" + Math.max(0, opened.b - VH);
  say(`   ${tab}/${name} 「${opened.title}」 h=${opened.h} 超出上/下=${over} Esc可关=${!afterEsc ? "是" : "★否"} 关后导航=${navOk === "设置" ? "OK" : "★失败(仍在" + navOk + ")"}`);
  all[`modal:${tab}:${name}`] = { opened, escClosed: !afterEsc, navAfter: navOk };
}

fs.writeFileSync(path.resolve(`.vr/fold-${VW}x${VH}.json`), JSON.stringify(all, null, 2));
say(`\n→ .vr/fold-${VW}x${VH}.json`);
await browser.close();
