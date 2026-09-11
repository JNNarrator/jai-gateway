#!/usr/bin/env node
/**
 * verify-borrow-report.mjs —— 《FreeLLMAPI → JAI 借鉴报告》的机械判据。
 *
 * 设计原则：报告里每一句「JAI 有/没有 X」都必须能被机器复核。
 *   1. --structure       报告存在、篇幅达标、三节齐全、P0/P1/P2 齐全、含表格
 *   2. --check-citations 报告引用的 `路径:行号` 必须真实存在（文件在、行号在范围内）
 *   3. --check-urls      报告引用的 FreeLLMAPI 文档 URL 必须格式合法且线上可达
 *   4. --lint            禁止空话；P0/P1/P2 每组至少 2 条实质条目
 *   5. --self-test       校验器自身能被证伪（好样例过 / 坏样例挂）
 *
 * 不带参数 = 全跑。退出码 0=全绿，1=有红。
 */
import fs from 'node:fs';
import path from 'node:path';
import os from 'node:os';
import { fileURLToPath } from 'node:url';

const REPO = '/Users/jiangnan/Documents/workspace/JAI';
const DEFAULT_REPORT = path.join(REPO, 'docs/design/freellmapi-借鉴报告.md');
const URL_PREFIX = 'https://github.com/tashfeenahmed/freellmapi';

const REQUIRED_SECTIONS = ['机制对照', '值得借鉴', '不适用'];
const VAGUE_PHRASES = ['建议加强', '可以考虑优化', '有待完善', '持续优化', '值得关注一下', '进一步加强'];
const MIN_CHARS = 3000;
const MIN_UNIQUE_CITATIONS = 10;

const CITATION_RE =
  /\b((?:crates|ui|src-tauri|docs|scripts)\/[A-Za-z0-9._\-/]+?\.(?:rs|ts|tsx|sql|md|json|toml|mjs)|README\.md):(\d+)(?:-(\d+))?/g;
const URL_RE = new RegExp(
  URL_PREFIX.replace(/[.*+?^${}()|[\]\\]/g, '\\$&') + '[A-Za-z0-9._\\-/#]*',
  'g',
);

const lineCountCache = new Map();
function lineCount(abs) {
  if (!lineCountCache.has(abs)) lineCountCache.set(abs, fs.readFileSync(abs, 'utf8').split('\n').length);
  return lineCountCache.get(abs);
}

// ---------- 各项检查：输入文本，返回 {ok, notes[], count?} ----------

export function checkStructure(text) {
  const notes = [];
  if (text.length < MIN_CHARS) notes.push(`篇幅 ${text.length} 字 < 下限 ${MIN_CHARS}`);
  for (const s of REQUIRED_SECTIONS) if (!text.includes(s)) notes.push(`缺章节关键词「${s}」`);
  for (const p of ['P0', 'P1', 'P2']) if (!text.includes(p)) notes.push(`缺优先级标记 ${p}`);
  const hasTable =
    text.split('\n').some((l) => /^\s*\|.*\|\s*$/.test(l)) && /^\s*\|[\s:|-]+\|\s*$/m.test(text);
  if (!hasTable) notes.push('缺 Markdown 表格（机制对照表）');
  return { ok: notes.length === 0, notes };
}

export function checkCitations(text, repo = REPO) {
  const notes = [];
  const seen = new Set();
  let count = 0;
  for (const m of text.matchAll(CITATION_RE)) {
    const [, rel, startS, endS] = m;
    const key = `${rel}:${startS}${endS ? '-' + endS : ''}`;
    if (seen.has(key)) continue;
    seen.add(key);
    count++;
    const abs = path.join(repo, rel);
    if (!fs.existsSync(abs)) {
      notes.push(`引用文件不存在：${key}`);
      continue;
    }
    const total = lineCount(abs);
    const start = Number(startS);
    const end = endS ? Number(endS) : start;
    if (start < 1 || start > total) notes.push(`行号越界：${key}（文件仅 ${total} 行）`);
    else if (end > total) notes.push(`行号区间越界：${key}（文件仅 ${total} 行）`);
    else if (end < start) notes.push(`行号区间倒置：${key}`);
  }
  if (count < MIN_UNIQUE_CITATIONS)
    notes.push(`JAI 引用仅 ${count} 条 < 下限 ${MIN_UNIQUE_CITATIONS}（证据密度不足）`);
  return { ok: notes.length === 0, notes, count };
}

export async function checkUrls(text, { live = true, fetchImpl = globalThis.fetch } = {}) {
  const notes = [];
  const urls = [...new Set(text.matchAll(URL_RE).map((m) => m[0].replace(/[.,)】。]+$/, '')))];
  if (urls.length < 3) notes.push(`FreeLLMAPI 文档 URL 仅 ${urls.length} 条 < 下限 3`);
  if (!/github\.com\/tashfeenahmed\/freellmapi/.test(text)) notes.push('缺 FreeLLMAPI 仓库链接');
  if (!live) return { ok: notes.length === 0, notes, urls };
  for (const u of urls) {
    let status = null;
    for (let attempt = 0; attempt < 2 && status === null; attempt++) {
      const ac = new AbortController();
      const t = setTimeout(() => ac.abort(), 20000);
      try {
        const res = await fetchImpl(u, { method: 'GET', signal: ac.signal, redirect: 'follow' });
        status = res.status;
      } catch {
        status = null;
      } finally {
        clearTimeout(t);
      }
    }
    if (status === null) notes.push(`URL 不可达（两次尝试）：${u}`);
    else if (status >= 400) notes.push(`URL 返回 ${status}：${u}`);
  }
  return { ok: notes.length === 0, notes, urls };
}

export function checkLint(text) {
  const notes = [];
  for (const v of VAGUE_PHRASES) if (text.includes(v)) notes.push(`空话命中：「${v}」`);
  for (const p of ['P0', 'P1', 'P2']) {
    const re = new RegExp(`^#{2,4}\\s*${p}\\b[\\s\\S]*?(?=^#{1,4}\\s*P[0-9]|$(?![\\s\\S]))`, 'm');
    const block = text.match(re);
    if (!block) {
      notes.push(`${p} 无独立小节`);
      continue;
    }
    const items = block[0]
      .split('\n')
      .filter((l) => /^\s*([-*]|\d+\.|\|)/.test(l) && l.trim().length > 8);
    if (items.length < 2) notes.push(`${p} 小节实质条目仅 ${items.length} 条 < 2`);
  }
  return { ok: notes.length === 0, notes };
}

/**
 * 校验仓内相对链接目标能否解析。
 * 报告位于 docs/design/，因此指向仓库文件的链接必须带 ../../ 前缀；
 * 裸写 `crates/...` 会让读者点开是 404（本检查即为此 bug 的回归闸）。
 */
export function checkLinks(text, reportDir = path.dirname(DEFAULT_REPORT)) {
  const notes = [];
  const seen = new Set();
  let count = 0;
  for (const m of text.matchAll(/\[[^\]]*\]\(([^)\s]+)\)/g)) {
    const target = m[1];
    if (/^https?:\/\//.test(target) || target.startsWith('#') || target.startsWith('mailto:')) continue;
    const clean = target.split('#')[0];
    if (!clean || seen.has(clean)) continue;
    seen.add(clean);
    count++;
    const abs = path.resolve(reportDir, clean);
    if (!fs.existsSync(abs)) notes.push(`链接目标不可解析：${clean} → 期望 ${abs}`);
  }
  if (count < 3) notes.push(`仓内相对链接仅 ${count} 条 < 下限 3（引用应当可点开）`);
  return { ok: notes.length === 0, notes, count };
}

// ---------- 校验器自证伪 ----------

const GOOD_FIXTURE = `# 借鉴报告
## 机制对照表
| 维度 | FreeLLMAPI | JAI |
| --- | --- | --- |
| 路由 | bandit | priority 见 crates/gateway-core/src/router/mod.rs:18 |
| 存储 | AES | 明文见 README.md:84 |
| 限速 | 四窗账本 | 仅鉴权限速 crates/gateway-core/src/server/ratelimit.rs:21 |
| 发现 | 签名目录 | 手动拉取 crates/gateway-core/src/discover.rs:99 |
| 日志 | p50/p95 | 仅总时长 ui/src/types.ts:80 |
## 值得借鉴（P0/P1/P2）
### P0 冷却记忆
- 落点 crates/gateway-core/src/router/mod.rs:113 —— 429 只换下游渠道不退避
- 落点 crates/gateway-core/src/store/mod.rs:471 —— 候选查询无冷却字段
### P1 观测
- 落点 crates/gateway-core/src/store/logs.rs:34 —— 只有总时长无首字节
- 落点 crates/gateway-core/src/server/proxy.rs:1 —— 失败归因单薄
### P2 端点
- 落点 crates/gateway-core/src/modality.rs:1 —— 模态集合已就位
- 落点 crates/gateway-core/src/codec/gemini.rs:1 —— 出站有入站无
## 不适用
- 目录 feed 聚合：https://github.com/tashfeenahmed/freellmapi
- 社区先验：https://github.com/tashfeenahmed/freellmapi/blob/main/README.md
- 许可条款：https://github.com/tashfeenahmed/freellmapi/blob/main/LICENSE
${'填充内容以达标篇幅。'.repeat(300)}`;

const BAD_CITATION_FIXTURE = GOOD_FIXTURE.replace(
  'crates/gateway-core/src/router/mod.rs:18',
  'crates/gateway-core/src/router/mod.rs:999999',
);

const NO_P0_FIXTURE = '# x\n## 机制对照\n## 值得借鉴\n## 不适用\n## P1 标题\n- 条目一条\n- 条目两条\n';

export async function selfTest({ quiet = false } = {}) {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), 'vbr-selftest-'));
  fs.writeFileSync(path.join(dir, 'good.md'), GOOD_FIXTURE);
  fs.writeFileSync(path.join(dir, 'bad.md'), BAD_CITATION_FIXTURE);
  const checks = [];
  const push = (name, pass) => checks.push({ name, pass });

  push('好样例·structure 通过', checkStructure(GOOD_FIXTURE).ok);
  push('好样例·citations 通过', checkCitations(GOOD_FIXTURE).ok);
  push('好样例·lint 通过', checkLint(GOOD_FIXTURE).ok);
  push('好样例·urls 格式通过（离线）', (await checkUrls(GOOD_FIXTURE, { live: false })).ok);
  push('坏样例·越界行号被拒', !checkCitations(BAD_CITATION_FIXTURE).ok);
  push('坏样例·缺 P0 小节被拒', !checkLint(NO_P0_FIXTURE).ok);
  push('坏样例·空话被拒', !checkLint(GOOD_FIXTURE + '\n建议加强路由。').ok);
  push('坏样例·缺章节被拒', !checkStructure('# 短\n').ok);
  push('坏样例·虚构文件被拒', !checkCitations('- 见 crates/gateway-core/src/not-real-file.rs:1').ok);

  const LINK_FIXTURE =
    '[a](crates/gateway-core/src/router/mod.rs) [b](crates/gateway-core/src/store/mod.rs) [c](README.md)';
  push('好样例·links 从仓库根解析通过', checkLinks(LINK_FIXTURE, REPO).ok);
  push('坏样例·死链被拒', !checkLinks(LINK_FIXTURE + ' [d](crates/gateway-core/src/nope.rs)', REPO).ok);
  push(
    '坏样例·缺 ../../ 前缀被拒（本次真实 bug 的回归闸）',
    !checkLinks(LINK_FIXTURE, path.join(REPO, 'docs/design')).ok,
  );

  fs.rmSync(dir, { recursive: true, force: true });
  const failed = checks.filter((c) => !c.pass);
  if (!quiet) {
    for (const c of checks) console.log(`${c.pass ? '✅' : '❌'} ${c.name}`);
    console.log(
      failed.length
        ? `\nself-test 红 ${failed.length} 项：校验器不可信`
        : `\nself-test 全绿 ${checks.length}/${checks.length}：校验器可证伪`,
    );
  }
  return { ok: failed.length === 0, total: checks.length, red: failed.length, checks };
}

// ---------- main ----------

async function main() {
  const args = process.argv.slice(2);
  if (args.includes('--self-test')) process.exit((await selfTest()).ok ? 0 : 1);

  const reportPath = process.env.VBR_REPORT || DEFAULT_REPORT;
  if (!fs.existsSync(reportPath)) {
    console.error(`❌ 报告不存在：${reportPath}`);
    process.exit(1);
  }
  const text = fs.readFileSync(reportPath, 'utf8');
  const all = args.length === 0;
  const results = [];
  if (all || args.includes('--structure')) results.push(['structure', checkStructure(text)]);
  if (all || args.includes('--check-citations')) results.push(['citations', checkCitations(text)]);
  if (args.includes('--check-urls') || all)
    results.push(['urls', await checkUrls(text, { live: !args.includes('--offline') })]);
  if (args.includes('--lint') || all) results.push(['lint', checkLint(text)]);
  if (args.includes('--check-links') || all)
    results.push(['links', checkLinks(text, path.dirname(reportPath))]);

  let red = 0;
  for (const [name, r] of results) {
    console.log(`${r.ok ? '✅' : '❌'} ${name}${r.count !== undefined ? `（${r.count} 条引用）` : ''}`);
    for (const n of r.notes) console.log(`   - ${n}`);
    if (!r.ok) red++;
  }
  console.log(red ? `\n共 ${red} 项红` : '\n全部通过');
  process.exit(red ? 1 : 0);
}

if (process.argv[1] && path.resolve(process.argv[1]) === path.resolve(fileURLToPath(import.meta.url))) {
  await main();
}
