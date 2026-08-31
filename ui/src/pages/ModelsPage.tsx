import { useEffect, useState } from "react";
import { api } from "../api";
import type { ModelRow, ProviderDto } from "../types";
import { toast } from "../lib/toast";
import { inputCls, btnGhost } from "../components/common/legacy";

export function ModelsPage() {
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
              <th className="w-1/3 px-4 py-2.5">模型名</th>
              <th className="w-1/6 px-4 py-2.5">上游模型 ID</th>
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
                onSave={async (ctx, out, alias) => {
                  await api.modelSetLimits({
                    modelId: m.id,
                    contextWindow: ctx,
                    maxOutputTokens: out,
                  });
                  await api.modelSetAlias(m.id, alias);
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
  onSave: (ctx: number | null, out: number, alias: string | null) => Promise<void>;
  onToggle: (enabled: boolean) => Promise<void>;
}) {
  const [ctx, setCtx] = useState(m.contextWindow ?? 128000);
  const [out, setOut] = useState(m.maxOutputTokens);
  const [alias, setAlias] = useState(m.upstreamModelId ?? "");
  const [saved, setSaved] = useState(false);

  async function handleSave() {
    await onSave(
      m.contextWindow == null && ctx === 128000 ? null : ctx,
      out,
      alias.trim() || null
    );
    setSaved(true);
    window.setTimeout(() => setSaved(false), 3000);
  }

  return (
    <tr className={m.enabled ? "" : "opacity-50"}>
      <td className="px-4 py-2 font-mono text-xs">{m.modelName}</td>
      <td className="px-4 py-2">
        <input
          className={`${inputCls} w-32`}
          value={alias}
          placeholder="同模型名"
          title="发给上游时使用的真实模型 ID；留空表示同名"
          onChange={(e) => {
            setSaved(false);
            setAlias(e.target.value);
          }}
        />
      </td>
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
