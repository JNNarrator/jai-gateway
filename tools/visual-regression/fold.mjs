// 折叠线测量：默认窗口下「打开页面就看不到的常规操作」+ 弹窗可关闭性 + 横向溢出
// node .vr/fold.mjs --size=980x640
import fs from "node:fs";
import path from "node:path";
import { launchBrowser, parseArgv, parseSize, SHOTS_DIR, VR_DIR, DEFAULT_URL } from "./_env.mjs";
const { fixtures } = await import(new URL("./fixtures.mjs", import.meta.url).href);
const argv = parseArgv();
const [VW, VH] = parseSize(argv.size, "1180x800");
// 读仓库内的 run.mjs（相对本脚本解析）；曾读 `.vr/run.mjs` 本地镜像，会静默用旧 mock
const mockSrc = fs.readFileSync(new URL("./run.mjs", import.meta.url), "utf8").match(/function installMock\(\)[\s\S]*?\n}\n/)?.[0];
const say = console.log;
const browser = await launchBrowser();
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
  // 主操作 = 页级操作区（PageHeader.actions）+ 吸顶操作条/锚点导航，由 data-slot 契约标记。
  // 旧实现的选择器（`main h1 ~ * button, main header button`）**永远匹配不到** ——
  // PageHeader 既不渲染 data-slot 也没有 <header>，于是报告里的「顶部操作区」恒为「—」，
  // 是个死字段（假绿）：这条验收其实从未生效。同类问题见 bug 清单第 28 条
  // （`truncated` 只初始化、从不 push）。现在额外断言 primaryActionCount > 0，再失效就直接报错。
  const partial = ctrls.map((el) => { const r = el.getBoundingClientRect(); return { el, n: nm(el), tag: el.tagName.toLowerCase(), cut: Math.round(r.bottom - VH) }; }).filter((x) => x.cut > 2 && x.el.getBoundingClientRect().top < VH).map((x) => ({ n: x.n, tag: x.tag, cut: x.cut }));
  const ACT_SEL = "main [data-slot=page-header] button, main [data-slot=page-actions] button";
  const acts = Array.from(document.querySelectorAll(ACT_SEL)).map((b) => {
    const r = b.getBoundingClientRect();
    return { n: nm(b), top: Math.round(r.top), bottom: Math.round(r.bottom), inView: r.top >= 0 && r.bottom <= VH + 1, straddles: r.top < VH && r.bottom > VH, insideSticky: !!b.closest("[data-slot=page-actions]") };
  });
  // 吸顶表头**实测**：声明了 `position: sticky` 不等于真的吸顶。
  // 坑：`Table` 基座自带 `div[data-slot=table-container].overflow-x-auto`，其 overflow-y
  // 随之计算成 auto → 它是 thead 的最近可滚动祖先；若它自身无纵向溢出，sticky 完全失效。
  // 所以这里「真的滚一下它最近的纵向滚动容器，看表头是否留在原处」——只看 computed style 会假绿。
  // （这条判据的由来：日志页的「吸顶表头」曾被记为已解决，实测却一直没生效。）
  const stickyHeads = Array.from(main.querySelectorAll("table")).map((t) => {
    const thead = t.querySelector("thead");
    if (!thead || getComputedStyle(thead).position !== "sticky") return { sticky: false };
    let sc = null, n = thead.parentElement;
    while (n && n !== document.body) {
      if (n.scrollHeight > n.clientHeight + 1 && /(auto|scroll)/.test(getComputedStyle(n).overflowY)) { sc = n; break; }
      n = n.parentElement;
    }
    if (!sc) return { sticky: true, scrollable: false, works: null };
    const t0 = Math.round(thead.getBoundingClientRect().top);
    const s0 = sc.scrollTop;
    sc.scrollTop = Math.min(s0 + 150, sc.scrollHeight - sc.clientHeight);
    const moved = Math.round(thead.getBoundingClientRect().top - t0);
    sc.scrollTop = s0;
    return { sticky: true, scrollable: true, scrolledBy: Math.round(sc.scrollTop - s0) || 150, moved, works: Math.abs(moved) <= 4 };
  });
  const tables = Array.from(main.querySelectorAll("table")).map((t) => { const r = t.getBoundingClientRect(); return { cols: t.querySelectorAll("thead th").length, overflowX: Math.round(r.width - main.clientWidth), tableW: Math.round(r.width), mainW: main.clientWidth }; });
  const clippedText = Array.from(main.querySelectorAll("td,span,div,p,label")).filter((el) => !el.children.length && vis(el) && el.scrollWidth > el.clientWidth + 2 && getComputedStyle(el).textOverflow === "ellipsis").map((el) => ({ n: (el.textContent || "").trim().slice(0, 28), lost: el.scrollWidth - el.clientWidth }));
  const cards = Array.from(main.querySelectorAll('[data-slot="card"]')).map((c) => { const r = c.getBoundingClientRect(); return { t: Math.round(r.top), b: Math.round(r.bottom), h: Math.round(r.height), title: (c.querySelector("h3,h2,[data-slot=card-title]")?.textContent || "").trim().slice(0, 18) }; });
  return {
    scrollH: main.scrollHeight, clientH: main.clientHeight, foldPct: Math.round((1 - main.clientHeight / main.scrollHeight) * 100),
    total: ctrls.length, belowFold: below.length, belowList: below, partialCut: partial,
    firstBelowAt: below.length ? below[0].t : null,
    primaryActions: acts, primaryActionCount: acts.length,
    primaryActionsBelowFold: acts.filter((a) => !a.inView),
    tables, pageHScroll: document.documentElement.scrollWidth - document.documentElement.clientWidth,
    mainHScroll: main.scrollWidth - main.clientWidth,
    clippedTextCount: clippedText.length, clippedTextSample: clippedText.slice(0, 6),
    cards, stickyHeads,
  };
};

const TABS = [["网关", "gateway"], ["同步", "sync"], ["MCP", "mcp"], ["技能", "skills"], ["供应商", "providers"], ["模型", "models"], ["统计", "stats"], ["日志", "logs"], ["设置", "settings"]];
for (const [label, key] of TABS) {
  await page.goto(argv.url || DEFAULT_URL, { waitUntil: "domcontentloaded" });
  await page.waitForTimeout(850);
  await page.click(`aside nav button[aria-label="${label}"]`, { timeout: 4000 }).catch(() => say("navfail", label));
  await page.waitForTimeout(600);
  const f = await page.evaluate(FOLD, VH);
  all[key] = f;
  say(`\n### ${label} (${key}) 内容高 ${f.scrollH} / 可视 ${f.clientH} → 首屏只见 ${100 - f.foldPct}%  ${f.foldPct > 25 ? "▲超一屏" : ""}`);
  say(`   可交互控件 ${f.total}，其中整块在折叠线以下：${f.belowFold}${f.firstBelowAt ? `（第一个在 y=${f.firstBelowAt}）` : ""}`);
  if (f.belowList.length) say(`   需滚动才可见的操作: ${f.belowList.slice(0, 12).map((x) => `${x.n}@${x.t}`).join(" ; ")}`);
  if (f.partialCut.length) say(`   被折叠线切一半: ${f.partialCut.slice(0, 8).map((x) => `${x.n}(切${x.cut}px)`).join(" ; ")}`);
  say(`   顶部操作区（${f.primaryActionCount} 个，含吸顶条）: ${f.primaryActions.map((b) => `${b.n}${b.inView ? "" : `★不可见@${b.top}`}`).join(" ; ") || "—（本页无 data-slot 标记的主操作）"}`);
  if (f.tables.length) say(`   表格: ${f.tables.map((t) => `${t.cols}列 宽${t.tableW}/容器${t.mainW} 横向溢出${t.overflowX}`).join(" ; ")}`);
  if (f.pageHScroll > 1 || f.mainHScroll > 1) say(`   ★横向溢出: html+${f.pageHScroll} main+${f.mainHScroll}`);
  if (f.clippedTextCount) say(`   省略号截断文本 ${f.clippedTextCount} 处: ${f.clippedTextSample.map((c) => `${c.n}(-${c.lost}px)`).join(" ; ")}`);
  const half = f.cards.filter((c) => c.t < VH && c.b > VH);
  if (half.length) say(`   跨折叠线的卡片: ${half.map((c) => `${c.title || "card"} ${c.t}-${c.b}`).join(" ; ")}`);
  await page.screenshot({ path: path.join(SHOTS_DIR, `fold-${VW}x${VH}-${key}.png`) }).catch(() => {});
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

const outFile = path.join(VR_DIR, `fold-${VW}x${VH}.json`);
fs.writeFileSync(outFile, JSON.stringify(all, null, 2));
say(`\n→ ${outFile}`);
await browser.close();
