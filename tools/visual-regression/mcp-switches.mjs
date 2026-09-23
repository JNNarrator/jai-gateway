// MCP 页「两个 per-server 开关」的说明性回归探针。
//
// 背景：`启用` 与 `代理执行` 两个开关此前的可见信息为零/一个「代理」小字，
// 用户无法判断各自作用与依赖关系（启用=网关是否连它；代理执行=是否让 Agent 调它的工具，
// 且需「启用」同时打开）。本探针把「有可见标签 + 有解释文案 + hover 有详情 + 点文字能切换」
// 固化成断言，避免以后再被静默删掉。
//
// 用法（仓库根目录，需先起 vite:5173）：
//   export TMPDIR=$PWD/.vr/tmp
//   node tools/visual-regression/mcp-switches.mjs --size=1180x800
//   node tools/visual-regression/mcp-switches.mjs --size=900x600
// 产出：.vr/out-mcp-switches-<size>.json + .vr/shots/mcp-switches-<size>.png
import fs from "node:fs";
import path from "node:path";
import { launchBrowser } from "./_env.mjs";

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

/** 最小 Tauri mock：只覆盖 MCP 页 + 外壳启动所需命令，其余返回 null（与 run.mjs 同口径）。 */
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
      // 关键：每次返回**新的**副本，模拟真实 IPC 的反序列化边界。
      // 若直接返回同一个数组引用，React 的 setList(next) 会因 Object.is(prev,next) 跳过重渲染，
      // 于是「切换开关后 UI 不更新」是 mock 假象而非真 bug（2026-09-16 实测踩到）。
      // 网关页（首页）会调这两个命令取「密钥管理」列表。**必须返回数组**：
      // 本 mock 的 `default: return null` 会让 `setKeys(null)` 在渲染时抛
      // `Cannot read properties of null`，整棵 React 树被卸载 → 连侧边栏都没有，
      // 探针切页时表现为「等按钮超时」。多密钥（D9-T6a）之后新增的依赖，
      // 这类极简 mock 都要跟着补。
      case "gateway_key_list": return [];
      case "gateway_key_rules_get": return { providerAllow: [], providerDeny: [], modelAllow: [], modelDeny: [] };
      case "mcp_list": return JSON.parse(JSON.stringify(fix.mcp_list));
      case "mcp_tools_list": return JSON.parse(JSON.stringify(fix.mcp_tools_list));
      case "mcp_export_config": return { mcpServers: {} };
      case "mcp_set_enabled": {
        const row = (fix.mcp_list || []).find((x) => x.id === args.id);
        if (row) row.enabled = !!args.enabled;
        return null;
      }
      case "mcp_set_proxy_allowed": {
        const row = (fix.mcp_list || []).find((x) => x.id === args.id);
        if (row) row.proxyAllowed = !!args.allowed;
        return null;
      }
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
  // `@tauri-apps/api` 的 `_unlisten` 会直接读这个对象（真实运行由 Tauri 注入）。
  // 不补它，TitleBar 的 `win.onResized()` 清理路径就抛
  // `Cannot read properties of undefined (reading 'unregisterListener')` ——
  // 该 pageerror 在**每个页面**都会发生，于是「无控制台报错」这条断言恒为假、永久失效
  // （2026-09-20 实测：既有 mcp-switches.mjs 也一直是红的，只是没人注意）。
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
const browser = await launchBrowser();
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
// 切到 MCP 页
await page.getByRole("button", { name: /^MCP$/ }).first().click();
await page.waitForTimeout(700);

// ① 顶部解释条：必须存在且有两条说明
const help = page.locator('[data-testid="mcp-switch-help"]');
const helpText = (await help.count()) ? (await help.innerText()).replace(/\s+/g, " ").trim() : "";
check("顶部解释条存在", await help.count() === 1);
check("解释条含「启用」说明", helpText.includes("启用") && helpText.includes("网关是否连接"), helpText.slice(0, 60));
check("解释条含「代理执行」说明", helpText.includes("代理执行") && helpText.includes("Agent 能调"), helpText.slice(0, 120));

// ② 两个开关都有可见文字标签（不能再是裸开关 / 只写「代理」）
const row = page.locator(".space-y-3 > div").first();
const rowText = (await row.innerText()).replace(/\s+/g, " ");
check("行内可见「启用」标签", /(^|\s)启用(\s|$)/.test(rowText), rowText.slice(0, 80));
check("行内可见「代理执行」标签", rowText.includes("代理执行"));
check("已去掉含混的「代理」孤字", !/(^|\s)代理(\s|$)/.test(rowText));

// ③ hover 标签 → 出现详情 tooltip（且内容与开关语义一致）
const rowProxyLabel = row.getByText("代理执行", { exact: true }).first();
await rowProxyLabel.hover();
await page.waitForTimeout(400);
const tipText = (await page.locator('[data-slot="tooltip-content"]').allInnerTexts()).join(" ").replace(/\s+/g, " ");
check("hover 出现代理执行详情", tipText.includes("转发") && tipText.includes("最小权限"), tipText.slice(0, 80));

// ④ 点标签文字能切换开关（label ↔ switch 关联有效），且真的发了命令
const before = await row.locator('[role="switch"]').first().getAttribute("aria-checked");
await row.getByText("启用", { exact: true }).first().click();
await page.waitForTimeout(500);
const after = await row.locator('[role="switch"]').first().getAttribute("aria-checked");
const setEnabledCalls = await page.evaluate(() =>
  (window.__JAI_CALLS__ || []).filter((c) => c.cmd === "mcp_set_enabled").map((c) => c.args),
);
check("点「启用」文字可切换开关", before === "true" && after === "false", `${before} → ${after}`);
check(
  "切换真的落命令 mcp_set_enabled（且参数正确）",
  setEnabledCalls.length === 1 && setEnabledCalls[0].enabled === false,
  JSON.stringify(setEnabledCalls),
);

// ⑤ 无横向溢出 / 右侧操作按钮未被挤出或遮挡
//    注意：行内操作按钮在 <1024px（lg 断点）下是纯图标，文字标签 `删除` 等被 `hidden lg:inline`
//    隐藏 → innerText 为空。故判定必须回落到 aria-label / title，否则 900×600 下会「找不到按钮」
//    并给出**假失败**（曾据此误判 900×600 已通过）。
const geo = await page.evaluate(() => {
  const vw = window.innerWidth;
  const docW = document.documentElement.scrollWidth;
  const btns = [...document.querySelectorAll("button")]
    .map((b) => {
      const r = b.getBoundingClientRect();
      const name = (b.innerText || b.getAttribute("aria-label") || b.title || "").trim();
      return { text: name.slice(0, 4), right: Math.round(r.right), visible: r.width > 0 && r.height > 0 };
    })
    .filter((b) => /删除|编辑|列出工具|测试连接/.test(b.text));
  const worst = btns.sort((a, b) => b.right - a.right)[0];
  return { vw, docW, matched: btns.length, worst };
});
check("无横向溢出", geo.docW <= geo.vw + 1, `scrollWidth=${geo.docW} viewport=${geo.vw}`);
check(
  "右侧操作按钮完整可见",
  geo.matched > 0 && geo.worst && geo.worst.visible && geo.worst.right <= geo.vw,
  `matched=${geo.matched} ${JSON.stringify(geo.worst)}`,
);

// ⑥ 「已停用 + 代理执行开」组合必须显式提示「暂不生效」（最容易被误读的状态）
//    上一步刚把「启用」点关，而 fixture 里该行 proxyAllowed=true → 正好是这个组合
const inactiveHint = row.locator('[data-testid="mcp-proxy-inactive"]');
const hintText = (await inactiveHint.count()) ? (await inactiveHint.innerText()).trim() : "";
check("停用+代理执行开 → 出现「暂不生效」提示", await inactiveHint.count() === 1, hintText);
// 「存在 + 文案对」不等于「看得见」：必须不被祖先 truncate 裁掉、在行内、在视口内
// （2026-09-16 踩到：提示曾放在 truncate 的路径行里，count/innerText 全过、肉眼全无）
const hintGeo = await inactiveHint.evaluate((el) => {
  const r = el.getBoundingClientRect();
  const row = el.closest(".rounded-lg");
  const rr = row.getBoundingClientRect();
  return {
    w: Math.round(r.width), h: Math.round(r.height),
    right: Math.round(r.right), rowRight: Math.round(rr.right),
    visible: r.width > 0 && r.height > 0,
    inViewport: r.right <= window.innerWidth + 1,
    inRow: r.right <= rr.right + 1 && r.left >= rr.left - 1,
  };
});
check("该提示真的可见（未被裁切/未溢出）", hintGeo.visible && hintGeo.inViewport && hintGeo.inRow, JSON.stringify(hintGeo));
// 留一张该组合的现场图（终态截图是重新启用后的，看不到提示）
out.inactiveShot = path.join(SHOTS, `mcp-switches-inactive-${VW}x${VH}.png`);
await page.screenshot({ path: out.inactiveShot });
// 再点回「启用」→ 提示应消失（避免长期占位）
await row.getByText("启用", { exact: true }).first().click();
await page.waitForTimeout(500);
check("重新启用后提示消失", (await row.locator('[data-testid="mcp-proxy-inactive"]').count()) === 0);

check("无控制台报错", out.errors.length === 0, out.errors.slice(0, 3).join(" | "));

const shot = path.join(SHOTS, `mcp-switches-${VW}x${VH}.png`);
await page.screenshot({ path: shot });
out.screenshot = shot;
out.passed = out.checks.every((c) => c.ok);
out.finishedAt = new Date().toISOString();
const outFile = path.join(ROOT, `out-mcp-switches-${VW}x${VH}.json`);
fs.writeFileSync(outFile, JSON.stringify(out, null, 2));
console.log(`\n${out.passed ? "全部通过" : "存在失败项"} → ${outFile}\n截图：${shot}`);

await browser.close();
process.exit(out.passed ? 0 : 1);
