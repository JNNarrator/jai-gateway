// 渠道草稿端点探测（D9-T1）面板探针：验证「添加供应商」弹窗里的探测区块。
//
//   node tools/visual-regression/probe-endpoint.mjs --size=980x640
//
// 判据（写进本文件，gate.mjs 只读 JSON）：
//   1. 面板真的渲染出每一行（含信息性行），且行内有状态/分类中文短语/延迟
//   2. 信息性行灰显（与主探测行颜色不同）
//   3. 截断的摘要自带 title（UI 门禁「截断有 title 兜底」判据是逐元素看的）
//   4. 被拦截的地址走 url_blocked 文案（不发请求的那条路径在 UI 上也说人话）
//   5. 探测不改表单：脏状态标记不出现（`useDirtyGuard` 不受影响）
import fs from "node:fs";
import path from "node:path";
import { launchBrowser, VR_DIR, SHOTS_DIR, parseArgv, parseSize } from "./_env.mjs";

const { fixtures } = await import(new URL("./fixtures.mjs", import.meta.url).href);
const mock = fs
  .readFileSync(new URL("./run.mjs", import.meta.url), "utf8")
  .match(/function installMock\(\)[\s\S]*?\n}\n/)[0];

const argv = parseArgv();
const [VW, VH] = parseSize(argv.size, "980x640");
const TAG = `probe-endpoint-${VW}x${VH}`;

const browser = await launchBrowser();
const ctx = await browser.newContext({
  viewport: { width: VW, height: VH },
  deviceScaleFactor: 1,
  locale: "zh-CN",
});
await ctx.addInitScript({
  content: `window.__JAI_FIX__ = ${JSON.stringify(fixtures)}; window.__VR_VW__=${VW}; window.__VR_VH__=${VH};`,
});
await ctx.addInitScript({ content: `(${mock})();` });
const page = await ctx.newPage();
await page.goto("http://127.0.0.1:5173/");
await page.waitForTimeout(1200);

// 进「供应商」→ 打开「添加供应商」
await page.click('aside nav button[aria-label="供应商"]');
await page.waitForTimeout(600);
await page.click("button:has-text('添加供应商')");
await page.waitForTimeout(400);

// 面板初始态：还没有结论
const before = await page.evaluate(() => ({
  hasPanel: !!document.body.textContent.includes("端点探测"),
  hasRows: !!document.querySelector('[role="dialog"] ul li'),
}));

// 填一个探测用模型，然后点「端点探测」（**不动其它表单字段**）
await page.fill("#pf-probe-model", "gpt-4o-mini");
await page.click("button:has-text('端点探测')");
await page.waitForTimeout(500);

const readRows = () =>
  page.evaluate(() => {
    const dlg = document.querySelector('[role="dialog"]');
    const rows = [...dlg.querySelectorAll("ul li")].map((li) => {
      const spans = [...li.querySelectorAll("span")];
      const msgSpan = spans[spans.length - 1];
      const cs = getComputedStyle(li);
      return {
        text: li.textContent.trim().replace(/\s+/g, " ").slice(0, 60),
        endpoint: spans[0]?.textContent?.trim() ?? "",
        status: spans[1]?.textContent?.trim() ?? "",
        latency: spans[2]?.textContent?.trim() ?? "",
        opacity: Number(cs.opacity),
        msgColor: msgSpan ? getComputedStyle(msgSpan).color : "",
        msgTitle: msgSpan?.getAttribute("title") ?? null,
        clipped: msgSpan ? msgSpan.scrollWidth > msgSpan.clientWidth + 1 : false,
        hasIcon: !!li.querySelector("svg"),
      };
    });
    return {
      rows,
      mainOpacity: rows[0]?.opacity ?? null,
      infoOpacity: rows[1]?.opacity ?? null,
      billedHint: dlg.textContent.includes("可能产生计费"),
      runLine: (dlg.textContent.match(/探测结论（run [0-9a-f]{8}）/) || [""])[0],
    };
  });

const first = await readRows();
await page.screenshot({
  path: path.join(SHOTS_DIR, `${TAG}-probed.png`),
  fullPage: false,
});

// 探测不该把表单标脏：试着重开一次弹窗（脏状态只在「离开」时弹拦截框）。
// 判据是「导航成功、没有出现『有未保存的改动』」。
await page.keyboard.press("Escape");
await page.waitForTimeout(300);
await page.click('aside nav button[aria-label="网关"]');
await page.waitForTimeout(500);
const navAway = await page.evaluate(() => ({
  alertOpen: !!document.querySelector('[role="alertdialog"]'),
  onProviders: !!document.body.textContent.includes("添加供应商"),
}));

// 换一个会被 SSRF 校验拦下的地址 → url_blocked 文案
await page.click('aside nav button[aria-label="供应商"]');
await page.waitForTimeout(400);
await page.click("button:has-text('添加供应商')");
await page.waitForTimeout(400);
await page.fill("#pf-probe-model", "gpt-4o-mini");
await page.fill("#pf-url", "http://169.254.169.254/latest/meta-data/");
await page.click("button:has-text('端点探测')");
await page.waitForTimeout(400);
const blocked = await readRows();
await page.screenshot({
  path: path.join(SHOTS_DIR, `${TAG}-blocked.png`),
  fullPage: false,
});

const after = await page.evaluate(() => ({
  dialogOpen: !!document.querySelector('[role="dialog"]'),
}));

const out = {
  size: `${VW}x${VH}`,
  before,
  first,
  navAway,
  blocked,
  after,
  shots: [`${TAG}-probed.png`, `${TAG}-blocked.png`],
};
fs.writeFileSync(path.join(VR_DIR, `out-${TAG}.json`), JSON.stringify(out, null, 1));
console.log(`[probe-endpoint] ${TAG} 行数=${first.rows.length} 拦截行=${blocked.rows.length}`);
console.log(JSON.stringify(out, null, 1));
await browser.close();
