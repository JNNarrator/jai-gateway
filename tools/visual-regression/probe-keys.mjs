// 网关多密钥（D9-T6a）面板探针：验证 GatewayPage 的密钥列表交互。
//
//   node tools/visual-regression/probe-keys.mjs --size=1180x800
//
// 判据（写进本文件，gate.mjs 只读 JSON）：
//   1. 列表渲染全部未吊销密钥，每行有前缀 / 备注 / 创建时间 / 最后使用
//   2. 「显示全文」后该行显示完整密钥（比前缀长），再点回到前缀
//   3. 「吊销」**先弹二次确认**（不是直接删）；确认后该行消失、其余行还在
//   4. 「新建密钥」把新行插到最前，且新行直接显示全文（唯一一次能拿到全文的时机）
//   5. 全部吊销后显示空态文案（不留下一个空列表）
//   6. 截断列自带 title（UI 门禁「截断有 title 兜底」是逐元素判的）
import fs from "node:fs";
import path from "node:path";
import { launchBrowser, VR_DIR, SHOTS_DIR, parseArgv, parseSize } from "./_env.mjs";

const { fixtures } = await import(new URL("./fixtures.mjs", import.meta.url).href);
const mock = fs
  .readFileSync(new URL("./run.mjs", import.meta.url), "utf8")
  .match(/function installMock\(\)[\s\S]*?\n}\n/)[0];

const argv = parseArgv();
const [VW, VH] = parseSize(argv.size, "1180x800");
const TAG = `probe-keys-${VW}x${VH}`;

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
const errors = [];
page.on("pageerror", (e) => errors.push(String(e).slice(0, 200)));
await page.goto("http://127.0.0.1:5173/");
await page.waitForTimeout(1400);

const readList = () =>
  page.evaluate(() => {
    const ul = document.querySelector('[data-testid="gateway-key-list"]');
    const empty = document.querySelector('[data-testid="gateway-keys-empty"]');
    const rows = [...(ul?.querySelectorAll('[data-testid="gateway-key-row"]') || [])].map(
      (li) => {
        const spans = [...li.querySelectorAll("span")];
        return {
          prefix: li.getAttribute("data-prefix"),
          text: li.textContent.trim().replace(/\s+/g, " ").slice(0, 70),
          shown: spans[0]?.textContent?.trim() ?? "",
          label: spans[1]?.textContent?.trim() ?? "",
          created: spans[2]?.textContent?.trim() ?? "",
          used: spans[3]?.textContent?.trim() ?? "",
          createdTitle: spans[2]?.getAttribute("title") ?? null,
          prefixTitle: spans[0]?.getAttribute("title") ?? null,
          clipped: spans
            .slice(0, 4)
            .some((s) => s.scrollWidth > s.clientWidth + 1 && !s.getAttribute("title")),
          buttons: [...li.querySelectorAll("button")].map((b) =>
            b.getAttribute("aria-label") || b.textContent.trim(),
          ),
        };
      },
    );
    return { emptyText: empty ? empty.textContent.trim() : null, rows };
  });

const initial = await readList();

// 显示全文 → 该行显示完整密钥；再点一次回到前缀
await page.click('button[aria-label="显示全文 sk-jai-pnxAR"]');
await page.waitForTimeout(250);
const revealed = await readList();
await page.click('button[aria-label="隐藏全文 sk-jai-pnxAR"]');
await page.waitForTimeout(250);
const hiddenAgain = await readList();

// 吊销：先弹二次确认（此刻列表不变），确认后该行消失
await page.click('button[aria-label="吊销密钥 sk-jai-Lp7Qm"]');
await page.waitForTimeout(250);
const confirmOpen = await page.evaluate(() => {
  const d = document.querySelector('[role="alertdialog"]');
  return d
    ? {
        title: (d.querySelector("h2")?.textContent || "").trim(),
        buttons: [...d.querySelectorAll("button")].map((b) => b.textContent.trim()),
        rowsStillThere: document.querySelectorAll('[data-testid="gateway-key-row"]')
          .length,
      }
    : null;
});
await page.screenshot({ path: path.join(SHOTS_DIR, `${TAG}-revoke-confirm.png`) });
await page.click('[role="alertdialog"] button:has-text("吊销")');
await page.waitForTimeout(400);
const afterRevoke = await readList();

// 新建：插到最前，且新行直接显示全文
await page.fill('input[aria-label="新密钥备注"]', "临时排查");
await page.click("button:has-text('新建密钥')");
await page.waitForTimeout(400);
const afterCreate = await readList();
await page.screenshot({ path: path.join(SHOTS_DIR, `${TAG}-after-create.png`) });

// 全部吊销 → 空态
for (const r of afterCreate.rows) {
  await page.click(`button[aria-label="吊销密钥 ${r.prefix}"]`);
  await page.waitForTimeout(150);
  await page.click('[role="alertdialog"] button:has-text("吊销")');
  await page.waitForTimeout(250);
}
const afterAllRevoked = await readList();
await page.screenshot({ path: path.join(SHOTS_DIR, `${TAG}-empty.png`) });

const out = {
  size: `${VW}x${VH}`,
  initial,
  revealed,
  hiddenAgain,
  confirmOpen,
  afterRevoke,
  afterCreate,
  afterAllRevoked,
  errors,
  shots: [
    `${TAG}-revoke-confirm.png`,
    `${TAG}-after-create.png`,
    `${TAG}-empty.png`,
  ],
};
fs.writeFileSync(path.join(VR_DIR, `out-${TAG}.json`), JSON.stringify(out, null, 1));
console.log(
  `[probe-keys] ${TAG} 初始=${initial.rows.length} 吊销后=${afterRevoke.rows.length} 新建后=${afterCreate.rows.length} 全吊销=${afterAllRevoked.rows.length}`,
);
await browser.close();
