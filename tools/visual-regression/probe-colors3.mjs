// 用审计脚本自身的色彩/对比度函数，在页面内复核具体元素的 fg/bg/ratio 原始值
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
const ctx = await browser.newContext({ viewport: { width: 980, height: 640 }, locale: "zh-CN" });
await ctx.addInitScript({ content: "window.__JAI_FIX__=" + JSON.stringify(fixtures) + ";" });
await ctx.addInitScript({ content: "(" + mock + ")();" });
const page = await ctx.newPage();
await page.goto("http://127.0.0.1:5173/");
await page.waitForTimeout(1100);
if (process.env.DARK === "1") {
  await page.evaluate(() => document.documentElement.classList.add("dark"));
  await page.waitForTimeout(300);
}
await page.click('aside nav button[aria-label="同步"]');
await page.waitForTimeout(700);
const vars = await page.evaluate(() => {
  const cs = getComputedStyle(document.documentElement);
  return { htmlClass: document.documentElement.className, primary: cs.getPropertyValue("--primary"), primaryFg: cs.getPropertyValue("--primary-foreground"), bg: cs.getPropertyValue("--background"), fg: cs.getPropertyValue("--foreground"), destructive: cs.getPropertyValue("--destructive"), destructiveFg: cs.getPropertyValue("--destructive-foreground") };
});
console.log("THEME", process.env.DARK === "1" ? "forced-dark" : "app-default", JSON.stringify(vars));

const res = await page.evaluate((helpers) => {
  const H = new Function(helpers + "\n;return {rgba, bgChain, ratio, lum, blend};")();
  const out = [];
  for (const el of document.querySelectorAll("main button")) {
    const t = (el.textContent || "").trim();
    if (!["保存配置", "导入", "测试连接", "推送"].includes(t)) continue;
    const cs = getComputedStyle(el);
    const fg = H.rgba(cs.color);
    const bgW = H.bgChain(el, [255, 255, 255, 1]);
    const bgB = H.bgChain(el, [0, 0, 0, 1]);
    out.push({
      text: t, disabled: el.disabled, opacity: cs.opacity,
      rawFg: cs.color, fg, bgRaw: cs.backgroundColor,
      bgW, bgB, rWhite: H.ratio(fg, bgW), rBlack: H.ratio(fg, bgB),
      innerOpacity: (() => { let n = el, chain = []; while (n && n !== document.body) { chain.push(getComputedStyle(n).opacity); n = n.parentElement; } return chain.slice(0, 4); })(),
    });
  }
  return out;
}, HELPERS);
for (const r of res) console.log(JSON.stringify(r));
await browser.close();
