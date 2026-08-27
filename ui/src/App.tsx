import { useEffect, useState } from "react";
import { api } from "./api";
import type {
  GatewayKeyInfo,
  LogRowView,
  ModelRow,
  ProviderDto,
} from "./types";

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
    if (!confirm("轮换后旧 Key 立即失效，已配置的客户端需要更新。继续？")) return;
    const k = await api.gatewayKeyRegenerate();
    setKey(k);
    setRevealKey(k.key);
  }

  async function doExport() {
    const json = await api.exportConfigJson();
    const blob = new Blob([json], { type: "application/json" });
    const a = document.createElement("a");
    a.href = URL.createObjectURL(blob);
    a.download = `jai-export-${Date.now()}.json`;
    a.click();
    URL.revokeObjectURL(a.href);
  }

  return (
    <div className="mx-auto max-w-2xl space-y-4">
      {err && (
        <div className="rounded border border-red-900 bg-red-950/40 px-3 py-2 text-sm text-red-300">
          {err}
        </div>
      )}

      <Card title="网关状态">
        <div className="flex items-center gap-3">
          <span
            className={`inline-block h-3 w-3 rounded-full ${
              status?.running ? "bg-emerald-500" : "bg-neutral-600"
            }`}
          />
          <div className="font-mono text-sm">
            {status?.running
              ? `127.0.0.1:${status.port} · 运行中`
              : "未运行"}
          </div>
          <div className="ml-auto flex gap-2">
            <button
              className={btnPrimary}
              disabled={busy || !!status?.running}
              onClick={() => toggle(true)}
            >
              启动
            </button>
            <button
              className={btnGhost}
              disabled={busy || !status?.running}
              onClick={() => toggle(false)}
            >
              停止
            </button>
          </div>
        </div>
        <p className="mt-3 text-xs leading-relaxed text-neutral-500">
          停机时端口被占用会自动顺延（1314 起）。所有业务端点强制鉴权：
          Host 仅接受本机回环地址，浏览器跨域需在「设置」中添加白名单。
        </p>
      </Card>

      <Card title="客户端接入">
        <div className="space-y-2 text-sm">
          <div className="text-neutral-400">Base URL</div>
          <code className="block rounded bg-black/60 px-3 py-2 font-mono text-emerald-400">
            http://127.0.0.1:{status?.port ?? 1314}/v1
          </code>
          <div className="pt-2 text-neutral-400">API Key</div>
          <div className="flex items-center gap-2">
            <code className="flex-1 truncate rounded bg-black/60 px-3 py-2 font-mono text-emerald-400">
              {revealKey ||
                (key ? `${key.prefix}…（点击右侧显示全文）` : "加载中")}
            </code>
            <button className={btnGhost} onClick={doReveal}>
              {revealKey ? "隐藏" : "显示"}
            </button>
            <button className={btnDanger} onClick={doRegen}>
              轮换
            </button>
          </div>
          <pre className="mt-3 overflow-x-auto rounded bg-black/60 p-3 text-xs leading-relaxed text-neutral-400">{`# DeepSeek harness / OpenAI SDK 兼容客户端
OPENAI_API_BASE=http://127.0.0.1:${status?.port ?? 1314}/v1
OPENAI_API_KEY=sk-jai-…`}</pre>
        </div>
      </Card>

      <Card title="配置迁移">
        <div className="flex items-center gap-2">
          <button className={btnGhost} onClick={doExport}>
            导出 JSON（无敏感字段）
          </button>
          <span className="text-xs text-neutral-500">
            导入功能在 M7 提供；API Key 保存在各设备钥匙串中不随导出迁移。
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
    try {
      await api.webdavConfigSet({ ...cfg, password: pw || null });
      setPw("");
      setMsg("WebDAV 连接配置已保存");
    } catch (e) {
      setErr(String(e));
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
        <div className="mt-3 flex gap-2">
          <button className={btnPrimary} onClick={doImport}>
            导入
          </button>
          <span className="text-xs text-neutral-500">
            导出入口仍在「网关」页。
          </span>
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
            密码（存入钥匙串，留空保持原密码）
            <input
              className={`${inputCls} mt-1`}
              type="password"
              value={pw}
              onChange={(e) => setPw(e.target.value)}
            />
          </label>
        </div>
        <div className="mt-4 flex flex-wrap gap-2">
          <button className={btnPrimary} onClick={saveCfg}>
            保存配置
          </button>
          <button className={btnGhost} disabled={busy === "push"} onClick={doPush}>
            {busy === "push" ? "推送中…" : "推送"}
          </button>
          <button className={btnGhost} disabled={busy === "pull"} onClick={doPull}>
            {busy === "pull" ? "拉取中…" : "拉取"}
          </button>
        </div>
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
              添加 stdio / SSE / HTTP 类型的 MCP Server 配置，当前版本先做管理与启停。
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

  async function submit() {
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
    <div className="mt-3 grid grid-cols-2 gap-3 rounded border-t border-neutral-800 pt-4">
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
            <input className={`${inputCls} mt-1 font-mono`} value={args} onChange={(e) => setArgs(e.target.value)} placeholder='["-y","@modelcontextprotocol/server-filesystem"]' />
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

  async function refresh() {
    setList(await api.skillList());
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
        <h1 className="text-lg font-semibold">技能（Skill）管理</h1>
        <button className={btnPrimary} onClick={() => setShowNew(true)}>
          + 添加技能
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
        <SkillForm
          onCancel={() => setShowNew(false)}
          onDone={() => {
            setShowNew(false);
            refresh();
          }}
        />
      )}
      <div className="space-y-3">
        {list.map((s) => (
          <div key={s.id} className="rounded-lg border border-neutral-800 bg-neutral-900/60 p-4">
            <div className="flex items-center gap-3">
              <input
                type="checkbox"
                checked={s.enabled}
                onChange={(e) => act(() => api.skillSetEnabled(s.id, e.target.checked))}
                className="accent-amber-500"
              />
              <div className="min-w-0 flex-1">
                <div className="font-medium">{s.name}</div>
                <div className="truncate text-xs text-neutral-500">{s.description}</div>
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
              技能即一组可复用的指令/提示词/工作流定义；当前版本先做管理与启停。
            </p>
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
    <div className="mt-3 grid grid-cols-2 gap-3 rounded border-t border-neutral-800 pt-4">
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
  const [extraHeaders, setExtraHeaders] = useState("");
  const [err, setErr] = useState("");

  async function submit() {
    setErr("");
    try {
      let eh: string | null = null;
      if (extraHeaders.trim()) {
        JSON.parse(extraHeaders); // 校验合法
        eh = extraHeaders;
      }
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
      <div className="grid grid-cols-2 gap-3">
        <label className="col-span-1 text-xs text-neutral-400">
          名称
          <input className={`${inputCls} mt-1`} value={name} onChange={(e) => setName(e.target.value)} placeholder="官方 / 某中转…" />
        </label>
        <label className="col-span-1 text-xs text-neutral-400">
          协议族
          <select className={`${inputCls} mt-1`} value={family} onChange={(e) => setFamily(e.target.value)}>
            <option value="openai_compat">OpenAI 兼容</option>
            <option value="anthropic">Anthropic</option>
            <option value="gemini">Gemini</option>
          </select>
        </label>
        <label className="col-span-2 text-xs text-neutral-400">
          Base URL
          <input className={`${inputCls} mt-1`} value={baseUrl} onChange={(e) => setBaseUrl(e.target.value)} placeholder="https://api.deepseek.com/v1" />
          <span className="mt-1 block text-[11px] text-neutral-600">
            OpenAI 兼容填到 /v1；Anthropic / Gemini 只填主机根。
          </span>
        </label>
        <label className="col-span-2 text-xs text-neutral-400">
          API Key（存入系统钥匙串）
          <input className={`${inputCls} mt-1`} type="password" value={apiKey} onChange={(e) => setApiKey(e.target.value)} />
        </label>
        <label className="text-xs text-neutral-400">
          路由优先级（小者先试）
          <input className={`${inputCls} mt-1`} type="number" value={priority} onChange={(e) => setPriority(Number(e.target.value))} />
        </label>
        <label className="text-xs text-neutral-400">
          追加请求头（可选，JSON 对象）
          <input className={`${inputCls} mt-1`} value={extraHeaders} onChange={(e) => setExtraHeaders(e.target.value)} placeholder='{"HTTP-Referer":"…"}' />
        </label>
      </div>
      {err && <div className="mt-3 text-xs text-red-400">{err}</div>}
      <div className="mt-4 flex gap-2">
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

  const selected = providers.find((p) => p.id === selId);

  return (
    <div className="mx-auto max-w-3xl space-y-4">
      <h1 className="text-lg font-semibold">模型默认值</h1>
      <div className="flex items-center gap-2">
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
        <span className="text-xs text-neutral-500">
          这些值是跨协议转换（M4+）与调用方提示的基础；仅本机模型名对齐时直通也会透传。
        </span>
      </div>

      <div className="overflow-hidden rounded-lg border border-neutral-800">
        <table className="w-full text-sm">
          <thead className="bg-neutral-900 text-left text-xs uppercase tracking-wider text-neutral-500">
            <tr>
              <th className="px-4 py-2.5">模型名</th>
              <th className="px-4 py-2.5">上下文</th>
              <th className="px-4 py-2.5">最大输出</th>
              <th className="px-4 py-2.5">启用</th>
            </tr>
          </thead>
          <tbody className="divide-y divide-neutral-800/70">
            {models.map((m) => (
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
        {models.length === 0 && selected && (
          <div className="px-4 py-8 text-center text-sm text-neutral-500">
            该供应商还没有模型 —— 在「供应商」页点击「拉取模型」自动发现入库。
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

  return (
    <tr className={m.enabled ? "" : "opacity-50"}>
      <td className="px-4 py-2 font-mono text-xs">{m.modelName}</td>
      <td className="px-4 py-2">
        <input
          className={`${inputCls} w-28`}
          type="number"
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
          value={out}
          onChange={(e) => {
            setSaved(false);
            setOut(Number(e.target.value));
          }}
        />
      </td>
      <td className="px-4 py-2">
        <div className="flex items-center gap-3">
          <input
            type="checkbox"
            checked={m.enabled}
            onChange={(e) => onToggle(e.target.checked)}
            className="accent-amber-500"
          />
          <button
            className={btnGhost}
            onClick={async () => {
              await onSave(m.contextWindow == null && ctx === 128000 ? null : ctx, out);
              setSaved(true);
            }}
          >
            {saved ? "✓" : "保存"}
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

  async function refresh() {
    setRows(await api.logsRecent(200));
  }

  useEffect(() => {
    refresh();
    if (!auto) return;
    const t = setInterval(refresh, 3000);
    return () => clearInterval(t);
  }, [auto]);

  return (
    <div className="mx-auto max-w-5xl space-y-4">
      <div className="flex items-center justify-between">
        <h1 className="text-lg font-semibold">请求日志</h1>
        <label className="flex items-center gap-2 text-sm text-neutral-400">
          <input type="checkbox" checked={auto} onChange={(e) => setAuto(e.target.checked)} className="accent-amber-500" />
          自动刷新
        </label>
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
              <th className="px-3 py-2">输入</th>
              <th className="px-3 py-2">输出</th>
              <th className="px-3 py-2">错误</th>
            </tr>
          </thead>
          <tbody className="divide-y divide-neutral-800/70 font-mono">
            {rows.map((r) => (
              <tr key={r.id} className={r.httpStatus >= 400 ? "bg-red-950/20" : ""}>
                <td className="whitespace-nowrap px-3 py-1.5 text-neutral-500">
                  {new Date(r.ts).toLocaleTimeString()}
                </td>
                <td className="max-w-48 truncate px-3 py-1.5">{r.modelName}</td>
                <td className={`px-3 py-1.5 ${r.httpStatus >= 400 ? "text-red-400" : "text-emerald-400"}`}>
                  {r.httpStatus}
                </td>
                <td className="px-3 py-1.5 text-neutral-400">{r.durationMs}ms</td>
                <td className="px-3 py-1.5 text-neutral-500">{r.isStream ? "SSE" : "—"}</td>
                <td className="px-3 py-1.5 text-neutral-400">{r.usageInput ?? "·"}</td>
                <td className="px-3 py-1.5 text-neutral-400">{r.usageOutput ?? "·"}</td>
                <td className="max-w-64 truncate px-3 py-1.5 text-red-400" title={r.errorSummary ?? ""}>
                  {r.errorKind ?? ""}
                </td>
              </tr>
            ))}
          </tbody>
        </table>
        {rows.length === 0 && (
          <div className="px-4 py-10 text-center text-sm text-neutral-500">
            还没有请求记录。用任意 OpenAI 兼容客户端打一发试试。
          </div>
        )}
      </div>
      <p className="text-xs text-neutral-600">
        日志只含元数据（不含 prompt 与响应内容），滚动保留 30 天 / 上限 50,000 行，每日自动清理（策略见「设置」页）。
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
    await api.settingsSetPort(n);
    setPortMsg("已保存：重启网关后生效（端口占用自动顺延）");
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
            className={logsEnabled ? btnPrimary : btnGhost}
            onClick={() => toggleLogs(true)}
          >
            记录
          </button>
          <button
            className={!logsEnabled ? btnPrimary : btnGhost}
            onClick={() => toggleLogs(false)}
          >
            关闭
          </button>
          <span className="text-xs text-neutral-500">
            当前：{logsEnabled ? "记录中" : "已关闭"}
          </span>
        </div>
        <p className="mt-3 text-xs leading-relaxed text-neutral-500">
          保留策略：{retentionDays} 天 / 上限 {logRowCap.toLocaleString("zh-CN")} 行
          （每日自动清理）。日志仅含元数据，不含 prompt 与响应明文。
        </p>
        {logMsg && <div className="mt-2 text-xs text-emerald-400">{logMsg}</div>}
      </Card>

      <Card title="浏览器跨域白名单（CORS）">
        <p className="mb-3 text-xs leading-relaxed text-neutral-500">
          默认拒绝一切远程网页来源访问网关（安全基线）。
          如果你使用网页版聊天应用（如自部署的 NextChat 等），
          把它的 Origin 加进白名单，每行一条；通配符 * 表示放行全部（不推荐）。
          本机来源（localhost / 127.0.0.1）始终放行。
        </p>
        <textarea
          className={`${inputCls} h-32 font-mono`}
          value={raw}
          onChange={(e) => setRaw(e.target.value)}
          placeholder={"https://chat.example.com\nhttp://192.168.1.5:3000"}
        />
        <div className="mt-3 flex items-center gap-2">
          <button className={btnPrimary} onClick={save}>
            保存
          </button>
          {saved && <span className="text-xs text-emerald-400">已生效</span>}
        </div>
      </Card>

      <Card title="关于密钥存储">
        <ul className="list-disc space-y-1.5 pl-5 text-xs leading-relaxed text-neutral-500">
          <li>上游 API Key：保存在 macOS 钥匙串 / Windows 凭据管理器，数据库只存引用。</li>
          <li>网关 Key sk-jai-*：按设计决策明文存放本地 SQLite（非敏感级别），前缀展示、轮换采用吊销+新建保留审计痕迹、永不导出。</li>
          <li>删除供应商会同时清除钥匙串中的对应条目。</li>
        </ul>
      </Card>

      <Card title="数据位置">
        <code className="font-mono text-xs text-neutral-400">
          jai.db 位于系统应用数据目录（Tauri appDataDir）/jai.db
        </code>
      </Card>
    </div>
  );
}
