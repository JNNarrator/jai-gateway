// 验证：① 网关页吸顶操作条滚动后仍在视口内；② 设置页锚点导航的覆盖面。
import fs from "node:fs";
import { createRequire } from "node:module";
const require = createRequire("/Users/jiangnan/Documents/workspace/deepseek-harness/node_modules/.pnpm/playwright@1.61.1/node_modules/playwright/");
const { chromium } = require("playwright");
const { fixtures } = await import(new URL("./fixtures.mjs", import.meta.url).href);
// 读仓库内源文件（相对本脚本解析）；曾读 `.vr/` 本地镜像，改了源文件会静默用旧副本
const mock = fs.readFileSync(new URL("./run.mjs", import.meta.url), "utf8").match(/function installMock\(\)[\s\S]*?\n}\n/)[0];

const browser = await chromium.launch({ executablePath: "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome", headless: true });
const ctx = await browser.newContext({ viewport: { width: 1180, height: 800 }, locale: "zh-CN" });
await ctx.addInitScript({ content: "window.__JAI_FIX__=" + JSON.stringify(fixtures) + ";" });
await ctx.addInitScript({ content: "(" + mock + ")();" });
const page = await ctx.newPage();
await page.goto("http://127.0.0.1:5173/");
await page.waitForTimeout(1200);

// ① 网关页
await page.click('aside nav button[aria-label="网关"]');
await page.waitForTimeout(900);
const before = await page.evaluate(() => {
  const b = [...document.querySelectorAll("main button")].find((x) => /复制 MCP 配置/.test(x.textContent));
  const r = b?.getBoundingClientRect();
  return r ? { top: Math.round(r.top), bottom: Math.round(r.bottom) } : null;
});
await page.evaluate(() => { const m = document.querySelector("main"); m.scrollTop = 700; });
await page.waitForTimeout(400);
const after = await page.evaluate(() => {
  const b = [...document.querySelectorAll("main button")].find((x) => /复制 MCP 配置/.test(x.textContent));
  const r = b?.getBoundingClientRect();
  const m = document.querySelector("main");
  return { scrollTop: m.scrollTop, rect: r ? { top: Math.round(r.top), bottom: Math.round(r.bottom) } : null, inViewport: r ? r.top >= 0 && r.bottom <= innerHeight : false };
});
console.log("① 网关页「复制 MCP 配置」 滚动前:", JSON.stringify(before), " 滚动 700px 后:", JSON.stringify(after));

// ② 设置页锚点导航
await page.click('aside nav button[aria-label="设置"]');
await page.waitForTimeout(800);
const nav = await page.evaluate(() => {
  const n = document.querySelector("main nav");
  if (!n) return { err: "无 nav" };
  const cs = getComputedStyle(n);
  return {
    sticky: cs.position,
    items: [...n.querySelectorAll("button")].map((b) => ({ text: b.textContent.trim(), target: b.getAttribute("data-target") || null })),
    cards: [...document.querySelectorAll("main [id]")].map((c) => c.id),
  };
});
console.log("② 设置页锚点导航:", JSON.stringify(nav, null, 1));
await browser.close();
