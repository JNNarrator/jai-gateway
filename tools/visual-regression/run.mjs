// JAI 视觉回归测试台（在仓库根目录运行；勿从 `.vr/` 的本地副本运行）
//   node tools/visual-regression/run.mjs --mode=walk  --size=1180x800
//   node tools/visual-regression/run.mjs --mode=steps --size=1180x800
//   node tools/visual-regression/run.mjs --mode=walk  --size=900x600 --tabs=logs,models
// 产出：.vr/shots/*.png + .vr/out-<mode>-<size>.json
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
const MODE = argv.mode || "walk";
const [VW, VH] = (argv.size || "980x640").split("x").map(Number);
const ONLY_TABS = argv.tabs ? String(argv.tabs).split(",") : null;
const URL_ = argv.url || "http://127.0.0.1:5173/";
const TAG = `${MODE}-${VW}x${VH}`;
const TABS = ["gateway", "sync", "mcp", "skills", "providers", "models", "stats", "logs", "settings"];
const TAB_LABEL = {
  gateway: "网关", sync: "同步", mcp: "MCP", skills: "技能", providers: "供应商",
  models: "模型", stats: "统计", logs: "日志", settings: "设置",
};

// ───────────────────────── mock ─────────────────────────
function installMock() {
  const fix = window.__JAI_FIX__;
  const state = { running: true, providers: null, models: null, mcp: null, skills: null };
  const calls = [];
  const opened = [];
  window.__JAI_CALLS__ = calls;
  window.__JAI_OPENED__ = opened;
  const cb = new Map();
  let cbId = 1;
  function appInvoke(cmd, args) {
    calls.push({ cmd, args });
    switch (cmd) {
      case "gateway_status": return { ...fix.gateway_status, running: state.running };
      case "gateway_start": state.running = true; return { ...fix.gateway_status, running: true };
      case "gateway_stop": state.running = false; return { ...fix.gateway_status, running: false };
      case "model_list": return fix.model_list[args.providerId] || [];
      case "logs_recent": return fix.logs_recent.slice(0, args?.limit ?? 100);
      case "stats_usage": return fix.stats_usage.slice(0, args?.days ?? 7);
      case "gateway_key_reveal": return fix.gateway_key_info;
      case "export_config_json": return JSON.stringify({ format: "jai-export/v1", providers: fix.provider_list, models: fix.model_list["01a0565e-792a-7aa3-a1cb-a4c8a689568c"] }, null, 2);
      case "export_config_to_file": return "/Users/jiangnan/Documents/JAI/jai-export-1788401052890.json";
      case "skill_export_markdown": return "# rust-review\n\n对 Rust 改动做逐条评审……\n";
      case "mcp_export_config": return { mcpServers: Object.fromEntries(fix.mcp_list.map((m) => [m.name, m.kind === "stdio" ? { command: m.command, args: JSON.parse(m.args || "[]") } : { url: m.url }])) };
      case "mcp_tools_list": return fix.mcp_tools_list;
      case "mcp_tools_call": return { content: [{ type: "text", text: JSON.stringify({ ok: true, echoed: args?.arguments ?? null }, null, 2) }] };
      case "mcp_import": return { imported: 1, updated: 0, skipped: ["already-exists"] };
      case "skill_import_zip": return 2;
      case "provider_test": return "连通正常，返回 21 个模型（耗时 812ms）";
      case "provider_test_draft": return { ok: true, count: 21, modelNames: fix.model_list["01a0565e-792a-7aa3-a1cb-a4c8a689568c"].map((m) => m.modelName) };
      case "provider_discover_models": return [21, 3];
      case "webdav_test": return "连接成功：目录 jai/config 可读写（延迟 236ms）";
      case "proxy_test": return "代理可用：经 127.0.0.1:7890 访问 https://api.anthropic.com 返回 200";
      case "read_env_var": return "";
      case "port_in_use": return false;
      case "settings_set_port": return (args && args.port) || 1314;
      case "settings_set_logs_enabled": return !!(args && args.enabled);
      case "webdav_pull":
      case "webdav_snapshot_restore":
      case "webdav_backup_restore":
      case "config_import": return fix.config_import;
      default:
        if (cmd in fix) return fix[cmd];
        return null;
    }
  }
  function pluginInvoke(cmd, args) {
    switch (cmd) {
      case "plugin:event|listen": return 1;
      case "plugin:window|is_maximized": return false;
      case "plugin:window|is_focused": return true;
      case "plugin:window|is_visible": return true;
      case "plugin:window|is_fullscreen": return false;
      case "plugin:window|is_decorated": return false;
      case "plugin:window|scale_factor": return 2;
      case "plugin:window|inner_size":
      case "plugin:window|outer_size": return { width: window.__VR_VW__ || 980, height: window.__VR_VH__ || 640 };
      case "plugin:window|inner_position":
      case "plugin:window|outer_position": return { x: 100, y: 100 };
      case "plugin:window|theme": return "dark";
      case "plugin:window|cursor_position": return { x: 0, y: 0 };
      case "plugin:window|get_all_windows": return ["main"];
      case "plugin:opener|open_url":
      case "plugin:opener|open_path": opened.push(args); return null;
      case "plugin:opener|allowlist": return { urls: [], paths: [] };
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
      try {
        return Promise.resolve(cmd.indexOf("plugin:") === 0 ? pluginInvoke(cmd, args) : appInvoke(cmd, args));
      } catch (e) { return Promise.reject(String(e)); }
    },
    convertFileSrc: (p) => "data:,",
    registerPlugin() {},
  };
  window.__TAURI__ = { event: {}, window: {}, core: {} };
  // 见 mcp-switches.mjs 同处注释：`_unlisten` 需要 Tauri 注入的事件插件内部对象，
  // 缺了它 TitleBar 的 onResized 清理会抛 pageerror。
  window.__TAURI_EVENT_PLUGIN_INTERNALS__ = {
    unregisterListener(_event, id) { cb.delete(id); },
  };
}

// ───────────────────────── 探针 ─────────────────────────
function probe() {
  const vw = window.innerWidth, vh = window.innerHeight;
  const INTER = ["button", "a[href]", "input", "select", "textarea", "[role=button]", "[role=menuitem]",
    "[role=menuitemcheckbox]", "[role=option]", "[role=tab]", "[role=switch]", "[role=checkbox]",
    "[role=radio]", "[role=combobox]", "[role=link]"].join(",");
  const pathOf = (el) => {
    const parts = []; let n = el;
    for (let i = 0; i < 5 && n && n.tagName !== "BODY"; i++) {
      let s = n.tagName.toLowerCase();
      const dl = n.getAttribute("data-slot");
      if (dl) s += `[${dl}]`;
      else if (n.id) s += "#" + n.id;
      else {
        const c = (typeof n.className === "string" ? n.className : "").split(/\s+/).filter(Boolean)[0];
        if (c) s += "." + c.slice(0, 22);
      }
      parts.unshift(s); n = n.parentElement;
    }
    return parts.join(">");
  };
  const desc = (el) => {
    const t = (el.getAttribute("aria-label") || el.textContent || el.getAttribute("title") || el.getAttribute("placeholder") || el.value || "").trim().replace(/\s+/g, " ");
    return { tag: el.tagName.toLowerCase(), text: t.slice(0, 60), path: pathOf(el), disabled: !!el.disabled || el.getAttribute("aria-disabled") === "true" };
  };
  const scrollAncestor = (el, axis) => {
    let n = el.parentElement;
    while (n) {
      const cs = getComputedStyle(n);
      const prop = axis === "y" ? cs.overflowY : cs.overflowX;
      if (/(auto|scroll|overlay)/.test(prop)) {
        const over = axis === "y" ? n.scrollHeight > n.clientHeight + 1 : n.scrollWidth > n.clientWidth + 1;
        if (over) return { node: pathOf(n), slot: n.getAttribute("data-slot"), fixed: cs.position === "fixed" };
      }
      if (cs.position === "fixed") return null; // fixed 容器内无法靠外部滚动到达
      n = n.parentElement;
    }
    return null;
  };
  const inDialog = (el) => !!el.closest('[role="dialog"],[role="alertdialog"]');
  const res = {
    vw, vh,
    doc: { scrollW: document.documentElement.scrollWidth, clientW: document.documentElement.clientWidth, scrollH: document.documentElement.scrollHeight, clientH: document.documentElement.clientHeight },
    unreachable: [], covered: [], offscreenScroll: [], clipped: [], clippedEdge: [], clippedInScroller: [], underSticky: [], dialogs: [], toasts: [], menus: [],
    counts: { inter: 0 },
  };
  const nodes = Array.from(document.querySelectorAll(INTER));
  res.counts.inter = nodes.length;
  for (const el of nodes) {
    const cs = getComputedStyle(el);
    if (cs.visibility === "hidden" || cs.display === "none" || cs.opacity === "0") continue;
    if (el.closest("[hidden]") || el.closest('[aria-hidden="true"]')) continue;
    const r = el.getBoundingClientRect();
    if (r.width < 1 || r.height < 1) continue;
    const d = desc(el);
    const outTop = -r.top, outBot = r.bottom - vh, outLeft = -r.left, outRight = r.right - vw;
    const fullyIn = r.top >= -1 && r.left >= -1 && r.bottom <= vh + 1 && r.right <= vw + 1;
    if (!fullyIn) {
      const can = outTop > 1 || outBot > 1 ? scrollAncestor(el, "y") : null;
      const canX = outLeft > 1 || outRight > 1 ? scrollAncestor(el, "x") : null;
      const item = { ...d, rect: { x: Math.round(r.x), y: Math.round(r.y), w: Math.round(r.width), h: Math.round(r.height) }, out: { top: Math.round(outTop), bottom: Math.round(outBot), left: Math.round(outLeft), right: Math.round(outRight) }, inDialog: inDialog(el), scroll: can || canX || null };
      (can || canX ? res.offscreenScroll : res.unreachable).push(item);
      continue;
    }
    // 元素被某个可滚动祖先裁在可视框外（含弹窗正文滚动区、表格内滚容器）→ 属于「需滚动」而非「被遮挡」
    {
      let n = el.parentElement, clip = null;
      while (n && n !== document.body) {
        const cs2 = getComputedStyle(n);
        if (/(auto|scroll)/.test(cs2.overflowY) || /(auto|scroll)/.test(cs2.overflowX)) {
          const r2 = n.getBoundingClientRect();
          if (r.bottom > r2.bottom + 1 || r.top < r2.top - 1 || r.right > r2.right + 1 || r.left < r2.left - 1) { clip = n; break; }
        }
        n = n.parentElement;
      }
      if (clip) { res.clippedInScroller.push({ ...d, rect: { x: Math.round(r.x), y: Math.round(r.y), w: Math.round(r.width), h: Math.round(r.height) }, container: (clip.getAttribute("data-slot") || clip.className.toString().split(/\s+/)[0] || clip.tagName).slice(0, 40) }); continue; }
    }
    const cx = Math.round(r.left + r.width / 2), cy = Math.round(r.top + r.height / 2);
    const top = document.elementFromPoint(Math.max(1, Math.min(cx, vw - 1)), Math.max(1, Math.min(cy, vh - 1)));
    if (!top || !(top === el || el.contains(top) || top.contains(el))) {
      const item = { ...d, hitBy: { ...desc(top), z: getComputedStyle(top).zIndex, pos: getComputedStyle(top).position }, rect: { x: Math.round(r.x), y: Math.round(r.y), w: Math.round(r.width), h: Math.round(r.height) }, inDialog: inDialog(el) };
      // 元素被滚动容器的上边缘切开（中心落到标题栏）→ 归入 clippedEdge，不算「点不到」
      const hdr = document.querySelector("header");
      const hdrBottom = hdr ? hdr.getBoundingClientRect().bottom : 0;
      if (top && top.tagName === "HEADER" && hdrBottom > 0 && r.top < hdrBottom) res.clippedEdge.push(item);
      else if (top && getComputedStyle(top).position === "sticky") res.underSticky.push(item); // 被页面吸顶操作条/表头压住，属可见性设计
      else res.covered.push(item);
    }
  }
  for (const dlg of document.querySelectorAll('[role="dialog"],[role="alertdialog"]')) {
    const r = dlg.getBoundingClientRect();
    const cs = getComputedStyle(dlg);
    const kids = Array.from(dlg.querySelectorAll(INTER));
    const outKids = kids.filter((k) => { const kr = k.getBoundingClientRect(); return kr.top < -1 || kr.bottom > vh + 1 || kr.left < -1 || kr.right > vw + 1; });
    const unreachable = outKids.filter((k) => !(scrollAncestor(k, "y") || scrollAncestor(k, "x")));
    res.dialogs.push({
      slot: dlg.getAttribute("data-slot"),
      title: (dlg.querySelector('[data-slot=dialog-title],[data-slot=alert-dialog-title],[role=heading]')?.textContent || "").trim().slice(0, 40),
      rect: { x: Math.round(r.x), y: Math.round(r.y), w: Math.round(r.width), h: Math.round(r.height) },
      overflowTop: Math.round(-r.top), overflowBottom: Math.round(r.bottom - vh),
      contentH: dlg.scrollHeight, clientH: dlg.clientHeight, overflowY: cs.overflowY, maxHeight: cs.maxHeight,
      selfScrollable: /(auto|scroll)/.test(cs.overflowY) && dlg.scrollHeight > dlg.clientHeight + 1,
      interactive: kids.length, unreachableCount: unreachable.length,
      unreachable: unreachable.slice(0, 14).map((k) => { const kr = k.getBoundingClientRect(); return { ...desc(k), y: Math.round(kr.y), bottom: Math.round(kr.bottom) }; }),
    });
  }
  for (const t of document.querySelectorAll('[data-sonner-toaster],[data-sonner-toast]')) {
    const r = t.getBoundingClientRect();
    if (r.width > 0) res.toasts.push({ kind: t.hasAttribute("data-sonner-toaster") ? "toaster" : "toast", rect: { x: Math.round(r.x), y: Math.round(r.y), w: Math.round(r.width), h: Math.round(r.height) }, text: (t.textContent || "").trim().slice(0, 50) });
  }
  for (const m of document.querySelectorAll('[role="menu"],[role="listbox"],[data-slot=dropdown-menu-content],[data-slot=select-content]')) {
    const r = m.getBoundingClientRect();
    const cs = getComputedStyle(m);
    res.menus.push({ slot: m.getAttribute("data-slot"), role: m.getAttribute("role"), rect: { x: Math.round(r.x), y: Math.round(r.y), w: Math.round(r.width), h: Math.round(r.height) }, overflowBottom: Math.round(r.bottom - vh), overflowTop: Math.round(-r.top), items: m.querySelectorAll('[role=menuitem],[role=option]').length, scrollable: /(auto|scroll)/.test(cs.overflowY) && m.scrollHeight > m.clientHeight + 1 });
  }
  const tw = document.createTreeWalker(document.body, NodeFilter.SHOW_TEXT);
  let tn, checked = 0;
  while ((tn = tw.nextNode()) && checked < 5000) {
    const el = tn.parentElement;
    if (!el || !tn.textContent.trim()) continue;
    checked++;
    const cs = getComputedStyle(el);
    if (/(auto|scroll)/.test(cs.overflowX) || cs.textOverflow === "ellipsis" || el.closest("pre,textarea,code,svg")) continue;
    if (el.scrollWidth > el.clientWidth + 2 && el.clientWidth > 0) {
      const r = el.getBoundingClientRect();
      if (r.width > 0 && r.right > vw + 1 && !scrollAncestor(el, "x")) {
        res.clipped.push({ text: tn.textContent.trim().slice(0, 60), path: pathOf(el), right: Math.round(r.right), extra: el.scrollWidth - el.clientWidth });
      }
    }
  }
  return res;
}

// ───────────────────────── 运行 ─────────────────────────
const browser = await chromium.launch({
  executablePath: "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome",
  headless: true,
});
const ctx = await browser.newContext({ viewport: { width: VW, height: VH }, deviceScaleFactor: 2, locale: "zh-CN", timezoneId: "Asia/Shanghai" });
await ctx.addInitScript({ content: `window.__JAI_FIX__ = ${JSON.stringify(fixtures)}; window.__VR_VW__=${VW}; window.__VR_VH__=${VH};` });
await ctx.addInitScript({ content: `(${installMock.toString()})();` });
await ctx.addInitScript({ content: `try{localStorage.setItem('jai-sidebar-collapsed','0');}catch(e){}` });

const page = await ctx.newPage();
const consoleErrors = [];
page.on("console", (m) => { if (m.type() === "error") consoleErrors.push("error: " + m.text().slice(0, 200)); });
page.on("pageerror", (e) => consoleErrors.push("pageerror: " + String(e.message).slice(0, 200)));

await page.goto(URL_, { waitUntil: "domcontentloaded" });
await page.waitForTimeout(1200);

const out = { mode: MODE, vw: VW, vh: VH, startedAt: new Date().toISOString(), steps: [] };
let shotIdx = 0;
const slug = (s) => String(s).replace(/[^\w\u4e00-\u9fa5-]+/g, "_").slice(0, 50);

async function snap(label, note) {
  const idx = String(++shotIdx).padStart(3, "0");
  const file = path.join(SHOTS, `${TAG}-${idx}-${slug(label)}.png`);
  await page.screenshot({ path: file }).catch(() => {});
  let p;
  try { p = await page.evaluate(probe); } catch (e) { p = { probeError: String(e.message) }; }
  out.steps.push({ idx, label, note: note || "", shot: path.relative(".", file), probe: p });
  const dlgBad = (p.dialogs || []).reduce((a, d) => a + d.unreachableCount, 0);
  const n = (p.unreachable?.length || 0) + (p.covered?.length || 0) + dlgBad;
  console.log(`[${idx}] ${label} :: out=${p.unreachable?.length ?? "?"} covered=${p.covered?.length ?? "?"} dlg=${(p.dialogs || []).map((d) => `${d.title || d.slot}:${d.overflowTop > 1 || d.overflowBottom > 1 ? "OVERFLOW" : "fit"}(unreach ${d.unreachableCount})`).join(" | ") || "-"} menus=${(p.menus || []).length} BAD=${n} clippedEdge=${p.clippedEdge?.length ?? 0} inScroller=${p.clippedInScroller?.length ?? 0} underSticky=${p.underSticky?.length ?? 0}`);
  fs.writeFileSync(path.join(ROOT, `out-${TAG}.json`), JSON.stringify({ ...out, consoleErrors: [...new Set(consoleErrors)] }, null, 2));
  return p;
}

async function gotoTab(tab) {
  await page.click(`aside nav button[aria-label="${TAB_LABEL[tab]}"]`, { timeout: 3000 });
  await page.waitForTimeout(600);
}
async function closeOverlays() {
  for (let i = 0; i < 3; i++) {
    const open = await page.locator('[role="dialog"],[role="alertdialog"],[role="menu"],[role="listbox"]').count();
    if (!open) break;
    await page.keyboard.press("Escape");
    await page.waitForTimeout(250);
  }
}
const curTabLabel = () => page.evaluate(() => {
  const b = Array.from(document.querySelectorAll("aside nav button")).find((x) => /bg-accent/.test(x.className));
  return b ? b.getAttribute("aria-label") : "?";
});

async function walkTab(tab) {
  await gotoTab(tab);
  await snap(`tab:${tab}`, "进入标签页");
  const seen = new Set();
  const LIMIT = Number(argv.limit || 90);
  for (let iter = 0; iter < LIMIT; iter++) {
    const list = await page.evaluate((prevSig) => {
      const sigOf = (el, i) => [i, (el.getAttribute("aria-label") || el.textContent || "").trim().replace(/\s+/g, " ").slice(0, 40), el.getAttribute("role") || el.tagName, el.className.toString().slice(0, 30)].join("|");
      const scope = document.querySelectorAll("main button, main [role=button], main [role=combobox], main [role=switch], main input[type=checkbox], main [role=menuitem]");
      const items = [];
      scope.forEach((el, i) => {
        el.setAttribute("data-vr-i", String(i));
        const r = el.getBoundingClientRect();
        if (r.width < 1 || r.height < 1) return;
        items.push({ i, sig: sigOf(el, i) });
      });
      return items;
    });
    const next = list.find((x) => !seen.has(x.sig));
    if (!next) break;
    seen.add(next.sig);
    const loc = page.locator(`main [data-vr-i="${next.i}"]`).first();
    if (!(await loc.isVisible().catch(() => false))) continue;
    await loc.scrollIntoViewIfNeeded().catch(() => {});
    const label = `${tab}#${seen.size}:${next.sig.split("|")[1] || next.sig.split("|")[2]}`;
    const clicked = await loc.click({ timeout: 2500 }).then(() => true).catch((e) => { out.steps.push({ label, clickError: String(e.message).slice(0, 160) }); console.log(`[skip] ${label} :: 点击失败 ${String(e.message).slice(0, 80)}`); return false; });
    if (!clicked) continue;
    await page.waitForTimeout(420);
    await snap(`click:${label}`);
    // 若点开弹窗：把弹窗内控件也点一遍（保存/取消/添加一行/字段下拉 → 观察高度增长与遮挡）
    let dlgCount = await page.locator('[role="dialog"],[role="alertdialog"]').count();
    if (dlgCount) {
      const dseen = new Set();
      for (let k = 0; k < 14 && dlgCount; k++) {
        const dl = await page.evaluate(() => {
          const scope = document.querySelectorAll('[role="dialog"] button, [role="alertdialog"] button, [role="dialog"] [role=combobox], [role="dialog"] [role=switch], [role="dialog"] input, [role="dialog"] textarea, [role="dialog"] [role=checkbox]');
          return Array.from(scope).map((el, i) => {
            el.setAttribute("data-vr-d", String(i));
            const txt = (el.getAttribute("aria-label") || el.textContent || el.getAttribute("placeholder") || "").trim().replace(/\s+/g, " ").slice(0, 40);
            return { i, sig: `${txt || el.tagName.toLowerCase()}|${el.tagName}` };
          });
        });
        const t = dl.find((x) => !dseen.has(x.sig));
        if (!t) break;
        dseen.add(t.sig);
        const dloc = page.locator(`[data-vr-d="${t.i}"]`).first();
        const opened = await dloc.click({ timeout: 2000 }).then(() => true).catch(() => false);
        await page.waitForTimeout(380);
        if (opened) await snap(`dlg:${label}>${t.sig}`);
        // 弹窗内弹出的下拉/listbox 先关掉，再继续点下一个控件
        const lb = await page.locator('[role="listbox"],[role="menu"]').count();
        if (lb) { await page.keyboard.press("Escape"); await page.waitForTimeout(200); }
        dlgCount = await page.locator('[role="dialog"],[role="alertdialog"]').count();
      }
    }
    await closeOverlays();
    if ((await curTabLabel()) !== TAB_LABEL[tab]) await gotoTab(tab);
  }
}

if (MODE === "walk") {
  for (const tab of (ONLY_TABS || TABS)) await walkTab(tab);
} else {
  await snap("boot", "初始网关页");
  await page.keyboard.press("Meta+K");
  await page.waitForTimeout(400);
  await snap("cmdk", "Cmd+K 命令面板");
  await page.keyboard.press("Escape");
  for (const tab of (ONLY_TABS || TABS)) { await gotoTab(tab); await snap(`tab:${tab}`); }
}

fs.writeFileSync(path.join(ROOT, `out-${TAG}.json`), JSON.stringify({ ...out, consoleErrors: [...new Set(consoleErrors)] }, null, 2));
console.log("\n== console 错误 ==");
for (const e of [...new Set(consoleErrors)].slice(0, 20)) console.log(" -", e);
console.log(`截图 ${shotIdx} 张 → ${SHOTS}`);
await browser.close();
