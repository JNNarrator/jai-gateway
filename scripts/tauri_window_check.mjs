// 最小窗口尺寸门禁（零依赖，只读配置文本）。
//
// 存在意义（这是一个**真实发生过的静默失效**，不是假想）：
//   `tauri.macos.conf.json` 用 `app.windows` 覆盖窗口配置，而 Tauri 的平台配置合并
//   走的是 **JSON Merge Patch (RFC 7396)** —— 数组是**整体替换**，不是按下标逐字段合并。
//   于是基础配置里的 `label/title/width/height/minWidth/minHeight` 在 macOS 上被**全部丢弃**：
//     · 窗口退回 Tauri 默认 800×600（而非设计值 1180×800）
//     · **`minWidth/minHeight` 变成 None → macOS 上完全没有最小尺寸限制**
//       （用户可以把窗口拖到极小，UI 直接错乱）
//   实测（`tauri_utils::config::parse::read_from(Target::MacOS)`，改动前）：
//     app.windows = [{ decorations, hiddenTitle, titleBarStyle, transparent, windowEffects }]
//   而 900×600 这个下限本身是**实测出来的**（见下），偏偏在主力平台没生效。
//
// 两条判据：
//   ① **键集完整性**：平台配置里的窗口对象必须重新声明基础配置里的**每一个**键。
//      数组整体替换 ⇒ 平台文件必须自带完整窗口规格，漏一个就静默丢一个。
//      （值可以不同：decorations / windowEffects 正是按平台故意不同的。）
//   ② **下限不低于 UI 验收尺寸**：解析后的 minWidth/minHeight 必须 ≥ `gate.mjs` 的
//      最小验收尺寸。**单一来源**：这里不硬编码 900×600，而是从 gate.mjs 读它的
//      `--sizes` 默认值取最小的一组 —— 改验收尺寸只需改 gate.mjs 一处。
//
// 用法：node scripts/tauri_window_check.mjs
// 退出码：0 = 全过；1 = 有违规
import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const HERE = path.dirname(fileURLToPath(import.meta.url));
const REPO = path.join(HERE, "..");
const TAURI_DIR = path.join(REPO, "src-tauri");

const violations = [];
const fail = (what, detail) => violations.push({ what, detail });

/** RFC 7396 JSON Merge Patch —— 与 Tauri 侧 `json_patch::merge` 语义一致。
 *  **关键**：patch 不是对象（含数组）时整体替换，这正是本门禁要防的坑。 */
function mergePatch(target, patch) {
  if (patch === null || typeof patch !== "object" || Array.isArray(patch)) {
    return structuredClone(patch);
  }
  const out =
    target && typeof target === "object" && !Array.isArray(target) ? structuredClone(target) : {};
  for (const [k, v] of Object.entries(patch)) {
    if (v === null) delete out[k];
    else out[k] = mergePatch(out[k], v);
  }
  return out;
}

const readJson = (p) => JSON.parse(fs.readFileSync(p, "utf8"));

/** gate.mjs 的最小验收尺寸（单一来源，解析失败即报错——不允许静默变绿）。 */
function acceptanceMinSize() {
  const src = fs.readFileSync(path.join(REPO, "tools", "visual-regression", "gate.mjs"), "utf8");
  const m = src.match(/SIZES\s*=\s*String\(argv\.sizes\s*\|\|\s*"([^"]+)"\)/);
  if (!m) {
    throw new Error(
      "未能从 tools/visual-regression/gate.mjs 解析出默认 --sizes（正则失配）。" +
        "判据必须能读到验收尺寸，否则本门禁会静默失效。",
    );
  }
  const sizes = m[1].split(",").map((s) => s.trim().split("x").map(Number));
  return sizes.reduce((a, b) => [Math.min(a[0], b[0]), Math.min(a[1], b[1])]);
}

const BASE = path.join(TAURI_DIR, "tauri.conf.json");
const PLATFORMS = [
  ["macos", path.join(TAURI_DIR, "tauri.macos.conf.json")],
  ["windows", path.join(TAURI_DIR, "tauri.windows.conf.json")],
  ["linux", path.join(TAURI_DIR, "tauri.linux.conf.json")],
];

if (!fs.existsSync(BASE)) {
  console.error(`✗ 找不到 ${path.relative(REPO, BASE)}`);
  process.exit(1);
}

const base = readJson(BASE);
const baseWin = base?.app?.windows?.[0];
if (!baseWin) {
  fail("基础配置有窗口定义", "src-tauri/tauri.conf.json 的 app.windows[0] 不存在，无法校验");
}

let min = null;
try {
  min = acceptanceMinSize();
} catch (e) {
  fail("能读到 UI 验收尺寸", String(e.message));
}

// 判据 ①：平台配置必须重新声明基础配置的每个窗口键
// 判据 ②：解析后的最小尺寸不得低于 UI 验收尺寸
const targets = [["base", BASE, true], ...PLATFORMS.map(([n, p]) => [n, p, false])];
for (const [name, file, isBase] of targets) {
  if (!isBase && !fs.existsSync(file)) continue;
  const resolved = isBase ? base : mergePatch(base, readJson(file));
  const win = resolved?.app?.windows?.[0];
  const label = `src-tauri/${path.basename(file)}`;

  if (!win) {
    fail(`${name} 有窗口定义`, `${label} 解析后 app.windows[0] 不存在`);
    continue;
  }

  if (!isBase && baseWin) {
    const platformWin = readJson(file)?.app?.windows?.[0] ?? {};
    // 括号必须显式：`!(k in platformWin || {})` 因优先级恒为 false → 判据静默失效。
    const missing = Object.keys(baseWin).filter((k) => !(k in platformWin));
    if (missing.length) {
      fail(
        `${name} 窗口规格完整（RFC 7396 数组整体替换）`,
        `${label} 的 app.windows[0] 漏了基础配置里的 ${missing.join(", ")} —— ` +
          `平台配置合并时数组是整体替换，这些键会在 ${name} 上被静默丢弃` +
          `（若含 minWidth/minHeight，该平台就**没有最小尺寸限制**）`,
      );
    }
  }

  if (min) {
    const [mw, mh] = min;
    const { minWidth, minHeight } = win;
    if (typeof minWidth !== "number" || typeof minHeight !== "number") {
      fail(
        `${name} 声明了最小尺寸`,
        `${label} 解析后 minWidth=${minWidth} minHeight=${minHeight}（应为数字，` +
          `缺失即该平台无最小尺寸限制，窗口可被拖到 UI 错乱）`,
      );
    } else if (minWidth < mw || minHeight < mh) {
      fail(
        `${name} 最小尺寸 ≥ UI 验收尺寸`,
        `${label} minWidth/minHeight = ${minWidth}×${minHeight} < 验收下限 ${mw}×${mh}`,
      );
    }
  }
}

if (violations.length) {
  console.error("✗ 最小窗口尺寸门禁不通过：\n");
  for (const v of violations) console.error(`  · [${v.what}]\n    ${v.detail}`);
  console.error("");
  process.exit(1);
}

console.log(
  `✓ 最小窗口尺寸门禁通过（macOS / Windows / Linux 解析后均为 ` +
    `${baseWin.minWidth}×${baseWin.minHeight}，默认 ${baseWin.width}×${baseWin.height}；` +
    `UI 验收下限 ${min[0]}×${min[1]}）`,
);
