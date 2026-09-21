// 网关页「接入地址」的说明性回归探针。
//
// 背景：网关页原先只提供 Base URL（`http://127.0.0.1:<端口>/v1`）。部分客户端把配置项当
// **精确请求地址**用、不会再补路径（典型：Reasonix 的「API 地址」即 request_url），
// 用户把 Base URL 复制进去会打到 `/v1` 上得到 404，而报错只说「endpoint not found」，
// 排查成本很高。本探针把「Base URL 与完整地址**两个功能都在**、且复制内容正确」固化成断言，
// 防止以后任一被静默删掉或改错。
//
// 用法（仓库根目录，需先起 vite:5173）：
//   export TMPDIR=$PWD/.vr/tmp
//   node tools/visual-regression/gateway-endpoints.mjs --size=1180x800
//   node tools/visual-regression/gateway-endpoints.mjs --size=900x600
// 产出：.vr/out-gateway-endpoints-<size>.json + .vr/shots/gateway-endpoints-<size>.png
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

/** 最小 Tauri mock：只覆盖网关页所需命令，其余返回 null（与 run.mjs 同口径）。 */
function installMock() {
  const fix = window.__JAI_FIX__;
  const calls = [];
  window.__JAI_CALLS__ = calls;
  const cb = new Map();
  let cbId = 1;
  function invokeCmd(cmd, args = {}) {
    calls.push({ cmd, args });
    switch (cmd) {
      case "gateway_status": return { ...fix.gateway_status, running: true };
      case "gateway_key_info": return { ...fix.gateway_key_info };
      case "gateway_key_reveal": return { ...fix.gateway_key_info };
      case "health_summary": return JSON.parse(JSON.stringify(fix.health_summary));
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

// 与本目录其它探针同口径：走本机 Chrome（playwright 自带的 headless shell 未安装）
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
// 剪贴板断言需要读写授权；否则 navigator.clipboard.readText() 抛 NotAllowedError，
// 会把「复制成功但读不到」误判成功能失败。
await ctx.grantPermissions(["clipboard-read", "clipboard-write"]);
await ctx.addInitScript({ content: `window.__JAI_FIX__ = ${JSON.stringify(fixtures)};` });
await ctx.addInitScript({ content: `(${installMock.toString()})();` });
await ctx.addInitScript({ content: `try{localStorage.setItem('jai-sidebar-collapsed','0');localStorage.setItem('theme','dark');}catch(e){}` });

const page = await ctx.newPage();
page.on("console", (m) => { if (m.type() === "error") out.errors.push("console: " + m.text().slice(0, 200)); });
page.on("pageerror", (e) => out.errors.push("pageerror: " + String(e.message).slice(0, 200)));

const readClip = () => page.evaluate(() => navigator.clipboard.readText());

await page.goto(URL_, { waitUntil: "domcontentloaded" });
await page.waitForTimeout(1200);
// 首页即网关页；若落在别处则显式切回
if (!(await page.locator('[data-testid="gateway-endpoints"]').count())) {
  await page.getByRole("button", { name: /^网关$/ }).first().click();
  await page.waitForTimeout(700);
}

const PORT = fixtures.gateway_status.port; // 1314
const ORIGIN = `http://127.0.0.1:${PORT}`;
const BASE = `${ORIGIN}/v1`;
const EXPECT = [
  ["Chat Completions", "POST", `${ORIGIN}/v1/chat/completions`],
  ["Anthropic Messages", "POST", `${ORIGIN}/v1/messages`],
  ["Responses", "POST", `${ORIGIN}/v1/responses`],
  ["模型列表", "GET", `${ORIGIN}/v1/models`],
  ["MCP 元数据", "POST", `${ORIGIN}/mcp`],
];

// ① 「复制 Base URL」功能必须仍然存在（本次新增不得把它替换掉）
const baseRow = page.locator('[data-testid="gateway-base-url-row"]');
check("Base URL 复制字段仍在", (await baseRow.count()) === 1);
const baseShown = (await baseRow.locator("code").first().innerText()).trim();
check("Base URL 展示的是 /v1", baseShown === BASE, baseShown);
await baseRow.locator('button[aria-label="复制"]').first().click();
await page.waitForTimeout(350);
const baseClip = await readClip();
check("复制 Base URL 内容正确", baseClip === BASE, JSON.stringify(baseClip));

// ② 「完整请求地址」区块存在，且条数与端点表一致
const list = page.locator('[data-testid="gateway-endpoints"]');
check("完整请求地址区块存在", (await list.count()) === 1);
const rows = page.locator('[data-testid="gateway-endpoint"]');
const n = await rows.count();
check("完整请求地址条数正确", n === EXPECT.length, `实际 ${n}，期望 ${EXPECT.length}`);

// ③ 每条都必须是**完整端点**（不能退化成裸 Base URL —— 那正是 404 的成因）
const urls = await rows.evaluateAll((els) => els.map((e) => e.dataset.url));
check(
  "每条都是完整端点（非裸 /v1）",
  urls.length === EXPECT.length && urls.every((u) => /\/v1\/.+|\/mcp$/.test(u)),
  JSON.stringify(urls),
);
check(
  "端点集合与期望一致（顺序也一致）",
  JSON.stringify(urls) === JSON.stringify(EXPECT.map((e) => e[2])),
  JSON.stringify(urls),
);

// ④ 逐条点复制图标 → 剪贴板必须等于该条完整地址
let copyOk = 0;
const copyDetail = [];
for (let i = 0; i < n; i++) {
  const row = rows.nth(i);
  await row.scrollIntoViewIfNeeded();
  await row.locator('button[aria-label="复制"]').first().click();
  await page.waitForTimeout(320);
  const got = await readClip();
  const want = EXPECT[i]?.[2];
  if (got === want) copyOk++;
  else copyDetail.push(`#${i} got=${JSON.stringify(got)} want=${JSON.stringify(want)}`);
}
check("逐条复制内容全部正确", copyOk === EXPECT.length, `${copyOk}/${EXPECT.length} ${copyDetail.join(" | ")}`);

// ⑤ 顶部常驻条的「复制完整地址」一次性复制：含 Base URL 与全部完整地址
const bulk = page.locator('[data-testid="gateway-copy-endpoints"]');
check("「复制完整地址」按钮存在", (await bulk.count()) === 1);
await bulk.click();
await page.waitForTimeout(400);
const bulkText = await readClip();
check("一次性复制含 Base URL", bulkText.includes(`Base URL`) && bulkText.includes(BASE), bulkText.slice(0, 80));
check("一次性复制含 API Key", /API Key\s+sk-jai-/.test(bulkText), bulkText.split("\n").slice(0, 5).join(" / "));
check(
  "一次性复制含全部完整地址",
  EXPECT.every(([, , u]) => bulkText.includes(u)),
  `缺少：${EXPECT.filter(([, , u]) => !bulkText.includes(u)).map(([l]) => l).join("、") || "无"}`,
);

// ⑥ 说明文案必须点出「精确地址 / 404」这个坑（否则用户仍不知何时该用哪一项）
const cardText = (await page.locator("main, body").first().innerText()).replace(/\s+/g, " ");
check("文案点出「精确请求地址」", cardText.includes("精确请求地址"), "");
check("文案点出 404 后果", cardText.includes("404"), "");

// ⑦ 无横向溢出（新增行含固定宽标签 + 等宽 URL，窄屏最易溢出）
const geo = await page.evaluate(() => ({
  vw: window.innerWidth,
  docW: document.documentElement.scrollWidth,
  rowOverflow: [...document.querySelectorAll('[data-testid="gateway-endpoint"]')]
    .map((el) => Math.round(el.scrollWidth - el.clientWidth))
    .filter((d) => d > 1),
}));
check("无横向溢出", geo.docW <= geo.vw + 1, `scrollWidth=${geo.docW} viewport=${geo.vw}`);
check("地址行内部无横向溢出", geo.rowOverflow.length === 0, JSON.stringify(geo.rowOverflow));

check("无控制台报错", out.errors.length === 0, out.errors.join(" | ").slice(0, 300));

out.shot = path.join(SHOTS, `gateway-endpoints-${VW}x${VH}.png`);
await page.screenshot({ path: out.shot, fullPage: true });

out.passed = out.checks.filter((c) => c.ok).length;
out.total = out.checks.length;
out.ok = out.passed === out.total;
const outFile = path.join(ROOT, `out-gateway-endpoints-${VW}x${VH}.json`);
fs.writeFileSync(outFile, JSON.stringify(out, null, 2));
console.log(`\n${out.ok ? "全绿" : "有失败"} ${out.passed}/${out.total} → ${outFile}`);

await browser.close();
process.exit(out.ok ? 0 : 1);
