// 定位暗色 toast：预置 localStorage.theme=dark 后，检查 html class、sonner 的 data-theme/变量、toast 真实底色。
import fs from "node:fs";
import { createRequire } from "node:module";
const require = createRequire("/Users/jiangnan/Documents/workspace/deepseek-harness/node_modules/.pnpm/playwright@1.61.1/node_modules/playwright/");
const { chromium } = require("playwright");
const { fixtures } = await import(new URL("./fixtures.mjs", import.meta.url).href);
// 读仓库内源文件（相对本脚本解析）；曾读 `.vr/` 本地镜像，改了源文件会静默用旧副本
const mock = fs.readFileSync(new URL("./run.mjs", import.meta.url), "utf8").match(/function installMock\(\)[\s\S]*?\n}\n/)[0];
const src = fs.readFileSync(new URL("./audit.mjs", import.meta.url), "utf8");
const HELPERS = src.slice(src.indexOf("const oklabToLinear"), src.indexOf("  const vis =")).replace(/^\s{2}/gm, "");

const browser = await chromium.launch({ executablePath: "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome", headless: true });
const ctx = await browser.newContext({ viewport: { width: 1180, height: 800 }, locale: "zh-CN" });
await ctx.addInitScript({ content: "window.__JAI_FIX__=" + JSON.stringify(fixtures) + ";" });
await ctx.addInitScript({ content: "(" + mock + ")();" });
await ctx.addInitScript({ content: "try{localStorage.setItem('theme','dark');}catch(e){}" });
const page = await ctx.newPage();
await page.goto("http://127.0.0.1:5173/");
await page.waitForTimeout(1400);

console.log("html class =", await page.evaluate(() => document.documentElement.className));
console.log("localStorage.theme =", await page.evaluate(() => localStorage.getItem("theme")));
console.log("prefers-color-scheme dark? =", await page.evaluate(() => matchMedia("(prefers-color-scheme: dark)").matches));

await page.click('aside nav button[aria-label="设置"]');
await page.waitForTimeout(700);
await page.locator('main button', { hasText: /^保存$/ }).first().click();
await page.waitForTimeout(500);

const dump = await page.evaluate((helpers) => {
  const H = new Function(helpers + "\n;return {rgba, bgChain, ratio};")();
  const li = document.querySelector("ol.toaster li") || document.querySelector("[data-sonner-toast]");
  if (!li) return { err: "无 toast" };
  const cs = getComputedStyle(li);
  const vars = {};
  for (const k of ["--normal-bg", "--normal-text", "--success-bg", "--success-text", "--error-bg", "--error-text", "--foreground"]) {
    vars[k] = cs.getPropertyValue(k).trim();
  }
  const inner = li.querySelector("div > div") || li;
  const ics = getComputedStyle(inner);
  const sonnerRoot = li.closest("section") || li.parentElement;
  return {
    vars, liBg: cs.backgroundColor, liColor: cs.color,
    dataTheme: li.getAttribute("data-theme") || sonnerRoot?.getAttribute("data-theme") || document.querySelector("[data-sonner-toaster]")?.getAttribute("data-theme"),
    toasterAttrs: [...document.querySelectorAll("*")].filter((e) => e.hasAttribute("data-sonner-toaster")).map((e) => ({ tag: e.tagName, theme: e.getAttribute("data-theme"), style: e.getAttribute("style")?.slice(0, 200) })),
    innerColor: ics.color, innerBg: ics.backgroundColor,
    ratioW: H.ratio(H.rgba(ics.color), H.bgChain(inner, [255, 255, 255, 1])),
    ratioB: H.ratio(H.rgba(ics.color), H.bgChain(inner, [0, 0, 0, 1])),
    pointerEvents: cs.pointerEvents,
  };
}, HELPERS);
console.log(JSON.stringify(dump, null, 1));
await browser.close();
