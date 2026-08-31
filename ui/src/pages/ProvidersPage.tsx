import { useEffect, useState } from "react";
import { api } from "../api";
import type { ProviderDto } from "../types";
import { toast } from "../lib/toast";
import { fmtClock } from "../lib/format";
import { Card, inputCls, btnGhost, btnPrimary, btnDanger } from "../components/common/legacy";

const FAMILY_LABEL: Record<string, string> = {
  openai_compat: "OpenAI 兼容",
  openai_responses: "OpenAI Responses",
  anthropic: "Anthropic",
  gemini: "Gemini",
};

export function ProvidersPage() {
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
              优先级 {p.priority} · 权重 {p.weight}
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
  const [weight, setWeight] = useState(1);
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
        weight,
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
        <label className="text-xs text-neutral-400">
          权重（同优先级按比例分发）
          <input className={`${inputCls} mt-1`} type="number" min={1} value={weight} onChange={(e) => setWeight(Math.max(1, Number(e.target.value)))} />
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
  const [weight, setWeight] = useState(p.weight);
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
        weight,
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
        权重（同优先级按比例分发）
        <input className={`${inputCls} mt-1`} type="number" min={1} value={weight} onChange={(e) => setWeight(Math.max(1, Number(e.target.value)))} />
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
