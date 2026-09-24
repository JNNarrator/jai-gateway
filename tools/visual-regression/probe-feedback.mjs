// 反馈落点探针（2026-09-24 全局整改）。
//
//   node tools/visual-regression/probe-feedback.mjs --size=1180x800
//
// 起因（用户反馈）：「按钮的 tips……有些反馈都在最顶部，窗口上下有滚动条。还得滚到
// 上面或者下面去看提示」。实测确认：页级 `msg/err` 横幅锚在 `PageHeader` 之后，而触发
// 它的按钮常在列表下方 —— 点完必须滚回页面顶部；右下角 toast 又在另一个对角。
//
// 判据（写进本文件，gate.mjs 只读 JSON）：
//   1. **页级反馈吸顶**：反馈条所在容器的 computed `position` 是 `sticky`，且**滚到
//      页面最底后仍在视口内**（这是「不用滚回去」的直接证明，不是看类名猜）
//   2. 页级反馈**不自动消失**（等 3s 仍在），点「关闭提示」才消失
//   3. **行级反馈落在该行内**：点第 N 行的「列出工具」→ 反馈元素出现在**第 N 行卡片**
//      里（按 `[data-testid=mcp-row]` 的下标核对），且页级吸顶条此时**为空**
//      —— 这条正是防「又汇总回页面顶部」的回归
//   4. **错误 toast 不自动消失且可关闭**：等 3s 仍在、有 `[data-close-button]`、
//      点它之后消失
//   5. 全程无页面报错
import fs from "node:fs";
import path from "node:path";
import { launchBrowser, VR_DIR, SHOTS_DIR, parseArgv, parseSize } from "./_env.mjs";

const { fixtures } = await import(new URL("./fixtures.mjs", import.meta.url).href);
const MOCK_OK = fs
  .readFileSync(new URL("./run.mjs", import.meta.url), "utf8")
  .match(/function installMock\(\)[\s\S]*?\n}\n/)[0];

/** 让 `gateway_key_list` 抛错 → 网关页出页级错误（走吸顶条）。
 *  throw 会被 mock 的 `invoke` 捕获并 reject（见 run.mjs 的 `__TAURI_INTERNALS__`）。 */
const KEYS_CASE = 'case "gateway_key_list": return state.keys.map((k) => ({ ...k }));';
if (!MOCK_OK.includes(KEYS_CASE)) throw new Error("run.mjs 的 gateway_key_list mock 变了，探针需要同步");
const MOCK_KEYS_FAIL = MOCK_OK.replace(
  KEYS_CASE,
  'case "gateway_key_list": throw new Error("probe: 模拟「读取密钥列表失败」（上游数据库忙）");',
);

/** 让剪贴板写入 reject → 复制类按钮走 `toast(..., "err")` 这条真路径。
 *  这是**刻意造**的失败（否则要点出错误 toast 只能等真实故障），但走的是产品代码里
 *  同一个错误分支，判据本身仍然有效。 */
const CLIP_FAIL = `Object.defineProperty(navigator, "clipboard", { value: { writeText: () => Promise.reject(new Error("probe: clipboard blocked")) } });`;

const argv = parseArgv();
const [VW, VH] = parseSize(argv.size, "1180x800");
const TAG = `probe-feedback-${VW}x${VH}`;

const browser = await launchBrowser();
const errors = [];

async function newPage(mockSrc, extraInit) {
  const ctx = await browser.newContext({
    viewport: { width: VW, height: VH },
    deviceScaleFactor: 1,
    locale: "zh-CN",
  });
  await ctx.addInitScript({
    content: `window.__JAI_FIX__ = ${JSON.stringify(fixtures)}; window.__VR_VW__=${VW}; window.__VR_VH__=${VH};`,
  });
  await ctx.addInitScript({ content: `(${mockSrc})();` });
  if (extraInit) await ctx.addInitScript({ content: extraInit });
  const page = await ctx.newPage();
  page.on("pageerror", (e) => errors.push(String(e).slice(0, 200)));
  await page.goto("http://127.0.0.1:5173/");
  await page.waitForTimeout(1500);
  return { ctx, page };
}

const goTab = async (page, name) => {
  await page.getByRole("button", { name: new RegExp(`^${name}$`) }).first().click();
  await page.waitForTimeout(700);
};

// ═══════════════ 场景 A：页级反馈吸顶 + 不自动消失 ═══════════════
const a = await newPage(MOCK_KEYS_FAIL, CLIP_FAIL);
const pageA = a.page;

const readPageFeedback = () =>
  pageA.evaluate(() => {
    const main = document.querySelector("main");
    // 页级反馈 = 不在任何 mcp-row 里的那个
    const el = [...document.querySelectorAll('[data-testid="page-feedback"]')].find(
      (n) => !n.closest('[data-testid="mcp-row"]'),
    );
    if (!el) return { present: false };
    // 往上找最近的那个吸顶容器
    let sticky = null;
    let n = el.parentElement;
    while (n && n !== main) {
      if (getComputedStyle(n).position === "sticky") {
        sticky = n;
        break;
      }
      n = n.parentElement;
    }
    const r = el.getBoundingClientRect();
    return {
      present: true,
      kind: el.getAttribute("data-kind"),
      text: el.textContent.trim().replace(/\s+/g, " ").slice(0, 60),
      stickyPosition: sticky ? getComputedStyle(sticky).position : null,
      stickySlot: sticky ? sticky.getAttribute("data-slot") : null,
      // 相对**滚动容器**（main）的可见性：这才是「在视口里」的口径
      top: Math.round(r.top),
      bottom: Math.round(r.bottom),
      viewportH: main.clientHeight,
      inView: r.top >= -1 && r.bottom <= main.clientHeight + 1,
      scrollTop: main.scrollTop,
    };
  });

const scrollBottom = () =>
  pageA.evaluate(() => {
    const m = document.querySelector("main");
    m.scrollTop = m.scrollHeight;
    return { scrollH: m.scrollHeight, clientH: m.clientHeight };
  });

const pageFeedbackAtTop = await readPageFeedback();
const scrolled = await scrollBottom();
await pageA.waitForTimeout(300);
const pageFeedbackAtBottom = await readPageFeedback();
await pageA.screenshot({ path: path.join(SHOTS_DIR, `${TAG}-sticky-feedback.png`) });

// 不自动消失：等 3s（成功类是 2.4s）
await pageA.waitForTimeout(3000);
const pageFeedbackAfter3s = await readPageFeedback();

// 点关闭 → 消失
const dismissWorks = await pageA.evaluate(async () => {
  const el = [...document.querySelectorAll('[data-testid="page-feedback"]')].find(
    (n) => !n.closest('[data-testid="mcp-row"]'),
  );
  if (!el) return { clicked: false };
  el.querySelector('button[aria-label="关闭提示"]')?.click();
  return { clicked: true };
});
await pageA.waitForTimeout(300);
const pageFeedbackDismissed = await readPageFeedback();

// ═══════════════ 场景 B：错误 toast 不自动消失 + 可关闭 ═══════════════
await pageA.getByRole("button", { name: /复制接入信息/ }).first().click();
await pageA.waitForTimeout(600);
const readToast = () =>
  pageA.evaluate(() => {
    const li = document.querySelector("[data-sonner-toast]");
    if (!li) return { present: false };
    const close = li.querySelector("[data-close-button]");
    const cs = close ? getComputedStyle(close) : null;
    const r = li.getBoundingClientRect();
    return {
      present: true,
      text: (li.textContent || "").trim().replace(/\s+/g, " ").slice(0, 60),
      hasClose: !!close,
      // 关闭按钮必须**可点**：容器整体是 pointer-events:none（防吞点击），
      // 只有这个小圆钮被单独恢复（index.css）
      closePointerEvents: cs ? cs.pointerEvents : null,
      inView: r.top >= 0 && r.bottom <= window.innerHeight,
    };
  });

const toastNow = await readToast();
// 等 4.6s：错误 toast 的 TTL 是 Infinity，但如果有人把 `duration: Infinity` 去掉，
// 兜底链是 Toaster 的 2400ms → sonner 自家的 4000ms。只等 3s 的话，**同时**把
// 两处都去掉就会假绿（4000 > 3600）。等的时长必须长于兜底默认值，判据才成立。
await pageA.waitForTimeout(4600);
const toastAfterWait = await readToast();
const toastCloseClicked = await pageA.evaluate(() => {
  const close = document.querySelector("[data-sonner-toast] [data-close-button]");
  if (!close) return false;
  close.click();
  return true;
});
await pageA.waitForTimeout(500);
const toastAfterClose = await readToast();
await a.ctx.close();

// ═══════════════ 场景 C：行级反馈落在该行内 ═══════════════
const b = await newPage(MOCK_OK);
const pageB = b.page;
await goTab(pageB, "MCP");

// 点**最后一行**的「列出工具」——这一行最可能被挤到页面下方
const rowCount = await pageB.locator('[data-testid="mcp-row"]').count();
const targetIdx = rowCount - 1;
await pageB.locator('[data-testid="mcp-row"]').nth(targetIdx).getByRole("button", { name: /列出工具/ }).click();
await pageB.waitForTimeout(600);

const rowFeedback = await pageB.evaluate(() => {
  const rows = [...document.querySelectorAll('[data-testid="mcp-row"]')];
  const idx = rows.findIndex((r) => r.querySelector('[data-testid="mcp-row-feedback"]'));
  const el = idx >= 0 ? rows[idx].querySelector('[data-testid="mcp-row-feedback"]') : null;
  // 页级吸顶条此时必须为空：行级反馈不许再汇总到页面顶部
  const pageLevel = [...document.querySelectorAll('[data-testid="page-feedback"]')].filter(
    (n) => !n.closest('[data-testid="mcp-row"]'),
  );
  const r = el?.getBoundingClientRect();
  const main = document.querySelector("main");
  return {
    rowCount: rows.length,
    feedbackRowIdx: idx,
    text: el ? el.textContent.trim().replace(/\s+/g, " ").slice(0, 60) : null,
    pageLevelCount: pageLevel.length,
    // 反馈是否在视口内（点最后一行时页面可能已滚动）
    inView: r ? r.top >= 0 && r.bottom <= main.clientHeight + 1 : null,
  };
});
await pageB.screenshot({ path: path.join(SHOTS_DIR, `${TAG}-row-feedback.png`) });
await b.ctx.close();

const out = {
  size: `${VW}x${VH}`,
  pageFeedbackAtTop,
  pageFeedbackAtBottom,
  pageFeedbackAfter3s,
  pageFeedbackDismissed,
  scrolled,
  dismissWorks,
  toastNow,
  toastAfterWait,
  toastAfterClose,
  toastCloseClicked,
  rowCount,
  targetIdx,
  rowFeedback,
  errors,
  shots: [`${TAG}-sticky-feedback.png`, `${TAG}-row-feedback.png`],
};
fs.writeFileSync(path.join(VR_DIR, `out-${TAG}.json`), JSON.stringify(out, null, 1));
console.log(
  `[probe-feedback] ${TAG} 页级反馈 sticky=${pageFeedbackAtTop.stickyPosition}/${pageFeedbackAtTop.stickySlot} 滚到底仍在视口=${pageFeedbackAtBottom.inView} 3s后仍在=${pageFeedbackAfter3s.present} 关闭后=${pageFeedbackDismissed.present} | toast 4.6s后仍在=${toastAfterWait.present} 关闭钮=${toastNow.closePointerEvents} 点关后=${toastAfterClose.present} | 行级反馈落在第 ${rowFeedback.feedbackRowIdx} 行（点了第 ${targetIdx} 行，共 ${rowCount} 行）页级条=${rowFeedback.pageLevelCount}`,
);
await browser.close();
