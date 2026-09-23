// 网关「密钥管理」（D9-T6a）面板探针：验证 GatewayPage 的密钥列表交互与排版。
//
//   node tools/visual-regression/probe-keys.mjs --size=1180x800
//
// 判据（写进本文件，gate.mjs 只读 JSON）：
//   1. 列表渲染全部未吊销密钥，每行有前缀 / 备注 / 最后使用（创建时间在 title 里）
//   2. **每行只有一行高**（不换行）—— 2026-09-23 排版整改的核心：此前每行 4 个按钮
//      必然换行成两行、按钮掉到第二行与元数据对不齐
//   3. 点前缀就地显示全文（比前缀长），再点回到前缀
//   4. 「⋯」菜单里有「显示全文 / 吊销」—— 低频与危险动作不再各占一个行内按钮
//   5. 「吊销」**先弹二次确认**（不是直接删）；确认后该行消失、其余行还在
//   6. 「新建密钥」把新行插到最前，且新行直接显示全文（唯一一次能拿到全文的时机）
//   7. 全部吊销后显示空态文案（不留下一个空列表）
//   8. 排版硬指标：内容宽度吃满窗口、按钮高度档位收敛、**页面上没有实心红按钮**
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

// 夹具里三把的 id 顺序 = 「新建在前」（与 gw_keys_active 契约一致）
const K_CI = "sk-jai-Ci9Zx";
const K_LAPTOP = "sk-jai-Lp7Qm";
const K_MAIN = "sk-jai-pnxAR";

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

const rowSel = (prefix) => `[data-testid="gateway-key-row"][data-prefix="${prefix}"]`;

/** 读列表：每行按 data-testid 取字段（不按下标取 span —— 排版一改就错位） */
const readList = () =>
  page.evaluate(() => {
    const main = document.querySelector("main");
    const ul = document.querySelector('[data-testid="gateway-key-list"]');
    const empty = document.querySelector('[data-testid="gateway-keys-empty"]');
    const rows = [...(ul?.querySelectorAll('[data-testid="gateway-key-row"]') || [])].map((li) => {
      const q = (sel) => li.querySelector(`[data-testid="${sel}"]`);
      const shownEl = q("gateway-key-prefix");
      const labelEl = q("gateway-key-label");
      const usedEl = q("gateway-key-used");
      const rulesEl = q("gateway-key-rules");
      return {
        prefix: li.getAttribute("data-prefix"),
        shown: shownEl?.textContent?.trim() ?? "",
        shownTitle: shownEl?.getAttribute("title") ?? null,
        label: labelEl?.textContent?.trim() ?? "",
        used: usedEl?.textContent?.trim() ?? "",
        /** 创建时间移到了 hover 提示里（低频信息让位给「最后使用」） */
        usedTitle: usedEl?.getAttribute("title") ?? null,
        rulesState: rulesEl?.textContent?.trim() ?? "",
        limited: rulesEl?.getAttribute("data-limited") === "1",
        /** 行高（仅作记录；真正的「有没有换行」看下面的 childTops） */
        h: Math.round(li.getBoundingClientRect().height),
        /** 直接子元素各自的行**中心**（四舍五入到 4px 容差）：只要有一个不同 ⇒ 行内换行了。
         *  这是「按钮排版」回归的**真判据**：
         *  - 不能看行高：h-8 按钮 + py-2 = 50px，与旧版换行时的 52/58 几乎同高；
         *  - 也不能看 `top`：`items-center` 下 16px 的文本与 32px 的按钮本来就 top 不同
         *    （它们仍在同一行，只是各自垂直居中）。
         *  中心对齐才是「同一行」的正确判据。 */
        childCenters: [
          ...new Set(
            [...li.children].map((c) => {
              const r = c.getBoundingClientRect();
              return Math.round((r.top + r.height / 2) / 4) * 4;
            }),
          ),
        ],
        clipped: [shownEl, labelEl, usedEl].some(
          (s) => s && s.scrollWidth > s.clientWidth + 1 && !s.getAttribute("title"),
        ),
        /** 行内的**设计系统按钮**（复制 / ⋯）—— 前缀与规则 chip 是文本/徽标控件，
         *  不算「按钮」，别把两种affordance混成一个数字。 */
        buttons: [...li.querySelectorAll('button[data-variant][data-size]')].map(
          (b) => b.getAttribute("aria-label") || b.textContent.trim(),
        ),
      };
    });
    // 排版硬指标
    const box = document.querySelector("main > div")?.getBoundingClientRect();
    // 只统计设计系统按钮（data-slot=button）：文本链接 / chip 的盒高不该混进来
    const heights = [...main.querySelectorAll("button[data-variant][data-size]")].map((b) =>
      Math.round(b.getBoundingClientRect().height),
    );
    return {
      emptyText: empty ? empty.textContent.trim() : null,
      rows,
      mainW: main.clientWidth,
      contentW: box ? Math.round(box.width) : 0,
      scrollH: main.scrollHeight,
      viewportH: main.clientHeight,
      buttonHeights: [...new Set(heights)].sort((a, b) => a - b),
      // 实心红按钮（Button variant=destructive）在**页面主体**上一个都不该有
      filledDanger: document.querySelectorAll('main button[data-variant="destructive"]').length,
      rowButtons: rows.length ? rows[0].buttons.length : 0,
    };
  });

const openRowMenu = async (prefix) => {
  await page.click(`${rowSel(prefix)} button[aria-label="更多操作 ${prefix}"]`);
  await page.waitForTimeout(220);
};

const initial = await readList();

// ── 显示全文 → 点前缀就地展开；再点回到前缀
await page.click(`${rowSel(K_MAIN)} [data-testid="gateway-key-prefix"]`);
await page.waitForTimeout(250);
const revealed = await readList();
await page.click(`${rowSel(K_MAIN)} [data-testid="gateway-key-prefix"]`);
await page.waitForTimeout(250);
const hiddenAgain = await readList();

// ── 「⋯」菜单：低频 / 危险动作收在这里
await openRowMenu(K_LAPTOP);
const menu = await page.evaluate(() => {
  const items = [...document.querySelectorAll('[role="menuitem"]')].map((el) =>
    el.textContent.trim(),
  );
  return { items };
});
await page.screenshot({ path: path.join(SHOTS_DIR, `${TAG}-row-menu.png`) });

// ── 吊销：先弹二次确认（此刻列表不变），确认后该行消失
//
// 菜单项**可能不存在**（真回归）。这里刻意不硬点 —— 硬点会超时崩溃、退出码非 0，
// 于是 gate.mjs 只能报「探针可运行」这条泛化判据，而**真正该报的那条**
//（「「⋯」菜单含低频/危险动作」）永远没机会说话。所以缺项就记录、把后续字段留 null，
// 让判据去指出具体缺了什么。
const hasRevoke = menu.items.some((t) => t.includes("吊销"));
if (hasRevoke) {
  await page.click('[role="menuitem"]:has-text("吊销")');
  await page.waitForTimeout(250);
} else {
  // 菜单还开着的话会拦截后续所有点击（`<html> intercepts pointer events`），
  // 探针会以一个「看起来无关」的超时崩掉。先关掉，让流程继续走到断言。
  await page.keyboard.press("Escape");
  await page.waitForTimeout(200);
}
const confirmOpen = !hasRevoke
  ? null
  : await page.evaluate(() => {
  const d = document.querySelector('[role="alertdialog"]');
  return d
    ? {
        title: (d.querySelector("h2")?.textContent || "").trim(),
        buttons: [...d.querySelectorAll("button")].map((b) => b.textContent.trim()),
        rowsStillThere: document.querySelectorAll('[data-testid="gateway-key-row"]').length,
        // 二次确认里的确认按钮**必须是**实心红（危险动作的唯一一处实心红）
        filledDangerInside: d.querySelectorAll('button[data-variant="destructive"]').length,
      }
    : null;
});
if (hasRevoke) {
  await page.screenshot({ path: path.join(SHOTS_DIR, `${TAG}-revoke-confirm.png`) });
  await page.click('[role="alertdialog"] button:has-text("吊销")');
  await page.waitForTimeout(400);
}
const afterRevoke = hasRevoke ? await readList() : null;

// ── 新建：插到最前，且新行直接显示全文
await page.fill('input[aria-label="新密钥备注"]', "临时排查");
await page.click("button:has-text('新建密钥')");
await page.waitForTimeout(400);
const afterCreate = await readList();
await page.screenshot({ path: path.join(SHOTS_DIR, `${TAG}-after-create.png`) });

// ── 全部吊销 → 空态
for (const r of hasRevoke ? afterCreate.rows : []) {
  await openRowMenu(r.prefix);
  await page.click('[role="menuitem"]:has-text("吊销")');
  await page.waitForTimeout(180);
  await page.click('[role="alertdialog"] button:has-text("吊销")');
  await page.waitForTimeout(220);
}
const afterAllRevoked = hasRevoke ? await readList() : null;
await page.screenshot({ path: path.join(SHOTS_DIR, `${TAG}-empty.png`) });

const out = {
  size: `${VW}x${VH}`,
  initial,
  revealed,
  hiddenAgain,
  menu,
  confirmOpen,
  afterRevoke,
  afterCreate,
  afterAllRevoked,
  errors,
  shots: [
    `${TAG}-row-menu.png`,
    `${TAG}-revoke-confirm.png`,
    `${TAG}-after-create.png`,
    `${TAG}-empty.png`,
  ],
};
fs.writeFileSync(path.join(VR_DIR, `out-${TAG}.json`), JSON.stringify(out, null, 1));
console.log(
  `[probe-keys] ${TAG} 初始=${initial.rows.length} 行内行数=${initial.rows.map((r) => r.childCenters.length).join("/")} 行内按钮=${initial.rowButtons} 内容宽=${initial.contentW}/${initial.mainW} 按钮高度档=${initial.buttonHeights.join("/")} 实心红=${initial.filledDanger} 吊销后=${afterRevoke?.rows?.length ?? "-"} 新建后=${afterCreate.rows.length} 全吊销=${afterAllRevoked?.rows?.length ?? "-"}`,
);
await browser.close();
