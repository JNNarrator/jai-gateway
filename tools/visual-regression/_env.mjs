// 探针共享环境：Playwright / Chrome 的**可移植**定位 + 输出目录 + argv 解析。
//
// 为什么有这个文件：此前 15 个探针各自把 Playwright 路径写死成
//   /Users/jiangnan/Documents/workspace/deepseek-harness/node_modules/.pnpm/playwright@1.61.1/...
// 换一台机器（或 CI）就全都跑不起来，而门禁必须能在任何机器上跑才有意义。
// 现在解析顺序（第一个命中即用）：
//   1. 环境变量 PLAYWRIGHT_PATH / CHROME_PATH（显式覆盖，CI 用）
//   2. 仓库内 node_modules：ui/node_modules（playwright → playwright-core）
//   3. 都没有 → 让 Playwright 用它自带的 chromium（不传 executablePath）
// ui 的 devDependency 是 **playwright-core**：它只驱动系统 Chrome、不下载浏览器，
// 所以门禁不需要额外的浏览器安装步骤（CI 上同理）。
import fs from "node:fs";
import path from "node:path";
import { createRequire } from "node:module";
import { fileURLToPath } from "node:url";

const HERE = path.dirname(fileURLToPath(import.meta.url));
export const REPO_ROOT = path.resolve(HERE, "../..");
export const UI_DIR = path.join(REPO_ROOT, "ui");
export const VR_DIR = path.join(REPO_ROOT, ".vr");
export const SHOTS_DIR = path.join(VR_DIR, "shots");
export const DEFAULT_URL = process.env.JAI_VR_URL || "http://127.0.0.1:5173/";

fs.mkdirSync(SHOTS_DIR, { recursive: true });

/** 找到可用的 playwright 包目录；返回 null 表示只能靠 NODE_PATH / 全局解析。 */
function findPlaywrightDir() {
  const cands = [];
  if (process.env.PLAYWRIGHT_PATH) cands.push(process.env.PLAYWRIGHT_PATH);
  for (const name of ["playwright", "playwright-core"]) {
    cands.push(path.join(UI_DIR, "node_modules", name));
    cands.push(path.join(REPO_ROOT, "node_modules", name));
  }
  for (const c of cands) {
    if (fs.existsSync(path.join(c, "package.json"))) return c;
  }
  return null;
}

/** 找到可用的 Chrome/Chromium 可执行文件；返回 null 表示用 Playwright 自带 chromium。 */
function findChromePath() {
  if (process.env.CHROME_PATH) {
    if (fs.existsSync(process.env.CHROME_PATH)) return process.env.CHROME_PATH;
    throw new Error(`CHROME_PATH 指向的文件不存在: ${process.env.CHROME_PATH}`);
  }
  const cands = [
    "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome",
    "/Applications/Chromium.app/Contents/MacOS/Chromium",
    "/usr/bin/google-chrome",
    "/usr/bin/google-chrome-stable",
    "/usr/bin/chromium-browser",
    "/usr/bin/chromium",
    "C:\\Program Files\\Google\\Chrome\\Application\\chrome.exe",
  ];
  for (const c of cands) if (fs.existsSync(c)) return c;
  return null;
}

export const PLAYWRIGHT_DIR = findPlaywrightDir();
export const CHROME_PATH = findChromePath();

function loadChromium() {
  // createRequire 的基准文件决定 node_modules 向上查找的起点：
  // 传 <dir>/package.json → 起点是 <dir>，会依次尝试 <dir>/node_modules、
  // 父目录的 node_modules……因此传入「包目录」即可正确解析到该包。
  const bases = [];
  if (PLAYWRIGHT_DIR) bases.push(path.join(PLAYWRIGHT_DIR, "package.json"));
  if (process.env.PLAYWRIGHT_PATH) bases.push(path.join(process.env.PLAYWRIGHT_PATH, "package.json"));
  bases.push(path.join(UI_DIR, "package.json"));
  bases.push(path.join(REPO_ROOT, "package.json"));
  const tried = [];
  for (const b of bases) {
    for (const name of ["playwright", "playwright-core"]) {
      try {
        return createRequire(b)(name).chromium;
      } catch (e) {
        tried.push(`${b} → ${name}: ${String(e.message).slice(0, 70)}`);
      }
    }
  }
  throw new Error(
    "找不到 playwright / playwright-core。请先 `cd ui && pnpm i`，" +
      "或用 PLAYWRIGHT_PATH=<playwright 包目录> 显式指定。\n尝试过：\n  " +
      tried.join("\n  "),
  );
}

export const chromium = loadChromium();

/** 统一的浏览器启动：headless + 可移植的 executablePath。 */
export function launchBrowser(extra = {}) {
  return chromium.launch({
    ...(CHROME_PATH ? { executablePath: CHROME_PATH } : {}),
    headless: true,
    ...extra,
  });
}

/** 统一 argv 解析：--k=v / --k / 裸词。 */
export function parseArgv(argv = process.argv.slice(2)) {
  return Object.fromEntries(
    argv.map((s) => {
      const m = s.match(/^--([^=]+)(?:=(.*))?$/);
      return m ? [m[1], m[2] ?? true] : [s, true];
    }),
  );
}

/** --size=WxH → [W, H]。 */
export function parseSize(s, fallback = "1180x800") {
  const [w, h] = String(s || fallback).split("x").map(Number);
  if (!Number.isFinite(w) || !Number.isFinite(h)) throw new Error(`--size 格式应为 WxH，收到: ${s}`);
  return [w, h];
}

export const TABS = ["gateway", "sync", "mcp", "skills", "providers", "models", "stats", "logs", "settings"];
export const TAB_LABEL = {
  gateway: "网关", sync: "同步", mcp: "MCP", skills: "技能", providers: "供应商",
  models: "模型", stats: "统计", logs: "日志", settings: "设置",
};

/** 从 run.mjs 提取 installMock 源码（单一来源，避免各探针各写一份 mock 而漂移）。 */
export function installMockSrc() {
  const runSrc = fs.readFileSync(path.join(HERE, "run.mjs"), "utf8");
  const m = runSrc.match(/function installMock\(\)[\s\S]*?\n}\n/)?.[0];
  if (!m) throw new Error("未能从 run.mjs 提取 installMock");
  return m;
}

/** 建一个已装好 mock + fixtures 的页面（多数探针的开场样板）。 */
export async function newMockPage({
  vw,
  vh,
  fixtures,
  theme = null,
  url = DEFAULT_URL,
  deviceScaleFactor = 1,
  waitMs = 1000,
} = {}) {
  const browser = await launchBrowser();
  const ctx = await browser.newContext({ viewport: { width: vw, height: vh }, deviceScaleFactor, locale: "zh-CN" });
  await ctx.addInitScript({
    content: `window.__JAI_FIX__ = ${JSON.stringify(fixtures)}; window.__VR_VW__=${vw}; window.__VR_VH__=${vh};`,
  });
  await ctx.addInitScript({ content: `(${installMockSrc()})();` });
  await ctx.addInitScript({ content: `try{localStorage.setItem('jai-sidebar-collapsed','0');}catch(e){}` });
  if (theme) await ctx.addInitScript({ content: `try{localStorage.setItem('theme','${theme}');}catch(e){}` });
  const page = await ctx.newPage();
  const errors = [];
  page.on("console", (m) => { if (m.type() === "error") errors.push("console: " + m.text().slice(0, 200)); });
  page.on("pageerror", (e) => errors.push("pageerror: " + String(e.message).slice(0, 200)));
  await page.goto(url, { waitUntil: "domcontentloaded" });
  await page.waitForTimeout(waitMs);
  if (theme) {
    await page.evaluate((t) => {
      document.documentElement.classList.toggle("dark", t === "dark");
      try { localStorage.setItem("theme", t); } catch (e) {}
    }, theme);
    await page.waitForTimeout(300);
  }
  return { browser, ctx, page, errors };
}
