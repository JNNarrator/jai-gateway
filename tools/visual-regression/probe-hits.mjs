// 验证「有效命中区」：视觉盒子之外的点击是否仍落在该控件上（伪元素/label 扩展对
// rect 测量的审计不可见，故用 elementFromPoint 逐点探测）。
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
await page.waitForTimeout(1300);

// 返回 sel 命中的第一个元素的有效命中区（元素→其自身或其后代/祖先 label 都被算作"命中"）
const measure = `(sel) => {
  const el = document.querySelector(sel);
  if (!el) return { err: "未找到 " + sel };
  const r = el.getBoundingClientRect();
  const cx = r.left + r.width / 2, cy = r.top + r.height / 2;
  // 只认「命中元素本身 / 其后代 / 包裹它的 label」；祖先（父 td、父 span）不算命中，
  // 否则整行都算命中，会得出虚高的有效命中区。
  const owns = (t) => !!t && (t === el || el.contains(t) || (t.tagName === "LABEL" && t.contains(el)));
  const reach = (dx, dy) => { const t = document.elementFromPoint(cx + dx, cy + dy); return owns(t); };
  let maxX = 0, maxY = 0;
  for (let d = 0; d <= 40; d++) { if (reach(d, 0) && reach(-d, 0)) maxX = d; else break; }
  for (let d = 0; d <= 40; d++) { if (reach(0, d) && reach(0, -d)) maxY = d; else break; }
  // 采样自中心，故有效尺寸 = 2×可达半径（对称探测）
  return { rect: { w: Math.round(r.width), h: Math.round(r.height) }, effective: { w: 2 * maxX, h: 2 * maxY }, reachX: maxX, reachY: maxY };
}`;

const report = async (page_, label, sel, nav) => {
  if (nav) { await page_.click(`aside nav button[aria-label="${nav}"]`); await page_.waitForTimeout(900); }
  const r = await page_.evaluate(new Function("sel", "return (" + measure + ")(sel)"), sel);
  console.log(`${label}\n   ${JSON.stringify(r)}`);
};

await report(page, "① 模型页「复制模型名」按钮（视觉 16×16）", 'button[aria-label^="复制模型名"]', "模型");
await report(page, "② 技能页批量勾选框（视觉 16×16）", 'input[aria-label^="选择"]', "技能");
await report(page, "③ 供应商页「官网」链接（视觉 40×16）", "main button.inline-flex[title^='http']", "供应商");

await browser.close();
