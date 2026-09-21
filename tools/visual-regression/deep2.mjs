// 定向解剖 v2：每个用例前先重载页面，避免弹窗残留串味
// node .vr/deep2.mjs --size=980x640
import fs from "node:fs";
import path from "node:path";
import { launchBrowser } from "./_env.mjs";

const { fixtures } = await import(new URL("./fixtures.mjs", import.meta.url).href);
const argv = Object.fromEntries(process.argv.slice(2).map((s) => { const m = s.match(/^--([^=]+)(?:=(.*))?$/); return m ? [m[1], m[2] ?? true] : [s, true]; }));
const [VW, VH] = (argv.size || "980x640").split("x").map(Number);
// 读仓库内的 run.mjs（相对本脚本解析）；曾读 `.vr/run.mjs` 本地镜像，会静默用旧 mock
const mockSrc = fs.readFileSync(new URL("./run.mjs", import.meta.url), "utf8").match(/function installMock\(\)[\s\S]*?\n}\n/)?.[0];
const URL_ = argv.url || "http://127.0.0.1:5173/";
const say = console.log;

const browser = await launchBrowser();
const ctx = await browser.newContext({ viewport: { width: VW, height: VH }, deviceScaleFactor: 2, locale: "zh-CN" });
await ctx.addInitScript({ content: `window.__JAI_FIX__ = ${JSON.stringify(fixtures)}; window.__VR_VW__=${VW}; window.__VR_VH__=${VH};` });
await ctx.addInitScript({ content: `(${mockSrc})();` });
const page = await ctx.newPage();

const OUTLINE = () => {
  const dlg = document.querySelector('[role="dialog"],[role="alertdialog"]');
  if (!dlg) return null;
  const vh = innerHeight;
  const R = (el) => { const r = el.getBoundingClientRect(); return { t: Math.round(r.top), b: Math.round(r.bottom), h: Math.round(r.height) }; };
  const nm = (el) => (el.getAttribute("aria-label") || el.textContent || el.getAttribute("placeholder") || el.tagName).trim().replace(/\s+/g, " ").slice(0, 24);
  const all = [dlg, ...dlg.querySelectorAll("*")];
  const scrollers = all.filter((n) => /(auto|scroll)/.test(getComputedStyle(n).overflowY) && n.scrollHeight > n.clientHeight + 1)
    .map((n) => ({ node: n.getAttribute("data-slot") || n.tagName.toLowerCase() + "." + (n.className || "").toString().split(/\s+/).slice(0, 2).join("."), scrollH: n.scrollHeight, clientH: n.clientHeight, maxH: getComputedStyle(n).maxHeight, rect: R(n) }));
  const leaves = Array.from(dlg.querySelectorAll("button,input,textarea,[role=combobox],[role=switch]")).filter((e) => e.getBoundingClientRect().width > 0);
  const clipped = leaves.filter((el) => { const r = el.getBoundingClientRect(); const dr = dlg.getBoundingClientRect(); return r.bottom > dr.bottom + 1 || r.top < dr.top - 1; }).map((el) => ({ n: nm(el), ...R(el) }));
  const outVp = leaves.filter((el) => { const r = el.getBoundingClientRect(); return r.top < 0 || r.bottom > vh; }).map((el) => ({ n: nm(el), ...R(el) }));
  const reach = (el) => { let n = el.parentElement; while (n && n !== dlg.parentElement) { const s = getComputedStyle(n); if (/(auto|scroll)/.test(s.overflowY) && n.scrollHeight > n.clientHeight + 1) return true; n = n.parentElement; } return false; };
  const primaries = leaves.filter((el) => el.tagName === "BUTTON" && /保存|创建|更新|确定|推送|拉取|导入|恢复|删除|测试连接|复制/.test(el.textContent || ""))
    .map((el) => ({ n: (el.textContent || "").trim().slice(0, 8), ...R(el), visible: el.getBoundingClientRect().top >= 0 && el.getBoundingClientRect().bottom <= vh, scrollable: reach(el) }));
  const footer = dlg.querySelector('[data-slot="dialog-footer"],[data-slot="alert-dialog-footer"]');
  return {
    title: (dlg.querySelector("[data-slot=dialog-title],[data-slot=alert-dialog-title]")?.textContent || "").trim().slice(0, 24),
    rect: R(dlg), contentH: dlg.scrollHeight, overflowY: getComputedStyle(dlg).overflowY, maxH: getComputedStyle(dlg).maxHeight,
    scrollers, leaves: leaves.length, clippedByDlg: clipped, outViewport: outVp, primaries,
    footer: footer ? { ...R(footer), pos: getComputedStyle(footer).position, visible: footer.getBoundingClientRect().bottom <= vh && footer.getBoundingClientRect().top >= 0 } : null,
  };
};

async function fresh(tabLabel) {
  await page.goto(URL_, { waitUntil: "domcontentloaded" });
  await page.waitForTimeout(900);
  if (tabLabel) { await page.click(`aside nav button[aria-label="${tabLabel}"]`, { timeout: 4000 }); await page.waitForTimeout(650); }
}
const scrollBottomTry = () => page.evaluate(() => {
  const dlg = document.querySelector('[role="dialog"],[role="alertdialog"]');
  if (!dlg) return null;
  const sc = [dlg, ...dlg.querySelectorAll("*")].find((n) => /(auto|scroll)/.test(getComputedStyle(n).overflowY) && n.scrollHeight > n.clientHeight + 1);
  if (!sc) return { scrolled: false };
  sc.scrollTop = sc.scrollHeight;
  const vh = innerHeight;
  return { scrolled: true, after: Array.from(dlg.querySelectorAll("button")).filter((b) => /保存|创建|更新|确定|推送|恢复/.test(b.textContent || "")).map((b) => { const r = b.getBoundingClientRect(); return `${(b.textContent || "").trim().slice(0, 6)}@${Math.round(r.top)}-${Math.round(r.bottom)} ${r.top >= 0 && r.bottom <= vh ? "可见" : "不可见"}`; }) };
});

const RECIPES = [
  ["providers", "供应商", "添加供应商", "button:has-text('添加供应商')"],
  ["providers", "供应商", "编辑第1行", "main button:has-text('编辑') >> nth=0"],
  ["providers", "供应商", "删除确认", "main button:has-text('删除') >> nth=0"],
  ["models", "模型", "别名弹窗", "main button:has-text('别名') >> nth=0"],
  ["models", "模型", "限额弹窗", "main button:has-text('限额') >> nth=0"],
  ["models", "模型", "模态弹窗", "main button:has-text('模态') >> nth=0"],
  ["skills", "技能", "添加技能", "button:has-text('添加技能')"],
  ["skills", "技能", "编辑技能", "main button:has-text('编辑') >> nth=0"],
  ["mcp", "MCP", "添加 MCP", "button:has-text('添加 MCP Server')"],
  ["mcp", "MCP", "导入配置", "button:has-text('导入') >> nth=0"],
  ["mcp", "MCP", "工具列表", "button:has-text('列出工具') >> nth=0"],
  ["sync", "同步", "预览变更", "button:has-text('预览变更')"],
  ["sync", "同步", "推送差异", "button:has-text('推送') >> nth=0"],
  ["settings", "设置", "保留策略", "button:has-text('编辑保留策略')"],
  ["settings", "设置", "添加域名", "button:has-text('添加域名')"],
  ["logs", "日志", "请求详情", "main tbody tr >> nth=0"],
  ["logs", "日志", "清空确认", "button:has-text('清空') >> nth=0"],
];

const results = [];
for (const [key, tabLabel, name, sel] of RECIPES) {
  await fresh(tabLabel);
  const loc = page.locator(sel).first();
  if (!(await loc.count())) { say(`— ${key}/${name}: 入口未找到`); continue; }
  await loc.scrollIntoViewIfNeeded().catch(() => {});
  await loc.click({ timeout: 3000 }).catch(() => {});
  await page.waitForTimeout(500);
  const o = await page.evaluate(OUTLINE);
  if (!o) { say(`— ${key}/${name}: 点击后无弹窗`); continue; }
  const sc = await scrollBottomTry();
  await page.screenshot({ path: path.resolve(`.vr/shots/deep2-${VW}x${VH}-${key}-${name.replace(/\s/g, "")}.png`) }).catch(() => {});
  const overflowTop = Math.max(0, -o.rect.t), overflowBottom = Math.max(0, o.rect.b - VH);
  say(`\n### ${key}/${name} → 「${o.title}」  弹窗 ${o.rect.t}→${o.rect.b} (h=${o.rect.h}) 视口${VH}  超出: 上${overflowTop} 下${overflowBottom}`);
  say(`   overflowY=${o.overflowY} maxH=${o.maxH} 滚动容器=${o.scrollers.map((s) => `${s.node}[${s.scrollH}>${s.clientH}]`).join(";") || "无"}`);
  say(`   控件=${o.leaves} 被弹窗裁掉=${o.clippedByDlg.length} 视口外=${o.outViewport.length}`);
  say(`   footer=${o.footer ? `${o.footer.t}-${o.footer.b} ${o.footer.pos} ${o.footer.visible ? "可见" : "不可见"}` : "无"}`);
  say(`   主按钮: ${o.primaries.map((p) => `${p.n}@${p.t}-${p.b} ${p.visible ? "可见" : (p.scrollable ? "需内部滚动" : "★不可达")}`).join(" ; ")}`);
  if (o.clippedByDlg.length) say(`   被裁控件: ${o.clippedByDlg.slice(0, 6).map((c) => `${c.n}@${c.t}-${c.b}`).join(" ; ")}`);
  if (o.outViewport.length) say(`   视口外控件: ${o.outViewport.slice(0, 6).map((c) => `${c.n}@${c.t}-${c.b}`).join(" ; ")}`);
  results.push({ key, name, o, afterScroll: sc });
}

// toast 真实遮挡：点复制 → 量 toast 矩形与被打断的控件
await fresh("网关");
const copySel = "main button:has-text('复制') >> nth=0";
if (await page.locator(copySel).count()) {
  await page.locator(copySel).first().click();
  await page.waitForTimeout(320);
  const t = await page.evaluate(() => {
    const li = document.querySelector('[data-sonner-toast]');
    if (!li) return null;
    const r = li.getBoundingClientRect();
    const cs = getComputedStyle(li.closest("[data-sonner-toaster]") || li);
    const hits = [];
    document.querySelectorAll("button,input,textarea,[role=button],[role=combobox],[role=switch],[role=menuitem]").forEach((el) => {
      const rr = el.getBoundingClientRect();
      if (!rr.width) return;
      const cx = rr.left + rr.width / 2, cy = rr.top + rr.height / 2;
      if (cx >= r.left && cx <= r.right && cy >= r.top && cy <= r.bottom) {
        const top = document.elementFromPoint(cx, cy);
        if (top && top !== el && !el.contains(top) && !top.contains(el)) hits.push((el.getAttribute("aria-label") || el.textContent || el.getAttribute("placeholder") || el.tagName).trim().slice(0, 24));
      }
    });
    return { rect: { x: Math.round(r.x), y: Math.round(r.y), w: Math.round(r.width), h: Math.round(r.height) }, z: cs.zIndex, vw: innerWidth, vh: innerHeight, hits: [...new Set(hits)] };
  });
  say("\n### toast 遮挡（网关页复制后）", JSON.stringify(t));
  results.push({ toast: t });
  await page.screenshot({ path: path.resolve(`.vr/shots/deep2-${VW}x${VH}-toast-gateway.png`) }).catch(() => {});
}

// 弹窗内触发 toast：会不会压住弹窗自身的保存/取消
await fresh("模型");
const dlgToast = await (async () => {
  const b = page.locator("main button:has-text('别名') >> nth=0");
  if (!(await b.count())) return null;
  await b.click().catch(() => {});
  await page.waitForTimeout(400);
  const c = page.locator('[role="dialog"] button:has-text("复制"), [role="dialog"] [aria-label*="复制"]');
  if (!(await c.count())) return { noCopyBtn: true, dlg: await page.evaluate(OUTLINE) };
  await c.first().click().catch(() => {});
  await page.waitForTimeout(320);
  return await page.evaluate(() => {
    const li = document.querySelector('[data-sonner-toast]');
    const dlg = document.querySelector('[role="dialog"]');
    if (!li || !dlg) return { li: !!li, dlg: !!dlg };
    const lr = li.getBoundingClientRect(), dr = dlg.getBoundingClientRect();
    const overlap = Math.max(0, Math.min(lr.bottom, dr.bottom) - Math.max(lr.top, dr.top));
    const btns = Array.from(dlg.querySelectorAll("button")).map((x) => { const r = x.getBoundingClientRect(); const cx = r.left + r.width / 2, cy = r.top + r.height / 2; const top = document.elementFromPoint(cx, cy); return { n: (x.textContent || x.getAttribute("aria-label") || "").trim().slice(0, 10), y: Math.round(r.top), covered: !!(top && !x.contains(top) && !top.contains(x) && top.closest('[role="dialog"]') === null) }; });
    return { toast: { t: Math.round(lr.top), b: Math.round(lr.bottom), h: Math.round(lr.height) }, dialog: { t: Math.round(dr.top), b: Math.round(dr.bottom) }, overlapPx: Math.round(overlap), coveredBtns: btns.filter((x) => x.covered) };
  });
})();
say("\n### 弹窗内 toast 重叠:", JSON.stringify(dlgToast));
await page.screenshot({ path: path.resolve(`.vr/shots/deep2-${VW}x${VH}-toast-in-dialog.png`) }).catch(() => {});
results.push({ dlgToast });

fs.writeFileSync(path.resolve(`.vr/deep2-${VW}x${VH}.json`), JSON.stringify(results, null, 2));
say(`\n→ .vr/deep2-${VW}x${VH}.json`);
await browser.close();
