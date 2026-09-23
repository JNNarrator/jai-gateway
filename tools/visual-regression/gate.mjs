// UI 门禁（单一退出码）—— 把「双尺寸 × 双主题」的 UI 规范验收收成一条命令。
//
//   node tools/visual-regression/gate.mjs
//   node tools/visual-regression/gate.mjs --sizes=1180x800,900x600 --themes=light,dark
//
// 它跑四个已有探针（都读仓库内源码，输出到 .vr/）：
//   audit.mjs      对比度 / 字号 / 截断 / 横向溢出 / 弹窗几何 / 弹窗与 toast 带冲突
//   fold.mjs       折叠线：主操作首屏可达、横向溢出、行内控件被切
//   probe-hits.mjs 有效命中区（命中区判据的唯一归属）
//   以及 scripts/ui_lint.sh（静态规范：字号/可访问名/key/命中区手法）
//
// 判据集中写在**本文件**里（不散落到各探针），便于一处审阅、一处修改。
// 任何一条不满足 → 退出码 1，并打印「哪一条、哪个尺寸/主题、具体是什么」。
//
// 依赖：vite dev server 已在 127.0.0.1:5173 上跑（`cd ui && npx vite`）。
import fs from "node:fs";
import path from "node:path";
import { spawnSync } from "node:child_process";
import { fileURLToPath } from "node:url";

const HERE = path.dirname(fileURLToPath(import.meta.url));
const REPO = path.resolve(HERE, "../..");
const VR = path.join(REPO, ".vr");
fs.mkdirSync(VR, { recursive: true });
// 探针会被传 TMPDIR=<VR>/tmp（Playwright 在它下面 mkdtemp）。**这里必须自己建好**，
// 否则全新克隆 / CI 上第一次跑就 `ENOENT: mkdtemp '…/.vr/tmp/playwright-artifacts-…'` 崩掉。
fs.mkdirSync(path.join(VR, "tmp"), { recursive: true });

const argv = Object.fromEntries(
  process.argv.slice(2).map((s) => {
    const m = s.match(/^--([^=]+)(?:=(.*))?$/);
    return m ? [m[1], m[2] ?? true] : [s, true];
  }),
);
const SIZES = String(argv.sizes || "1180x800,900x600").split(",").map((s) => s.trim());
const THEMES = String(argv.themes || "light,dark").split(",").map((s) => s.trim());
const SKIP_PROBES = !!argv["skip-probes"];

/** toast 带的固定几何（sonner bottom-right：宽 356、高 54、距边 24） */
const TOAST_W = 356;
const TOAST_H = 54;
const TOAST_OFFSET = 24;

const failures = [];
const notes = [];
const fail = (criterion, where, detail) => failures.push({ criterion, where, detail });

const run = (label, cmd, args, opts = {}) => {
  process.stdout.write(`\n── ${label}\n`);
  const r = spawnSync(cmd, args, { cwd: REPO, encoding: "utf8", env: { ...process.env, TMPDIR: path.join(VR, "tmp") }, ...opts });
  const out = (r.stdout || "") + (r.stderr || "");
  process.stdout.write(out.split("\n").slice(0, 200).join("\n") + "\n");
  return { code: r.status, out };
};
const readJson = (p) => {
  if (!fs.existsSync(p)) return null;
  try { return JSON.parse(fs.readFileSync(p, "utf8")); } catch { return null; }
};
/** 跑一个探针：**先删掉它的输出文件**，跑完若退出码非 0 直接判失败。
 *  早期版本只 readJson → 探针崩溃时会读到**上一次**的旧 JSON 而静默变绿
 *  （fold.mjs 因少了一个变量而崩溃时，门禁就是这样假绿的）。 */
const runProbe = (label, file, outName) => {
  const outPath = path.join(VR, outName);
  fs.rmSync(outPath, { force: true });
  const r = run(label, "node", [path.posix.join("tools/visual-regression", file), ...label.split(" ").slice(1)]);
  if (r.code !== 0) fail("探针可运行", label, `退出码 ${r.code}（读旧结果会假绿，故直接判失败）：${r.out.split("\n").filter(Boolean).slice(-3).join(" | ").slice(0, 300)}`);
  return readJson(outPath);
};

// ─────────────────────── 1. 静态规范（零依赖） ───────────────────────
const lint = run("scripts/ui_lint.sh（静态 UI 规范）", "bash", ["scripts/ui_lint.sh"]);
if (lint.code !== 0) fail("静态规范 ui_lint", "全仓库", "见上方违规清单");

// 最小窗口尺寸门禁（零依赖）：平台配置合并（RFC 7396，数组整体替换）会把基础配置里的
// `minWidth/minHeight` 静默丢掉 —— 曾经 macOS 上**完全没有最小尺寸限制**，
// 而 900×600 这个下限本身是实测出来的（890 宽起出现静默截断）。
// 判据的验收尺寸单一来源就是本文件的 SIZES，故放在这里而不是另立一处。
const winCfg = run("scripts/tauri_window_check.mjs（最小窗口尺寸）", "node", [
  "scripts/tauri_window_check.mjs",
]);
if (winCfg.code !== 0) {
  fail("最小窗口尺寸在各平台都生效", "src-tauri/*.conf.json", winCfg.out.split("\n").filter(Boolean).slice(-6).join("\n"));
}

// ─────────────────────── 2. 逐尺寸的探针 ───────────────────────
const hitResults = {};
for (const size of SIZES) {
  const hits = SKIP_PROBES
    ? readJson(path.join(VR, `probe-hits-${size}.json`))
    : runProbe(`probe-hits --size=${size}`, "probe-hits.mjs", `probe-hits-${size}.json`);
  if (!hits) {
    fail("命中区 probe-hits", size, "未产出结果文件（探针未跑或崩溃）");
    continue;
  }
  hitResults[size] = hits;
  if (hits.fails?.length) {
    for (const c of hits.fails.slice(0, 10)) {
      fail("命中区 ≥24×24", `${size} ${c.where}`, `rect ${c.rect.w}×${c.rect.h} → 有效 ${c.effective.w}×${c.effective.h}「${c.name}」`);
    }
  }
  if (hits.missingKinds?.length) {
    fail("命中区探针覆盖完整", size, `以下控件类型未被扫描到（探针可能已静默失效）：${hits.missingKinds.join(", ")}`);
  }
  if (hits.errors?.length) {
    fail("无控制台报错", `${size} probe-hits`, hits.errors.slice(0, 3).join(" | "));
  }

  // ── fold：折叠线 / 横向溢出 / 被切行内控件 / 弹窗可关闭性 ──
  const fold = SKIP_PROBES
    ? readJson(path.join(VR, `fold-${size}.json`))
    : runProbe(`fold --size=${size}`, "fold.mjs", `fold-${size}.json`);
  if (!fold) {
    fail("折叠线 fold", size, "未产出结果文件");
  } else {
    for (const [key, f] of Object.entries(fold)) {
      if (key.startsWith("modal:")) {
        if (!f.escClosed) fail("弹窗 Esc 可关闭", `${size} ${key}`, "Esc 关不掉");
        if (f.navAfter !== "设置") fail("弹窗关闭后仍可导航", `${size} ${key}`, `关闭后导航失败（停在「${f.navAfter}」）`);
        const over = f.opened && (f.opened.t < -1 || f.opened.b > f.opened.vh + 1);
        if (over) fail("弹窗不越出视口", `${size} ${key}`, `top=${f.opened.t} bottom=${f.opened.b} vh=${f.opened.vh}`);
        continue;
      }
      if (f.primaryActionsBelowFold?.length) {
        fail("主操作首屏可达", `${size} ${key}`, f.primaryActionsBelowFold.map((a) => `「${a.n}」@${a.top}`).join(" ; "));
      }
      if (f.primaryActionCount === 0) {
        fail("主操作已被标记", `${size} ${key}`, "本页没有 data-slot=page-header/page-actions 标记的主操作（选择器契约失效）");
      }
      if (f.pageHScroll > 1 || f.mainHScroll > 1) {
        fail("无横向溢出", `${size} ${key}`, `html+${f.pageHScroll} main+${f.mainHScroll}`);
      }
      // 折叠线判据（**2026-09-21 修订，原判据不合理**）：
      // 原文是「partialCut 里不得出现 input/button/textarea/select」，但长页面在固定视口下
      // **必然**有某个控件跨在视口底边上（设置页内容 1848px / 可视 564px，模型页 1259/564）。
      // 而且「底边露出半个控件」本身是**正常的滚动提示**（告诉用户下面还有内容），
      // 不是缺陷 —— 原判据会稳定误报，且逼人去改正常布局（这正是「为绿而改」的反面）。
      // 真正要保证的是：① **主操作**必须完整可见（不只是「不在折叠线以下」，
      // 而是不能跨在折叠线上）；② 跨线的控件必须能靠滚动完整看到（由 run.mjs 的
      // `unreachable` 覆盖）；③ 表格不得溢出容器（P1-5 的实质，单独一条判据）。
      // partialCut 仍逐页打印为**信息**（见下方 notes），只是不再当失败。
      const cutPrimary = (f.primaryActions || []).filter((a) => a.straddles);
      if (cutPrimary.length) {
        fail("主操作不被折叠线切半", `${size} ${key}`, cutPrimary.map((a) => `「${a.n}」top=${a.top} bottom=${a.bottom}`).join(" ; "));
      }
      // 吸顶表头**实测**（声明 sticky ≠ 真的吸顶；日志页就曾假绿很久）
      for (const h of f.stickyHeads || []) {
        if (h.sticky && h.scrollable && h.works === false) {
          fail("吸顶表头真的吸顶", `${size} ${key}`, `滚动 ${h.scrolledBy}px 后表头位移 ${h.moved}px（sticky 失效）`);
        }
      }
      const cutCtrls = (f.partialCut || []).filter((c) => ["input", "button", "textarea", "select"].includes(c.tag));
      if (cutCtrls.length) {
        notes.push(`[信息·非失败] ${size} ${key}：${cutCtrls.length} 个控件跨在折叠线上（可滚动到达，属正常滚动提示）：` + cutCtrls.slice(0, 4).map((c) => `${c.tag}「${c.n}」切${c.cut}px`).join(" ; "));
      }
      if ((f.tables || []).some((t) => t.overflowX > 1)) {
        fail("表格不溢出容器", `${size} ${key}`, (f.tables || []).filter((t) => t.overflowX > 1).map((t) => `${t.cols}列 宽${t.tableW}/容器${t.mainW} 溢出${t.overflowX}`).join(" ; "));
      }
    }
  }

  // ── audit：对比度 / 字号 / 截断 / 溢出 / 弹窗几何 / toast 带冲突 ──
  for (const theme of THEMES) {
    const audit = SKIP_PROBES
      ? readJson(path.join(VR, `audit-${theme}-${size}.json`))
      : runProbe(`audit --size=${size} --theme=${theme}`, "audit.mjs", `audit-${theme}-${size}.json`);
    if (!audit) {
      fail("视觉审计 audit", `${size}/${theme}`, "未产出结果文件");
      continue;
    }
    const contrast = [], tiny = [], trunc = [], hscroll = [], dlgBad = [], toastBand = [];
    for (const st of audit.steps || []) {
      const e = st.ext;
      if (!e || typeof e !== "object") continue;
      for (const c of e.contrast || []) contrast.push(`${st.label}: "${c.text}" ${c.worst}<${c.need}`);
      for (const c of e.tiny || []) tiny.push(`${st.label}: ${c.size}px "${c.text}"`);
      for (const c of e.truncated || []) trunc.push(`${st.label}: "${c.text}" 截${c.cut}px`);
      for (const c of e.hScroll || []) hscroll.push(`${st.label}: ${c.path} +${c.extra}`);
      // toast 带 = 右下角固定占位（用 toaster 自身几何，即使当前没有 toast 也成立）
      const toaster = (e.toasts || []).find((t) => t.kind === "toaster");
      const band = toaster && toaster.rect?.w
        ? { x: toaster.rect.x, w: toaster.rect.w, y: (audit.vh ?? toaster.rect.y) - TOAST_OFFSET - TOAST_H, h: TOAST_H }
        : { x: (audit.vw ?? 0) - TOAST_OFFSET - TOAST_W, w: TOAST_W, y: (audit.vh ?? 0) - TOAST_OFFSET - TOAST_H, h: TOAST_H };
      for (const d of e.dialogs || []) {
        if (d.overTop > 1 || d.overBottom > 1) dlgBad.push(`${st.label}「${d.title}」越界 上${d.overTop}/下${d.overBottom}`);
        if (d.unreachable > 0) dlgBad.push(`${st.label}「${d.title}」有 ${d.unreachable} 个控件滚不到：${(d.unreachableNames || []).join(",")}`);
        for (const b of d.primaryButtons || []) {
          if (b.x == null) continue;
          const ox = Math.max(0, Math.min(b.x + b.w, band.x + band.w) - Math.max(b.x, band.x));
          const oy = Math.max(0, Math.min(b.bottom, band.y + band.h) - Math.max(b.top, band.y));
          if (ox > 0 && oy > 0) {
            toastBand.push(`${st.label}「${d.title}」按钮「${b.text}」与 toast 带重叠 ${ox}×${oy}px²`);
          }
        }
      }
    }
    if (contrast.length) fail("对比度 AA", `${size}/${theme}`, `${contrast.length} 处，最差：${contrast.slice(0, 4).join(" ; ")}`);
    if (tiny.length) fail("字号 ≥11px", `${size}/${theme}`, `${tiny.length} 处：${tiny.slice(0, 4).join(" ; ")}`);
    if (trunc.length) fail("截断有 title 兜底", `${size}/${theme}`, `${trunc.length} 处：${trunc.slice(0, 4).join(" ; ")}`);
    if (hscroll.length) fail("无横向溢出", `${size}/${theme}`, `${hscroll.length} 处：${hscroll.slice(0, 4).join(" ; ")}`);
    if (dlgBad.length) fail("弹窗几何自洽", `${size}/${theme}`, `${dlgBad.length} 处：${dlgBad.slice(0, 4).join(" ; ")}`);
    if (toastBand.length) fail("弹窗 footer 不与 toast 带重叠", `${size}/${theme}`, toastBand.slice(0, 6).join(" ; "));
    if (audit.errors?.length) fail("无页面报错", `${size}/${theme} audit`, audit.errors.slice(0, 3).join(" | "));
  }

  // ── probe-endpoint：渠道草稿端点探测面板（D9-T1） ──
  // 探测面板的结论行是**点击后**才出现的，audit/fold 都不会点到它，所以单独一条探针。
  const pe = SKIP_PROBES
    ? readJson(path.join(VR, `out-probe-endpoint-${size}.json`))
    : runProbe(`probe-endpoint --size=${size}`, "probe-endpoint.mjs", `out-probe-endpoint-${size}.json`);
  if (!pe) {
    fail("端点探测面板 probe-endpoint", size, "未产出结果文件（探针未跑或崩溃）");
  } else {
    const where = `${size} 添加供应商`;
    if (!pe.before?.hasPanel) fail("端点探测面板存在", where, "弹窗里没有「端点探测」区块");
    if (pe.before?.hasRows) fail("端点探测初始无结论", where, "还没探测就渲染出了结论行（可能是旧结果没清）");
    const rows = pe.first?.rows || [];
    if (rows.length < 2) {
      fail("端点探测逐端点出结论", where, `只渲染出 ${rows.length} 行（chat/completions + 信息性 responses 应各一行）`);
    }
    for (const r of rows) {
      if (!r.endpoint || !r.status || !r.hasIcon) {
        fail("端点探测行内容完整", where, `行缺端点名/状态/图标：${JSON.stringify(r)}`);
      }
    }
    // 信息性行必须**看得出**是灰的（opacity 区分，不只是文字颜色）
    if (!(pe.first?.infoOpacity < pe.first?.mainOpacity)) {
      fail("信息性探测行灰显", where, `main=${pe.first?.mainOpacity} info=${pe.first?.infoOpacity}（信息性行必须比主探测行淡）`);
    }
    // 被截断的摘要必须自带 title（UI 门禁「截断有 title 兜底」是逐元素判的）
    for (const r of rows.concat(pe.blocked?.rows || [])) {
      if (r.clipped && !r.msgTitle) {
        fail("探测摘要截断有 title", where, `被截断且无 title：「${r.text}」`);
      }
    }
    if (!pe.first?.billedHint) fail("探测提示可能计费", where, "没有「可能产生计费」提示（探测真的会调用模型）");
    const blocked = pe.blocked?.rows?.[0];
    if (!blocked) {
      fail("被拦截地址有结论行", where, "SSRF 拦截没有渲染出任何行");
    } else {
      if (blocked.status !== "地址被拦截") fail("被拦截地址文案", where, `状态显示为「${blocked.status}」`);
      if (blocked.latency !== "—") fail("被拦截地址零延迟", where, `延迟显示为「${blocked.latency}」（一个请求都没发，不该有耗时）`);
    }
    // 探测不改变表单：离开弹窗时不该弹「有未保存的改动」
    if (pe.navAway?.alertOpen) {
      fail("探测不改表单脏状态", where, "探测后离开触发了「有未保存的改动」拦截");
    }
  }
}

// ─────────────────────── 汇总 ───────────────────────
const byCriterion = {};
for (const f of failures) (byCriterion[f.criterion] ||= []).push(f);

console.log("\n" + "═".repeat(72));
console.log(`UI 门禁：尺寸 ${SIZES.join(" / ")}，主题 ${THEMES.join(" / ")}`);
console.log("═".repeat(72));
for (const [crit, list] of Object.entries(byCriterion)) {
  console.log(`\n✗ ${crit}  —— ${list.length} 处`);
  for (const f of list.slice(0, 12)) console.log(`   [${f.where}] ${f.detail}`);
  if (list.length > 12) console.log(`   …另有 ${list.length - 12} 处`);
}
for (const n of notes) console.log(`· ${n}`);
if (!failures.length) {
  console.log("\n✓ 全部通过");
} else {
  console.log(`\n✗ 不通过：${failures.length} 处，涉及 ${Object.keys(byCriterion).length} 条判据`);
}
process.exit(failures.length ? 1 : 0);
