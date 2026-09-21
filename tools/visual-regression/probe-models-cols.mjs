// 检查模型表在窄窗口下的列可见性与表宽（验证「模态（入/出）」列降级）。
import fs from "node:fs";
import { createRequire } from "node:module";
const require = createRequire("/Users/jiangnan/Documents/workspace/deepseek-harness/node_modules/.pnpm/playwright@1.61.1/node_modules/playwright/");
const { chromium } = require("playwright");
const { fixtures } = await import(new URL("./fixtures.mjs", import.meta.url).href);
// 读仓库内源文件（相对本脚本解析）；曾读 `.vr/` 本地镜像，改了源文件会静默用旧副本
const mock = fs.readFileSync(new URL("./run.mjs", import.meta.url), "utf8").match(/function installMock\(\)[\s\S]*?\n}\n/)[0];

const size = (process.argv.find((a) => a.startsWith("--size=")) || "--size=900x600").split("=")[1];
const [W, H] = size.split("x").map(Number);

const browser = await chromium.launch({ executablePath: "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome", headless: true });
const ctx = await browser.newContext({ viewport: { width: W, height: H }, locale: "zh-CN" });
await ctx.addInitScript({ content: "window.__JAI_FIX__=" + JSON.stringify(fixtures) + ";" });
await ctx.addInitScript({ content: "(" + mock + ")();" });
const page = await ctx.newPage();
await page.goto("http://127.0.0.1:5173/");
await page.waitForTimeout(1300);
await page.click('aside nav button[aria-label="模型"]');
await page.waitForTimeout(900);

const out = await page.evaluate(() => {
  const table = document.querySelector("main table");
  const heads = [...table.querySelectorAll("thead th")].map((th) => ({
    text: th.textContent.trim().slice(0, 12),
    display: getComputedStyle(th).display,
    w: Math.round(th.getBoundingClientRect().width),
  }));
  const firstRow = table.querySelector("tbody tr");
  const badge = firstRow ? firstRow.querySelector("[title^='模态标注']") : null;
  return {
    viewport: innerWidth,
    tableW: Math.round(table.getBoundingClientRect().width),
    containerW: Math.round(table.parentElement.getBoundingClientRect().width),
    heads,
    compactBadge: badge ? { text: badge.textContent.trim(), display: getComputedStyle(badge).display, title: badge.getAttribute("title") } : null,
    hintVisible: (() => { const p = [...document.querySelectorAll("main p")].find((x) => /窗口较窄/.test(x.textContent)); return p ? getComputedStyle(p).display : "无提示元素"; })(),
  };
});
console.log(size, JSON.stringify(out, null, 1));
await browser.close();
