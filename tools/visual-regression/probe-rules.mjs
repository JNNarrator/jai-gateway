// 密钥白/黑名单（D9-T6b）弹窗探针：验证 GatewayPage 的「规则」弹窗。
//
//   node tools/visual-regression/probe-rules.mjs --size=1180x800
//
// 判据（写进本文件，gate.mjs 只读 JSON）：
//   1. 弹窗渲染候选清单（渠道去重 2 行 / 模型 4 行），且**回填已保存的规则**
//      （夹具里 k-laptop 白名单只有 Anthropic ⇒ 该行必须是「允许」态）
//   2. 「当前密钥可访问的模型」预览跟着草稿实时变：白名单 ⇒ 2；加允许主渠道 ⇒ 4；
//      两边都拒绝 ⇒ 0 且给出「会返回 403」的红字提示
//   3. 保存时提交的四个数组与三态一致（**逐字**核对 payload，不是只看没报错）
//   4. 保存后弹窗关闭，重新打开能读回刚保存的那份
//   5. 列表上「已限制」徽标只出现在配过规则的密钥行上
//   6. 全程无页面报错
import fs from "node:fs";
import path from "node:path";
import { launchBrowser, VR_DIR, SHOTS_DIR, parseArgv, parseSize } from "./_env.mjs";

const { fixtures } = await import(new URL("./fixtures.mjs", import.meta.url).href);
const mock = fs
  .readFileSync(new URL("./run.mjs", import.meta.url), "utf8")
  .match(/function installMock\(\)[\s\S]*?\n}\n/)[0];

const argv = parseArgv();
const [VW, VH] = parseSize(argv.size, "1180x800");
const TAG = `probe-rules-${VW}x${VH}`;

const P_MAIN = fixtures.key_rules_options[0].providerId;
const P_ANTH = fixtures.key_rules_options[2].providerId;

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

/** 密钥列表上规则状态 chip 的出现情况（2026-09-23 起由「已限制」徽标改为「不限 / 已限制」chip） */
const readBadges = () =>
  page.evaluate(() =>
    [...document.querySelectorAll('[data-testid="gateway-key-row"]')].map((li) => {
      const chip = li.querySelector('[data-testid="gateway-key-rules"]');
      return {
        prefix: li.getAttribute("data-prefix"),
        limited: chip?.getAttribute("data-limited") === "1",
        state: chip?.textContent?.trim() ?? null,
      };
    }),
  );

/** 弹窗内部状态：候选行 + 三态 + 预览计数 */
const readDialog = () =>
  page.evaluate(() => {
    const d = document.querySelector('[data-testid="key-rules-dialog"]');
    if (!d) return null;
    const rows = (sel) =>
      [...d.querySelectorAll(`[data-testid="${sel}"]`)].map((li) => ({
        id: li.getAttribute("data-id"),
        state: li.getAttribute("data-state"),
      }));
    const preview = d.querySelector('[data-testid="key-rules-preview"]');
    return {
      title: (d.querySelector("h2")?.textContent || "").trim(),
      legend: (d.querySelector('[data-testid="key-rules-legend"]')?.textContent || "")
        .trim()
        .replace(/\s+/g, " "),
      providers: rows("key-rules-provider-row"),
      models: rows("key-rules-model-row"),
      previewCount: Number(preview?.getAttribute("data-count") ?? -1),
      previewText: (preview?.textContent || "").trim().replace(/\s+/g, " "),
      saveDisabled: !!d.querySelector('[data-testid="key-rules-save"]')?.disabled,
    };
  });

const pick = async (kind, id, tri) => {
  await page.click(
    `[data-testid="key-rules-${kind}-row"][data-id="${id}"] [data-testid="key-rules-${kind}-${tri}"]`,
  );
  await page.waitForTimeout(120);
};

/** 最近一次 gateway_key_rules_set 的入参 */
const lastSetPayload = () =>
  page.evaluate(() => {
    const c = (window.__JAI_CALLS__ || []).filter((x) => x.cmd === "gateway_key_rules_set");
    return c.length ? c[c.length - 1].args : null;
  });

const openRules = async (prefix) => {
  await page.click(`button[aria-label="设置规则 ${prefix}"]`);
  await page.waitForTimeout(450);
};

// ── 1) 列表徽标：只有配过规则的密钥才有「已限制」
const badges = await readBadges();

// ── 2) 打开 k-laptop（夹具里白名单只有 Anthropic）→ 回填 + 预览
await openRules("sk-jai-Lp7Qm");
const opened = await readDialog();
await page.screenshot({ path: path.join(SHOTS_DIR, `${TAG}-opened.png`) });

// ── 3) 预览跟着草稿变
await pick("provider", P_MAIN, "allow"); // 白名单 = {ANTH, MAIN} → 4
const previewTwoAllow = (await readDialog()).previewCount;
await pick("provider", P_ANTH, "deny"); // 只留 MAIN → 2
const previewMainOnly = (await readDialog()).previewCount;
await pick("provider", P_MAIN, "deny"); // 全拒 → 0
const previewNone = await readDialog();
await page.screenshot({ path: path.join(SHOTS_DIR, `${TAG}-preview-empty.png`) });

// ── 4) 收敛成一份确定的规则：白名单 {MAIN} + 模型白名单 {kimi-k2.7-code} → 1
await pick("provider", P_MAIN, "allow");
await pick("provider", P_ANTH, "default");
const previewMainAll = (await readDialog()).previewCount;
await pick("model", "kimi-k2.7-code", "allow");
const previewOneModel = await readDialog();
await page.screenshot({ path: path.join(SHOTS_DIR, `${TAG}-draft.png`) });

// ── 5) 保存 → payload 逐字核对 + 弹窗关闭
await page.click('[data-testid="key-rules-save"]');
await page.waitForTimeout(500);
const payload = await lastSetPayload();
const dialogAfterSave = await readDialog();
const badgesAfterSave = await readBadges();

// ── 6) 重新打开 → 读回刚保存的那份
await openRules("sk-jai-Lp7Qm");
const reopened = await readDialog();
await page.screenshot({ path: path.join(SHOTS_DIR, `${TAG}-reopened.png`) });

const out = {
  size: `${VW}x${VH}`,
  // 供门禁核对 payload / 回填用的两个渠道 id（门禁不 import 夹具，只读本文件）
  mainId: P_MAIN,
  anthId: P_ANTH,
  badges,
  opened,
  previewTwoAllow,
  previewMainOnly,
  previewNone: {
    count: previewNone.previewCount,
    text: previewNone.previewText,
  },
  previewMainAll,
  previewOneModel: previewOneModel.previewCount,
  payload,
  dialogAfterSave,
  badgesAfterSave,
  reopened,
  errors,
  shots: [
    `${TAG}-opened.png`,
    `${TAG}-preview-empty.png`,
    `${TAG}-draft.png`,
    `${TAG}-reopened.png`,
  ],
};
fs.writeFileSync(path.join(VR_DIR, `out-${TAG}.json`), JSON.stringify(out, null, 1));
console.log(
  `[probe-rules] ${TAG} 候选渠道=${opened?.providers?.length} 模型=${opened?.models?.length} 预览 2→${previewTwoAllow}→${previewMainOnly}→0→${previewOneModel.previewCount} 保存后弹窗=${dialogAfterSave ? "仍开" : "已关"}`,
);
await browser.close();
