import { useEffect, useRef, useState } from "react";
import { api } from "./api";
import type {
  GatewayKeyInfo,
  LogRowView,
  ModelRow,
  ProviderDto,
} from "./types";

// ---------------------------------------------------------------- 全局 toast

type ToastKind = "ok" | "err";

function toast(msg: string, kind: ToastKind = "ok") {
  window.dispatchEvent(new CustomEvent("jai-toast", { detail: { msg, kind } }));
}

function copyText(text: string) {
  navigator.clipboard?.writeText(text).then(
    () => toast("已复制"),
    () => toast("复制失败", "err")
  );
}

function goTab(tab: string) {
  window.dispatchEvent(new CustomEvent("jai-goto-tab", { detail: { tab } }));
}

// ---------------------------------------------------------------- 通用小组件

function Card({
  title,
  children,
}: {
  title: string;
  children: React.ReactNode;
}) {
  return (
    <div className="rounded-lg border border-neutral-800 bg-neutral-900/60 p-4">
      <h2 className="mb-3 text-sm font-semibold tracking-wide text-neutral-300">
        {title}
      </h2>
      {children}
    </div>
  );
}

const inputCls =
  "w-full rounded border border-neutral-700 bg-neutral-950 px-2 py-1.5 text-sm text-neutral-100 outline-none focus:border-amber-500";
const btnCls =
  "rounded px-3 py-1.5 text-sm font-medium transition-colors disabled:cursor-not-allowed disabled:opacity-40";
const btnPrimary = `${btnCls} bg-amber-600 text-black hover:bg-amber-500`;
const btnGhost = `${btnCls} border border-neutral-700 text-neutral-300 hover:border-neutral-500`;
const btnDanger = `${btnCls} border border-red-900/60 text-red-400 hover:bg-red-950`;

function fmtClock(ms: number | null | undefined): string {
  if (!ms) return "—";
  return new Date(ms).toLocaleString("zh-CN", {
    hour12: false,
    month: "2-digit",
    day: "2-digit",
    hour: "2-digit",
    minute: "2-digit",
  });
}

// ---------------------------------------------------------------- 页面

type Tab = "gateway" | "sync" | "mcp" | "skills" | "providers" | "models" | "logs" | "settings";

export default function App() {
  const [tab, setTab] = useState<Tab>("gateway");
  const [toastMsg, setToastMsg] = useState<{ msg: string; kind: ToastKind } | null>(null);
  const tabs: [Tab, string][] = [
    ["gateway", "网关"],
    ["sync", "同步"],
    ["mcp", "MCP"],
    ["skills", "技能"],
    ["providers", "供应商"],
    ["models", "模型"],
    ["logs", "日志"],
    ["settings", "设置"],
  ];

  useEffect(() => {
    const onToast = (e: Event) => {
      const detail = (e as CustomEvent).detail as { msg: string; kind: ToastKind };
      setToastMsg({ msg: detail.msg, kind: detail.kind ?? "ok" });
      window.setTimeout(() => setToastMsg(null), 2500);
    };
    const onGoto = (e: Event) => {
      const detail = (e as CustomEvent).detail as { tab: string };
      setTab(detail.tab as Tab);
    };
    window.addEventListener("jai-toast", onToast);
    window.addEventListener("jai-goto-tab", onGoto);
    return () => {
      window.removeEventListener("jai-toast", onToast);
      window.removeEventListener("jai-goto-tab", onGoto);
    };
  }, []);

  return (
    <div className="flex h-screen flex-col bg-neutral-950 text-neutral-200">
      {/* 顶栏 */}
      <header className="flex items-center gap-1 border-b border-neutral-800 px-4 py-2">
        <span className="mr-4 font-mono text-sm font-bold text-amber-500">
          JAI
        </span>
        {tabs.map(([k, label]) => (
          <button
            key={k}
            onClick={() => setTab(k)}
            className={`rounded px-3 py-1.5 text-sm ${
              tab === k
                ? "bg-neutral-800 font-medium text-white"
                : "text-neutral-400 hover:text-neutral-200"
            }`}
          >
            {label}
          </button>
        ))}
      </header>

      <main className="flex-1 space-y-4 overflow-y-auto p-6">
        {tab === "gateway" && <GatewayTab />}
        {tab === "sync" && <SyncTab />}
        {tab === "mcp" && <McpTab />}
        {tab === "skills" && <SkillsTab />}
        {tab === "providers" && <ProvidersTab />}
        {tab === "models" && <ModelsTab />}
        {tab === "logs" && <LogsTab />}
        {tab === "settings" && <SettingsTab />}
      </main>

      {toastMsg && (
        <div
          className={`fixed bottom-6 left-1/2 z-50 -translate-x-1/2 rounded-lg border px-4 py-2 text-sm shadow-lg ${
            toastMsg.kind === "err"
              ? "border-red-800 bg-red-950 text-red-200"
              : "border-emerald-800 bg-emerald-950 text-emerald-200"
          }`}
        >
          {toastMsg.msg}
        </div>
      )}
    </div>
  );
}

// ---------------------------------------------------------------- 网关

function GatewayTab() {
  const [status, setStatus] = useState<import("./types").GwStatus | null>(null);
  const [key, setKey] = useState<GatewayKeyInfo | null>(null);
  const [revealKey, setRevealKey] = useState<string>("");
  const [busy, setBusy] = useState(false);
  const [err, setErr] = useState("");
  const [sdk, setSdk] = useState<"deepseek" | "openai" | "curl">("deepseek");

  const port = status?.port ?? 1314;
  const baseUrl = `http://127.0.0.1:${port}/v1`;
  const fullKey = revealKey || (key ? `${key.prefix}…` : "");

  async function refresh() {
    try {
      setStatus(await api.status());
    } catch (e) {
      setErr(String(e));
    }
  }

  useEffect(() => {
    refresh();
    const t = setInterval(refresh, 2000);
    return () => clearInterval(t);
  }, []);

  useEffect(() => {
    api.gatewayKeyInfo().then((k) => setKey(k)).catch(() => {});
  }, []);

  async function toggle(run: boolean) {
    setBusy(true);
    setErr("");
    try {
      setStatus(run ? await api.start() : await api.stop());
    } catch (e) {
      setErr(String(e));
    } finally {
      setBusy(false);
    }
  }

  async function doReveal() {
    if (revealKey) {
      setRevealKey("");
      return;
    }
    const full = await api.gatewayKeyReveal();
    setRevealKey(full.key);
  }

  async function doRegen() {
    if (!confirm("轮换密钥：旧密钥将立即失效，已配置的客户端需要更新。继续？")) return;
    const k = await api.gatewayKeyRegenerate();
    setKey(k);
    setRevealKey(k.key);
    toast("密钥已轮换");
  }

  async function doExport() {
    const path = await api.exportConfigToFile();
    toast("已导出（不含 API Key）");
    try {
      await api.revealInFolder(path);
    } catch {
      // 平台不支持打开目录时忽略
    }
  }

  const snippet =
    sdk === "deepseek"
      ? `# DeepSeek Harness（dsh）
export DEEPSEEK_API_KEY=${fullKey || "sk-jai-…"}
# dsh 的 provider 配置里 baseURL 填 ${baseUrl}`
      : sdk === "openai"
        ? `# OpenAI SDK
OPENAI_API_BASE=${baseUrl}
OPENAI_API_KEY=${fullKey || "sk-jai-…"}`
        : `curl ${baseUrl}/chat/completions \\
  -H "Authorization: Bearer ${fullKey || "sk-jai-…"}" \\
  -H "Content-Type: application/json" \\
  -d '{"model":"deepseek-v4-flash","messages":[{"role":"user","content":"hi"}]}'`;

  return (
    <div className="mx-auto max-w-2xl space-y-4">
      {err && (
        <div className="rounded border border-red-900 bg-red-950/40 px-3 py-2 text-sm text-red-300">
          {err}
        </div>
      )}

      <Card title="网关状态">
        <div className="flex flex-wrap items-center gap-3">
          <span
            className={`inline-block h-3 w-3 rounded-full ${
              status?.running ? "bg-emerald-500" : "bg-neutral-600"
            }`}
          />
          {status?.running ? (
            <>
              <span className="rounded bg-emerald-950 px-2 py-0.5 text-xs font-semibold text-emerald-400">
                运行中
              </span>
              <span className="font-mono text-lg font-bold text-white">
                127.0.0.1:{port}
              </span>
            </>
          ) : (
            <>
              <span className="rounded bg-neutral-800 px-2 py-0.5 text-xs font-semibold text-neutral-400">
                已停止
              </span>
              <span className="text-lg font-semibold text-neutral-500">
                未运行
              </span>
            </>
          )}
          <div className="ml-auto flex gap-2">
            {status?.running ? (
              <button
                className={btnDanger}
                disabled={busy}
                onClick={() => toggle(false)}
              >
                停止
              </button>
            ) : (
              <button
                className={btnPrimary}
                disabled={busy}
                onClick={() => toggle(true)}
              >
                启动
              </button>
            )}
          </div>
        </div>
        <p className="mt-3 text-xs leading-relaxed text-neutral-600" title="端口被占用时从 1314 起自动顺延">
          端口被占用会自动顺延（1314 起）。所有业务端点强制鉴权：Host 仅接受本机回环地址，浏览器跨域需在「设置」中添加白名单。
        </p>
      </Card>

      <Card title="客户端接入">
        <div className="space-y-3 text-sm">
          <div>
            <div className="mb-1 text-neutral-400">Base URL</div>
            <div className="flex items-center gap-2">
              <code className="flex-1 rounded bg-black/60 px-3 py-2 font-mono text-emerald-400">
                {baseUrl}
              </code>
              <button className={btnGhost} onClick={() => copyText(baseUrl)} title="复制 Base URL">
                复制
              </button>
            </div>
          </div>
          <div>
            <div className="mb-1 text-neutral-400">API Key</div>
            <div className="flex items-center gap-2">
              <code className="flex-1 truncate rounded bg-black/60 px-3 py-2 font-mono text-emerald-400">
                {revealKey
                  ? fullKey
                  : key
                    ? `${key.prefix}…（点击右侧显示全文）`
                    : "加载中"}
              </code>
              <button className={btnGhost} onClick={() => revealKey && copyText(revealKey)} disabled={!revealKey} title="复制 API Key">
                复制
              </button>
              <button className={btnGhost} onClick={doReveal}>
                {revealKey ? "隐藏" : "显示"}
              </button>
              <button className={btnDanger} onClick={doRegen}>
                轮换密钥
              </button>
            </div>
          </div>
          <div>
            <div className="mb-1 flex items-center justify-between">
              <span className="text-neutral-400">接入示例</span>
              <select
                className="rounded border border-neutral-700 bg-neutral-950 px-2 py-1 text-xs text-neutral-300 outline-none focus:border-amber-500"
                value={sdk}
                onChange={(e) => setSdk(e.target.value as typeof sdk)}
              >
                <option value="deepseek">DeepSeek Harness</option>
                <option value="openai">OpenAI SDK</option>
                <option value="curl">curl</option>
              </select>
            </div>
            <pre className="relative mt-1 overflow-x-auto rounded bg-black/60 p-3 font-mono text-xs leading-relaxed text-neutral-300">
              <button
                className="absolute right-2 top-2 rounded border border-neutral-700 px-2 py-0.5 text-[10px] text-neutral-400 hover:text-neutral-200"
                onClick={() => copyText(snippet)}
              >
                复制
              </button>
              {snippet}
            </pre>
          </div>
        </div>
      </Card>

      <Card title="配置迁移">
        <div className="flex flex-wrap items-center gap-2">
          <button className={btnGhost} onClick={doExport}>
            导出配置（无敏感字段）
          </button>
          <span className="text-xs text-neutral-500">
            密钥不会包含在导出文件中；导入后需在「供应商」页补录 API Key。
          </span>
        </div>
      </Card>
    </div>
  );
}

// ---------------------------------------------------------------- 同步（M7：导入 + WebDAV）

function SyncTab() {
  const [importText, setImportText] = useState("");
  const [cfg, setCfg] = useState<{ url: string; username: string; directory: string }>({
    url: "",
    username: "",
    directory: "",
  });
  const [pw, setPw] = useState("");
  const [showPw, setShowPw] = useState(false);
  const [msg, setMsg] = useState("");
  const [err, setErr] = useState("");
  const [busy, setBusy] = useState("");

  useEffect(() => {
    api
      .webdavConfigGet()
      .then((c) => {
        if (c) setCfg(c);
      })
      .catch(() => {});
  }, []);

  async function doImport() {
    setErr("");
    if (!importText.trim()) {
      setErr("请先粘贴 JSON 内容");
      return;
    }
    if (!confirm("导入会将 JSON 合并进当前本地配置，确定继续？")) return;
    try {
      const r = await api.configImport(importText, false);
      setMsg(
        `导入完成：新增供应商 ${r.providersImported}，重复跳过 ${r.providersSkippedDuplicate}，模型 ${r.modelsImported}；待补密钥：${r.missingKeys.join("、") || "无"}`,
      );
    } catch (e) {
      setErr(String(e));
    }
  }

  async function saveCfg() {
    setErr("");
    const url = cfg.url.trim();
    if (!url) {
      setErr("WebDAV 根地址不能为空");
      return;
    }
    const normalized = url.endsWith("/") ? url : `${url}/`;
    try {
      await api.webdavConfigSet({ ...cfg, url: normalized, password: pw || null });
      setCfg({ ...cfg, url: normalized });
      setPw("");
      setMsg("WebDAV 连接配置已保存");
    } catch (e) {
      setErr(String(e));
    }
  }

  async function testWebdav() {
    setErr("");
    setMsg("");
    const url = cfg.url.trim();
    if (!url) {
      setErr("WebDAV 根地址不能为空");
      return;
    }
    try {
      const result = await api.webdavTest({
        url,
        username: cfg.username,
        password: pw || null,
      });
      setMsg(result);
      toast("WebDAV 连接成功");
    } catch (e) {
      setErr(String(e));
      toast("WebDAV 连接失败", "err");
    }
  }

  async function doPush() {
    setBusy("push");
    setErr("");
    try {
      await api.webdavPush();
      setMsg("已推送到 WebDAV，推送前本地快照已留存");
    } catch (e) {
      setErr(String(e));
    } finally {
      setBusy("");
    }
  }

  async function doPull() {
    setBusy("pull");
    setErr("");
    try {
      const r = await api.webdavPull();
      setMsg(
        `拉取并导入完成：新增供应商 ${r.providersImported}，模型 ${r.modelsImported}；待补密钥：${r.missingKeys.join("、") || "无"}`,
      );
    } catch (e) {
      setErr(String(e));
    } finally {
      setBusy("");
    }
  }

  return (
    <div className="mx-auto max-w-2xl space-y-4">
      <h1 className="text-lg font-semibold">配置同步</h1>
      {err && (
        <div className="rounded border border-red-900 bg-red-950/40 px-3 py-2 text-sm text-red-300">
          {err}
        </div>
      )}
      {msg && (
        <div className="rounded border border-emerald-900 bg-emerald-950/40 px-3 py-2 text-sm text-emerald-300">
          {msg}
        </div>
      )}

      <Card title="导入导出 JSON">
        <p className="mb-3 text-xs leading-relaxed text-neutral-500">
          导出产物只含供应商与模型定义，不含任何 API Key。导入后请在「供应商」页逐项补录凭据。
        </p>
        <textarea
          className={`${inputCls} h-32 font-mono`}
          value={importText}
          onChange={(e) => setImportText(e.target.value)}
          placeholder="粘贴从另一台设备导出的 jai-export JSON…"
        />
        <div className="mt-3 flex flex-wrap items-center gap-2">
          <button className={btnPrimary} onClick={doImport}>
            导入
          </button>
          <button
            className={btnGhost}
            onClick={async () => {
              try {
                const text = await navigator.clipboard.readText();
                setImportText(text);
                toast("已粘贴");
              } catch {
                toast("无法读取剪贴板", "err");
              }
            }}
          >
            粘贴
          </button>
          <span className="text-xs text-neutral-500">
            导出入口仍在「网关」页。
          </span>
          <button className={btnGhost} onClick={() => goTab("gateway")}>
            前往网关页 →
          </button>
        </div>
      </Card>

      <Card title="WebDAV 同步">
        <p className="mb-3 text-xs leading-relaxed text-neutral-500">
          手动推/拉：推送使用本机当前配置覆盖远端；拉取使用远端配置覆盖本机
          （last-write-wins）。推送前会在本地留存一份快照。
        </p>
        <div className="grid grid-cols-2 gap-3">
          <label className="text-xs text-neutral-400">
            WebDAV 根地址
            <input
              className={`${inputCls} mt-1 font-mono`}
              value={cfg.url}
              onChange={(e) => setCfg({ ...cfg, url: e.target.value })}
              placeholder="https://dav.example.com/remote.php/dav/files/u"
            />
          </label>
          <label className="text-xs text-neutral-400">
            用户名
            <input
              className={`${inputCls} mt-1`}
              value={cfg.username}
              onChange={(e) => setCfg({ ...cfg, username: e.target.value })}
            />
          </label>
          <label className="text-xs text-neutral-400">
            目录（可选）
            <input
              className={`${inputCls} mt-1 font-mono`}
              value={cfg.directory}
              onChange={(e) => setCfg({ ...cfg, directory: e.target.value })}
              placeholder="jai/backups"
            />
          </label>
          <label className="text-xs text-neutral-400">
            <span className="flex items-center justify-between">
              <span>密码（存入钥匙串，留空保持原密码）</span>
              <button
                className={btnGhost}
                type="button"
                onClick={() => setShowPw(!showPw)}
              >
                {showPw ? "隐藏" : "显示"}
              </button>
            </span>
            <input
              className={`${inputCls} mt-1`}
              type={showPw ? "text" : "password"}
              value={pw}
              onChange={(e) => setPw(e.target.value)}
            />
          </label>
        </div>
        <div className="mt-4 flex flex-wrap gap-2">
          <button className={btnPrimary} onClick={saveCfg}>
            保存配置
          </button>
          <button className={btnGhost} onClick={testWebdav}>
            测试连接
          </button>
          <button className={btnGhost} disabled={busy === "push"} onClick={doPush}>
            {busy === "push" ? "推送中…" : "推送"}
          </button>
          <button className={btnGhost} disabled={busy === "pull"} onClick={doPull}>
            {busy === "pull" ? "拉取中…" : "拉取"}
          </button>
        </div>
        {busy && (
          <div className="mt-3 h-1 w-full overflow-hidden rounded bg-neutral-800">
            <div className="h-full w-1/3 animate-pulse rounded bg-amber-500" />
          </div>
        )}
      </Card>
    </div>
  );
}

// ---------------------------------------------------------------- MCP 管理

function McpTab() {
  const [list, setList] = useState<import("./types").McpServerRow[]>([]);
  const [msg, setMsg] = useState("");
  const [err, setErr] = useState("");
  const [showNew, setShowNew] = useState(false);

  async function refresh() {
    setList(await api.mcpList());
  }
  useEffect(() => {
    refresh().catch((e) => setErr(String(e)));
  }, []);

  async function act(fn: () => Promise<unknown>) {
    setErr("");
    try {
      await fn();
      await refresh();
    } catch (e) {
      setErr(String(e));
    }
  }

  return (
    <div className="mx-auto max-w-3xl space-y-4">
      <div className="flex items-center justify-between">
        <h1 className="text-lg font-semibold">MCP Server 管理</h1>
        <button className={btnPrimary} onClick={() => setShowNew(true)}>
          + 添加 MCP Server
        </button>
      </div>
      {err && (
        <div className="rounded border border-red-900 bg-red-950/40 px-3 py-2 text-sm text-red-300">
          {err}
        </div>
      )}
      {msg && (
        <div className="rounded border border-emerald-900 bg-emerald-950/40 px-3 py-2 text-sm text-emerald-300">
          {msg}
        </div>
      )}
      {showNew && (
        <McpForm
          onCancel={() => setShowNew(false)}
          onDone={() => {
            setShowNew(false);
            refresh();
          }}
        />
      )}
      <div className="space-y-3">
        {list.map((m) => (
          <div key={m.id} className="rounded-lg border border-neutral-800 bg-neutral-900/60 p-4">
            <div className="flex items-center gap-3">
              <input
                type="checkbox"
                checked={m.enabled}
                onChange={(e) => act(() => api.mcpSetEnabled(m.id, e.target.checked))}
                className="accent-amber-500"
              />
              <div className="min-w-0 flex-1">
                <div className="flex items-baseline gap-2">
                  <span className="font-medium">{m.name}</span>
                  <span className="rounded bg-neutral-800 px-1.5 py-0.5 text-[11px] text-neutral-400">
                    {m.kind}
                  </span>
                  {!m.enabled && <span className="text-[11px] text-neutral-500">已停用</span>}
                </div>
                <div className="truncate font-mono text-xs text-neutral-500">
                  {m.kind === "stdio"
                    ? `${m.command ?? ""} ${m.args ?? ""}`
                    : m.url ?? ""}
                </div>
              </div>
              <button
                className={btnGhost}
                onClick={() =>
                  act(async () => {
                    await api.mcpToolsList(m.id);
                    toast(`MCP「${m.name}」连接正常`);
                  })
                }
              >
                测试连接
              </button>
              <button
                className={btnGhost}
                onClick={() =>
                  act(async () => {
                    const tools = await api.mcpToolsList(m.id);
                    setMsg(
                      tools.length
                        ? `${m.name} 工具：${tools.map((t) => t.name).join("、")}`
                        : `${m.name} 未暴露工具`,
                    );
                  })
                }
              >
                列出工具
              </button>
              <button
                className={btnDanger}
                onClick={() => {
                  if (!confirm(`删除 MCP Server「${m.name}」？`)) return;
                  act(() => api.mcpDelete(m.id));
                }}
              >
                删除
              </button>
            </div>
            <McpForm
              initial={m}
              onCancel={() => {}}
              onDone={() => {
                setMsg("MCP Server 已更新");
                setTimeout(() => setMsg(""), 2000);
                refresh();
              }}
            />
          </div>
        ))}
        {list.length === 0 && !showNew && (
          <Card title="还没有 MCP Server">
            <p className="text-sm text-neutral-500">
              点击上方按钮添加一个 stdio / SSE / HTTP 类型的服务。
            </p>
          </Card>
        )}
      </div>
    </div>
  );
}

function McpForm({
  initial,
  onDone,
  onCancel,
}: {
  initial?: import("./types").McpServerRow;
  onDone: () => void;
  onCancel: () => void;
}) {
  const [name, setName] = useState(initial?.name ?? "");
  const [kind, setKind] = useState<string>(initial?.kind ?? "stdio");
  const [command, setCommand] = useState(initial?.command ?? "");
  const [args, setArgs] = useState(initial?.args ?? "");
  const [url, setUrl] = useState(initial?.url ?? "");
  const [argsErr, setArgsErr] = useState("");

  async function submit() {
    if (kind === "stdio" && args.trim()) {
      try {
        const parsed = JSON.parse(args);
        if (!Array.isArray(parsed)) throw new Error("必须是数组");
        setArgsErr("");
      } catch {
        setArgsErr("参数需为合法 JSON 数组，例如 [\"-y\",\"包名\"]");
        return;
      }
    }
    if (initial) {
      await api.mcpUpdate({
        id: initial.id,
        name,
        kind,
        command: command || null,
        args: args || null,
        url: url || null,
      });
    } else {
      await api.mcpCreate({
        name,
        kind,
        command: command || null,
        args: args || null,
        url: url || null,
      });
    }
    onDone();
  }

  return (
    <div className="mt-3 grid grid-cols-1 gap-3 rounded border-t border-neutral-800 pt-4 md:grid-cols-2">
      <label className="text-xs text-neutral-400">
        名称
        <input className={`${inputCls} mt-1`} value={name} onChange={(e) => setName(e.target.value)} />
      </label>
      <label className="text-xs text-neutral-400">
        类型
        <select className={`${inputCls} mt-1`} value={kind} onChange={(e) => setKind(e.target.value)}>
          <option value="stdio">stdio</option>
          <option value="sse">sse</option>
          <option value="http">http</option>
        </select>
      </label>
      {kind === "stdio" ? (
        <>
          <label className="text-xs text-neutral-400">
            命令
            <input className={`${inputCls} mt-1 font-mono`} value={command} onChange={(e) => setCommand(e.target.value)} placeholder="npx / node / python" />
          </label>
          <label className="text-xs text-neutral-400">
            参数（JSON 数组）
            <input className={`${inputCls} mt-1 font-mono`} value={args} onChange={(e) => { setArgs(e.target.value); setArgsErr(""); }} placeholder='["-y","@modelcontextprotocol/server-filesystem"]' />
            {argsErr && <span className="mt-1 block text-[11px] text-red-400">{argsErr}</span>}
          </label>
        </>
      ) : (
        <label className="col-span-2 text-xs text-neutral-400">
          URL
          <input className={`${inputCls} mt-1 font-mono`} value={url} onChange={(e) => setUrl(e.target.value)} placeholder="https://mcp.example.com/sse" />
        </label>
      )}
      <div className="col-span-2 flex gap-2">
        <button className={btnPrimary} onClick={submit}>
          {initial ? "保存" : "创建"}
        </button>
        {!initial && (
          <button className={btnGhost} onClick={onCancel}>
            取消
          </button>
        )}
      </div>
    </div>
  );
}

// ---------------------------------------------------------------- 技能（Skill）管理

function SkillsTab() {
  const [list, setList] = useState<import("./types").SkillRow[]>([]);
  const [err, setErr] = useState("");
  const [msg, setMsg] = useState("");
  const [showNew, setShowNew] = useState(false);
  const [importing, setImporting] = useState(false);
  const [dragOver, setDragOver] = useState(false);
  const [selected, setSelected] = useState<Set<string>>(new Set());
  const fileRef = useRef<HTMLInputElement>(null);

  async function refresh() {
    setList(await api.skillList());
  }
  useEffect(() => {
    refresh().catch((e) => setErr(String(e)));
  }, []);

  function toggleSelect(id: string) {
    setSelected((prev) => {
      const next = new Set(prev);
      if (next.has(id)) next.delete(id);
      else next.add(id);
      return next;
    });
  }

  async function batchSetEnabled(enabled: boolean) {
    for (const id of selected) {
      await api.skillSetEnabled(id, enabled);
    }
    setSelected(new Set());
    toast(enabled ? "已批量启用" : "已批量禁用");
    await refresh();
  }

  async function batchDelete() {
    if (!confirm(`确定删除选中的 ${selected.size} 个技能？`)) return;
    for (const id of selected) {
      await api.skillDelete(id);
    }
    setSelected(new Set());
    toast("已批量删除");
    await refresh();
  }

  async function act(fn: () => Promise<unknown>) {
    setErr("");
    try {
      await fn();
      await refresh();
    } catch (e) {
      setErr(String(e));
    }
  }

  async function importFile(file: File) {
    if (!file.name.toLowerCase().endsWith(".zip")) {
      setErr("请选择 ZIP 文件");
      return;
    }
    setImporting(true);
    setErr("");
    setMsg("");
    try {
      const buf = await file.arrayBuffer();
      const data = Array.from(new Uint8Array(buf));
      const n = await api.skillImportZip(data);
      setMsg(`已从 ${file.name}（${(file.size / 1024).toFixed(1)} KB）导入 ${n} 个技能`);
      await refresh();
    } catch (e2) {
      setErr(String(e2));
    } finally {
      setImporting(false);
    }
  }

  async function importZip(e: React.ChangeEvent<HTMLInputElement>) {
    const file = e.target.files?.[0];
    e.target.value = "";
    if (file) await importFile(file);
  }

  async function createExample() {
    await api.skillCreate({
      name: "代码审查",
      description: "一个示例技能：按提交变更做代码评审",
      content: "请按以下步骤进行代码审查：\n1. 阅读 diff\n2. 指出风险\n3. 给出修改建议",
    });
    toast("示例技能已创建");
    await refresh();
  }

  return (
    <div
      className={`mx-auto max-w-3xl space-y-4 ${dragOver ? "opacity-80" : ""}`}
      onDragOver={(e) => {
        e.preventDefault();
        setDragOver(true);
      }}
      onDragLeave={() => setDragOver(false)}
      onDrop={(e) => {
        e.preventDefault();
        setDragOver(false);
        const file = e.dataTransfer.files?.[0];
        if (file) importFile(file);
      }}
    >
      <div className="flex items-center justify-between">
        <h1 className="text-lg font-semibold">技能（Skill）管理</h1>
        <div className="flex gap-2">
          <input
            ref={fileRef}
            type="file"
            accept=".zip,application/zip"
            className="hidden"
            onChange={importZip}
          />
          <button
            className={btnGhost}
            disabled={importing}
            onClick={() => fileRef.current?.click()}
          >
            {importing ? "导入中…" : "导入 ZIP"}
          </button>
          <button className={btnPrimary} onClick={() => setShowNew(true)}>
            + 添加技能
          </button>
        </div>
      </div>
      {err && (
        <div className="rounded border border-red-900 bg-red-950/40 px-3 py-2 text-sm text-red-300">
          {err}
        </div>
      )}
      {msg && (
        <div className="rounded border border-emerald-900 bg-emerald-950/40 px-3 py-2 text-sm text-emerald-300">
          {msg}
        </div>
      )}
      {selected.size > 0 && (
        <div className="flex flex-wrap items-center gap-2 rounded border border-amber-800 bg-amber-950/30 px-3 py-2 text-sm text-amber-200">
          已选 {selected.size} 项
          <button className={btnGhost} onClick={() => batchSetEnabled(true)}>
            批量启用
          </button>
          <button className={btnGhost} onClick={() => batchSetEnabled(false)}>
            批量禁用
          </button>
          <button className={btnDanger} onClick={batchDelete}>
            批量删除
          </button>
          <button className={btnGhost} onClick={() => setSelected(new Set())}>
            取消选择
          </button>
        </div>
      )}
      {showNew && (
        <div
          className="fixed inset-0 z-40 flex items-start justify-center bg-black/60 p-6 pt-16"
          onClick={() => setShowNew(false)}
        >
          <div className="w-full max-w-2xl" onClick={(e) => e.stopPropagation()}>
            <SkillForm
              onCancel={() => setShowNew(false)}
              onDone={() => {
                setShowNew(false);
                refresh();
              }}
            />
          </div>
        </div>
      )}
      <div className="space-y-3">
        {list.map((s) => (
          <div key={s.id} className="rounded-lg border border-neutral-800 bg-neutral-900/60 p-4">
            <div className="flex items-center gap-3">
              <input
                type="checkbox"
                checked={selected.has(s.id)}
                onChange={() => toggleSelect(s.id)}
                className="accent-amber-500"
                title="选择用于批量操作"
              />
              <input
                type="checkbox"
                checked={s.enabled}
                onChange={(e) => act(() => api.skillSetEnabled(s.id, e.target.checked))}
                className="accent-emerald-500"
                title="启用/停用"
              />
              <div className="min-w-0 flex-1">
                <div className="font-medium">{s.name}</div>
                <div className="truncate text-xs text-neutral-500">{s.description}</div>
                <div className="mt-0.5 text-[11px] text-neutral-600">
                  更新于 {new Date(s.updatedAt).toLocaleString()}
                </div>
              </div>
              <button
                className={btnDanger}
                onClick={() => {
                  if (!confirm(`删除技能「${s.name}」？`)) return;
                  act(() => api.skillDelete(s.id));
                }}
              >
                删除
              </button>
            </div>
            <SkillForm
              initial={s}
              onCancel={() => {}}
              onDone={() => {
                setMsg("技能已更新");
                setTimeout(() => setMsg(""), 2000);
                refresh();
              }}
            />
          </div>
        ))}
        {list.length === 0 && !showNew && (
          <Card title="还没有技能">
            <p className="text-sm text-neutral-500">
              技能是一组可复用的指令/提示词/工作流定义，点击添加或导入 ZIP 开始。
            </p>
            <button className={`${btnGhost} mt-3`} onClick={createExample}>
              ✨ 创建示例技能
            </button>
          </Card>
        )}
      </div>
    </div>
  );
}

function SkillForm({
  initial,
  onDone,
  onCancel,
}: {
  initial?: import("./types").SkillRow;
  onDone: () => void;
  onCancel: () => void;
}) {
  const [name, setName] = useState(initial?.name ?? "");
  const [description, setDescription] = useState(initial?.description ?? "");
  const [content, setContent] = useState(initial?.content ?? "");

  async function submit() {
    if (initial) {
      await api.skillUpdate({ id: initial.id, name, description, content });
    } else {
      await api.skillCreate({ name, description, content });
    }
    onDone();
  }

  return (
    <div className="mt-3 grid grid-cols-1 gap-3 rounded border-t border-neutral-800 pt-4 md:grid-cols-2">
      <label className="text-xs text-neutral-400">
        名称
        <input className={`${inputCls} mt-1`} value={name} onChange={(e) => setName(e.target.value)} />
      </label>
      <label className="text-xs text-neutral-400">
        描述
        <input className={`${inputCls} mt-1`} value={description} onChange={(e) => setDescription(e.target.value)} />
      </label>
      <label className="col-span-2 text-xs text-neutral-400">
        内容/指令
        <textarea className={`${inputCls} mt-1 h-28 font-mono`} value={content} onChange={(e) => setContent(e.target.value)} />
      </label>
      <div className="col-span-2 flex gap-2">
        <button className={btnPrimary} onClick={submit}>
          {initial ? "保存" : "创建"}
        </button>
        {!initial && (
          <button className={btnGhost} onClick={onCancel}>
            取消
          </button>
        )}
      </div>
    </div>
  );
}

// ---------------------------------------------------------------- 供应商

const FAMILY_LABEL: Record<string, string> = {
  openai_compat: "OpenAI 兼容",
  openai_responses: "OpenAI Responses",
  anthropic: "Anthropic",
  gemini: "Gemini",
};

function ProvidersTab() {
  const [list, setList] = useState<ProviderDto[]>([]);
  const [busy, setBusy] = useState("");
  const [msg, setMsg] = useState<{ id: string; ok: boolean; text: string } | null>(null);
  const [showNew, setShowNew] = useState(false);

  async function refresh() {
    setList(await api.providerList());
  }
  useEffect(() => {
    refresh().catch((e) => setMsg({ id: "", ok: false, text: String(e) }));
    const onRefresh = () => refresh().catch(() => {});
    window.addEventListener("jai-refresh-providers", onRefresh);
    return () => window.removeEventListener("jai-refresh-providers", onRefresh);
  }, []);

  async function act(id: string, fn: () => Promise<unknown>) {
    setBusy(id);
    setMsg(null);
    try {
      await fn();
      await refresh();
    } catch (e) {
      setMsg({ id, ok: false, text: String(e) });
    } finally {
      setBusy("");
    }
  }

  return (
    <div className="mx-auto max-w-3xl space-y-4">
      <div className="flex items-center justify-between">
        <h1 className="text-lg font-semibold">上游供应商</h1>
        <button className={btnPrimary} onClick={() => setShowNew(true)}>
          + 添加供应商
        </button>
      </div>

      {msg && !msg.id && (
        <div className="rounded border border-red-900 bg-red-950/40 px-3 py-2 text-sm text-red-300">
          {msg.text}
        </div>
      )}

      {!showNew && list.length === 0 && (
        <Card title="还没有供应商">
          <p className="text-sm text-neutral-500">
            添加第一个上游渠道 —— API Key 会立即写入系统钥匙串（macOS 钥匙串 /
            Windows 凭据管理器），数据库只保存引用地址，绝不落盘明文。
          </p>
        </Card>
      )}

      {showNew && (
        <NewProviderForm
          onCancel={() => setShowNew(false)}
          onDone={() => {
            setShowNew(false);
            refresh();
          }}
        />
      )}

      <div className="space-y-3">
        {list.map((p) => (
          <ProviderCard
            key={p.id}
            p={p}
            busy={busy === p.id}
            msg={msg?.id === p.id ? msg.text : null}
            onTest={() =>
              act(p.id, async () => {
                const t = await api.providerTest(p.id);
                setMsg({ id: p.id, ok: true, text: t });
              })
            }
            onDiscover={() =>
              act(p.id, async () => {
                const [total, added] = await api.providerDiscoverModels(p.id);
                setMsg({
                  id: p.id,
                  ok: true,
                  text: `发现 ${total} 个模型，新增 ${added} 个（已有模型不会被覆盖）`,
                });
              })
            }
            onToggle={(v) => act(p.id, () => api.providerSetEnabled(p.id, v))}
            onDelete={() => {
              if (!confirm(`删除供应商「${p.name}」？其模型映射与钥匙串凭据一并清除。`))
                return;
              act("", () => api.providerDelete(p.id));
            }}
          />
        ))}
      </div>
    </div>
  );
}

function ProviderCard(props: {
  p: ProviderDto;
  busy: boolean;
  msg: string | null;
  onTest: () => void;
  onDiscover: () => void;
  onToggle: (enabled: boolean) => void;
  onDelete: () => void;
}) {
  const { p } = props;
  const [editKey, setEditKey] = useState(false);

  return (
    <div className="rounded-lg border border-neutral-800 bg-neutral-900/60 p-4">
      <div className="flex items-center gap-3">
        <input
          type="checkbox"
          checked={p.enabled}
          onChange={(e) => props.onToggle(e.target.checked)}
          className="accent-amber-500"
          title="启用/禁用该渠道"
        />
        <div className="min-w-0 flex-1">
          <div className="flex items-baseline gap-2">
            <span className="font-medium">{p.name}</span>
            <span className="rounded bg-neutral-800 px-1.5 py-0.5 text-[11px] text-neutral-400">
              {FAMILY_LABEL[p.family] ?? p.family}
            </span>
            <span className="text-[11px] text-neutral-500">
              优先级 {p.priority}
            </span>
            {!p.hasKey && (
              <span className="text-[11px] text-amber-500">⚠ 缺少凭据</span>
            )}
            {p.lastOkAt && !p.lastErrAt && (
              <span className="flex items-center gap-1 text-[11px] text-emerald-500">
                <span className="inline-block h-1.5 w-1.5 rounded-full bg-emerald-500" />
                最近成功
              </span>
            )}
            {p.lastErrAt && (!p.lastOkAt || (p.lastErrAt ?? 0) > (p.lastOkAt ?? 0)) && (
              <span className="flex items-center gap-1 text-[11px] text-red-400">
                <span className="inline-block h-1.5 w-1.5 rounded-full bg-red-500" />
                最近失败
              </span>
            )}
          </div>
          <div className="truncate font-mono text-xs text-neutral-500">
            {p.baseUrl}
          </div>
          {(p.lastErrAt && (p.lastOkAt ?? 0) >= (p.lastErrAt ?? 0)) && (
            <div className="mt-1 text-[11px] text-emerald-600/70">
              最近成功：{fmtClock(p.lastOkAt)}
            </div>
          )}
          {(p.lastErrAt && (p.lastErrAt ?? 0) > (p.lastOkAt ?? 0)) && (
            <div className="mt-1 text-xs text-red-400">
              最近失败（{fmtClock(p.lastErrAt)}）：{p.lastErrMsg}
            </div>
          )}
          {props.msg && (
            <div
              className={`mt-1 text-xs ${
                props.msg.startsWith("连接成功") ? "text-emerald-400" : "text-red-400"
              }`}
            >
              {props.msg}
            </div>
          )}
        </div>
        <div className="flex shrink-0 gap-2">
          <button className={btnGhost} disabled={props.busy} onClick={props.onTest}>
            测试
          </button>
          <button className={btnGhost} disabled={props.busy} onClick={props.onDiscover}>
            拉取模型
          </button>
          <button
            className={btnGhost}
            onClick={() => {
              setEditKey(!editKey);
            }}
            title="替换 API Key / 修改地址"
          >
            编辑
          </button>
          <button className={btnDanger} onClick={props.onDelete}>
            删除
          </button>
        </div>
      </div>

      {editKey && (
        <EditProviderForm
          p={p}
          onDone={async () => {
            setEditKey(false);
            window.dispatchEvent(new CustomEvent("jai-refresh-providers"));
          }}
          onCancel={() => setEditKey(false)}
        />
      )}
    </div>
  );
}

function NewProviderForm({
  onDone,
  onCancel,
}: {
  onDone: () => void;
  onCancel: () => void;
}) {
  const [name, setName] = useState("");
  const [baseUrl, setBaseUrl] = useState("");
  const [family, setFamily] = useState("openai_compat");
  const [apiKey, setApiKey] = useState("");
  const [priority, setPriority] = useState(100);
  const [headerRows, setHeaderRows] = useState<{ key: string; value: string }[]>([
    { key: "", value: "" },
  ]);
  const [err, setErr] = useState("");
  const [testMsg, setTestMsg] = useState<{ ok: boolean; text: string } | null>(null);
  const [testing, setTesting] = useState(false);

  const familyMeta: Record<
    string,
    { placeholder: string; hint: string }
  > = {
    openai_compat: {
      placeholder: "https://api.deepseek.com/v1",
      hint: "OpenAI 兼容填到 /v1，网关自动拼 /chat/completions",
    },
    openai_responses: {
      placeholder: "https://one-model.com/v1",
      hint: "OpenAI Responses 填到 /v1，网关自动拼 /responses",
    },
    anthropic: {
      placeholder: "https://api.anthropic.com",
      hint: "Anthropic 只填主机根，网关自动拼 /v1/messages",
    },
    gemini: {
      placeholder: "https://generativelanguage.googleapis.com",
      hint: "Gemini 只填主机根，网关自动拼 /v1beta/models",
    },
  };
  const meta = familyMeta[family] ?? familyMeta.openai_compat;

  async function testConnection() {
    setErr("");
    setTestMsg(null);
    setTesting(true);
    try {
      const r = await api.providerTestDraft({ baseUrl, family, apiKey });
      const preview = r.modelNames.slice(0, 3).join(", ");
      setTestMsg({
        ok: true,
        text:
          `连接成功 · 发现 ${r.count} 个模型` +
          (r.count ? `：${preview}${r.count > 3 ? "…" : ""}` : ""),
      });
    } catch (e) {
      setTestMsg({
        ok: false,
        text: `连接失败：${e}。请检查网络、Base URL、API Key 是否匹配该协议族。`,
      });
    } finally {
      setTesting(false);
    }
  }

  async function submit() {
    setErr("");
    try {
      const headersObj: Record<string, string> = {};
      for (const row of headerRows) {
        const k = row.key.trim();
        const v = row.value.trim();
        if (k || v) {
          if (!k) throw new Error("请求头名称不能为空");
          headersObj[k] = v;
        }
      }
      const eh = Object.keys(headersObj).length ? JSON.stringify(headersObj) : null;
      await api.providerCreate({
        name,
        baseUrl,
        family,
        priority,
        extraHeaders: eh,
        apiKey,
      });
      onDone();
    } catch (e) {
      setErr(String(e));
    }
  }

  return (
    <Card title="添加供应商">
      <div className="grid grid-cols-1 gap-3 md:grid-cols-2">
        <label className="text-xs text-neutral-400">
          名称
          <input className={`${inputCls} mt-1`} value={name} onChange={(e) => setName(e.target.value)} placeholder="官方 / 某中转…" />
        </label>
        <label className="text-xs text-neutral-400">
          协议族
          <select className={`${inputCls} mt-1`} value={family} onChange={(e) => setFamily(e.target.value)}>
            <option value="openai_compat">OpenAI 兼容（chat/completions）</option>
            <option value="openai_responses">OpenAI Responses（/responses）</option>
            <option value="anthropic">Anthropic</option>
            <option value="gemini">Gemini</option>
          </select>
        </label>
        <label className="col-span-1 text-xs text-neutral-400 md:col-span-2">
          Base URL
          <input className={`${inputCls} mt-1`} value={baseUrl} onChange={(e) => setBaseUrl(e.target.value)} placeholder={meta.placeholder} />
          <span className="mt-1 block text-[11px] text-neutral-600">{meta.hint}</span>
        </label>
        <label className="text-xs text-neutral-400 md:col-span-2">
          <div className="flex items-center justify-between">
            <span>API Key（凭据存储方式见设置页）</span>
            <button
              className={btnGhost}
              type="button"
              onClick={async () => {
                const name = window.prompt("输入环境变量名，例如 DEEPSEEK_API_KEY");
                if (!name) return;
                try {
                  setApiKey(await api.readEnvVar(name));
                  toast("已从环境变量导入");
                } catch (e) {
                  setErr(String(e));
                }
              }}
            >
              从环境变量导入
            </button>
          </div>
          <input className={`${inputCls} mt-1`} type="password" value={apiKey} onChange={(e) => setApiKey(e.target.value)} />
        </label>
        <label className="text-xs text-neutral-400">
          路由优先级（数字越小越优先）
          <input className={`${inputCls} mt-1`} type="number" value={priority} onChange={(e) => setPriority(Number(e.target.value))} />
        </label>
        <div className="text-xs text-neutral-400 md:col-span-2">
          <div className="mb-1">追加请求头（可选，结构化编辑）</div>
          <div className="space-y-2">
            {headerRows.map((row, i) => (
              <div key={i} className="flex gap-2">
                <input
                  className={`${inputCls} flex-1`}
                  placeholder="Header 名，如 HTTP-Referer"
                  value={row.key}
                  onChange={(e) => {
                    const next = [...headerRows];
                    next[i] = { ...next[i], key: e.target.value };
                    setHeaderRows(next);
                  }}
                />
                <input
                  className={`${inputCls} flex-1`}
                  placeholder="值"
                  value={row.value}
                  onChange={(e) => {
                    const next = [...headerRows];
                    next[i] = { ...next[i], value: e.target.value };
                    setHeaderRows(next);
                  }}
                />
                <button
                  className={btnGhost}
                  onClick={() => setHeaderRows(headerRows.filter((_, idx) => idx !== i))}
                  title="删除此行"
                >
                  ✕
                </button>
              </div>
            ))}
          </div>
          <button
            className={`${btnGhost} mt-2`}
            onClick={() => setHeaderRows([...headerRows, { key: "", value: "" }])}
          >
            + 添加请求头
          </button>
        </div>
      </div>
      {err && <div className="mt-3 text-xs text-red-400">{err}</div>}
      {testMsg && (
        <div className={`mt-3 text-xs ${testMsg.ok ? "text-emerald-400" : "text-red-400"}`}>
          {testMsg.text}
        </div>
      )}
      <div className="mt-4 flex gap-2">
        <button className={btnGhost} disabled={testing} onClick={testConnection}>
          {testing ? "测试中…" : "测试连接"}
        </button>
        <button className={btnPrimary} onClick={submit}>
          创建
        </button>
        <button className={btnGhost} onClick={onCancel}>
          取消
        </button>
      </div>
    </Card>
  );
}

function EditProviderForm({
  p,
  onDone,
  onCancel,
}: {
  p: ProviderDto;
  onDone: () => void;
  onCancel: () => void;
}) {
  const [name, setName] = useState(p.name);
  const [baseUrl, setBaseUrl] = useState(p.baseUrl);
  const [priority, setPriority] = useState(p.priority);
  const [apiKey, setApiKey] = useState("");
  const [err, setErr] = useState("");

  async function submit() {
    setErr("");
    try {
      await api.providerUpdate({
        id: p.id,
        name,
        baseUrl,
        priority,
        apiKey: apiKey || undefined,
      });
      onDone();
    } catch (e) {
      setErr(String(e));
    }
  }

  return (
    <div className="mt-4 grid grid-cols-2 gap-3 rounded border-t border-neutral-800 pt-4">
      <label className="text-xs text-neutral-400">
        名称
        <input className={`${inputCls} mt-1`} value={name} onChange={(e) => setName(e.target.value)} />
      </label>
      <label className="text-xs text-neutral-400">
        Base URL
        <input className={`${inputCls} mt-1`} value={baseUrl} onChange={(e) => setBaseUrl(e.target.value)} />
      </label>
      <label className="text-xs text-neutral-400">
        优先级
        <input className={`${inputCls} mt-1`} type="number" value={priority} onChange={(e) => setPriority(Number(e.target.value))} />
      </label>
      <label className="text-xs text-neutral-400">
        替换 API Key（留空不变）
        <input className={`${inputCls} mt-1`} type="password" value={apiKey} onChange={(e) => setApiKey(e.target.value)} placeholder={p.hasKey ? "•••• 已存于钥匙串" : "尚未录入"} />
      </label>
      {err && <div className="col-span-2 text-xs text-red-400">{err}</div>}
      <div className="col-span-2 flex gap-2">
        <button className={btnPrimary} onClick={submit}>保存</button>
        <button className={btnGhost} onClick={onCancel}>取消</button>
      </div>
    </div>
  );
}

// ---------------------------------------------------------------- 模型

function ModelsTab() {
  const [providers, setProviders] = useState<ProviderDto[]>([]);
  const [selId, setSelId] = useState<string>("");
  const [models, setModels] = useState<ModelRow[]>([]);
  const [q, setQ] = useState("");
  const [sortAsc, setSortAsc] = useState(true);

  useEffect(() => {
    api.providerList().then(setProviders).catch(() => {});
  }, []);

  useEffect(() => {
    if (providers.length && !selId) setSelId(providers[0].id);
  }, [providers, selId]);

  useEffect(() => {
    if (!selId) return;
    api.modelList(selId).then(setModels).catch(() => {});
  }, [selId]);

  const filtered = models
    .filter((m) => m.modelName.toLowerCase().includes(q.trim().toLowerCase()))
    .sort((a, b) =>
      sortAsc
        ? a.modelName.localeCompare(b.modelName)
        : b.modelName.localeCompare(a.modelName)
    );

  async function setAll(enabled: boolean) {
    for (const m of filtered) {
      if (m.enabled !== enabled) {
        await api.modelToggle(m.id, enabled);
      }
    }
    if (selId) setModels(await api.modelList(selId));
    toast(enabled ? "已全部启用" : "已全部禁用");
  }

  return (
    <div className="mx-auto max-w-4xl space-y-4">
      <div className="flex flex-wrap items-center justify-between gap-2">
        <h1 className="text-lg font-semibold">模型默认值</h1>
        <div className="flex flex-wrap items-center gap-2">
          <button className={btnGhost} onClick={() => setAll(true)}>
            全部启用
          </button>
          <button className={btnGhost} onClick={() => setAll(false)}>
            全部禁用
          </button>
        </div>
      </div>
      <div className="flex flex-wrap items-center gap-2">
        <select
          className={`${inputCls} max-w-xs`}
          value={selId}
          onChange={(e) => setSelId(e.target.value)}
        >
          {providers.length === 0 && <option value="">先在「供应商」页添加并拉取模型</option>}
          {providers.map((p) => (
            <option key={p.id} value={p.id}>
              {p.name}
            </option>
          ))}
        </select>
        <input
          className={`${inputCls} max-w-xs`}
          placeholder="搜索模型名…"
          value={q}
          onChange={(e) => setQ(e.target.value)}
        />
        <button
          className={btnGhost}
          onClick={() => setSortAsc(!sortAsc)}
          title="按名称排序"
        >
          名称 {sortAsc ? "↑" : "↓"}
        </button>
        <span className="text-xs text-neutral-500">
          这些值是跨协议转换与调用方提示的基础；仅本机模型名对齐时直通也会透传。
        </span>
      </div>

      <div className="overflow-hidden rounded-lg border border-neutral-800">
        <table className="w-full text-sm">
          <thead className="bg-neutral-900 text-left text-xs uppercase tracking-wider text-neutral-500">
            <tr>
              <th className="w-1/2 px-4 py-2.5">模型名</th>
              <th className="w-1/6 px-4 py-2.5">上下文</th>
              <th className="w-1/6 px-4 py-2.5">最大输出</th>
              <th className="w-1/6 px-4 py-2.5 text-center">启用</th>
            </tr>
          </thead>
          <tbody className="divide-y divide-neutral-800/70">
            {filtered.map((m) => (
              <ModelRowEditor
                key={m.id}
                m={m}
                onSave={async (ctx, out) => {
                  await api.modelSetLimits({
                    modelId: m.id,
                    contextWindow: ctx,
                    maxOutputTokens: out,
                  });
                  setModels(await api.modelList(selId));
                }}
                onToggle={async (v) => {
                  await api.modelToggle(m.id, v);
                  setModels(await api.modelList(selId));
                }}
              />
            ))}
          </tbody>
        </table>
        {filtered.length === 0 && (
          <div className="px-4 py-8 text-center text-sm text-neutral-500">
            {models.length === 0
              ? "该供应商还没有模型 —— 在「供应商」页点击「拉取模型」自动发现入库。"
              : "没有匹配的模型"}
          </div>
        )}
      </div>
    </div>
  );
}

function ModelRowEditor({
  m,
  onSave,
  onToggle,
}: {
  m: ModelRow;
  onSave: (ctx: number | null, out: number) => Promise<void>;
  onToggle: (enabled: boolean) => Promise<void>;
}) {
  const [ctx, setCtx] = useState(m.contextWindow ?? 128000);
  const [out, setOut] = useState(m.maxOutputTokens);
  const [saved, setSaved] = useState(false);

  async function handleSave() {
    await onSave(m.contextWindow == null && ctx === 128000 ? null : ctx, out);
    setSaved(true);
    window.setTimeout(() => setSaved(false), 3000);
  }

  return (
    <tr className={m.enabled ? "" : "opacity-50"}>
      <td className="px-4 py-2 font-mono text-xs">{m.modelName}</td>
      <td className="px-4 py-2">
        <input
          className={`${inputCls} w-28`}
          type="number"
          step={1024}
          value={ctx}
          onChange={(e) => {
            setSaved(false);
            setCtx(Number(e.target.value));
          }}
        />
      </td>
      <td className="px-4 py-2">
        <input
          className={`${inputCls} w-24`}
          type="number"
          step={1024}
          value={out}
          onChange={(e) => {
            setSaved(false);
            setOut(Number(e.target.value));
          }}
        />
      </td>
      <td className="px-4 py-2">
        <div className="flex items-center justify-center gap-3">
          <button
            role="switch"
            aria-checked={m.enabled}
            aria-label={`启用 ${m.modelName}`}
            onClick={() => onToggle(!m.enabled)}
            className={`relative h-5 w-9 rounded-full transition-colors ${
              m.enabled ? "bg-emerald-600" : "bg-neutral-700"
            }`}
          >
            <span
              className={`absolute top-0.5 h-4 w-4 rounded-full bg-white transition-all ${
                m.enabled ? "left-[18px]" : "left-0.5"
              }`}
            />
          </button>
          <button
            className={btnGhost}
            onClick={handleSave}
            title="保存上下文/最大输出"
          >
            {saved ? "✓ 已保存" : "保存"}
          </button>
        </div>
      </td>
    </tr>
  );
}

// ---------------------------------------------------------------- 日志

function LogsTab() {
  const [rows, setRows] = useState<LogRowView[]>([]);
  const [auto, setAuto] = useState(true);
  const [intervalMs, setIntervalMs] = useState(3000);
  const [modelFilter, setModelFilter] = useState("");
  const [statusFilter, setStatusFilter] = useState("");
  const [limit, setLimit] = useState(500);

  async function refresh() {
    setRows(await api.logsRecent(limit));
  }

  useEffect(() => {
    refresh();
    if (!auto) return;
    const t = setInterval(refresh, intervalMs);
    return () => clearInterval(t);
  }, [auto, intervalMs, limit]);

  const filtered = rows.filter((r) => {
    if (modelFilter && !r.modelName.toLowerCase().includes(modelFilter.toLowerCase()))
      return false;
    if (statusFilter && String(r.httpStatus) !== statusFilter) return false;
    return true;
  });

  function statusClass(s: number) {
    if (s >= 500) return "text-red-700";
    if (s >= 400) return "text-red-400";
    if (s >= 200 && s < 300) return "text-emerald-400";
    return "text-neutral-400";
  }

  function exportCsv() {
    const header = "时间,模型,状态,耗时,流式,输入,输出,错误\n";
    const body = filtered
      .map((r) =>
        [
          new Date(r.ts).toISOString(),
          `"${r.modelName.replaceAll('"', '""')}"`,
          r.httpStatus,
          r.durationMs,
          r.isStream ? "SSE" : "",
          r.usageInput ?? "",
          r.usageOutput ?? "",
          `"${(r.errorKind ?? "").replaceAll('"', '""')}"`,
        ].join(",")
      )
      .join("\n");
    const blob = new Blob([header + body], { type: "text/csv;charset=utf-8" });
    const a = document.createElement("a");
    a.href = URL.createObjectURL(blob);
    a.download = `jai-logs-${Date.now()}.csv`;
    a.click();
    URL.revokeObjectURL(a.href);
    toast("已导出 CSV");
  }

  function exportJson() {
    const blob = new Blob([JSON.stringify(filtered, null, 2)], {
      type: "application/json",
    });
    const a = document.createElement("a");
    a.href = URL.createObjectURL(blob);
    a.download = `jai-logs-${Date.now()}.json`;
    a.click();
    URL.revokeObjectURL(a.href);
    toast("已导出 JSON");
  }

  return (
    <div className="mx-auto max-w-5xl space-y-4">
      <div className="flex flex-wrap items-center justify-between gap-2">
        <h1 className="text-lg font-semibold">请求日志</h1>
        <div className="flex flex-wrap items-center gap-2">
          <label className="flex items-center gap-2 text-sm text-neutral-400">
            <input type="checkbox" checked={auto} onChange={(e) => setAuto(e.target.checked)} className="accent-amber-500" />
            自动刷新
          </label>
          <select
            className={`${inputCls} w-28`}
            value={intervalMs}
            onChange={(e) => setIntervalMs(Number(e.target.value))}
            disabled={!auto}
          >
            <option value={3000}>3s</option>
            <option value={5000}>5s</option>
            <option value={10000}>10s</option>
            <option value={30000}>30s</option>
          </select>
          <input
            className={`${inputCls} w-36`}
            placeholder="筛选模型…"
            value={modelFilter}
            onChange={(e) => setModelFilter(e.target.value)}
          />
          <input
            className={`${inputCls} w-24`}
            placeholder="状态码"
            value={statusFilter}
            onChange={(e) => setStatusFilter(e.target.value)}
          />
          <button className={btnGhost} onClick={exportCsv}>导出 CSV</button>
          <button className={btnGhost} onClick={exportJson}>导出 JSON</button>
        </div>
      </div>
      <div className="overflow-x-auto rounded-lg border border-neutral-800">
        <table className="w-full text-xs">
          <thead className="bg-neutral-900 text-left uppercase tracking-wider text-neutral-500">
            <tr>
              <th className="px-3 py-2">时间</th>
              <th className="px-3 py-2">模型</th>
              <th className="px-3 py-2">状态</th>
              <th className="px-3 py-2">耗时</th>
              <th className="px-3 py-2">流式</th>
              <th className="px-3 py-2 text-right">输入</th>
              <th className="px-3 py-2 text-right">输出</th>
              <th className="px-3 py-2">错误</th>
            </tr>
          </thead>
          <tbody className="divide-y divide-neutral-800/70 font-mono">
            {filtered.map((r) => (
              <tr key={r.id} className={r.httpStatus >= 400 ? "bg-red-950/20" : ""}>
                <td className="whitespace-nowrap px-3 py-1.5 text-neutral-500">
                  {new Date(r.ts).toLocaleTimeString()}
                </td>
                <td className="max-w-48 truncate px-3 py-1.5">{r.modelName}</td>
                <td className={`px-3 py-1.5 font-semibold ${statusClass(r.httpStatus)}`}>
                  {r.httpStatus}
                </td>
                <td className="px-3 py-1.5 text-neutral-400">{r.durationMs}ms</td>
                <td className="px-3 py-1.5 text-neutral-500">
                  {r.isStream ? <span title="SSE 流式">⇄</span> : "—"}
                </td>
                <td className="px-3 py-1.5 text-right text-neutral-400">{r.usageInput ?? "·"}</td>
                <td className="px-3 py-1.5 text-right text-neutral-400">{r.usageOutput ?? "·"}</td>
                <td className="max-w-64 truncate px-3 py-1.5 text-red-400" title={r.errorSummary ?? ""}>
                  {r.errorKind ?? ""}
                </td>
              </tr>
            ))}
          </tbody>
        </table>
        {filtered.length === 0 && (
          <div className="px-4 py-10 text-center text-sm text-neutral-500">
            没有匹配的日志。用任意 OpenAI 兼容客户端打一发试试。
          </div>
        )}
        {filtered.length > 0 && (
          <div className="flex justify-center border-t border-neutral-800/70 py-2">
            <button className={btnGhost} onClick={() => setLimit(limit + 500)}>
              加载更多（当前显示 {rows.length} 条）
            </button>
          </div>
        )}
      </div>
      <p className="text-xs text-neutral-600">
        日志不含具体内容，保留 30 天或 5 万行，每日自动清理。
      </p>
    </div>
  );
}

// ---------------------------------------------------------------- 设置

function SettingsTab() {
  const [raw, setRaw] = useState("");
  const [saved, setSaved] = useState(false);
  const [port, setPort] = useState(1314);
  const [logsEnabled, setLogsEnabled] = useState(true);
  const [retentionDays, setRetentionDays] = useState(30);
  const [logRowCap, setLogRowCap] = useState(50000);
  const [portMsg, setPortMsg] = useState("");
  const [logMsg, setLogMsg] = useState("");
  const [vaultKind, setVaultKind] = useState("…");
  const corsHasWildcard = raw.split("\n").some((s) => s.trim() === "*");

  useEffect(() => {
    api
      .corsAllowGet()
      .then((l) => setRaw(l.join("\n")))
      .catch(() => {});
    api
      .settingsGet()
      .then((s) => {
        setPort(s.preferredPort);
        setLogsEnabled(s.logsEnabled);
        setRetentionDays(s.retentionDays);
        setLogRowCap(s.logRowCap);
      })
      .catch(() => {});
    api.vaultStorageKind().then(setVaultKind).catch(() => setVaultKind("unknown"));
  }, []);

  async function save() {
    const lines = raw.split("\n").map((s) => s.trim()).filter(Boolean);
    await api.corsAllowSet(lines);
    setRaw(lines.join("\n"));
    setSaved(true);
    setTimeout(() => setSaved(false), 1500);
  }

  async function savePort() {
    setPortMsg("");
    const n = Number(port);
    if (!Number.isInteger(n) || n < 1 || n > 65535) {
      setPortMsg("端口需为 1–65535 的整数");
      return;
    }
    if (await api.portInUse(n)) {
      if (!confirm(`端口 ${n} 当前被占用，保存后网关会自动顺延到可用端口。继续？`)) {
        return;
      }
    }
    await api.settingsSetPort(n);
    setPortMsg("已保存：重启网关后生效（端口占用自动顺延）");
    toast("端口已保存");
  }

  async function toggleLogs(on: boolean) {
    await api.settingsSetLogsEnabled(on);
    setLogsEnabled(on);
    setLogMsg(on ? "日志记录已开启" : "日志记录已关闭（新请求不再落库）");
    setTimeout(() => setLogMsg(""), 2500);
  }

  return (
    <div className="mx-auto max-w-2xl space-y-4">
      <h1 className="text-lg font-semibold">设置</h1>

      <Card title="网关端口">
        <div className="flex items-center gap-2">
          <input
            className={`${inputCls} w-32 font-mono`}
            type="number"
            value={port}
            onChange={(e) => setPort(Number(e.target.value))}
          />
          <button className={btnPrimary} onClick={savePort}>
            保存
          </button>
          <span className="text-xs text-neutral-500">
            默认 1314；被占用时顺延，实际端口见「网关」页
          </span>
        </div>
        {portMsg && <div className="mt-2 text-xs text-emerald-400">{portMsg}</div>}
      </Card>

      <Card title="请求日志">
        <div className="flex items-center gap-3">
          <button
            role="switch"
            aria-checked={logsEnabled}
            onClick={() => toggleLogs(!logsEnabled)}
            className={`relative h-5 w-9 rounded-full transition-colors ${
              logsEnabled ? "bg-emerald-600" : "bg-neutral-700"
            }`}
          >
            <span
              className={`absolute top-0.5 h-4 w-4 rounded-full bg-white transition-all ${
                logsEnabled ? "left-[18px]" : "left-0.5"
              }`}
            />
          </button>
          <span className="text-sm text-neutral-300">
            日志记录：{logsEnabled ? "记录中" : "已关闭"}
          </span>
        </div>
        <p className="mt-3 text-xs leading-relaxed text-neutral-500">
          保留策略：{retentionDays} 天 / 上限 {logRowCap.toLocaleString("zh-CN")} 行
          （每日自动清理）。日志仅含元数据，不含 prompt 与响应明文。
        </p>
        <button
          className={`${btnGhost} mt-2`}
          onClick={async () => {
            const daysStr = window.prompt("保留天数（至少 1）", String(retentionDays));
            if (!daysStr) return;
            const days = Number(daysStr);
            const capStr = window.prompt("日志行数上限（至少 1000）", String(logRowCap));
            if (!capStr) return;
            const cap = Number(capStr);
            try {
              await api.settingsSetRetention(days, cap);
              setRetentionDays(days);
              setLogRowCap(cap);
              toast("保留策略已更新");
            } catch (e) {
              setLogMsg(String(e));
            }
          }}
        >
          编辑保留策略
        </button>
        {logMsg && <div className="mt-2 text-xs text-emerald-400">{logMsg}</div>}
      </Card>

      <Card title="浏览器跨域白名单（CORS）">
        <p className="mb-3 text-xs leading-relaxed text-neutral-500">
          默认拒绝一切远程网页来源访问网关（安全基线）。
          如果你使用网页版聊天应用（如自部署的 NextChat 等），
          把它的 Origin 加进白名单，每行一条；通配符 * 表示放行全部（不推荐）。
          本机来源（localhost / 127.0.0.1）始终放行。
        </p>
        {corsHasWildcard && (
          <div className="mb-3 rounded border border-amber-800 bg-amber-950/40 px-3 py-2 text-xs text-amber-300">
            ⚠ 通配符 * 会放行所有来源，生产环境不建议使用。
          </div>
        )}
        <textarea
          className={`${inputCls} h-32 font-mono`}
          value={raw}
          onChange={(e) => setRaw(e.target.value)}
          placeholder={"https://chat.example.com\nhttp://192.168.1.5:3000"}
        />
        <div className="mt-3 flex flex-wrap items-center gap-2">
          <button className={btnPrimary} onClick={save}>
            保存
          </button>
          <button
            className={btnGhost}
            onClick={() => {
              const origin = window.prompt("输入允许的来源 Origin，例如 https://chat.example.com");
              if (!origin) return;
              const lines = raw.split("\n").map((s) => s.trim()).filter(Boolean);
              if (!lines.includes(origin)) {
                lines.push(origin);
                setRaw(lines.join("\n"));
              }
            }}
          >
            + 添加域名
          </button>
          <button
            className={btnGhost}
            onClick={() => setRaw("https://chat.example.com\nhttp://192.168.1.5:3000")}
          >
            填入示例
          </button>
          {saved && <span className="text-xs text-emerald-400">已生效</span>}
        </div>
      </Card>

      <Card title="关于密钥存储">
        <p className="mb-2 text-xs text-neutral-500">
          当前凭据存储：{" "}
          <span className={vaultKind === "file" ? "text-amber-400" : "text-emerald-400"}>
            {vaultKind === "keyring"
              ? "🔑 系统钥匙串"
              : vaultKind === "file"
                ? "📁 文件降级（0600）"
                : vaultKind}
          </span>
        </p>
        <ul className="list-disc space-y-1.5 pl-5 text-xs leading-relaxed text-neutral-500">
          <li>🔑 上游 API Key：优先保存在 macOS 钥匙串 / Windows 凭据管理器，数据库只存引用。</li>
          <li>📁 若系统凭据存储不可用（如沙箱/CI 权限受限），自动降级为数据目录下的 vault_fallback.json（Unix 0600）。</li>
          <li>🔐 网关 Key sk-jai-*：按设计决策明文存放本地 SQLite（非敏感级别），前缀展示、轮换采用吊销+新建保留审计痕迹、永不导出。</li>
          <li>🗑 删除供应商会同时清除对应凭据。</li>
        </ul>
        <button className={`${btnGhost} mt-3`} onClick={() => goTab("providers")}>
          前往供应商页补录/查看凭据 →
        </button>
      </Card>

      <Card title="数据位置">
        <code className="font-mono text-xs text-neutral-400">
          jai.db 位于系统应用数据目录（Tauri appDataDir）/jai.db
        </code>
      </Card>
    </div>
  );
}
