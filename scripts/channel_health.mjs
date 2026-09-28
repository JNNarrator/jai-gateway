#!/usr/bin/env node
// 更新通道健康检查（v0.4.3 发版事故后新增）。
//
// ## 为什么需要它
//
// 2026-09-28 实测事故：GitHub 上**同时存在两个 v0.4.3 release** —— CI 建的草稿
// （`github-actions[bot]`、8 个资产、含 `latest.json`）与**人工从同一 tag 手建的已发布
// release**（`JNNarrator`、0 资产、name/body 全空）。后者虽是空壳，但"已发布"的身份让它
// 抢占了 `/releases/latest` ⇒ `…/latest/download/latest.json` 变成 **404** ⇒ Tauri 端
// 报 `Could not fetch a valid release JSON from the remote`。
//
// 这个故障**全程没有任何报错**：Release 工作流 3/3 job success、`release_check.sh` 全绿，
// 但**所有用户**的「检查更新」都坏了。人工发现（用户点了才知道）之前的窗口约 9 分钟，
// 而若无人点击则可能持续数天。
//
// 本脚本把「更新通道是否真的可用」变成一条可自动执行的判据：
//   ① `latest.json` 能取到且是合法 JSON；
//   ② 它的 `version` 与「最新已发布 release」的 tag 一致（防「latest 指向一个没有 feed 的
//      release」—— 本次事故的形态）；
//   ③ 那个 release **确实带资产**（0 资产的空壳是本次的直接原因）；
//   ④ 平台键齐全、每个平台的 `signature` 非空；
//   ⑤ 每个平台的资产 URL **真能取到**（HTTP 200/206）。
//
// 只读：只发 GET/HEAD，不改任何远端状态。可安全地在任意时刻跑。
//
// ## 用法
//
//   node scripts/channel_health.mjs                          # 默认仓库，自动比对最新 tag
//   node scripts/channel_health.mjs --expect-version 0.4.3   # 发版后校验「确实切到这一版」
//   node scripts/channel_health.mjs --repo owner/name
//
// 退出码：0 = 通道健康；1 = 有判据不通过（逐条打印原因与建议）。
//
// ## 已知边界
//
// - GitHub 的 `/releases/latest` **不含草稿、也不含 prerelease**。所以「发版后忘了点
//   Publish（草稿还躺着）」在这里表现为「version 仍是上一版」⇒ 判据 ② 会红，
//   提示语会直接点出「草稿尚未发布」这个可能。
// - 资源 URL 走 CDN 重定向，且**响应可能被 CDN 缓存**（见 `docs/design/release.md` 记的
//   坑）。故：feed 请求一律带 `Cache-Control: no-cache` + 随机 query，且对「版本尚未切换」
//   做**有界重试**（默认 5 次 × 15s），避免把 CDN 传播延迟误报成故障。

import { randomUUID } from "node:crypto";

// macOS 目前只出 aarch64（Intel 缺口见 docs/design/release.md §1），
// 故期望键集就是「arm64 mac + x64 windows」。补 Intel 后需同步这里。
const EXPECTED_PLATFORMS = [
  "darwin-aarch64",
  "darwin-aarch64-app",
  "windows-x86_64",
  "windows-x86_64-msi",
  "windows-x86_64-nsis",
];

const DEFAULT_REPO = "JNNarrator/jai-gateway";
// `JAI_CHANNEL_API` / `JAI_CHANNEL_FEED_URL` 只为**可测试性**存在：默认走真实 GitHub；
// 置成 mock 地址即可对「0 资产」「feed 404」这类故障形态做注入验证（见 release.md §4 的
// 自测说明）。生产路径不读它们。
const API = process.env.JAI_CHANNEL_API ?? "https://api.github.com";

function parseArgs(argv) {
  const out = { repo: DEFAULT_REPO, expectVersion: null, retries: 5, retryDelayMs: 15000 };
  for (let i = 0; i < argv.length; i++) {
    const a = argv[i];
    if (a === "--repo") out.repo = argv[++i];
    else if (a === "--expect-version") out.expectVersion = argv[++i];
    else if (a === "--retries") out.retries = Number(argv[++i]);
    else if (a === "--retry-delay-ms") out.retryDelayMs = Number(argv[++i]);
    else if (a === "--help" || a === "-h") {
      console.log("用法: node scripts/channel_health.mjs [--repo owner/name] [--expect-version X.Y.Z]");
      process.exit(0);
    } else {
      console.error(`未知参数: ${a}`);
      process.exit(2);
    }
  }
  return out;
}

const args = parseArgs(process.argv.slice(2));

let failures = 0;
let checks = 0;

function ok(msg) {
  checks++;
  console.log(`  ✓ ${msg}`);
}
function fail(msg, hint) {
  checks++;
  failures++;
  console.log(`  ✗ ${msg}`);
  if (hint) console.log(`      → ${hint}`);
}
function info(msg) {
  console.log(`    ${msg}`);
}

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

/** 带重试的 GET（返回解析后的 JSON；非 2xx / 非 JSON 抛错）。 */
async function getJson(url, { headers = {}, retries = 1 } = {}) {
  let lastErr;
  for (let i = 0; i < retries; i++) {
    try {
      const res = await fetch(url, { headers, redirect: "follow" });
      if (!res.ok) throw new Error(`HTTP ${res.status}`);
      return await res.json();
    } catch (e) {
      lastErr = e;
      if (i < retries - 1) await sleep(2000);
    }
  }
  throw lastErr;
}

/** feed 专用取法：**必须**带 cache-bust，否则可能拿到 CDN 缓存的上一个版本。 */
async function fetchFeed(repo) {
  const base =
    process.env.JAI_CHANNEL_FEED_URL ??
    `https://github.com/${repo}/releases/latest/download/latest.json`;
  const url = `${base}?cb=${randomUUID()}`;
  const res = await fetch(url, {
    headers: { "Cache-Control": "no-cache, no-store", Pragma: "no-cache" },
    redirect: "follow",
  });
  const text = await res.text();
  return { status: res.status, text };
}

async function assetReachable(url) {
  // 用 Range 取首字节：既证明资产存在，又不下整包（macOS dmg 有几十 MB）
  const res = await fetch(url, {
    headers: { Range: "bytes=0-0", "Cache-Control": "no-cache" },
    redirect: "follow",
  });
  return res.status;
}

function main() {
  console.log("更新通道健康检查");
  console.log(`仓库: ${args.repo}`);
  if (args.expectVersion) console.log(`期望版本: ${args.expectVersion}`);
  console.log("");
}

async function run() {
  main();

  // ---- ① 最新已发布 release（不含草稿 / prerelease）
  console.log("【1】最新已发布 release");
  let latest;
  try {
    latest = await getJson(`${API}/repos/${args.repo}/releases/latest`);
    ok(`latest = ${latest.tag_name}（${latest.assets.length} 个资产）`);
  } catch (e) {
    fail(`取不到 /releases/latest：${e.message}`, "仓库是否公开、是否已有任何已发布 release？");
    return;
  }

  const latestVersion = String(latest.tag_name).replace(/^v/, "");
  const hasFeedAsset = latest.assets.some((a) => a.name === "latest.json");
  if (latest.assets.length === 0) {
    fail(
      "最新已发布 release 的资产数为 0 —— 这正是 v0.4.3 事故的形态",
      "空壳 release 会抢占 /releases/latest，使更新通道对所有人失效。" +
        "请核对是否有「人工从已有 tag 手建」的 release，并与 CI 草稿（作者应为 github-actions[bot]）比对。"
    );
  } else {
    ok(`资产数 = ${latest.assets.length}`);
  }
  if (hasFeedAsset) ok("release 内含 latest.json");
  else fail("release 内没有 latest.json", "updater feed 就是它；缺它则 latest.json 必然 404");

  // ---- ② feed 可取到且合法
  console.log("");
  console.log("【2】updater feed（带 cache-bust）");
  let feed = null;
  for (let attempt = 0; attempt < args.retries; attempt++) {
    let status, text;
    try {
      ({ status, text } = await fetchFeed(args.repo));
    } catch (e) {
      if (attempt < args.retries - 1) {
        await sleep(args.retryDelayMs);
        continue;
      }
      fail(`feed 请求失败：${e.message}`);
      return;
    }
    if (status !== 200) {
      if (attempt < args.retries - 1) {
        info(`第 ${attempt + 1} 次: HTTP ${status}，${args.retryDelayMs / 1000}s 后重试（CDN 传播）`);
        await sleep(args.retryDelayMs);
        continue;
      }
      fail(
        `feed 返回 HTTP ${status}（期望 200）`,
        "典型原因：「最新已发布 release」是个没有资产的空壳 ⇒ 该 URL 404。" +
          "注意 GitHub 的 latest 不含草稿/prerelease。"
      );
      return;
    }
    try {
      feed = JSON.parse(text);
    } catch {
      if (attempt < args.retries - 1) {
        info(`第 ${attempt + 1} 次: 非 JSON，${args.retryDelayMs / 1000}s 后重试`);
        await sleep(args.retryDelayMs);
        continue;
      }
      fail(`feed 不是合法 JSON（前 120 字节: ${text.slice(0, 120)}）`);
      return;
    }
    // 版本尚未切换时也可能只是 CDN 传播延迟 —— 有界重试后再判
    const want = args.expectVersion ?? latestVersion;
    if (feed.version !== want && attempt < args.retries - 1) {
      info(
        `第 ${attempt + 1} 次: version=${feed.version} 期望 ${want}，` +
          `${args.retryDelayMs / 1000}s 后重试（CDN 传播 / 尚未发布）`
      );
      await sleep(args.retryDelayMs);
      continue;
    }
    break;
  }
  if (!feed) {
    fail("多次重试后仍未取到 feed");
    return;
  }
  ok(`feed 可取到且是合法 JSON（version = ${feed.version}）`);

  // ---- ③ 版本一致性
  console.log("");
  console.log("【3】版本一致性");
  if (args.expectVersion && feed.version !== args.expectVersion) {
    fail(
      `feed version = ${feed.version}，期望 ${args.expectVersion}`,
      "若刚发版：草稿可能还没点 Publish（GitHub 的 latest 不含草稿），或 CDN 仍在传播。"
    );
  } else if (!args.expectVersion && feed.version !== latestVersion) {
    fail(
      `feed version = ${feed.version} 与最新 release tag ${latest.tag_name} 不一致`,
      "「latest 指向的 release」与「feed 里的版本」必须是同一版；不一致说明有多个 release 争用同一 tag。"
    );
  } else {
    ok(`feed version = ${feed.version} 与最新 release 一致`);
  }

  // ---- ④ 平台键与签名
  console.log("");
  console.log("【4】平台键与 updater 签名");
  const platforms = feed.platforms ?? {};
  const keys = Object.keys(platforms);
  const missing = EXPECTED_PLATFORMS.filter((k) => !keys.includes(k));
  if (missing.length === 0) {
    ok(`平台键齐全（${keys.length} 个）`);
  } else {
    fail(
      `缺少平台键: ${missing.join(", ")}`,
      `期望集 ${EXPECTED_PLATFORMS.join(", ")}。macOS 目前只出 aarch64（Intel 缺口见 release.md §1），` +
        "若这是刻意变更，请同步本脚本的 EXPECTED_PLATFORMS。"
    );
  }
  const noSig = keys.filter((k) => !String(platforms[k]?.signature ?? "").trim());
  if (noSig.length === 0 && keys.length > 0) ok("每个平台都有非空 signature");
  else if (keys.length === 0) fail("platforms 为空");
  else fail(`以下平台 signature 为空: ${noSig.join(", ")}`, "缺签名则客户端会拒绝该更新包");

  // ---- ⑤ 资产真的能取到
  console.log("");
  console.log("【5】资产可达性（Range 取首字节，不下整包）");
  for (const k of keys) {
    const url = platforms[k]?.url;
    if (!url) {
      fail(`${k}: 缺 url`);
      continue;
    }
    try {
      const st = await assetReachable(url);
      if (st === 200 || st === 206) ok(`${k} → HTTP ${st}`);
      else fail(`${k} → HTTP ${st}`, `资产不可达: ${url}`);
    } catch (e) {
      fail(`${k} → 请求失败: ${e.message}`, url);
    }
  }

  console.log("");
  // 注意：**不在这里**判退出码 —— 上面的早期 `return` 会跳过它。
  // 统一交给最后的 `finish()`（见文件末尾），否则「判据红了但退出码 0」会让 CI 假绿。
}

/** 统一收尾：打印结论并据 `failures` 决定进程退出码（任何路径都必须经过这里）。 */
function finish() {
  console.log("────────────────────────────────────────────");
  if (failures === 0) {
    console.log(`更新通道健康（${checks} 项判据全过）`);
    process.exit(0);
  }
  console.log(`更新通道不健康：${failures}/${checks} 项判据未通过`);
  process.exit(1);
}

run().then(finish, (e) => {
  console.error(`检查过程本身出错（不是通道故障）：${e?.stack ?? e}`);
  process.exit(2);
});
