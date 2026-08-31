import { useEffect, useRef, useState } from "react";
import { api } from "../api";
import { toast } from "../lib/toast";
import { copyText } from "../lib/clipboard";
import { Card, inputCls, btnGhost, btnPrimary, btnDanger } from "../components/common/legacy";

export function SkillsPage() {
  const [list, setList] = useState<import("../types").SkillRow[]>([]);
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
          <button
            className={btnGhost}
            onClick={() =>
              act(async () => {
                const md = await api.skillExportMarkdown();
                copyText(md);
                toast("已复制技能包");
              })
            }
          >
            复制技能包
          </button>
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
  initial?: import("../types").SkillRow;
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
