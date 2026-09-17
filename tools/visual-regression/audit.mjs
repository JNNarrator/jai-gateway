// 扩展视觉审计：逐屏 + 逐个弹窗，做「肉眼类」缺陷的可量化测量
//   对比度(WCAG, 含半透明根底的白/黑最坏情况) / 字号 / 命中区尺寸 / 无可访问名
//   容器底部半行截断 / 横向溢出 / 弹窗高度增长 / toast 遮挡
// 用法：node .vr/audit.mjs --size=980x640 --theme=dark
import { createRequire } from "node:module";
import fs from "node:fs";
import path from "node:path";

const require = createRequire(
  "/Users/jiangnan/Documents/workspace/deepseek-harness/node_modules/.pnpm/playwright@1.61.1/node_modules/playwright/",
);
const { chromium } = require("playwright");
const { fixtures } = await import(new URL("./fixtures.mjs", import.meta.url).href);

const argv = Object.fromEntries(
  process.argv.slice(2).map((s) => {
    const m = s.match(/^--([^=]+)(?:=(.*))?$/);
    return m ? [m[1], m[2] ?? true] : [s, true];
  }),
);
const [VW, VH] = (argv.size || "980x640").split("x").map(Number);
const THEME = argv.theme || "dark";
const TABS = (argv.tabs ? String(argv.tabs).split(",") : ["gateway", "sync", "mcp", "skills", "providers", "models", "stats", "logs", "settings"]);
const TAB_LABEL = { gateway: "网关", sync: "同步", mcp: "MCP", skills: "技能", providers: "供应商", models: "模型", stats: "统计", logs: "日志", settings: "设置" };
const TAG = `audit-${THEME}-${VW}x${VH}`;
const SHOTS = path.resolve(".vr/shots");
const ROOT = path.resolve(".vr");

// 复用 run.mjs 里的 invoke mock（避免两份实现漂移）
const runSrc = fs.readFileSync(path.join(ROOT, "run.mjs"), "utf8");
const mockSrc = runSrc.match(/function installMock\(\)[\s\S]*?\n}\n/)?.[0];
if (!mockSrc) throw new Error("未能从 run.mjs 提取 installMock");

const EXT_PROBE = function () {
  const vw = innerWidth, vh = innerHeight;
  const oklabToLinear = (L, A, B) => {
    const l_ = L + 0.3963377774 * A + 0.2158037573 * B;
    const m_ = L - 0.1055613458 * A - 0.0638541728 * B;
    const s_ = L - 0.0894841775 * A - 1.291485548 * B;
    const l = l_ * l_ * l_, m = m_ * m_ * m_, s = s_ * s_ * s_;
    return [4.0767416621 * l - 3.3077115913 * m + 0.2309699292 * s, -1.2684380046 * l + 2.6097574011 * m - 0.3413193965 * s, -0.0041960863 * l - 0.7034186147 * m + 1.707614701 * s];
  };
  const l2s = (v) => (v >= 0.0031308 ? 1.055 * Math.pow(v, 1 / 2.4) - 0.055 : 12.92 * v);
  const num = (t) => (String(t).endsWith("%") ? parseFloat(t) / 100 : parseFloat(t));
  const fin = (lin, alpha) => lin.map((v) => Math.round(Math.max(0, Math.min(1, l2s(v))) * 255)).concat(alpha);
  const rgba = (str) => {
    const s = String(str).trim();
    let m = s.match(/^#([0-9a-f]{3,8})$/i);
    if (m) { const n = m[1]; if (n.length <= 4) return [parseInt(n[0] + n[0], 16), parseInt(n[1] + n[1], 16), parseInt(n[2] + n[2], 16), n.length === 4 ? parseInt(n[3] + n[3], 16) / 255 : 1]; return [parseInt(n.slice(0, 2), 16), parseInt(n.slice(2, 4), 16), parseInt(n.slice(4, 6), 16), n.length >= 8 ? parseInt(n.slice(6, 8), 16) / 255 : 1]; }
    m = s.match(/^(rgba?)\(([^)]+)\)$/i);
    if (m) { const p = m[2].split(/[\s,/]+/).filter(Boolean); return [parseFloat(p[0]), parseFloat(p[1]), parseFloat(p[2]), p.length > 3 ? num(p[3]) : 1]; }
    m = s.match(/^oklch\(([^)]+)\)$/i);
    if (m) { const p = m[1].split(/[\s,/]+/).filter(Boolean); const L = num(p[0]), C = parseFloat(p[1]) || 0, H = (parseFloat(p[2]) || 0) * Math.PI / 180; return fin(oklabToLinear(L, C * Math.cos(H), C * Math.sin(H)), p.length > 3 ? num(p[3]) : 1); }
    m = s.match(/^oklab\(([^)]+)\)$/i);
    if (m) { const p = m[1].split(/[\s,/]+/).filter(Boolean); return fin(oklabToLinear(num(p[0]), num(p[1]), num(p[2])), p.length > 3 ? num(p[3]) : 1); }
    m = s.match(/^color\(srgb([^)]*)\)$/i);
    if (m) { const p = m[1].split(/[\s,/]+/).filter(Boolean).map(parseFloat); return [Math.round(p[0] * 255), Math.round(p[1] * 255), Math.round(p[2] * 255), p.length > 3 ? p[3] : 1]; }
    return null;
  };
  const lum = ([r, g, b]) => {
    const f = (v) => { v /= 255; return v <= 0.03928 ? v / 12.92 : Math.pow((v + 0.055) / 1.055, 2.4); };
    return 0.2126 * f(r) + 0.7152 * f(g) + 0.0722 * f(b);
  };
  const ratio = (a, b) => { const L1 = lum(a), L2 = lum(b); return +(((Math.max(L1, L2) + 0.05) / (Math.min(L1, L2) + 0.05))).toFixed(2); };
  const blend = (fg, bg) => [0, 1, 2].map((i) => fg[i] * fg[3] + bg[i] * (1 - fg[3])).concat(1);
  const pathOf = (el) => {
    const parts = []; let n = el;
    for (let i = 0; i < 4 && n && n.tagName !== "BODY"; i++) {
      let s = n.tagName.toLowerCase();
      const dl = n.getAttribute("data-slot");
      if (dl) s += `[${dl}]`;
      else {
        const c = (typeof n.className === "string" ? n.className : "").split(/\s+/).filter(Boolean)[0];
        if (c) s += "." + c.slice(0, 20);
      }
      parts.unshift(s); n = n.parentElement;
    }
    return parts.join(">");
  };
  const bgChain = (el, backdrop) => {
    let n = el;
    const layers = [];
    while (n) {
      const c = rgba(getComputedStyle(n).backgroundColor);
      if (c && c[3] > 0) layers.push(c);
      if (c && c[3] >= 0.999) break;
      n = n.parentElement;
    }
    let out = backdrop;
    for (let i = layers.length - 1; i >= 0; i--) out = blend(layers[i], out);
    return out;
  };
  const vis = (el) => {
    const cs = getComputedStyle(el);
    if (cs.visibility === "hidden" || cs.display === "none" || +cs.opacity === 0) return false;
    const r = el.getBoundingClientRect();
    return r.width > 0 && r.height > 0;
  };
  const res = {
    vw, vh, theme: document.documentElement.classList.contains("dark") ? "dark" : "light",
    rootAlpha: (() => { const c = rgba(getComputedStyle(document.body).backgroundColor); return c[3]; })(),
    contrast: [], tiny: [], smallTargets: [], noName: [], truncated: [],
    clippedRows: [], hScroll: [], dialogs: [], toasts: [], focusBelowFold: [],
  };

  // 1) 文本对比度 / 字号（白底与黑底两种 backdrop 各算一次，取最坏）
  const texts = Array.from(document.querySelectorAll("body *")).filter((el) => {
    if (!el.children.length && el.textContent.trim().length > 0 && el.textContent.trim().length < 120) return vis(el);
    return false;
  }).slice(0, 900);
  for (const el of texts) {
    const cs = getComputedStyle(el);
    const fg = rgba(cs.color);
    if (!fg) continue;
    const size = parseFloat(cs.fontSize);
    const bold = +cs.fontWeight >= 700;
    const w = bgChain(el, [255, 255, 255, 1]);
    const b = bgChain(el, [0, 0, 0, 1]);
    const r1 = ratio(fg, w), r2 = ratio(fg, b);
    const worst = Math.min(r1, r2);
    const need = size >= 18.5 || (size >= 14 && bold) ? 3 : 4.5;
    const inDlg = !!el.closest('[role="dialog"],[role="alertdialog"]');
    if (worst < need) {
      res.contrast.push({ text: el.textContent.trim().slice(0, 44), size: +size.toFixed(1), weight: cs.fontWeight, worst, need, onWhite: r1, onBlack: r2, inDlg, path: pathOf(el) });
    }
    if (size < 11) res.tiny.push({ text: el.textContent.trim().slice(0, 40), size: +size.toFixed(1), path: pathOf(el) });
  }

  // 2) 交互控件：命中区尺寸 / 可访问名
  const INTER = "button,a[href],input,select,textarea,[role=button],[role=menuitem],[role=switch],[role=checkbox],[role=combobox],[role=radio],[role=tab]";
  for (const el of document.querySelectorAll(INTER)) {
    if (!vis(el)) continue;
    const r = el.getBoundingClientRect();
    const name = (el.getAttribute("aria-label") || el.textContent || el.getAttribute("title") || el.getAttribute("placeholder") || "").trim();
    const hasSvg = !!el.querySelector("svg");
    if ((r.width < 24 || r.height < 26) && r.width > 0) {
      res.smallTargets.push({ name: name.slice(0, 30), w: Math.round(r.width), h: Math.round(r.height), path: pathOf(el), inDlg: !!el.closest('[role="dialog"]') });
    }
    if (!name && hasSvg && r.width > 0) {
      res.noName.push({ path: pathOf(el), cls: (el.className.toString() || "").slice(0, 40) });
    }
  }

  // 3) 容器底部「半行截断」：滚动容器里被切一半的行/卡片
  const containers = [document.documentElement, document.body, ...document.querySelectorAll("main,[data-slot=dialog-content],[role=dialog],aside nav,overflow-y-auto")];
  for (const c of containers) {
    const cs = getComputedStyle(c);
    const scrollable = /(auto|scroll)/.test(cs.overflowY);
    const cr = c.getBoundingClientRect();
    if (cr.width < 1 || cr.height < 1) continue;
    const bottom = scrollable ? Math.min(cr.bottom, vh) : Math.min(cr.bottom, vh);
    const kids = c === document.documentElement || c === document.body ? document.querySelectorAll("main > *, [data-slot=dialog-content] > *") : c.querySelectorAll(":scope > *");
    for (const k of kids) {
      if (!vis(k)) continue;
      const kr = k.getBoundingClientRect();
      const cut = kr.bottom - bottom;
      if (cut > 6 && kr.height > cut + 8 && kr.top < bottom && !scrollable) {
        res.clippedRows.push({ container: pathOf(c), child: pathOf(k), cutBy: Math.round(cut), childH: Math.round(kr.height), scrollable, text: (k.textContent || "").trim().slice(0, 40) });
      }
    }
    if (scrollable && c.scrollHeight > c.clientHeight + 1) {
      const lastRow = Array.from(kids).filter(vis).pop();
      if (lastRow) {
        const lr = lastRow.getBoundingClientRect();
        if (lr.bottom > bottom + 6 && c.scrollTop === 0) {
          // 需要滚动才能看到的最后一行/操作区
          res.focusBelowFold.push({ container: pathOf(c), lastChild: pathOf(lastRow), belowPx: Math.round(lr.bottom - bottom), scrollH: c.scrollHeight, clientH: c.clientHeight, text: (lastRow.textContent || "").trim().slice(0, 40) });
        }
      }
    }
  }

  // 3b) 文本截断：视觉被 ellipsis 截掉、且没有 title 可读全量。
  //     （原实现只初始化了 `res.truncated` 却从不 push，字段恒为空——
  //      「MCP 注册 URL 截 22px、供应商 base URL 截 21px」这类问题因此无法被追踪。）
  for (const el of document.querySelectorAll("body *")) {
    if (!vis(el)) continue;
    const cs = getComputedStyle(el);
    const ellipsis = cs.textOverflow === "ellipsis" || /\btruncate\b/.test(String(el.className || ""));
    if (!ellipsis || el.scrollWidth <= el.clientWidth + 1) continue;
    if (!(el.textContent || "").trim()) continue;
    if (el.getAttribute("title")) continue; // 有 title：hover 可读全量，不算问题
    res.truncated.push({
      text: (el.textContent || "").trim().slice(0, 40),
      w: Math.round(el.clientWidth),
      need: Math.round(el.scrollWidth),
      cut: Math.round(el.scrollWidth - el.clientWidth),
      path: pathOf(el),
      inDlg: !!el.closest('[role="dialog"]'),
    });
  }

  // 4) 横向溢出（页面级 & 容器级不可滚动）
  for (const el of document.querySelectorAll("body *")) {
    if (!vis(el)) continue;
    if (el.scrollWidth > el.clientWidth + 2 && !/(auto|scroll)/.test(getComputedStyle(el).overflowX) && getComputedStyle(el).textOverflow !== "ellipsis") {
      const r = el.getBoundingClientRect();
      if (r.right > vw + 1) res.hScroll.push({ path: pathOf(el), extra: el.scrollWidth - el.clientWidth, text: (el.textContent || "").trim().slice(0, 40) });
    }
  }
  if (document.documentElement.scrollWidth > document.documentElement.clientWidth + 1) {
    res.hScroll.push({ path: "html", extra: document.documentElement.scrollWidth - document.documentElement.clientWidth, text: "__page__" });
  }

  // 5) 弹窗几何（含内部滚动、按钮是否可滚到）
  const INTER2 = "button,input,select,textarea,[role=button],[role=combobox],[role=switch],[role=checkbox]";
  for (const dlg of document.querySelectorAll('[role="dialog"],[role="alertdialog"]')) {
    const r = dlg.getBoundingClientRect();
    const cs = getComputedStyle(dlg);
    const kids = Array.from(dlg.querySelectorAll(INTER2)).filter(vis);
    const unreachable = kids.filter((k) => {
      const kr = k.getBoundingClientRect();
      if (kr.top >= -1 && kr.bottom <= vh + 1) return false;
      let n = k.parentElement, scrollable = null;
      while (n) {
        const s = getComputedStyle(n);
        if (/(auto|scroll)/.test(s.overflowY) && n.scrollHeight > n.clientHeight + 1) { scrollable = n; break; }
        if (s.position === "fixed") break;
        n = n.parentElement;
      }
      return !scrollable;
    });
    const footerBtn = kids.filter((k) => /保存|确定|创建|更新|推送|拉取|导入|删除|恢复|启动|连接|测试/.test((k.textContent || "").trim()));
    const fr = footerBtn.map((k) => { const rr = k.getBoundingClientRect(); return { text: k.textContent.trim().slice(0, 16), top: Math.round(rr.top), bottom: Math.round(rr.bottom), visible: rr.top >= 0 && rr.bottom <= vh }; });
    res.dialogs.push({
      slot: dlg.getAttribute("data-slot"),
      title: (dlg.querySelector("[data-slot=dialog-title],[data-slot=alert-dialog-title],[role=heading]")?.textContent || "").trim().slice(0, 30),
      h: Math.round(r.height), top: Math.round(r.top), bottom: Math.round(r.bottom), vh,
      overTop: Math.round(Math.max(0, -r.top)), overBottom: Math.round(Math.max(0, r.bottom - vh)),
      overflowY: cs.overflowY, maxHeight: cs.maxHeight,
      selfScrollable: /(auto|scroll)/.test(cs.overflowY) && dlg.scrollHeight > dlg.clientHeight + 1,
      interactive: kids.length, unreachable: unreachable.length,
      unreachableNames: unreachable.slice(0, 10).map((k) => (k.getAttribute("aria-label") || k.textContent || k.tagName).trim().slice(0, 20)),
      primaryButtons: fr,
    });
  }

  // 6) toast
  for (const t of document.querySelectorAll('[data-sonner-toaster],[data-sonner-toast]')) {
    const r = t.getBoundingClientRect();
    if (r.width > 0) res.toasts.push({ kind: t.hasAttribute("data-sonner-toaster") ? "toaster" : "toast", rect: { x: Math.round(r.x), y: Math.round(r.y), w: Math.round(r.width), h: Math.round(r.height) }, z: getComputedStyle(t).zIndex, pos: getComputedStyle(t).position, text: (t.textContent || "").trim().slice(0, 40) });
  }
  return res;
};

const browser = await chromium.launch({ executablePath: "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome", headless: true });
const ctx = await browser.newContext({ viewport: { width: VW, height: VH }, deviceScaleFactor: 2, locale: "zh-CN" });
await ctx.addInitScript({ content: `window.__JAI_FIX__ = ${JSON.stringify(fixtures)}; window.__VR_VW__=${VW}; window.__VR_VH__=${VH};` });
await ctx.addInitScript({ content: `(${mockSrc})();` });
await ctx.addInitScript({ content: `try{localStorage.setItem('jai-sidebar-collapsed','0');}catch(e){}` });
// 主题必须在**页面加载前**写进 localStorage：next-themes 只在初始化时读一次，
// 之前只在 goto 之后 add/remove `.dark` 类，导致「应用是暗色、组件库仍是浅色」的错配——
// toast 走 sonner 自带调色板（受 next-themes 驱动），于是测出「浅绿底 + 近白字」的
// 假失败（比值 1.01）。预置 localStorage 后两者同源，暗色审计才可信。
await ctx.addInitScript({ content: `try{localStorage.setItem('theme','${THEME}');}catch(e){}` });
const page = await ctx.newPage();
const errors = [];
page.on("pageerror", (e) => errors.push(String(e.message).slice(0, 160)));

await page.goto(argv.url || "http://127.0.0.1:5173/", { waitUntil: "domcontentloaded" });
await page.waitForTimeout(900);
await page.evaluate((t) => {
  const root = document.documentElement;
  if (t === "dark") root.classList.add("dark"); else root.classList.remove("dark");
  try { localStorage.setItem("theme", t); } catch (e) {}
}, THEME);
await page.waitForTimeout(400);

const out = { tag: TAG, vw: VW, vh: VH, theme: THEME, steps: [] };
let idx = 0;
const slug = (s) => String(s).replace(/[^\w\u4e00-\u9fa5-]+/g, "_").slice(0, 46);
async function snap(label) {
  const n = String(++idx).padStart(3, "0");
  const file = path.join(SHOTS, `${TAG}-${n}-${slug(label)}.png`);
  await page.screenshot({ path: file }).catch(() => {});
  let p;
  try { p = await page.evaluate(EXT_PROBE); } catch (e) { p = { err: String(e.message) }; }
  out.steps.push({ n, label, shot: path.relative(".", file), ext: p });
  const c = p.contrast?.length ?? 0, t = p.tiny?.length ?? 0, s = p.smallTargets?.length ?? 0, d = (p.dialogs || []).filter((x) => x.overTop > 1 || x.overBottom > 1 || x.unreachable > 0).length;
  console.log(`[${n}] ${label} :: contrast=${c} tiny=${t} smallTarget=${s} noName=${p.noName?.length ?? 0} trunc=${p.truncated?.length ?? 0} clipRow=${p.clippedRows?.length ?? 0} hScroll=${p.hScroll?.length ?? 0} dlgBad=${d}`);
  fs.writeFileSync(path.join(ROOT, `${TAG}.json`), JSON.stringify({ ...out, errors: [...new Set(errors)] }, null, 2));
}

const TRIGGER = /添加|新增|新建|编辑|导入|导出|设置|高级|详情|历史|恢复|备份|清理|别名|限额|输出|模态|工具|连接|重新生成|显示|清空|删除|重命名|取消|保存|推送|拉取|测试/;

for (const tab of TABS) {
  await page.click(`aside nav button[aria-label="${TAB_LABEL[tab]}"]`, { timeout: 4000 }).catch(() => {});
  await page.waitForTimeout(650);
  await snap(`tab:${tab}`);
  const seen = new Set();
    const closeHard = async (hint) => {
      for (let k = 0; k < 3; k++) {
        if (!(await page.locator('[role="dialog"],[role="alertdialog"],[role="menu"],[role="listbox"]').count())) return;
        await page.keyboard.press("Escape");
        await page.waitForTimeout(200);
      }
      await page.locator('[role="dialog"] [data-slot="dialog-close"], [role="dialog"] button:has-text("取消")').first().click({ timeout: 1200 }).catch(() => {});
      await page.waitForTimeout(200);
      if (await page.locator('[role="dialog"],[role="alertdialog"]').count()) {
        await page.goto(argv.url || "http://127.0.0.1:5173/", { waitUntil: "domcontentloaded" });
        await page.waitForTimeout(700);
        await page.click(`aside nav button[aria-label="${TAB_LABEL[tab]}"]`).catch(() => {});
        await page.waitForTimeout(450);
        console.log(`   [重载] ${tab}/${hint} 覆盖层无法关闭`);
      }
    };
  for (let iter = 0; iter < 26; iter++) {
    const list = await page.evaluate(() => {
      const scope = document.querySelectorAll("main button, main [role=button], main [role=combobox]");
      return Array.from(scope).map((el, i) => {
        el.setAttribute("data-vr-a", String(i));
        return { i, text: (el.getAttribute("aria-label") || el.textContent || "").trim().replace(/\s+/g, " ").slice(0, 40) };
      });
    });
    const norm = (x) => x.replace(/[\s\u4e00-\u9fa5]*[A-Za-z0-9._\-]{2,}[\s」]*$/, "").trim() || x;
    const hit = list.find((x) => x.text && TRIGGER.test(x.text) && !seen.has(norm(x.text)));
    if (!hit) break;
    seen.add(norm(hit.text));
    const loc = page.locator(`main [data-vr-a="${hit.i}"]`).first();
    if (!(await loc.isVisible().catch(() => false))) continue;
    await loc.scrollIntoViewIfNeeded().catch(() => {});
    await loc.click({ timeout: 2500 }).catch(() => {});
    await page.waitForTimeout(420);
    const dlgOpen = await page.locator('[role="dialog"],[role="alertdialog"]').count();
    await snap(`dlg:${tab}:${hit.text}`);
    if (dlgOpen) {
      // 在弹窗里尽量把内容变长（添加一行/多行文本），再量一次高度
      const addBtn = await page.evaluate(() => {
        const b = Array.from(document.querySelectorAll('[role="dialog"] button')).find((x) => /添加一行|添加 Header|新增|添加环境变量|添加模型|\+ 添加|添加/.test(x.textContent || ""));
        if (b) { b.setAttribute("data-vr-add", "1"); return (b.textContent || "").trim().slice(0, 20); }
        return null;
      });
      if (addBtn) {
        for (let k = 0; k < 4; k++) {
          await page.locator('[data-vr-add="1"]').click({ timeout: 1500 }).catch(() => {});
          await page.waitForTimeout(300);
        }
        await snap(`dlg+:${tab}:${hit.text}(+4行)`);
      }
      // 长文本灌入 textarea / input，看是否把弹窗顶出视口
      const filled = await page.evaluate(() => {
        const ta = document.querySelector('[role="dialog"] textarea');
        const big = "这是一段很长的技能正文内容，用于测试在默认 980×640 窗口下弹窗是否会超出可视区域。\n".repeat(12);
        if (ta) {
          const proto = Object.getOwnPropertyDescriptor(window.HTMLTextAreaElement.prototype, "value");
          proto?.set?.call(ta, big);
          ta.dispatchEvent(new Event("input", { bubbles: true }));
          return true;
        }
        return false;
      });
      if (filled) { await page.waitForTimeout(350); await snap(`dlg!:${tab}:${hit.text}(长文本)`); }
      // 尝试在弹窗内滚动到底，记录能否到达主按钮
      const scrollTry = await page.evaluate(() => {
        const dlg = document.querySelector('[role="dialog"],[role="alertdialog"]');
        if (!dlg) return null;
        const cands = [dlg, ...dlg.querySelectorAll("*")].filter((n) => /(auto|scroll)/.test(getComputedStyle(n).overflowY) && n.scrollHeight > n.clientHeight + 1);
        if (!cands.length) return { scrollable: false };
        cands[0].scrollTop = cands[0].scrollHeight;
        const btns = Array.from(dlg.querySelectorAll("button")).map((b) => { const r = b.getBoundingClientRect(); return { t: (b.textContent || "").trim().slice(0, 12), vis: r.top >= 0 && r.bottom <= innerHeight }; });
        return { scrollable: true, node: cands[0].className.toString().slice(0, 30), buttons: btns.filter((x) => x.t) };
      });
      if (scrollTry) out.steps.push({ label: `scrollTry:${tab}:${hit.text}`, scrollTry });
      await closeHard(hit.text);
    }
  }
    await closeHard("tab-end");
  const cur = await page.evaluate(() => Array.from(document.querySelectorAll("aside nav button")).find((x) => /bg-accent/.test(x.className))?.getAttribute("aria-label"));
  if (cur !== TAB_LABEL[tab]) { await page.click(`aside nav button[aria-label="${TAB_LABEL[tab]}"]`).catch(() => {}); await page.waitForTimeout(400); }
}

fs.writeFileSync(path.join(ROOT, `${TAG}.json`), JSON.stringify({ ...out, errors: [...new Set(errors)] }, null, 2));
console.log(`\n完成 ${idx} 步 → ${ROOT}/${TAG}.json`);
if (errors.length) console.log("page errors:", [...new Set(errors)].slice(0, 8).join(" | "));
await browser.close();
