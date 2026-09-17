// 验证顶部渐隐遮罩（item 11）：① 渐变真的渲染；② 滚动后贴在滚动区真正的裁切边（偏移 0）；
// ③ 不拦截点击（命中测试穿透到下层元素）；④ 不占布局高度（main.scrollHeight 不变）。
import fs from "node:fs";
import path from "node:path";
import { createRequire } from "node:module";
const require = createRequire("/Users/jiangnan/Documents/workspace/deepseek-harness/node_modules/.pnpm/playwright@1.61.1/node_modules/playwright/");
const { chromium } = require("playwright");
const { fixtures } = await import(new URL("./fixtures.mjs", import.meta.url).href);
const mock = fs.readFileSync(path.resolve(".vr/run.mjs"), "utf8").match(/function installMock\(\)[\s\S]*?\n}\n/)[0];

const browser = await chromium.launch({ executablePath: "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome", headless: true });
const ctx = await browser.newContext({ viewport: { width: 1180, height: 800 }, locale: "zh-CN" });
await ctx.addInitScript({ content: "window.__JAI_FIX__=" + JSON.stringify(fixtures) + ";" });
await ctx.addInitScript({ content: "(" + mock + ")();" });
const page = await ctx.newPage();
await page.goto("http://127.0.0.1:5173/");
await page.waitForTimeout(1300);

const fadeInfo = `() => {
  const host = document.querySelector("main + div[aria-hidden], div[aria-hidden]:has(+ main)");
  if (!host) return { err: "无遮罩元素" };
  const inner = host;
  const main = document.querySelector("main");
  const mr = main.getBoundingClientRect();
  const ir = inner.getBoundingClientRect();
  const hcs = getComputedStyle(host), ics = getComputedStyle(inner);
  // 在遮罩带中点做命中测试：应穿透到下层（不是遮罩自己）
  const probeX = mr.left + mr.width / 2;
  const midY = mr.top + ir.height / 2;
  const hit = document.elementFromPoint(probeX, midY);
  return {
    gradient: ics.backgroundImage.slice(0, 60),
    hostPointerEvents: hcs.pointerEvents,
    innerPointerEvents: ics.pointerEvents,
    zIndex: hcs.zIndex,
    fadeTopOffsetFromMainTop: Math.round(ir.top - mr.top),
    fadeHeight: Math.round(ir.height),
    hitTestAtBand: hit ? (hit === host || host.contains(hit) ? "遮罩自己(会拦点击)" : hit.tagName + "(穿透OK)") : "null",
    scrollHeight: main.scrollHeight,
  };
}`;

for (const [nav, label] of [["技能", "技能页(无吸顶条)"], ["网关", "网关页(有吸顶条)"], ["日志", "日志页(有吸顶表头)"]]) {
  await page.click(`aside nav button[aria-label="${nav}"]`);
  await page.waitForTimeout(700);
  const at0 = await page.evaluate(new Function("return (" + fadeInfo + ")()"));
  await page.evaluate(() => { document.querySelector("main").scrollTop = 400; });
  await page.waitForTimeout(300);
  const at400 = await page.evaluate(new Function("return (" + fadeInfo + ")()"));
  console.log(`\n${label}\n  未滚动: ${JSON.stringify(at0)}\n  滚动后: ${JSON.stringify(at400)}`);
}

// 吸顶条按钮仍可点到（遮罩不覆盖 z-10 的条）
await page.click('aside nav button[aria-label="网关"]');
await page.waitForTimeout(700);
await page.evaluate(() => { document.querySelector("main").scrollTop = 500; });
await page.waitForTimeout(300);
const clickable = await page.evaluate(() => {
  const btn = [...document.querySelectorAll("main button")].find((b) => /复制 MCP 配置/.test(b.textContent));
  const r = btn.getBoundingClientRect();
  const hit = document.elementFromPoint(r.left + r.width / 2, r.top + r.height / 2);
  return { buttonTop: Math.round(r.top), hitIsButton: hit === btn || btn.contains(hit) };
});
console.log("\n网关页吸顶条按钮命中:", JSON.stringify(clickable));
await browser.close();
