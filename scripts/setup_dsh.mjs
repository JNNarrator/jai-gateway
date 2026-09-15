#!/usr/bin/env node
/**
 * setup_dsh.mjs — 把 JAI 网关作为一条 provider route 结构化合并进 dsh 的 settings.yaml。
 *
 * schema 实证来源（本机实测，非猜测）：
 *   - `~/.dsh/settings.yaml` L5-L37：`llm-pi-ai.providers.<route>` 下 `apiKeyEnv` /
 *     `baseURL` / `api: openai-responses` / `models[].{id,contextWindow,input}`。
 *   - `@deepseek-ai/dsh-llm-pi-ai`（本机 npm 全局安装）README「Configure provider routes」
 *     与 `lib/types/config.d.ts` / `lib/types/catalog.d.ts`：字段全集与取值域。
 *   - `@deepseek-ai/dsh-credentials-local` README L78：`$DSH_HOME/.env` 是凭据引用的
 *     读取回退层 —— key 写这里即可被 `apiKeyEnv` 解析。
 *   - `crates/gateway-core/src/server/mod.rs:90-108`：JAI 入站路由表（含 /v1/responses）。
 *   - `crates/gateway-core/src/server/proxy.rs:491`：`GET /v1/models` 返回
 *     `{data:[{id:"<owner>/<model>", contextWindow, inputModalities, ...}]}`。
 *
 * 只做三件事：settings.yaml 结构化合并、.env 写 key（0600）、改动前时间戳备份。
 * 幂等：内容无变化则一个字节都不写、也不备份。
 */
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { createRequire } from 'node:module';
import { pathToFileURL } from 'node:url';
import { execFileSync } from 'node:child_process';
import process from 'node:process';

const DEFAULT_BASE_URL = 'http://127.0.0.1:1314/v1';
const DEFAULT_ROUTE = 'jai';
const DEFAULT_KEY_ENV = 'JAI_API_KEY';
const SUPPORTED_APIS = ['openai-responses', 'openai-completions', 'anthropic-messages'];
/** JAI 自己的模块名 + dsh 的 provider 命名空间，二者拼接成 settings.yaml 顶层 key。 */
const SETTINGS_NAMESPACE = 'llm-pi-ai';

// ---------------------------------------------------------------- argv

function parseArgs(argv) {
  const o = {
    dshHome: process.env.DSH_HOME || path.join(os.homedir(), '.dsh'),
    baseUrl: DEFAULT_BASE_URL,
    route: DEFAULT_ROUTE,
    keyEnv: DEFAULT_KEY_ENV,
    api: 'openai-responses',
    key: null,
    db: null,
    dryRun: false,
    validate: true,
    autodetectKey: true,
    timeoutMs: 10000,
    help: false,
  };
  const need = (i, name) => {
    if (i + 1 >= argv.length) throw new Error(`${name} 需要一个参数值`);
    return argv[i + 1];
  };
  for (let i = 0; i < argv.length; i++) {
    const a = argv[i];
    switch (a) {
      case '--dsh-home': o.dshHome = need(i, a); i++; break;
      case '--base-url': o.baseUrl = need(i, a); i++; break;
      case '--route': case '--route-name': o.route = need(i, a); i++; break;
      case '--key': o.key = need(i, a); i++; break;
      case '--key-env': o.keyEnv = need(i, a); i++; break;
      case '--api': o.api = need(i, a); i++; break;
      case '--db': o.db = need(i, a); i++; break;
      case '--timeout-ms': o.timeoutMs = Number(need(i, a)); i++; break;
      case '--dry-run': case '-n': o.dryRun = true; break;
      case '--no-validate': o.validate = false; break;
      case '--no-key-autodetect': o.autodetectKey = false; break;
      case '--help': case '-h': o.help = true; break;
      default:
        throw new Error(`未知参数：${a}（--help 查看用法）`);
    }
  }
  return o;
}

const USAGE = `用法: node scripts/setup_dsh.mjs [选项]

把 JAI 网关写成 dsh 的一条 provider route（settings.yaml 结构化合并 + .env 写 key）。

选项:
  --dsh-home <dir>      dsh 家目录（默认 $DSH_HOME，否则 ~/.dsh）
  --route <name>        provider route 名（默认 ${DEFAULT_ROUTE}）
  --api <proto>         ${SUPPORTED_APIS.join(' | ')}
                        （默认 openai-responses；JAI 两条线都实现了）
  --base-url <url>      JAI 的 OpenAI 兼容根（默认 ${DEFAULT_BASE_URL}）
  --key <sk-jai-...>    网关 Key（默认 $JAI_GATEWAY_KEY → $JAI_API_KEY → 从 JAI 数据库只读探测）
  --key-env <NAME>      dsh 侧引用的环境变量名（默认 ${DEFAULT_KEY_ENV}）
  --db <path>           JAI 数据库路径（仅用于 --key 自动探测）
  --no-key-autodetect   不自动探测网关 Key
  --dry-run, -n         只打印将要发生的 diff，不写盘、不备份
  --no-validate         跳过用本机 dsh 真实 schema 校验生成的 section
  --timeout-ms <n>      拉取模型目录的超时（默认 10000）
  -h, --help            显示本帮助

退出码: 0 成功（含"无变化"） 1 用法/环境错误 2 生成的配置未通过 dsh schema 校验`;

function log(...a) { console.log(...a); }
function warn(...a) { console.error('警告:', ...a); }
function die(msg, code = 1) { console.error(`错误: ${msg}`); process.exit(code); }

/** 只打印前缀，永不打印完整 key。 */
function redact(key) { return key ? `${key.slice(0, 11)}…(${key.length} 字符)` : '(空)'; }

// ---------------------------------------------------------------- 依赖解析（复用本机已装，不新增依赖）

const require_ = createRequire(import.meta.url);

/** 从某个 node_modules 目录里解析包的 ESM 入口文件绝对路径；失败返回 null。 */
function entryFromModulesDir(modulesDir, pkgName) {
  const pkgDir = path.join(modulesDir, ...pkgName.split('/'));
  const pkgJson = path.join(pkgDir, 'package.json');
  if (!fs.existsSync(pkgJson)) return null;
  let pkg;
  try { pkg = JSON.parse(fs.readFileSync(pkgJson, 'utf8')); } catch { return null; }
  const exp = pkg.exports && (pkg.exports['.'] ?? pkg.exports);
  let rel = null;
  if (exp && typeof exp === 'object') rel = exp.import ?? exp.node ?? exp.default ?? exp.require;
  else if (typeof exp === 'string') rel = exp;
  else if (pkg.main) rel = pkg.main;
  if (!rel) return null;
  const abs = path.resolve(pkgDir, rel);
  return fs.existsSync(abs) ? abs : null;
}

/** 在 $PATH 里找可执行文件（不走 shell，避免 DEP0190 与转义问题）。 */
function which(cmd) {
  const pathEnv = env('PATH') || '';
  for (const dir of pathEnv.split(path.delimiter)) {
    if (!dir) continue;
    const p = path.join(dir, cmd);
    try {
      const st = fs.statSync(p);
      if (st.isFile() && (st.mode & 0o111)) return p;
    } catch { /* 下一个 */ }
  }
  return null;
}

/** dsh 的 npm 全局安装前缀：`command -v dsh` → <prefix>/bin/dsh。 */
function dshPrefix() {
  const bin = which('dsh');
  return bin ? path.dirname(path.dirname(bin)) : null;
}

/** 从文件往上找最近的 package.json，定位 dsh 自己的包目录。 */
function nearestPackageDir(file) {
  let dir = path.dirname(file);
  for (let i = 0; i < 6; i++) {
    if (fs.existsSync(path.join(dir, 'package.json'))) return dir;
    const up = path.dirname(dir);
    if (up === dir) break;
    dir = up;
  }
  return null;
}

/** 收集所有可能藏着包的 node_modules 目录。 */
function candidateModulesDirs(dshHome) {
  const dirs = [];
  const prefix = dshPrefix();
  if (prefix) dirs.push(path.join(prefix, 'lib', 'node_modules'));
  const bin = which('dsh');
  if (bin) {
    try {
      const pkgDir = nearestPackageDir(fs.realpathSync(bin));
      if (pkgDir) dirs.push(path.join(pkgDir, 'node_modules'));
    } catch { /* 保持原样 */ }
  }
  try {
    const root = execFileSync('npm', ['root', '-g'], { encoding: 'utf8' }).trim();
    if (root) dirs.push(root);
  } catch { /* npm 不可用就算了 */ }
  if (env('JAI_NODE_MODULES')) dirs.push(env('JAI_NODE_MODULES'));
  if (dshHome) {
    dirs.push(path.join(dshHome, 'profiles', 'node_modules'));
    try {
      for (const e of fs.readdirSync(path.join(dshHome, 'profiles'), { withFileTypes: true })) {
        if (e.isDirectory()) dirs.push(path.join(dshHome, 'profiles', e.name, 'node_modules'));
      }
    } catch { /* 没有 profiles 目录 */ }
  }
  return [...new Set(dirs)];
}
function env(name) { return process.env[name] || ''; }

async function loadYaml(dshHome) {
  const tried = [];
  const explicit = env('JAI_YAML_PATH');
  if (explicit) {
    try { return (await import(pathToFileURL(explicit).href)).default; } catch (e) { tried.push(`${explicit} (${e.code})`); }
  }
  try { const m = await import('yaml'); return m.default ?? m; } catch (e) { tried.push(`bare import: ${e.code}`); }
  for (const dir of candidateModulesDirs(dshHome)) {
    const file = entryFromModulesDir(dir, 'yaml');
    if (!file) continue;
    try { const m = await import(pathToFileURL(file).href); return m.default ?? m; }
    catch (e) { tried.push(`${file} (${e.code || e.message})`); }
  }
  throw new Error(
    '找不到可用的 yaml (v2) 解析器。本仓库不新增依赖，请复用本机已有的 yaml，' +
    '或显式指定入口：JAI_YAML_PATH=/path/to/yaml/dist/index.js node scripts/setup_dsh.mjs\n' +
    `尝试过：\n  - ${tried.join('\n  - ')}`,
  );
}

/**
 * 用本机 dsh 的真实运行时 schema 校验生成的 section（最强的一手证据：
 * 不是我们复述 schema，而是 dsh 自己接受/拒绝）。
 * 找不到包时返回 {skipped:true}，不阻断。
 */
async function loadDshSchema(dshHome) {
  for (const dir of candidateModulesDirs(dshHome)) {
    const file = entryFromModulesDir(dir, '@deepseek-ai/dsh-llm-pi-ai');
    if (!file) continue;
    try { return await import(pathToFileURL(file).href); } catch { /* 试下一个 */ }
  }
  return null;
}

// ---------------------------------------------------------------- 网关 Key 探测

function jaiDbCandidates(explicit) {
  if (explicit) return [explicit];
  if (env('JAI_DB')) return [env('JAI_DB')];
  const base = {
    darwin: path.join(os.homedir(), 'Library', 'Application Support'),
    linux: path.join(os.homedir(), '.local', 'share'),
    win32: env('APPDATA') || path.join(os.homedir(), 'AppData', 'Roaming'),
  }[process.platform];
  return base ? [path.join(base, 'app.jai.gateway', 'jai.db')] : [];
}

/** 只读探测 JAI 自带的网关 Key；返回 {key, source} 或 null。 */
function autodetectKey(opts) {
  let sqlite3Ok = true;
  try { execFileSync('sqlite3', ['-version'], { stdio: 'ignore' }); } catch { sqlite3Ok = false; }
  for (const db of jaiDbCandidates(opts.db)) {
    if (!fs.existsSync(db)) continue;
    if (!sqlite3Ok) { warn(`发现 ${db}，但本机没有 sqlite3，跳过自动探测`); continue; }
    try {
      const uri = `file:${db}?mode=ro`;
      const rows = execFileSync('sqlite3', [uri, 'select key from gateway_keys order by created_at desc;'], { encoding: 'utf8' })
        .split('\n').map((s) => s.trim()).filter(Boolean);
      if (rows.length === 0) continue;
      if (rows.length > 1) warn(`数据库里有 ${rows.length} 个网关 Key，自动选用最近创建的那个；用 --key 指定其他的`);
      return { key: rows[0], source: `${db}（只读）` };
    } catch (e) { warn(`读取 ${db} 失败：${e.message}`); }
  }
  return null;
}

function resolveKey(opts) {
  if (opts.key) return { key: opts.key, source: '--key 参数' };
  if (env('JAI_GATEWAY_KEY')) return { key: env('JAI_GATEWAY_KEY'), source: '$JAI_GATEWAY_KEY' };
  if (env('JAI_API_KEY')) return { key: env('JAI_API_KEY'), source: '$JAI_API_KEY' };
  if (opts.autodetectKey) return autodetectKey(opts);
  return null;
}

// ---------------------------------------------------------------- 模型目录

/** 调 JAI 出站 `GET /v1/models`（与 dsh 自己的发现实现同一契约：bearer + {data:[...]}）。 */
async function fetchCatalog(baseUrl, key, timeoutMs) {
  const url = `${baseUrl.replace(/\/+$/, '')}/models`;
  let res;
  try {
    res = await fetch(url, {
      headers: { authorization: `Bearer ${key}`, accept: 'application/json' },
      signal: AbortSignal.timeout(timeoutMs),
    });
  } catch (e) {
    throw new Error(`拉取模型目录失败（${url}）：${e.message}。JAI 网关是否在运行？`);
  }
  const text = await res.text();
  if (!res.ok) {
    throw new Error(`拉取模型目录失败（${url}）：HTTP ${res.status} ${text.slice(0, 200)}`);
  }
  let body;
  try { body = JSON.parse(text); } catch { throw new Error(`模型目录不是合法 JSON：${text.slice(0, 200)}`); }
  const data = Array.isArray(body?.data) ? body.data : null;
  if (!data) throw new Error(`模型目录缺少 data 数组：${text.slice(0, 200)}`);
  return data;
}

/** JAI 的 inputModalities / supportsMultimodal → dsh 的 model.input（取值域只有 text|image）。 */
function toInput(modalities, supportsMultimodal) {
  const allowed = new Set(['text', 'image']);
  const dropped = [];
  let picked = [];
  if (Array.isArray(modalities)) {
    for (const m of modalities) {
      if (typeof m !== 'string') continue;
      if (allowed.has(m)) picked.push(m); else dropped.push(m);
    }
  } else if (supportsMultimodal === true) {
    picked = ['text', 'image'];
  }
  if (picked.length === 0) return { input: undefined, dropped };
  return { input: [...new Set(picked)], dropped };
}

/** JAI 目录条目 → dsh 的 models[] 条目（只写 dsh schema 真正接受的字段）。 */
function toModelEntries(data) {
  const models = [];
  const lostModalities = new Set();
  for (const m of data) {
    if (!m || typeof m.id !== 'string' || !m.id) continue;
    const entry = { id: m.id };
    if (Number.isFinite(m.contextWindow) && m.contextWindow > 0) entry.contextWindow = m.contextWindow;
    const { input, dropped } = toInput(m.inputModalities, m.supportsMultimodal);
    if (input) entry.input = input;
    for (const d of dropped) lostModalities.add(d);
    models.push(entry);
  }
  return { models, lostModalities: [...lostModalities] };
}

/**
 * 模型列表合并：目录里已存在的模型 id，保留用户自己在该条目上声明的字段
 * （reasoningEfforts / compat / name / maxTokens …），只刷新我们从 JAI 目录
 * 拿到的事实（contextWindow / input）。目录里没有的老条目按"目录即事实"移除。
 * 「不得破坏用户已有的其他配置」在这一层同样适用。
 */
const MANAGED_MODEL_FIELDS = new Set(['id', 'contextWindow', 'input']);

function mergeModels(existingModels, freshModels) {
  if (!Array.isArray(existingModels) || existingModels.length === 0) return freshModels;
  return freshModels.map((fresh) => {
    const prev = existingModels.find((m) => m && typeof m === 'object' && m.id === fresh.id);
    if (!prev) return fresh;
    const merged = { ...fresh };
    for (const [k, v] of Object.entries(prev)) {
      if (!MANAGED_MODEL_FIELDS.has(k) && v !== undefined) merged[k] = v;
    }
    return merged;
  });
}

// ---------------------------------------------------------------- .env 合并

const esc = (s) => s.replace(/[.*+?^${}()|[\]\\]/g, '\\$&');

/** 只替换目标键那一行，其余行（含注释、空行、别的变量、export 前缀）原样保留。 */
function mergeEnv(text, keyName, value) {
  const nl = text.includes('\r\n') ? '\r\n' : '\n';
  const lines = text.length ? text.split(/\r?\n/) : [];
  while (lines.length && lines[lines.length - 1] === '') lines.pop(); // 尾部空行单独处理，保证幂等
  const re = new RegExp(`^\\s*(?:export\\s+)?${esc(keyName)}\\s*=`);
  let replaced = false;
  const out = lines.map((line) => {
    if (!replaced && re.test(line)) {
      replaced = true;
      const prefix = /^\s*(export\s+)?/.exec(line)[1] || '';
      return `${prefix}${keyName}=${value}`;
    }
    return line;
  });
  if (!replaced) {
    if (out.length) out.push('');
    out.push(`${keyName}=${value}`);
  }
  return out.join(nl) + nl;
}

// ---------------------------------------------------------------- 极简 unified diff

function diffLines(aText, bText, redactions = []) {
  const red = (s) => redactions.reduce((acc, [from, to]) => acc.split(from).join(to), s);
  const a = aText.length ? aText.replace(/\n$/, '').split('\n') : [];
  const b = bText.length ? bText.replace(/\n$/, '').split('\n') : [];
  const n = a.length, m = b.length;
  // LCS 动态规划（配置文件是几百行量级，够用）
  const dp = Array.from({ length: n + 1 }, () => new Uint32Array(m + 1));
  for (let i = n - 1; i >= 0; i--) {
    for (let j = m - 1; j >= 0; j--) {
      dp[i][j] = a[i] === b[j] ? dp[i + 1][j + 1] + 1 : Math.max(dp[i + 1][j], dp[i][j + 1]);
    }
  }
  const out = [];
  let i = 0, j = 0;
  while (i < n && j < m) {
    if (a[i] === b[j]) { out.push(` ${red(a[i])}`); i++; j++; }
    else if (dp[i + 1][j] >= dp[i][j + 1]) { out.push(`-${red(a[i])}`); i++; }
    else { out.push(`+${red(b[j])}`); j++; }
  }
  while (i < n) out.push(`-${red(a[i++])}`);
  while (j < m) out.push(`+${red(b[j++])}`);
  return out.join('\n');
}

// ---------------------------------------------------------------- 备份

/** 与 dsh 自身同款命名：foo.yml.bak-20260911-143654。 */
function backupPath(file) {
  const d = new Date();
  const p = (x) => String(x).padStart(2, '0');
  const stamp = `${d.getFullYear()}${p(d.getMonth() + 1)}${p(d.getDate())}-${p(d.getHours())}${p(d.getMinutes())}${p(d.getSeconds())}`;
  let candidate = `${file}.bak-${stamp}`;
  let n = 2;
  while (fs.existsSync(candidate)) candidate = `${file}.bak-${stamp}-${n++}`;
  return candidate;
}

function writeFileSecure(file, content, mode) {
  const tmp = `${file}.tmp-${process.pid}`;
  fs.writeFileSync(tmp, content, { mode });
  fs.chmodSync(tmp, mode);
  fs.renameSync(tmp, file);
  fs.chmodSync(file, mode);
}

// ---------------------------------------------------------------- 主流程

async function main() {
  const opts = parseArgs(process.argv.slice(2));
  if (opts.help) { log(USAGE); return 0; }
  if (!SUPPORTED_APIS.includes(opts.api)) die(`--api 只接受 ${SUPPORTED_APIS.join(' / ')}（dsh schema 的实际取值域）`);
  if (!/^[A-Za-z_][A-Za-z0-9_]*$/.test(opts.keyEnv)) die(`--key-env 必须是环境变量名，收到 ${opts.keyEnv}`);

  const settingsPath = path.join(opts.dshHome, 'settings.yaml');
  const envPath = path.join(opts.dshHome, '.env');

  const YAML = await loadYaml(opts.dshHome);
  if (!fs.existsSync(opts.dshHome) && !opts.dryRun) fs.mkdirSync(opts.dshHome, { recursive: true, mode: 0o700 });

  log(`dsh home      : ${opts.dshHome}`);
  log(`settings.yaml : ${settingsPath}${fs.existsSync(settingsPath) ? '' : '（不存在，将新建）'}`);
  log(`route         : ${opts.route}   api: ${opts.api}`);
  log(`baseURL       : ${opts.baseUrl}`);
  log(`key env 名     : ${opts.keyEnv}`);

  // 1) key
  const resolved = resolveKey(opts);
  if (!resolved) {
    die('拿不到网关 Key。用 --key sk-jai-... 传入，或设 $JAI_GATEWAY_KEY，' +
      '或在 JAI 应用的「网关」页复制（自动探测需要本机 sqlite3 与 JAI 数据库）');
  }
  log(`网关 Key       : ${redact(resolved.key)}（来源：${resolved.source}）`);
  if (!resolved.key.startsWith('sk-jai-')) warn(`Key 不以 sk-jai- 开头，JAI 可能拒绝该凭据`);

  // 2) 实时模型目录
  const data = await fetchCatalog(opts.baseUrl, resolved.key, opts.timeoutMs);
  const { models, lostModalities } = toModelEntries(data);
  if (models.length === 0) die('JAI 返回的模型目录为空。dsh 拒绝没有 models 的自声明 route，请先在 JAI 里启用至少一个模型');
  log(`模型目录       : ${models.length} 个（GET ${opts.baseUrl.replace(/\/+$/, '')}/models）`);
  for (const m of models) log(`  - ${m.id}${m.contextWindow ? `  ctx=${m.contextWindow}` : ''}${m.input ? `  input=[${m.input.join(',')}]` : ''}`);
  if (lostModalities.length) warn(`JAI 报告的模态 ${lostModalities.join(',')} 不被 dsh 支持，已丢弃（dsh 只认 text/image）`);

  // 3) settings.yaml 结构化合并
  const existed = fs.existsSync(settingsPath);
  const before = existed ? fs.readFileSync(settingsPath, 'utf8') : '';
  const doc = YAML.parseDocument(before || '{}\n');
  if (doc.errors && doc.errors.length) die(`既有 settings.yaml 解析失败：${doc.errors[0].message}`);

  // 既有 route 上用户为同名模型写过的字段要留住，只按目录刷新我们管的那几个
  const existingModels = (() => {
    try { return YAML.parse(before)?.[SETTINGS_NAMESPACE]?.providers?.[opts.route]?.models ?? null; }
    catch { return null; }
  })();

  const routeFields = {
    apiKeyEnv: opts.keyEnv,
    api: opts.api,
    baseURL: opts.baseUrl,
    models: mergeModels(existingModels, models),
  };
  // 逐字段 setIn：route 上用户自己加的 displayName / headers / retryPolicy 等一律保留
  for (const [k, v] of Object.entries(routeFields)) {
    doc.setIn([SETTINGS_NAMESPACE, 'providers', opts.route, k], v);
  }
  const after = doc.toString();

  // 4) schema 校验（用 dsh 安装包里的真实运行时 schema）
  let schemaReport = '未校验（--no-validate）';
  if (opts.validate) {
    const schemaMod = await loadDshSchema(opts.dshHome);
    if (!schemaMod || typeof schemaMod.Config !== 'function') {
      schemaReport = '跳过（本机找不到 @deepseek-ai/dsh-llm-pi-ai）';
      warn(schemaReport);
    } else {
      const section = YAML.parse(after)[SETTINGS_NAMESPACE];
      try {
        schemaMod.Config(section, true);
        schemaReport = `通过 @deepseek-ai/dsh-llm-pi-ai.Config 校验`;
      } catch (e) {
        schemaReport = `未通过：${e.message}`;
        console.error(`错误: 生成的配置没有通过 dsh 自带 schema 校验：${e.message}`);
        return 2;
      }
    }
  }
  log(`schema 校验    : ${schemaReport}`);

  // 5) .env
  const envExisted = fs.existsSync(envPath);
  const envBefore = envExisted ? fs.readFileSync(envPath, 'utf8') : '';
  const envAfter = mergeEnv(envBefore, opts.keyEnv, resolved.key);

  const settingsChanged = after !== before;
  const envChanged = envAfter !== envBefore;
  if (!settingsChanged && !envChanged) {
    log('\n✓ 已是最新状态，未写入任何文件、未创建备份（幂等）。');
    return 0;
  }

  const redactions = [[resolved.key, `${resolved.key.slice(0, 11)}…(redacted)`]];
  if (opts.dryRun) {
    log('\n=== --dry-run：以下为将要发生的改动（不写盘、不备份）===');
    if (settingsChanged) {
      log(`\n--- ${settingsPath}\n+++ ${settingsPath} (合并后)`);
      log(diffLines(before, after, redactions));
    } else log(`\n${settingsPath} 无变化`);
    if (envChanged) {
      log(`\n--- ${envPath}  (写入后强制 0600)\n+++ ${envPath} (合并后)`);
      log(diffLines(envBefore, envAfter, redactions));
    } else log(`\n${envPath} 无变化`);
    log('\n=== dry-run 结束：未改动任何文件 ===');
    return 0;
  }

  // 6) 备份 + 落盘
  // settings.yaml 沿用原权限（dsh 自己建的是 0600）；.env 按需求强制 0600。
  const settingsMode = (() => {
    if (!existed) return 0o600;
    const m = fs.statSync(settingsPath).mode & 0o777;
    return m || 0o600;
  })();
  const backups = [];
  if (settingsChanged) {
    if (existed) { const b = backupPath(settingsPath); fs.copyFileSync(settingsPath, b); backups.push(b); }
    writeFileSecure(settingsPath, after, settingsMode);
  }
  if (envChanged) {
    if (envExisted) { const b = backupPath(envPath); fs.copyFileSync(envPath, b); backups.push(b); }
    writeFileSecure(envPath, envAfter, 0o600);
  }
  log('');
  for (const b of backups) log(`备份: ${b}`);
  if (settingsChanged) log(`✓ 已写入 ${settingsPath}（合并 ${SETTINGS_NAMESPACE}.providers.${opts.route}，其他 route/注释原位保留）`);
  if (envChanged) log(`✓ 已写入 ${envPath}（${opts.keyEnv}=${redact(resolved.key)}，权限 0600）`);
  log(`\n启动 dsh 即可用：dsh --profile web（模型选择器里会出现 ${models.length} 个 JAI 模型）`);
  return 0;
}

// 直接执行时才跑 main；被测试脚本 import 时不产生副作用。
const invokedDirectly = process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href;
if (invokedDirectly) {
  main()
    .then((code) => process.exit(code))
    .catch((e) => { console.error(`错误: ${e.message}`); process.exit(1); });
}

export {
  main,
  parseArgs,
  mergeEnv,
  diffLines,
  toModelEntries,
  toInput,
  loadYaml,
  loadDshSchema,
  candidateModulesDirs,
  entryFromModulesDir,
  mergeModels,
  nearestPackageDir,
  dshPrefix,
  backupPath,
  which,
};
