// 同步页「推送间隔 / 拉取间隔各自独立」的说明性回归探针（bug 清单 20）。
//
// 背景：此前全项目只有一个间隔字段 `auto_push_interval_min`，推送与拉取都读它，
// 同步页也只有一个「定时间隔」选择器 —— 「推送 6 小时 / 拉取 30 分钟」这种组合
// **根本无法配置**。改成两个独立间隔后，最容易静默退化的地方是：
//   ① 两个选择器又变回一个（或共享同一状态），改一个另一个跟着动；
//   ② 存盘时把拉取的值写进推送字段（或反之）—— 这类「串线」在 UI 上完全看不出来，
//      只有比对 IPC 参数才能发现。
// 本探针把这两件事固化成断言。
//
// 用法（仓库根目录，需先起 vite:5173）：
//   export TMPDIR=$PWD/.vr/tmp
//   node tools/visual-regression/sync-intervals.mjs --size=1180x800
//   node tools/visual-regression/sync-intervals.mjs --size=900x600
// 产出：.vr/out-sync-intervals-<size>.json + .vr/shots/sync-intervals-<size>.png
import { createRequire } from "node:module";
import fs from "node:fs";
import path from "node:path";

const require = createRequire(
  "/Users/jiangnan/Documents/workspace/deepseek-harness/node_modules/.pnpm/playwright@1.61.1/node_modules/playwright/",
);
const { chromium } = require("playwright");
const { fixtures } = await import(new URL("./fixtures.mjs", import.meta.url).href);

const ROOT = path.resolve(".vr");
const SHOTS = path.join(ROOT, "shots");
fs.mkdirSync(SHOTS, { recursive: true });

const argv = Object.fromEntries(
  process.argv.slice(2).map((s) => {
    const m = s.match(/^--([^=]+)(?:=(.*))?$/);
    return m ? [m[1], m[2] ?? true] : [s, true];
  }),
);
const [VW, VH] = (argv.size || "1180x800").split("x").map(Number);
const URL_ = argv.url || "http://127.0.0.1:5173/";

/**
 * 最小 Tauri mock：只覆盖同步页 + 外壳启动所需命令，其余返回 null（与 mcp-switches.mjs 同口径）。
 *
 * 注意 `webdav_config_get` 每次返回**新副本**：mock 若复用同一对象引用，
 * 后续 set 改的就是 fixture 本体，断言会拿到被污染的初始值（假通过）。
 */
function installMock() {
  const fix = window.__JAI_FIX__;
  const calls = [];
  window.__JAI_CALLS__ = calls;
  const cb = new Map();
  let cbId = 1;
  // 同步页的「已保存配置」按 IPC 语义保存在 mock 侧，set 之后 get 要能读到新值
  const state = { cfg: JSON.parse(JSON.stringify(fix.webdav_config_get)) };
  function invokeCmd(cmd, args = {}) {
    calls.push({ cmd, args });
    switch (cmd) {
      case "gateway_status": return { ...fix.gateway_status, running: true };
      case "webdav_config_get": return JSON.parse(JSON.stringify(state.cfg));
      case "webdav_config_set": {
        const i = args.input || {};
        // 与后端语义一致：只有显式传入的字段才覆盖（None 保持原值）
        for (const [k, v] of Object.entries(i)) {
          if (v !== null && v !== undefined) state.cfg[k] = v;
        }
        return null;
      }
      case "webdav_autopush_status": return fix.webdav_autopush_status;
      case "webdav_autopull_status": return fix.webdav_autopull_status;
      case "webdav_snapshot_info": return fix.webdav_snapshot_info;
      case "webdav_backups_list": return fix.webdav_backups_list || [];
      case "plugin:window|theme": return "dark";
      case "plugin:window|cursor_position": return { x: 0, y: 0 };
      case "plugin:updater|check": return null;
      default: return null;
    }
  }
  window.__TAURI_INTERNALS__ = {
    metadata: { currentWindow: { label: "main" }, currentWebview: { label: "main" }, plugins: {}, tauriScript: "" },
    callbacks: cb,
    transformCallback(callback = () => {}, once = false) { const id = cbId++; cb.set(id, callback); return id; },
    unregisterCallback(id) { cb.delete(id); },
    invoke(cmd, args = {}) {
      try { return Promise.resolve(invokeCmd(cmd, args)); }
      catch (e) { return Promise.reject(String(e)); }
    },
    convertFileSrc: (p) => "data:,",
    registerPlugin() {},
  };
  window.__TAURI__ = { event: {}, window: {}, core: {} };
  // 见 mcp-switches.mjs 同处注释：`_unlisten` 需要 Tauri 注入的事件插件内部对象，
  // 缺了它 TitleBar 的 onResized 清理会抛 pageerror，「无控制台报错」断言恒假。
  window.__TAURI_EVENT_PLUGIN_INTERNALS__ = {
    unregisterListener(_event, id) { cb.delete(id); },
  };
}

const out = { size: `${VW}x${VH}`, checks: [], errors: [], startedAt: new Date().toISOString() };
const check = (name, ok, detail = "") => {
  out.checks.push({ name, ok: !!ok, detail });
  console.log(`${ok ? "✅" : "❌"} ${name}${detail ? ` — ${detail}` : ""}`);
};

const browser = await chromium.launch({
  executablePath: "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome",
  headless: true,
});
const ctx = await browser.newContext({
  viewport: { width: VW, height: VH },
  deviceScaleFactor: 2,
  locale: "zh-CN",
  timezoneId: "Asia/Shanghai",
});
await ctx.addInitScript({ content: `window.__JAI_FIX__ = ${JSON.stringify(fixtures)};` });
await ctx.addInitScript({ content: `(${installMock.toString()})();` });
await ctx.addInitScript({ content: `try{localStorage.setItem('jai-sidebar-collapsed','0');}catch(e){}` });

const page = await ctx.newPage();
page.on("console", (m) => { if (m.type() === "error") out.errors.push("console: " + m.text().slice(0, 200)); });
page.on("pageerror", (e) => out.errors.push("pageerror: " + String(e.message).slice(0, 200)));

await page.goto(URL_, { waitUntil: "domcontentloaded" });
await page.waitForTimeout(1000);
await page.getByRole("button", { name: /^同步$/ }).first().click();
await page.waitForTimeout(700);

// ── ① 两个选择器都存在，且是**两个**不同的控件 ────────────────────────────────
// 按 aria-label 定位（不依赖视觉文案，文案可改、语义标识不该改）
const pushSel = page.locator('[aria-label="自动推送间隔"]');
const pullSel = page.locator('[aria-label="自动拉取间隔"]');
check("存在「自动推送间隔」选择器", (await pushSel.count()) === 1);
check("存在「自动拉取间隔」选择器", (await pullSel.count()) === 1);
check(
  "两个选择器是不同元素（不是同一个控件被复用）",
  (await pushSel.count()) === 1 &&
    (await pullSel.count()) === 1 &&
    (await pushSel.evaluate((el) => el.id || el.getAttribute("data-state") || "a")) !==
      (await pullSel.evaluate((el) => el.id || el.getAttribute("data-state") || "a")) ||
    (await pushSel.boundingBox())?.y !== (await pullSel.boundingBox())?.y,
  "以纵向位置区分",
);
const pushBox = await pushSel.boundingBox();
const pullBox = await pullSel.boundingBox();
check(
  "两个选择器纵向分离（各有自己的行）",
  !!pushBox && !!pullBox && Math.abs(pushBox.y - pullBox.y) > 10,
  `pushY=${Math.round(pushBox?.y)} pullY=${Math.round(pullBox?.y)}`,
);

// ── ② 初始值来自各自字段（fixture: 推送 30 / 拉取 360，刻意不同）─────────────
// 若实现把两者串成一个状态，这里两个值会相等 —— 立即变红
const textOf = async (loc) => (await loc.innerText()).replace(/\s+/g, " ").trim();
const pushInit = await textOf(pushSel);
const pullInit = await textOf(pullSel);
check("推送间隔初始值来自 autoPushIntervalMin（每 30 分钟）", pushInit.includes("30"), pushInit);
check("拉取间隔初始值来自 autoPullIntervalMin（每 6 小时）", pullInit.includes("6") || pullInit.includes("360"), pullInit);
check("两者初始值**不同**（证明是两个独立字段）", pushInit !== pullInit, `${pushInit} vs ${pullInit}`);

// ── ③ 改「拉取间隔」只写 autoPullIntervalMin，不碰推送 ────────────────────────
await pullSel.click();
await page.waitForTimeout(300);
await page.getByRole("option", { name: "每 1 小时" }).first().click();
await page.waitForTimeout(500);
const setCalls1 = await page.evaluate(() =>
  (window.__JAI_CALLS__ || []).filter((c) => c.cmd === "webdav_config_set").map((c) => c.args.input),
);
const c1 = setCalls1.at(-1) || {};
check(
  "改拉取间隔 → 落 autoPullIntervalMin=60",
  c1.autoPullIntervalMin === 60,
  JSON.stringify(c1),
);
check(
  "改拉取间隔 → **不得**改动 autoPushIntervalMin",
  !("autoPushIntervalMin" in c1) || c1.autoPushIntervalMin === 30,
  `autoPushIntervalMin=${c1.autoPushIntervalMin}`,
);
const pushAfterPullEdit = await textOf(pushSel);
check("改拉取间隔后，推送间隔显示值不变", pushAfterPullEdit === pushInit, `${pushInit} → ${pushAfterPullEdit}`);

// ── ④ 改「推送间隔」只写 autoPushIntervalMin，不碰拉取 ────────────────────────
await pushSel.click();
await page.waitForTimeout(300);
await page.getByRole("option", { name: "每 6 小时" }).first().click();
await page.waitForTimeout(500);
const setCalls2 = await page.evaluate(() =>
  (window.__JAI_CALLS__ || []).filter((c) => c.cmd === "webdav_config_set").map((c) => c.args.input),
);
const c2 = setCalls2.at(-1) || {};
check(
  "改推送间隔 → 落 autoPushIntervalMin=360",
  c2.autoPushIntervalMin === 360,
  JSON.stringify(c2),
);
check(
  "改推送间隔 → **不得**改动 autoPullIntervalMin",
  !("autoPullIntervalMin" in c2) || c2.autoPullIntervalMin === 60,
  `autoPullIntervalMin=${c2.autoPullIntervalMin}`,
);
const pullAfterPushEdit = await textOf(pullSel);
check(
  "改推送间隔后，拉取间隔显示值保持上一步改的 1 小时",
  pullAfterPushEdit.includes("1") && pullAfterPushEdit !== pullInit,
  `${pullInit} → ${pullAfterPushEdit}`,
);

// ── ⑤ 文案不再声称「共用间隔」（旧文案会误导用户以为两者绑定）────────────────
const cardText = (await page.locator("body").innerText()).replace(/\s+/g, " ");
check("文案已说明两者相互独立", cardText.includes("相互独立"), cardText.match(/.{0,20}相互独立.{0,20}/)?.[0] || "");
check("已去掉「按所选间隔」这类含糊共用措辞", !cardText.includes("按所选间隔"));

// ── ⑥ 无横向溢出；两个选择器都在容器内（不被挤出）────────────────────────────
const geo = await page.evaluate(() => {
  const vw = window.innerWidth;
  const docW = document.documentElement.scrollWidth;
  const one = (label) => {
    const el = document.querySelector(`[aria-label="${label}"]`);
    if (!el) return null;
    const r = el.getBoundingClientRect();
    return {
      w: Math.round(r.width),
      h: Math.round(r.height),
      right: Math.round(r.right),
      left: Math.round(r.left),
      visible: r.width > 0 && r.height > 0,
    };
  };
  return { vw, docW, push: one("自动推送间隔"), pull: one("自动拉取间隔") };
});
check("无横向溢出", geo.docW <= geo.vw + 1, `scrollWidth=${geo.docW} viewport=${geo.vw}`);
for (const [name, g] of [["推送", geo.push], ["拉取", geo.pull]]) {
  check(
    `${name}间隔选择器完整可见且在视口内`,
    g && g.visible && g.right <= geo.vw + 1 && g.left >= -1,
    JSON.stringify(g),
  );
}

check("无控制台报错", out.errors.length === 0, out.errors.slice(0, 3).join(" | "));

const shot = path.join(SHOTS, `sync-intervals-${VW}x${VH}.png`);
await page.screenshot({ path: shot });
out.screenshot = shot;
out.passed = out.checks.every((c) => c.ok);
out.finishedAt = new Date().toISOString();
const outFile = path.join(ROOT, `out-sync-intervals-${VW}x${VH}.json`);
fs.writeFileSync(outFile, JSON.stringify(out, null, 2));
console.log(`\n${out.passed ? "全部通过" : "存在失败项"} → ${outFile}\n截图：${shot}`);

await browser.close();
process.exit(out.passed ? 0 : 1);
