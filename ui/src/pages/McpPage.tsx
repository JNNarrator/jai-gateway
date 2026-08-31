import { useEffect, useState } from "react";
import { api } from "../api";
import { toast } from "../lib/toast";
import { copyText } from "../lib/clipboard";
import { Card, inputCls, btnGhost, btnPrimary, btnDanger } from "../components/common/legacy";

function McpImportForm({
  onDone,
  onCancel,
}: {
  onDone: (report: { imported: number; updated: number; skipped: string[] }) => void;
  onCancel: () => void;
}) {
  const [text, setText] = useState("");
  const [busy, setBusy] = useState(false);
  const [localErr, setLocalErr] = useState("");

  async function submit() {
    if (!text.trim()) {
      setLocalErr("请粘贴 mcpServers JSON");
      return;
    }
    setBusy(true);
    setLocalErr("");
    try {
      const report = await api.mcpImportFromJson(text);
      onDone(report);
    } catch (e) {
      setLocalErr(String(e));
      setBusy(false);
    }
  }

  return (
    <div className="rounded-lg border border-neutral-800 bg-neutral-900/60 p-4">
      <div className="mb-2 flex items-center justify-between">
        <span className="text-sm font-medium">粘贴 mcpServers JSON 导入</span>
        <button className={btnGhost} onClick={onCancel}>
          取消
        </button>
      </div>
      <p className="mb-2 text-xs text-neutral-500">
        支持 Claude Code 格式：{`{"mcpServers": {"名称": {"command": "...", "args": [...], "env": {...}}}}`}
        ，同名服务会被更新，不含 command/url 的条目自动跳过。
      </p>
      <textarea
        className={`${inputCls} min-h-[140px] w-full resize-y font-mono text-xs`}
        value={text}
        onChange={(e) => {
          setText(e.target.value);
          setLocalErr("");
        }}
        placeholder='{"mcpServers":{"netcatty-external":{"command":"/Applications/...","args":[],"env":{"KEY":"value"}}}}'
        spellCheck={false}
      />
      <div className="mt-2 flex items-center gap-2">
        <button className={btnPrimary} onClick={submit} disabled={busy}>
          {busy ? "导入中…" : "导入"}
        </button>
        <button
          className={btnGhost}
          onClick={() =>
            navigator.clipboard
              ?.readText()
              .then((t) => {
                setText(t);
                setLocalErr("");
              })
              .catch(() => setLocalErr("读取剪贴板失败"))
          }
        >
          粘贴
        </button>
        {localErr && <span className="text-xs text-red-400">{localErr}</span>}
      </div>
    </div>
  );
}

export function McpPage() {
  const [list, setList] = useState<import("../types").McpServerRow[]>([]);
  const [msg, setMsg] = useState("");
  const [err, setErr] = useState("");
  const [showNew, setShowNew] = useState(false);
  const [showImport, setShowImport] = useState(false);

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
        <div className="flex gap-2">
          <button
            className={btnGhost}
            onClick={() =>
              act(async () => {
                const cfg = await api.mcpExportConfig();
                copyText(JSON.stringify(cfg, null, 2));
                toast("已复制客户端 MCP 配置");
              })
            }
          >
            复制客户端配置
          </button>
          <button className={btnGhost} onClick={() => setShowImport((v) => !v)}>
            粘贴 JSON 导入
          </button>
          <button className={btnPrimary} onClick={() => setShowNew(true)}>
            + 添加 MCP Server
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
      {showImport && (
        <McpImportForm
          onDone={(report) => {
            setMsg(
              `导入完成：新增 ${report.imported}，更新 ${report.updated}${
                report.skipped.length ? `，跳过 ${report.skipped.length}` : ""
              }`,
            );
            setShowImport(false);
            refresh();
          }}
          onCancel={() => setShowImport(false)}
        />
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
                  {m.env ? (
                    <span className="text-neutral-600" title={m.env}>
                      {" "}
                      env:{" "}
                      {Object.keys(JSON.parse(m.env) as Record<string, string>).join(", ")}
                    </span>
                  ) : null}
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
  initial?: import("../types").McpServerRow;
  onDone: () => void;
  onCancel: () => void;
}) {
  const [name, setName] = useState(initial?.name ?? "");
  const [kind, setKind] = useState<string>(initial?.kind ?? "stdio");
  const [command, setCommand] = useState(initial?.command ?? "");
  const [args, setArgs] = useState(initial?.args ?? "");
  const [url, setUrl] = useState(initial?.url ?? "");
  const [env, setEnv] = useState(initial?.env ?? "");
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
        env: env || null,
      });
    } else {
      await api.mcpCreate({
        name,
        kind,
        command: command || null,
        args: args || null,
        url: url || null,
        env: env || null,
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
          <label className="col-span-2 text-xs text-neutral-400">
            环境变量（JSON 对象，可选）
            <input className={`${inputCls} mt-1 font-mono`} value={env} onChange={(e) => setEnv(e.target.value)} placeholder='{"API_KEY":"xxx"}' />
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
