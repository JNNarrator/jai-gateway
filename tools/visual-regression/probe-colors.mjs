// 颜色真值探针：确认主按钮/侧栏/卡片的实际 computed 颜色，校验对比度算法的输入
import fs from "node:fs";
import path from "node:path";
import { createRequire } from "node:module";
const require = createRequire("/Users/jiangnan/Documents/workspace/deepseek-harness/node_modules/.pnpm/playwright@1.61.1/node_modules/playwright/");
const { chromium } = require("playwright");
const { fixtures } = await import(new URL("./fixtures.mjs", import.meta.url).href);
const mock = fs.readFileSync(path.resolve(".vr/run.mjs"), "utf8").match(/function installMock\(\)[\s\S]*?\n}\n/)[0];

const browser = await chromium.launch({ executablePath: "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome", headless: true });
const ctx = await browser.newContext({ viewport: { width: 980, height: 640 }, locale: "zh-CN" });
await ctx.addInitScript({ content: "window.__JAI_FIX__=" + JSON.stringify(fixtures) + ";" });
await ctx.addInitScript({ content: "(" + mock + ")();" });
const page = await ctx.newPage();
await page.goto("http://127.0.0.1:5173/");
await page.waitForTimeout(1200);

const info = await page.evaluate(() => {
  const q = (s) => document.querySelector(s);
  const cs0 = getComputedStyle(document.documentElement);
  const out = {
    htmlClass: document.documentElement.className,
    vars: {
      primary: cs0.getPropertyValue("--primary"),
      primaryFg: cs0.getPropertyValue("--primary-foreground"),
      bg: cs0.getPropertyValue("--background"),
      fg: cs0.getPropertyValue("--foreground"),
      card: cs0.getPropertyValue("--card"),
      muted: cs0.getPropertyValue("--muted-foreground"),
    },
  };
  const btn = Array.from(document.querySelectorAll("main button")).find((x) => /保存配置|启动网关|复制配置/.test(x.textContent || ""));
  if (btn) {
    const cs = getComputedStyle(btn);
    out.btn = { text: (btn.textContent || "").trim().slice(0, 12), cls: String(btn.className).slice(0, 90), bg: cs.backgroundColor, color: cs.color };
    const inner = btn.querySelector("span") || btn;
    out.btnInner = { tag: inner.tagName, color: getComputedStyle(inner).color, text: (inner.textContent || "").trim().slice(0, 12) };
  }
  const side = q("aside");
  if (side) out.side = { bg: getComputedStyle(side).backgroundColor, color: getComputedStyle(side).color };
  const card = q('[data-slot="card"]');
  if (card) out.card = { bg: getComputedStyle(card).backgroundColor };
  const nav = q("aside nav button");
  if (nav) out.navBtn = { cls: String(nav.className).slice(0, 80), bg: getComputedStyle(nav).backgroundColor, color: getComputedStyle(nav).color };
  return out;
});
console.log(JSON.stringify(info, null, 1));

// 用探针里的真实解析函数算一遍这些颜色
const src = fs.readFileSync(path.resolve(".vr/audit.mjs"), "utf8");
const body = src.slice(src.indexOf("const oklabToLinear"), src.indexOf("const lum"));
const { rgba } = new Function(body + "\n;return {rgba};")();
for (const k of ["primary", "primaryFg", "bg", "fg", "card", "muted"]) {
  const v = info.vars[k];
  if (v) console.log(k.padEnd(10), v.trim().padEnd(34), JSON.stringify(rgba(v.trim())));
}
if (info.btn) {
  console.log("button bg", info.btn.bg, JSON.stringify(rgba(info.btn.bg)), "color", info.btn.color, JSON.stringify(rgba(info.btn.color)));
  const lum = ([r, g, b]) => { const f = (v) => { v /= 255; return v <= 0.03928 ? v / 12.92 : Math.pow((v + 0.055) / 1.055, 2.4); }; return 0.2126 * f(r) + 0.7152 * f(g) + 0.0722 * f(b); };
  const ratio = (a, b) => { const L1 = lum(a), L2 = lum(b); return +(((Math.max(L1, L2) + 0.05) / (Math.min(L1, L2) + 0.05))).toFixed(2); };
  const fgC = rgba(info.btnInner?.color || info.btn.color), bgC = rgba(info.btn.bg);
  if (fgC && bgC) console.log("=> 按钮文字对比度", ratio(fgC, bgC));
}
await browser.close();
