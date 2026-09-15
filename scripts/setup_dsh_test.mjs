#!/usr/bin/env node
/**
 * setup_dsh_test.mjs — scripts/setup_dsh.mjs 的可重复自测。
 *
 *   node scripts/setup_dsh_test.mjs            # 全部离线（自带假网关），tmp 目录跑完即删
 *   node scripts/setup_dsh_test.mjs --keep     # 保留临时 DSH_HOME 供人工检查
 *
 * 在临时 DSH_HOME 下断言：
 *   ① 既有其他 route 与注释被保留
 *   ② .env 权限为 0600
 *   ③ 重复执行两次结果一致（幂等，且不产生多余备份）
 *   ④ --dry-run 不写盘、不备份
 *   ⑤ 网关不可达时非 0 退出且不写盘
 *   ⑥ 生成结果能被本机 dsh 的真实运行时 schema 接受
 */
import assert from 'node:assert/strict';
import fs from 'node:fs';
import http from 'node:http';
import os from 'node:os';
import path from 'node:path';
import { execFile } from 'node:child_process';
import { promisify } from 'node:util';
import { fileURLToPath } from 'node:url';
import { test } from 'node:test';

import { loadYaml } from './setup_dsh.mjs';

const KEEP = process.argv.includes('--keep');
const HERE = path.dirname(fileURLToPath(import.meta.url));
const SCRIPT = path.join(HERE, 'setup_dsh.mjs');
const KEY = 'sk-jai-SELFTEST-0000000000000000';
const KEY_ENV = 'JAI_API_KEY';

/** 真实 JAI /v1/models 的实测响应形状（见 crates/gateway-core/src/server/proxy.rs:491）。 */
const CATALOG = {
  object: 'list',
  data: [
    {
      id: '基元律动/deepseek-flash',
      object: 'model',
      owned_by: '基元律动',
      contextWindow: 512000,
      supportsMultimodal: true,
      inputModalities: ['text', 'image'],
      outputModalities: null,
    },
    { id: '备选/plain-text', object: 'model', owned_by: '备选', contextWindow: 128000, supportsMultimodal: false, inputModalities: ['text'], outputModalities: null },
    { id: '未知/audio-only', object: 'model', owned_by: '未知', contextWindow: 32000, inputModalities: ['audio'] },
  ],
};

/** 假网关：同样的 bearer 鉴权与 {data:[...]} 契约，不碰真上游、不花额度。 */
function startFakeGateway() {
  const server = http.createServer((req, res) => {
    const auth = req.headers.authorization || '';
    if (auth !== `Bearer ${KEY}`) {
      res.writeHead(401, { 'content-type': 'application/json' });
      res.end(JSON.stringify({ error: { message: 'unauthorized' } }));
      return;
    }
    if (req.url === '/v1/models' || req.url === '/models') {
      res.writeHead(200, { 'content-type': 'application/json' });
      res.end(JSON.stringify(CATALOG));
      return;
    }
    res.writeHead(404).end();
  });
  return new Promise((resolve) => {
    server.listen(0, '127.0.0.1', () => resolve({ server, port: server.address().port }));
  });
}

/** 既有用户配置：注释 + 其他 namespace + 其他 route（含注释）+ 老 key。 */
const SEED_SETTINGS = `# 我的注释：请勿删除
ui-theme:
  preference: dark
llm-pi-ai:
  {
    providers:
      {
        # 用户早就手工配过 jai，这条 route 上的自加字段与模型级声明不能被抹掉
        jai:
          {
            apiKeyEnv: JAI_API_KEY,
            api: openai-responses,
            baseURL: http://127.0.0.1:1314/v1,
            displayName: 我的 JAI,
            headers: { X-User-Tag: keep-me },
            retryPolicy: { mode: normal, maxRetries: 3 },
            models:
              [
                {
                    id: 基元律动/deepseek-flash,
                    contextWindow: 128000,
                    input: [ text ],
                    reasoningEfforts: { off: null, high: high, max: max }
                  },
                { id: 已下线/removed-model, contextWindow: 4096 }
              ]
          },
        # 另一个网关，别动我
        other-gateway:
          {
            apiKeyEnv: OTHER_KEY,
            api: openai-completions,
            baseURL: https://other.example/v1,
            models: [ { id: other-model, contextWindow: 8000 } ]
          }
      }
  }
agent-default-model:
  provider: other-gateway
  model: other-model
# 文件尾注释
`;

const SEED_ENV = `# dsh 的本机环境变量
OTHER_KEY=keep-me
${KEY_ENV}=sk-jai-STALE-value-that-must-be-replaced
`;

function tmpHome(label) {
  return fs.mkdtempSync(path.join(os.tmpdir(), `jai-setup-dsh-${label}-`));
}

const execFileAsync = promisify(execFile);

/** 必须异步 exec：同步会把本进程事件循环堵死，假网关就没法应答了。 */
async function run(args, { expectFail = false } = {}) {
  try {
    const { stdout, stderr } = await execFileAsync(process.execPath, [SCRIPT, ...args], { encoding: 'utf8' });
    if (expectFail) throw new Error(`预期非 0 退出，实际成功。输出:\n${stdout}`);
    return { code: 0, stdout, stderr };
  } catch (e) {
    if (!expectFail) {
      throw new Error(`setup_dsh.mjs 失败（exit ${e.code}）:\n${e.stdout || ''}\n${e.stderr || ''}`);
    }
    return { code: e.code, stdout: e.stdout || '', stderr: e.stderr || '' };
  }
}

const read = (p) => fs.readFileSync(p, 'utf8');
const mode = (p) => (fs.statSync(p).mode & 0o777).toString(8);
const backups = (dir) => fs.readdirSync(dir).filter((f) => f.includes('.bak-')).sort();

test('setup_dsh: 全流程（合并 / 0600 / 幂等 / dry-run）', async (t) => {
  const { server, port } = await startFakeGateway();
  const baseUrl = `http://127.0.0.1:${port}/v1`;
  const homes = [];
  t.after(() => {
    server.close();
    if (!KEEP) for (const h of homes) fs.rmSync(h, { recursive: true, force: true });
    else console.log(`[--keep] 临时 DSH_HOME 保留在: ${homes.join(', ')}`);
  });

  const home = tmpHome('main');
  homes.push(home);
  const settings = path.join(home, 'settings.yaml');
  const envFile = path.join(home, '.env');
  fs.writeFileSync(settings, SEED_SETTINGS, { mode: 0o600 });
  fs.writeFileSync(envFile, SEED_ENV, { mode: 0o600 });
  const common = ['--dsh-home', home, '--base-url', baseUrl, '--key', KEY];

  await t.test('① 既有其他 route 与注释被保留，JAI route 被写入', async () => {
    const r = await run(common);
    assert.equal(r.code, 0);
    assert.match(r.stdout, /已写入 .*settings\.yaml/);
    assert.match(r.stdout, /通过 @deepseek-ai\/dsh-llm-pi-ai\.Config 校验/, '生成结果应通过 dsh 真实 schema');

    const after = read(settings);
    // 注释
    assert.ok(after.includes('# 我的注释：请勿删除'), '文件头注释应保留');
    assert.ok(after.includes('# 文件尾注释'), '文件尾注释应保留');
    assert.ok(after.includes('# 另一个网关，别动我'), '其他 route 的注释应保留');
    // 其他 namespace / route 数据
    assert.ok(after.includes('other-gateway'), '其他 route 应保留');
    assert.ok(after.includes('https://other.example/v1'), '其他 route 的 baseURL 应保留');
    assert.ok(after.includes('preference: dark'), '其他 namespace 应保留');
  });

  await t.test('② .env 权限 0600，且只改目标键、保留其余行', async () => {
    assert.equal(mode(envFile), '600', '.env 必须是 0600');
    const e = read(envFile);
    assert.match(e, new RegExp(`^${KEY_ENV}=${KEY}$`, 'm'), '目标键应被写入');
    assert.equal((e.match(new RegExp(`^${KEY_ENV}=`, 'gm')) || []).length, 1, '目标键不应重复');
    assert.ok(e.includes('OTHER_KEY=keep-me'), '其他变量应保留');
    assert.ok(e.includes('# dsh 的本机环境变量'), '.env 注释应保留');
    assert.ok(!e.includes('STALE'), '旧值应被替换');
  });

  await t.test('③ 改动前时间戳备份已生成', async () => {
    const b = backups(home);
    assert.ok(b.some((f) => f.startsWith('settings.yaml.bak-')), `应有 settings.yaml 备份，实际: ${b}`);
    assert.ok(b.some((f) => f.startsWith('.env.bak-')), `应有 .env 备份，实际: ${b}`);
  });

  await t.test('④ 路由内容源自实时模型目录（含模态映射）', async () => {
    const YAML = await loadYaml(home);
    const doc = YAML.parseDocument(read(settings));
    assert.equal(doc.errors.length, 0, '生成结果必须是合法 YAML');
    const route = doc.toJS()['llm-pi-ai'];
    assert.ok(route, 'settings.yaml 里应有 llm-pi-ai 段');
    const jai = route.providers.jai;
    assert.equal(jai.api, 'openai-responses');
    assert.equal(jai.baseURL, baseUrl);
    assert.equal(jai.apiKeyEnv, KEY_ENV);
    assert.deepEqual(jai.models.map((m) => m.id), CATALOG.data.map((m) => m.id));
    assert.equal(jai.models[0].contextWindow, 512000);
    assert.deepEqual(jai.models[0].input, ['text', 'image'], 'JAI 报的多模态应写进 input');
    assert.deepEqual(jai.models[1].input, ['text']);
    // audio-only 被 dsh schema 拒绝，必须不写进去（值域只有 text|image）
    assert.equal(jai.models[2].input, undefined, '不支持的模态不能写进 input');
    // 用户在 jai route 上自加的字段必须原样保留
    assert.equal(jai.displayName, '我的 JAI');
    assert.deepEqual(jai.headers, { 'X-User-Tag': 'keep-me' });
    assert.deepEqual(jai.retryPolicy, { mode: 'normal', maxRetries: 3 });
    // 用户在同名模型上写的 reasoningEfforts 保留，JAI 目录里的事实（ctx/模态）被刷新
    assert.deepEqual(jai.models[0].reasoningEfforts, { off: null, high: 'high', max: 'max' });
    assert.deepEqual(jai.models[1].reasoningEfforts, undefined, '新模型没有用户声明，不应凭空造');
    assert.equal(jai.models.some((m) => m.id === '已下线/removed-model'), false, '目录里没有的模型应被移除');
    // 其他 route 的结构化数据也要还在
    assert.equal(route.providers['other-gateway'].baseURL, 'https://other.example/v1');
  });

  const settingsAfter1 = read(settings);
  const envAfter1 = read(envFile);
  const backupsAfter1 = backups(home);

  await t.test('⑤ 幂等：第二次执行字节一致，且不产生新备份', async () => {
    const r = await run(common);
    assert.equal(r.code, 0);
    assert.match(r.stdout, /已是最新状态/);
    assert.equal(read(settings), settingsAfter1, 'settings.yaml 应逐字节一致');
    assert.equal(read(envFile), envAfter1, '.env 应逐字节一致');
    assert.deepEqual(backups(home), backupsAfter1, '幂等执行不应新增备份');
  });

  await t.test('⑥ --dry-run：打印 diff 但不写盘、不备份', async () => {
    // 同时改 route 名与 key 变量名，让两个文件都"将要变化"，diff 才会都打印
    const r = await run([...common, '--route', 'jai-dry', '--key-env', 'JAI_API_KEY_DRY', '--dry-run']);
    assert.equal(r.code, 0);
    assert.match(r.stdout, /dry-run/);
    assert.match(r.stdout, /\+.*jai-dry/, 'settings.yaml 的 diff 应出现新 route 名');
    assert.match(r.stdout, /\+JAI_API_KEY_DRY=/, '.env 的 diff 应出现新变量名');
    assert.ok(!r.stdout.includes(KEY), 'dry-run 输出不得回显完整 key');
    assert.ok(r.stdout.includes('(redacted)'), 'key 应被脱敏');
    assert.equal(read(settings), settingsAfter1, 'dry-run 不得改动 settings.yaml');
    assert.equal(read(envFile), envAfter1, 'dry-run 不得改动 .env');
    assert.deepEqual(backups(home), backupsAfter1, 'dry-run 不得产生备份');
  });

  await t.test('⑦ 网关不可达：非 0 退出且不写盘', async () => {
    const deadHome = tmpHome('dead');
    homes.push(deadHome);
    const s = path.join(deadHome, 'settings.yaml');
    const e = path.join(deadHome, '.env');
    fs.writeFileSync(s, SEED_SETTINGS, { mode: 0o600 });
    fs.writeFileSync(e, SEED_ENV, { mode: 0o600 });
    const before = fs.readdirSync(deadHome).sort();
    const r = await run(['--dsh-home', deadHome, '--base-url', 'http://127.0.0.1:1/v1', '--key', KEY], { expectFail: true });
    assert.notEqual(r.code, 0);
    assert.match(r.stderr, /拉取模型目录失败/);
    assert.equal(read(s), SEED_SETTINGS, '失败时 settings.yaml 不得改动');
    assert.equal(read(e), SEED_ENV, '失败时 .env 不得改动');
    assert.deepEqual(fs.readdirSync(deadHome).sort(), before, '失败时不得留下临时/备份文件');
  });

  await t.test('⑧ 全新 dsh home（无 settings.yaml）也能建起来，权限 0600', async () => {
    const freshHome = tmpHome('fresh');
    homes.push(freshHome);
    const r = await run(['--dsh-home', freshHome, '--base-url', baseUrl, '--key', KEY, '--api', 'openai-completions']);
    assert.equal(r.code, 0);
    assert.equal(mode(path.join(freshHome, 'settings.yaml')), '600');
    assert.equal(mode(path.join(freshHome, '.env')), '600');
    const out = read(path.join(freshHome, 'settings.yaml'));
    assert.ok(out.includes('api: openai-completions'));
    assert.ok(out.includes('llm-pi-ai'));
    assert.deepEqual(backups(freshHome), [], '首次创建不应有备份');
  });
});
