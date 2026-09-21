// 定向解剖：弹窗滚动结构、toast 遮挡范围、下拉/Select 在视口边界的行为
// node .vr/deep.mjs --size=980x640
import { createRequire } from "node:module";
import fs from "node:fs";
import path from "node:path";

const require = createRequire(
  "/Users/jiangnan/Documents/workspace/deepseek-harness/node_modules/.pnpm/playwright@1.61.1/node_modules/playwright/",
);
const { chromium } = require("playwright");
const { fixtures } = await import(new URL("./fixtures.mjs", import.meta.url).href);
const argv = Object.fromEntries(process.argv.slice(2).map((s) => { const m = s.match(/^--([^=]+)(?:=(.*))?$/); return m ? [m[1], m[2] ?? true] : [s, true]; }));
const [VW, VH] = (argv.size || "980x640").split("x").map(Number);
// 读仓库内的 run.mjs（相对本脚本解析）；曾读 `.vr/run.mjs` 本地镜像，会静默用旧 mock
const runSrc = fs.readFileSync(new URL("./run.mjs", import.meta.url), "utf8");
const mockSrc = runSrc.match(/function installMock\(\)[\s\S]*?\n}\n/)?.[0];
const out = { vw: VW, vh: VH, cases: [] };
const say = (...a) => console.log(...a);

const browser = await chromium.launch({ executablePath: "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome", headless: true });
const ctx = await browser.newContext({ viewport: { width: VW, height: VH }, deviceScaleFactor: 2, locale: "zh-CN" });
await ctx.addInitScript({ content: `window.__JAI_FIX__ = ${JSON.stringify(fixtures)}; window.__VR_VW__=${VW}; window.__VR_VH__=${VH};` });
await ctx.addInitScript({ content: `(${mockSrc})();` });
const page = await ctx.newPage();
await page.goto(argv.url || "http://127.0.0.1:5173/", { waitUntil: "domcontentloaded" });
await page.waitForTimeout(1100);

const tab = async (label) => { await page.click(`aside nav button[aria-label="${label}"]`); await page.waitForTimeout(600); };

// 弹窗结构剖析：谁在滚动、主按钮在不在视口内、滚到底能不能到
const outline = () => page.evaluate(() => {
  const dlg = document.querySelector('[role="dialog"],[role="alertdialog"]');
  if (!dlg) return null;
  const vh = innerHeight;
  const R = (el) => { const r = el.getBoundingClientRect(); return { t: Math.round(r.top), b: Math.round(r.bottom), h: Math.round(r.height), w: Math.round(r.width) }; };
  const name = (el) => { const s = el.getAttribute("data-slot"); return s ? `${el.tagName.toLowerCase()}[${s}]` : el.tagName.toLowerCase() + (typeof el.className === "string" && el.className ? "." + el.className.split(/\s+/).slice(0, 3).join(".") : ""); };
  const scrollers = [dlg, ...dlg.querySelectorAll("*")].filter((n) => /(auto|scroll)/.test(getComputedStyle(n).overflowY) && n.scrollHeight > n.clientHeight + 1).map((n) => ({ node: name(n), scrollH: n.scrollHeight, clientH: n.clientHeight, rect: R(n), overflowY: getComputedStyle(n).overflowY, maxH: getComputedStyle(n).maxHeight }));
  const leaves = Array.from(dlg.querySelectorAll("button,input,textarea,select,[role=combobox],[role=switch]")).filter((el) => { const r = el.getBoundingClientRect(); return r.width > 0; });
  const outside = leaves.filter((el) => { const r = el.getBoundingClientRect(); return r.top < 0 || r.bottom > vh; }).map((el) => ({ name: (el.getAttribute("aria-label") || el.textContent || el.getAttribute("placeholder") || el.tagName).trim().slice(0, 26), rect: R(el), inScroller: !![...scrollers].some((s) => el.closest("*") && false) }));
  const primary = leaves.filter((el) => /保存|创建|更新|确定|取消|推送|拉取|导入|删除|恢复|测试/.test((el.textContent || "").trim())).map((el) => ({ text: (el.textContent || "").trim().slice(0, 10), rect: R(el), visible: el.getBoundingClientRect().top >= 0 && el.getBoundingClientRect().bottom <= vh }));
  const sticky = leaves.filter((el) => /(sticky)/.test(getComputedStyle(el).position)).map((el) => ({ text: (el.textContent || "").trim().slice(0, 12), pos: "sticky", rect: R(el) }));
  const footer = dlg.querySelector('[data-slot="dialog-footer"],[data-slot="alert-dialog-footer"]');
  // 弹窗正文被自身裁掉的元素（超出 dialog 的 padding box）
  const dr = dlg.getBoundingClientRect();
  const clippedByDlg = leaves.filter((el) => { const r = el.getBoundingClientRect(); return r.bottom > dr.bottom + 1 || r.top < dr.top - 1; }).map((el) => ({ name: (el.getAttribute("aria-label") || el.textContent || el.getAttribute("placeholder") || el.tagName).trim().slice(0, 26), rect: R(el) }));
  return {
    title: (dlg.querySelector("[data-slot=dialog-title],[data-slot=alert-dialog-title]")?.textContent || "").trim(),
    dlgRect: R(dlg), contentH: dlg.scrollHeight, clientH: dlg.clientHeight,
    overflowY: getComputedStyle(dlg).overflowY, maxHeight: getComputedStyle(dlg).maxHeight,
    scrollers, leaves: leaves.length, outsideViewport: outside, primary, sticky,
    footer: footer ? { rect: R(footer), visible: footer.getBoundingClientRect().bottom <= vh && footer.getBoundingClientRect().top >= 0, position: getComputedStyle(footer).position } : null,
    clippedByDialog: clippedByDlg,
  };
});

async function openBy(matchText, near = "last") {
  const i = await page.evaluate((mt) => {
    const els = Array.from(document.querySelectorAll("main button, main [role=button]")).filter((el) => (el.getAttribute("aria-label") || el.textContent || "").trim().includes(mt));
    els.forEach((el, k) => el.setAttribute("data-vr-t", String(k)));
    return els.length ? (mt === "__first__" ? 0 : 0) : -1;
  }, matchText);
  if (i < 0) return false;
  await page.locator(`main [data-vr-t="${i}"]`).first().click({ timeout: 3000 });
  await page.waitForTimeout(500);
  return true;
}

const cases = [
  ["providers", "供应商", ["添加供应商", "编辑", "测试连接", "删除"]],
  ["models", "模型", ["别名", "限额", "模态", "上下文"]],
  ["skills", "技能", ["添加技能", "编辑", "导入", "导出"]],
  ["mcp", "MCP", ["添加 MCP Server", "粘贴配置导入", "列出工具", "编辑"]],
  ["sync", "同步", ["预览变更", "推送", "从快照恢复", "恢复"]],
  ["settings", "设置", ["编辑保留策略", "添加域名", "检查更新", "填入示例"]],
  ["logs", "日志", ["清空", "导出", "刷新"]],
  ["gateway", "网关", ["重新生成", "显示", "复制"]],
];

for (const [key, label, texts] of cases) {
  await tab(label);
  for (const t of texts) {
    const okc = await openBy(t);
    if (!okc) { say(`— ${key}/${t}: 未找到入口`); continue; }
    const o = await outline();
    if (!o) { await page.keyboard.press("Escape"); continue; }
    say(`\n### ${key} / 「${t}」→ 弹窗「${o.title}」 视口${VH}`);
    say(`   dialog rect=${JSON.stringify(o.dlgRect)} contentH=${o.contentH} clientH=${o.clientH} overflowY=${o.overflowY} maxH=${o.maxHeight}`);
    say(`   滚动容器: ${o.scrollers.map((s) => `${s.node} scrollH=${s.scrollH}/clientH=${s.clientH} maxH=${s.maxH}`).join(" ; ") || "（无）"}`);
    say(`   主按钮: ${o.primary.map((p) => `${p.text}@${p.rect.t}-${p.rect.b}${p.visible ? "" : " ✗不可见"}`).join(" ; ")}`);
    say(`   footer: ${o.footer ? JSON.stringify(o.footer) : "无"}`);
    say(`   视口外控件(${o.outsideViewport.length}): ${o.outsideViewport.slice(0, 8).map((x) => `${x.name}@${x.rect.t}-${x.rect.b}`).join(" ; ")}`);
    say(`   被弹窗自身裁掉(${o.clippedByDialog.length}): ${o.clippedByDialog.slice(0, 8).map((x) => `${x.name}@${x.rect.t}-${x.rect.b}`).join(" ; ")}`);
    // 滚到底后再量主按钮
    const afterScroll = await page.evaluate(() => {
      const dlg = document.querySelector('[role="dialog"],[role="alertdialog"]');
      const sc = [dlg, ...dlg.querySelectorAll("*")].find((n) => /(auto|scroll)/.test(getComputedStyle(n).overflowY) && n.scrollHeight > n.clientHeight + 1);
      if (sc) sc.scrollTop = sc.scrollHeight;
      const vh = innerHeight;
      return { scrolled: !!sc, primary: Array.from(dlg.querySelectorAll("button")).filter((b) => /保存|创建|更新|确定|推送|恢复|删除/.test(b.textContent || "")).map((b) => { const r = b.getBoundingClientRect(); return `${(b.textContent || "").trim().slice(0, 8)}@${Math.round(r.top)}-${Math.round(r.bottom)}${r.top >= 0 && r.bottom <= vh ? "可见" : "不可见"}`; }) };
    });
    say(`   滚动到底后: scrolled=${afterScroll.scrolled} ${afterScroll.primary.join(" ; ")}`);
    await page.screenshot({ path: path.resolve(`.vr/shots/deep-${VW}x${VH}-${key}-${t.replace(/\s/g, "")}.png`) });
    out.cases.push({ key, trigger: t, outline: o, afterScroll });
    for (let k = 0; k < 3; k++) { await page.keyboard.press("Escape"); await page.waitForTimeout(180); }
    await page.mouse.click(4, 4);
  }
}

// toast 遮挡范围
await tab("网关");
const victims = await page.evaluate(() => {
  const before = document.querySelectorAll("button,input,textarea,[role=button],[role=combobox]").length;
  return before;
});
await page.locator('main button:has-text("复制")').first().click().catch(() => {});
await page.waitForTimeout(500);
const toastInfo = await page.evaluate(() => {
  const t = document.querySelector("[data-sonner-toaster]");
  if (!t) return null;
  const r = t.getBoundingClientRect();
  const cs = getComputedStyle(t);
  const hits = [];
  document.querySelectorAll("button,input,textarea,[role=button],[role=combobox],[role=switch]").forEach((el) => {
    const rr = el.getBoundingClientRect();
    if (rr.width < 1) return;
    const cx = rr.left + rr.width / 2, cy = rr.top + rr.height / 2;
    if (cx >= r.left && cx <= r.right && cy >= r.top && cy <= r.bottom) {
      const top = document.elementFromPoint(cx, cy);
      if (top && top !== el && !el.contains(top) && !top.contains(el)) hits.push({ name: (el.getAttribute("aria-label") || el.textContent || el.getAttribute("placeholder") || el.tagName).trim().slice(0, 26), rect: { t: Math.round(rr.top), b: Math.round(rr.bottom), l: Math.round(rr.left), r: Math.round(rr.right) }, by: (top.getAttribute("data-sonner-toast") !== null ? "toast" : top.tagName.toLowerCase() + "." + (top.className || "").toString().split(/\s+/)[0]).slice(0, 20) });
    }
  });
  return { rect: { x: Math.round(r.x), y: Math.round(r.y), w: Math.round(r.width), h: Math.round(r.height) }, z: cs.zIndex, position: cs.position, vh: innerHeight, vw: innerWidth, toasts: document.querySelectorAll("[data-sonner-toast]").length, covered: hits, gap: Math.round(innerHeight - r.bottom) };
});
say("\n### toast 遮挡（网关页，点复制后）");
say("   toaster rect:", JSON.stringify(toastInfo?.rect), "bottomGap=", toastInfo?.gap, "z=", toastInfo?.z, "被遮住控件数=", toastInfo?.covered.length);
say("   样例:", (toastInfo?.covered || []).slice(0, 6).map((c) => `${c.name}(${c.by})`).join(" ; "));
out.toast = toastInfo;

// 打开弹窗后再触发 toast：toast 会不会盖住弹窗主按钮
await tab("模型");
await openBy("别名").catch(() => {});
const toastOverDialog = await page.evaluate(() => {
  const dlg = document.querySelector('[role="dialog"]');
  if (!dlg) return "no dialog";
  const t = document.querySelector("[data-sonner-toaster]");
  const dr = dlg.getBoundingClientRect();
  const tr = t?.getBoundingClientRect();
  return { dlgRect: { t: Math.round(dr.top), b: Math.round(dr.bottom) }, toastRect: tr ? { t: Math.round(tr.top), b: Math.round(tr.bottom) } : null, overlap: tr ? Math.max(0, Math.min(dr.bottom, tr.bottom) - Math.max(dr.top, tr.top)) : 0 };
});
say("\n### toast 与弹窗重叠:", JSON.stringify(toastOverDialog));

// 底部行内的下拉/Select：菜单会不会被视口切
await tab("模型");
const menuProbe = await page.evaluate(() => {
  const res = [];
  document.querySelectorAll('[role="combobox"], [aria-haspopup="menu"]').forEach((el) => {
    const r = el.getBoundingClientRect();
    if (r.width) res.push({ name: (el.getAttribute("aria-label") || el.textContent || "").trim().slice(0, 20), y: Math.round(r.top), h: Math.round(r.height) });
  });
  return res;
});
say("\n### 页内 combobox/menu 触发器:", JSON.stringify(menuProbe).slice(0, 400));
await page.screenshot({ path: path.resolve(`.vr/shots/deep-${VW}x${VH}-final.png`) });

fs.writeFileSync(path.resolve(`.vr/deep-${VW}x${VH}.json`), JSON.stringify(out, null, 2));
say(`\n→ .vr/deep-${VW}x${VH}.json`);
await browser.close();
