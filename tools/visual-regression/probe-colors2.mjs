// 诊断：对同步页上被判定"对比度不达标"的元素，打印原始 computed 颜色与背景层链
import fs from "node:fs";
import path from "node:path";
import { createRequire } from "node:module";
const require = createRequire("/Users/jiangnan/Documents/workspace/deepseek-harness/node_modules/.pnpm/playwright@1.61.1/node_modules/playwright/");
const { chromium } = require("playwright");
const { fixtures } = await import(new URL("./fixtures.mjs", import.meta.url).href);
const mock = fs.readFileSync(path.resolve(".vr/run.mjs"), "utf8").match(/function installMock\(\)[\s\S]*?\n}\n/)[0];
const src = fs.readFileSync(path.resolve(".vr/audit.mjs"), "utf8");
const body = src.slice(src.indexOf("const oklabToLinear"), src.indexOf("const lum"));
const { rgba } = new Function(body + "\n;return {rgba};")();

const browser = await chromium.launch({ executablePath: "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome", headless: true });
const ctx = await browser.newContext({ viewport: { width: 980, height: 640 }, locale: "zh-CN" });
await ctx.addInitScript({ content: "window.__JAI_FIX__=" + JSON.stringify(fixtures) + ";" });
await ctx.addInitScript({ content: "(" + mock + ")();" });
const page = await ctx.newPage();
await page.goto("http://127.0.0.1:5173/");
await page.waitForTimeout(1100);
await page.click('aside nav button[aria-label="同步"]');
await page.waitForTimeout(700);

const dump = await page.evaluate(() => {
  const raw = [];
  const walk = (el) => {
    const layers = [];
    let n = el;
    while (n) {
      const cs = getComputedStyle(n);
      layers.push({ node: n.tagName + "." + String(n.className || "").split(/\s+/).slice(0, 2).join("."), bg: cs.backgroundColor, bgImage: cs.backgroundImage.slice(0, 40) });
      if (cs.backgroundColor && !/rgba\(0, 0, 0, 0\)|transparent/.test(cs.backgroundColor)) break;
      n = n.parentElement;
    }
    return layers;
  };
  const labels = ["保存配置", "测试连接", "导入", "恢复", "删除", "推送", "拉取", "预览变更"];
  for (const el of document.querySelectorAll("main button")) {
    const t = (el.textContent || "").trim();
    if (!labels.includes(t)) continue;
    const cs = getComputedStyle(el);
    raw.push({ text: t, cls: String(el.className).slice(0, 120), color: cs.color, bg: cs.backgroundColor, layers: walk(el), disabled: el.disabled });
  }
  const dlgs = [];
  for (const d of document.querySelectorAll('[data-slot="alert-dialog-action"],[data-slot="dialog-footer"] button')) {
    const cs = getComputedStyle(d);
    dlgs.push({ text: (d.textContent || "").trim(), cls: String(d.className).slice(0, 100), color: cs.color, bg: cs.backgroundColor, layers: walk(d) });
  }
  return { raw, dlgs };
});
console.log("=== 同步页按钮原始颜色 ===");
for (const r of dump.raw) {
  console.log(`\n[${r.text}] disabled=${r.disabled}`);
  console.log("  cls:", r.cls);
  console.log("  color:", r.color, "→", JSON.stringify(rgba(r.color)));
  console.log("  bg   :", r.bg, "→", JSON.stringify(rgba(r.bg)));
  for (const l of r.layers) console.log("    层:", l.node, "| bg:", l.bg, "| bgImage:", l.bgImage);
}
console.log("\n=== 弹窗 footer 按钮 ===");
for (const r of dump.dlgs.slice(0, 2)) {
  console.log(`\n[${r.text}]`);
  console.log("  cls:", r.cls);
  console.log("  color:", r.color, "→", JSON.stringify(rgba(r.color)));
  console.log("  bg   :", r.bg, "→", JSON.stringify(rgba(r.bg)));
}
await browser.close();
