import { useEffect, useRef, useState } from "react";
import { Pencil, Plus, Sparkles, Trash2, Upload } from "lucide-react";
import { api } from "../api";
import type { SkillRow } from "../types";
import { toast } from "../lib/toast";
import { copyText } from "../lib/clipboard";
import { PageHeader } from "@/components/common/PageHeader";
import { SkeletonList } from "@/components/common/SkeletonList";
import { EmptyState } from "@/components/common/EmptyState";
import { ConfirmDialog } from "@/components/common/ConfirmDialog";
import { Button } from "@/components/ui/button";
import {
  Dialog,
  DialogContent,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { Input } from "@/components/ui/input";
import { Switch } from "@/components/ui/switch";
import { Textarea } from "@/components/ui/textarea";

export function SkillsPage() {
  const [list, setList] = useState<SkillRow[]>([]);
  const [err, setErr] = useState("");
  const [msg, setMsg] = useState("");
  const [dialog, setDialog] = useState<
    { mode: "create" } | { mode: "edit"; row: SkillRow } | null
  >(null);
  const [confirmDelete, setConfirmDelete] = useState<SkillRow | "batch" | null>(null);
  const [importing, setImporting] = useState(false);
  const [dragOver, setDragOver] = useState(false);
  const [selected, setSelected] = useState<Set<string>>(new Set());
  const [loading, setLoading] = useState(true);
  const fileRef = useRef<HTMLInputElement>(null);

  async function refresh() {
    setList(await api.skillList());
  }
  useEffect(() => {
    refresh()
      .catch((e) => setErr(String(e)))
      .finally(() => setLoading(false));
  }, []);

  function flash(text: string) {
    setMsg(text);
    setTimeout(() => setMsg(""), 2000);
  }

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

  function batchDelete() {
    setConfirmDelete("batch");
  }

  async function doBatchDelete() {
    if (!selected.size) return;
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
    try {
      await api.skillCreate({
        name: "代码审查",
        description: "一个示例技能：按提交变更做代码评审",
        content: "请按以下步骤进行代码审查：\n1. 阅读 diff\n2. 指出风险\n3. 给出修改建议",
      });
      toast("示例技能已创建");
      await refresh();
    } catch (e) {
      toast(String(e), "err");
    }
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
      <PageHeader
        title="技能（Skill）管理"
        description="技能登记台账，支持 ZIP 批量导入（拖拽文件到本页即可）。不再注入系统提示词；Agent 可通过网关 /mcp 元数据服务按需获取（见「网关」页接入说明）。"
        actions={
          <div className="flex items-center gap-2">
            <Button
              variant="outline"
              size="sm"
              onClick={() =>
                act(async () => {
                  const md = await api.skillExportMarkdown();
                  copyText(md);
                  toast("已复制技能包");
                })
              }
            >
              复制技能包
            </Button>
            <input
              ref={fileRef}
              type="file"
              accept=".zip,application/zip"
              className="hidden"
              onChange={importZip}
            />
            <Button
              variant="outline"
              size="sm"
              disabled={importing}
              onClick={() => fileRef.current?.click()}
            >
              <Upload aria-hidden />
              {importing ? "导入中…" : "导入 ZIP"}
            </Button>
            <Button size="sm" onClick={() => setDialog({ mode: "create" })}>
              <Plus aria-hidden />
              添加技能
            </Button>
          </div>
        }
      />

      {err && (
        <div
          role="alert"
          className="rounded-md border border-destructive/50 bg-destructive/10 px-3 py-2 text-sm text-destructive"
        >
          {err}
        </div>
      )}
      {msg && (
        <div className="rounded-md border border-primary/40 bg-primary/10 px-3 py-2 text-sm text-primary">
          {msg}
        </div>
      )}

      {selected.size > 0 && (
        <div className="flex flex-wrap items-center gap-2 rounded-md border border-primary/40 bg-primary/10 px-3 py-2 text-sm text-primary">
          已选 {selected.size} 项
          <Button variant="outline" size="sm" onClick={() => void batchSetEnabled(true)}>
            批量启用
          </Button>
          <Button variant="outline" size="sm" onClick={() => void batchSetEnabled(false)}>
            批量禁用
          </Button>
          <Button
            variant="outline"
            size="sm"
            className="text-destructive hover:bg-destructive/10 hover:text-destructive"
            onClick={batchDelete}
          >
            批量删除
          </Button>
          <Button variant="ghost" size="sm" onClick={() => setSelected(new Set())}>
            取消选择
          </Button>
        </div>
      )}

      {loading ? (
        <SkeletonList rows={3} />
      ) : (
        <>
      {list.length === 0 && !dialog && (
        <EmptyState
          icon={Sparkles}
          title="还没有技能"
          description="技能是一组可复用的指令/提示词/工作流定义，点击添加或导入 ZIP 开始。"
          className="py-16"
        >
          <Button variant="outline" size="sm" onClick={() => void createExample()}>
            <Sparkles aria-hidden />
            创建示例技能
          </Button>
        </EmptyState>
      )}

      <div className="space-y-3">
        {list.map((s) => (
          <div
            key={s.id}
            className="rounded-lg border bg-card p-4 text-card-foreground shadow-sm"
          >
            <div className="flex items-center gap-3">
              <input
                type="checkbox"
                checked={selected.has(s.id)}
                onChange={() => toggleSelect(s.id)}
                className="size-4 accent-primary"
                aria-label={`选择 ${s.name} 用于批量操作`}
              />
              <Switch
                checked={s.enabled}
                onCheckedChange={(v) => act(() => api.skillSetEnabled(s.id, v))}
                aria-label={`启用/停用 ${s.name}`}
              />
              <div className="min-w-0 flex-1">
                <div className="font-medium text-foreground">{s.name}</div>
                <div className="truncate text-xs text-muted-foreground">
                  {s.description}
                </div>
                <div className="mt-0.5 text-[11px] text-muted-foreground/70">
                  更新于 {new Date(s.updatedAt).toLocaleString()}
                </div>
              </div>
              <Button
                variant="outline"
                size="sm"
                onClick={() => setDialog({ mode: "edit", row: s })}
              >
                <Pencil aria-hidden />
                编辑
              </Button>
              <Button
                variant="outline"
                size="sm"
                className="text-destructive hover:bg-destructive/10 hover:text-destructive"
                onClick={() => setConfirmDelete(s)}
              >
                <Trash2 aria-hidden />
                删除
              </Button>
            </div>
          </div>
        ))}
      </div>

      {(dialog?.mode === "create" || dialog?.mode === "edit") && (
        <SkillDialog
          initial={dialog.mode === "edit" ? dialog.row : undefined}
          onClose={() => setDialog(null)}
          onDone={() => {
            setDialog(null);
            flash("技能已保存");
            refresh();
          }}
        />
      )}
        </>
      )}

      <ConfirmDialog
        open={!!confirmDelete}
        onOpenChange={(o) => !o && setConfirmDelete(null)}
        title={
          confirmDelete === "batch"
            ? `删除选中的 ${selected.size} 个技能？`
            : `删除技能「${confirmDelete ? confirmDelete.name : ""}」？`
        }
        description="删除后 Agent 将无法再通过 /mcp 元数据服务获取该技能，操作不可撤销。"
        confirmText="删除"
        destructive
        onConfirm={() => {
          if (confirmDelete === "batch") void doBatchDelete();
          else if (confirmDelete) act(() => api.skillDelete(confirmDelete.id));
        }}
      />
    </div>
  );
}

function SkillDialog({
  initial,
  onDone,
  onClose,
}: {
  initial?: SkillRow;
  onDone: () => void;
  onClose: () => void;
}) {
  const [name, setName] = useState(initial?.name ?? "");
  const [description, setDescription] = useState(initial?.description ?? "");
  const [content, setContent] = useState(initial?.content ?? "");
  const [nameErr, setNameErr] = useState("");

  async function submit() {
    if (!name.trim()) {
      setNameErr("名称不能为空");
      return;
    }
    if (initial) {
      await api.skillUpdate({ id: initial.id, name, description, content });
    } else {
      await api.skillCreate({ name, description, content });
    }
    onDone();
  }

  return (
    <Dialog open onOpenChange={(o) => !o && onClose()}>
      <DialogContent className="sm:max-w-lg">
        <DialogHeader>
          <DialogTitle>{initial ? `编辑「${initial.name}」` : "添加技能"}</DialogTitle>
        </DialogHeader>
        <div className="space-y-4">
          <div className="space-y-1.5">
            <label className="text-sm font-medium">名称</label>
            <Input
              value={name}
              onChange={(e) => {
                setName(e.target.value);
                setNameErr("");
              }}
            />
            {nameErr && (
              <p className="text-xs text-destructive" role="alert">
                {nameErr}
              </p>
            )}
          </div>
          <div className="space-y-1.5">
            <label className="text-sm font-medium">描述</label>
            <Input value={description} onChange={(e) => setDescription(e.target.value)} />
          </div>
          <div className="space-y-1.5">
            <label className="text-sm font-medium">内容/指令</label>
            <Textarea
              className="h-32 font-mono text-xs"
              value={content}
              onChange={(e) => setContent(e.target.value)}
            />
          </div>
        </div>
        <DialogFooter className="gap-2">
          <Button onClick={() => void submit()}>{initial ? "保存" : "创建"}</Button>
          <Button variant="ghost" onClick={onClose}>
            取消
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}
