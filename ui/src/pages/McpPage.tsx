import { useEffect, useState } from "react";
import { ClipboardPaste, Pencil, PlugZap, Plus, ListTree, Trash2 } from "lucide-react";
import { api } from "../api";
import type { McpServerRow } from "../types";
import { toast } from "../lib/toast";
import { copyText } from "../lib/clipboard";
import { cn } from "@/lib/utils";
import { PageHeader } from "@/components/common/PageHeader";
import { SkeletonList } from "@/components/common/SkeletonList";
import { EmptyState } from "@/components/common/EmptyState";
import { ConfirmDialog } from "@/components/common/ConfirmDialog";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { Input } from "@/components/ui/input";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import { Switch } from "@/components/ui/switch";

/** 安全解析 env JSON 的键名列表；非法或非对象时返回占位文案，避免渲染抛错白屏 */
function envKeySummary(env: string): string {
  try {
    const parsed: unknown = JSON.parse(env);
    if (parsed && typeof parsed === "object" && !Array.isArray(parsed)) {
      return Object.keys(parsed).join(", ");
    }
  } catch {
    /* 非法 JSON，落入下方占位文案 */
  }
  return "(非法 JSON)";
}

export function McpPage() {
  const [list, setList] = useState<McpServerRow[]>([]);
  const [msg, setMsg] = useState("");
  const [err, setErr] = useState("");
  const [dialog, setDialog] = useState<
    { mode: "create" } | { mode: "edit"; row: McpServerRow } | { mode: "import" } | null
  >(null);
  const [confirmDelete, setConfirmDelete] = useState<McpServerRow | null>(null);
  const [loading, setLoading] = useState(true);

  async function refresh() {
    setList(await api.mcpList());
  }
  useEffect(() => {
    refresh()
      .catch((e) => setErr(String(e)))
      .finally(() => setLoading(false));
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
      <PageHeader
        title="MCP Server 管理"
        description="本机 MCP Server 登记台账。网关不再把工具注入对话链路；Agent 可通过网关 /mcp 元数据服务查询此台账（见「网关」页接入说明）。"
        actions={
          <div className="flex items-center gap-2">
            <Button
              variant="outline"
              size="sm"
              onClick={() =>
                act(async () => {
                  const cfg = await api.mcpExportConfig();
                  copyText(JSON.stringify(cfg, null, 2));
                  toast("已复制客户端 MCP 配置");
                })
              }
            >
              <ClipboardPaste aria-hidden />
              复制客户端配置
            </Button>
            <Button
              variant="outline"
              size="sm"
              onClick={() => setDialog({ mode: "import" })}
            >
              <ClipboardPaste aria-hidden />
              粘贴配置导入
            </Button>
            <Button size="sm" onClick={() => setDialog({ mode: "create" })}>
              <Plus aria-hidden />
              添加 MCP Server
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
        <div className="rounded-md border border-primary/40 bg-primary/10 px-3 py-2 text-sm break-all text-primary">
          {msg}
        </div>
      )}

      {loading ? (
        <SkeletonList rows={3} />
      ) : (
        <>
      {list.length === 0 && !dialog && (
        <EmptyState
          icon={PlugZap}
          title="还没有 MCP Server"
          description="点击上方按钮添加一个 stdio / SSE / HTTP 类型的服务，或直接粘贴 mcpServers JSON / codex mcp add 命令 / Codex TOML 配置导入。"
          className="py-16"
        />
      )}

      <div className="space-y-3">
        {list.map((m) => (
          <div
            key={m.id}
            className="rounded-lg border bg-card p-4 text-card-foreground shadow-sm"
          >
            <div className="flex items-center gap-3">
              <Switch
                checked={m.enabled}
                onCheckedChange={(v) => act(() => api.mcpSetEnabled(m.id, v))}
                aria-label={`启用/停用 ${m.name}`}
              />
              <label className="flex shrink-0 cursor-pointer items-center gap-1.5">
                <Switch
                  checked={m.proxyAllowed}
                  onCheckedChange={(v) => act(() => api.mcpSetProxyAllowed(m.id, v))}
                  aria-label={`允许代理执行 ${m.name}`}
                />
                <span className="text-xs text-muted-foreground">代理</span>
              </label>
              <div className="min-w-0 flex-1">
                <div className="flex flex-wrap items-center gap-2">
                  <span className="font-medium text-foreground">{m.name}</span>
                  <Badge variant="secondary" className="text-[11px] font-normal">
                    {m.kind}
                  </Badge>
                  {!m.enabled && <Badge variant="outline">已停用</Badge>}
                </div>
                <div className="truncate font-mono text-xs text-muted-foreground">
                  {m.kind === "stdio"
                    ? `${m.command ?? ""} ${m.args ?? ""}`
                    : m.url ?? ""}
                  {m.env ? (
                    <span
                      className="text-muted-foreground/60"
                      title={m.env}
                    >
                      {" "}
                      env:{" "}
                      {envKeySummary(m.env)}
                    </span>
                  ) : null}
                </div>
              </div>
              <div className="flex shrink-0 gap-2">
                <Button
                  variant="outline"
                  size="sm"
                  onClick={() =>
                    act(async () => {
                      await api.mcpToolsList(m.id);
                      toast(`MCP「${m.name}」连接正常`);
                    })
                  }
                >
                  <PlugZap aria-hidden />
                  测试连接
                </Button>
                <Button
                  variant="outline"
                  size="sm"
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
                  <ListTree aria-hidden />
                  列出工具
                </Button>
                <Button
                  variant="outline"
                  size="sm"
                  onClick={() => setDialog({ mode: "edit", row: m })}
                >
                  <Pencil aria-hidden />
                  编辑
                </Button>
                <Button
                  variant="outline"
                  size="sm"
                  className="text-destructive hover:bg-destructive/10 hover:text-destructive"
                  onClick={() => setConfirmDelete(m)}
                >
                  <Trash2 aria-hidden />
                  删除
                </Button>
              </div>
            </div>
          </div>
        ))}
      </div>

      {dialog?.mode === "import" && (
        <McpImportDialog
          onClose={() => setDialog(null)}
          onDone={(report) => {
            setDialog(null);
            setMsg(
              `导入完成：新增 ${report.imported}，更新 ${report.updated}${
                report.skipped.length ? `，跳过 ${report.skipped.length}` : ""
              }`,
            );
            refresh();
          }}
        />
      )}
      {(dialog?.mode === "create" || dialog?.mode === "edit") && (
        <McpDialog
          initial={dialog.mode === "edit" ? dialog.row : undefined}
          onClose={() => setDialog(null)}
          onDone={() => {
            setDialog(null);
            setMsg("MCP Server 已保存");
            setTimeout(() => setMsg(""), 2000);
            refresh();
          }}
        />
      )}
        </>
      )}

      <ConfirmDialog
        open={!!confirmDelete}
        onOpenChange={(o) => !o && setConfirmDelete(null)}
        title={`删除 MCP Server「${confirmDelete?.name ?? ""}」？`}
        description="删除后网关不再合并其工具，操作不可撤销。"
        confirmText="删除"
        destructive
        onConfirm={() => {
          if (confirmDelete) act(() => api.mcpDelete(confirmDelete.id));
        }}
      />
    </div>
  );
}

function McpImportDialog({
  onDone,
  onClose,
}: {
  onDone: (report: { imported: number; updated: number; skipped: string[] }) => void;
  onClose: () => void;
}) {
  const [text, setText] = useState("");
  const [busy, setBusy] = useState(false);
  const [localErr, setLocalErr] = useState("");

  async function submit() {
    if (!text.trim()) {
      setLocalErr("请粘贴 MCP 配置（JSON / codex 命令行 / TOML 均可）");
      return;
    }
    setBusy(true);
    setLocalErr("");
    try {
      const report = await api.mcpImport(text);
      onDone(report);
    } catch (e) {
      setLocalErr(String(e));
      setBusy(false);
    }
  }

  return (
    <Dialog open onOpenChange={(o) => !o && onClose()}>
      <DialogContent className="sm:max-w-lg">
        <DialogHeader>
          <DialogTitle>粘贴配置导入 MCP Server</DialogTitle>
          <DialogDescription>
            自动识别三种格式，同名服务会被更新，不含 command/url 的条目自动跳过：
            <br />
            ① Claude Code JSON：
            {` {"mcpServers": {"名称": {"command": "...", "args": [...], "env": {...}}}}`}
            <br />
            ② Codex 命令行：
            {` codex mcp add 名称 --env K=V -- "命令" [参数...]`}
            <br />
            ③ Codex config.toml：
            {` [mcp_servers.名称] command = "..." / url = "..."`}
          </DialogDescription>
        </DialogHeader>
        <textarea
          className="min-h-[160px] w-full resize-y rounded-md border bg-transparent px-3 py-2 font-mono text-xs text-foreground placeholder:text-muted-foreground"
          value={text}
          onChange={(e) => {
            setText(e.target.value);
            setLocalErr("");
          }}
          placeholder={'codex mcp add my-server --env "KEY=value" -- "C:\\path\\to\\mcp.cmd"'}
          spellCheck={false}
        />
        {localErr && (
          <p className="text-xs text-destructive" role="alert">
            {localErr}
          </p>
        )}
        <DialogFooter className="gap-2">
          <Button
            type="button"
            variant="outline"
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
            <ClipboardPaste aria-hidden />
            粘贴
          </Button>
          <Button onClick={() => void submit()} disabled={busy}>
            {busy ? "导入中…" : "导入"}
          </Button>
          <Button variant="ghost" onClick={onClose}>
            取消
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}

function McpDialog({
  initial,
  onDone,
  onClose,
}: {
  initial?: McpServerRow;
  onDone: () => void;
  onClose: () => void;
}) {
  const [name, setName] = useState(initial?.name ?? "");
  const [kind, setKind] = useState<string>(initial?.kind ?? "stdio");
  const [command, setCommand] = useState(initial?.command ?? "");
  const [args, setArgs] = useState(initial?.args ?? "");
  const [url, setUrl] = useState(initial?.url ?? "");
  const [env, setEnv] = useState(initial?.env ?? "");
  const [argsErr, setArgsErr] = useState("");
  const [nameErr, setNameErr] = useState("");
  const [envErr, setEnvErr] = useState("");

  async function submit() {
    if (!name.trim()) {
      setNameErr("名称不能为空");
      return;
    }
    setNameErr("");
    if (kind === "stdio" && args.trim()) {
      try {
        const parsed = JSON.parse(args);
        if (!Array.isArray(parsed)) throw new Error("必须是数组");
        setArgsErr("");
      } catch {
        setArgsErr('参数需为合法 JSON 数组，例如 ["-y","包名"]');
        return;
      }
    }
    if (env.trim()) {
      try {
        const parsed: unknown = JSON.parse(env);
        if (!parsed || typeof parsed !== "object" || Array.isArray(parsed)) {
          throw new Error("必须是对象");
        }
        setEnvErr("");
      } catch {
        setEnvErr('环境变量需为合法 JSON 对象，例如 {"API_KEY":"xxx"}');
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
    <Dialog open onOpenChange={(o) => !o && onClose()}>
      <DialogContent className="sm:max-w-lg">
        <DialogHeader>
          <DialogTitle>{initial ? `编辑「${initial.name}」` : "添加 MCP Server"}</DialogTitle>
          <DialogDescription>
            stdio 类型由网关拉起子进程；sse / http 走远程 URL。
          </DialogDescription>
        </DialogHeader>
        <div className="grid grid-cols-1 gap-4 md:grid-cols-2">
          <div className="space-y-1.5">
            <label className="text-sm font-medium">名称</label>
            <Input value={name} onChange={(e) => { setName(e.target.value); setNameErr(""); }} />
            {nameErr && (
              <p className="text-xs text-destructive" role="alert">
                {nameErr}
              </p>
            )}
          </div>
          <div className="space-y-1.5">
            <label className="text-sm font-medium">类型</label>
            <Select value={kind} onValueChange={setKind}>
              <SelectTrigger className="w-full">
                <SelectValue />
              </SelectTrigger>
              <SelectContent>
                <SelectItem value="stdio">stdio</SelectItem>
                <SelectItem value="sse">sse</SelectItem>
                <SelectItem value="http">http</SelectItem>
              </SelectContent>
            </Select>
          </div>
          {kind === "stdio" ? (
            <>
              <div className="space-y-1.5">
                <label className="text-sm font-medium">命令</label>
                <Input
                  className="font-mono"
                  value={command}
                  onChange={(e) => setCommand(e.target.value)}
                  placeholder="npx / node / python"
                />
              </div>
              <div className="space-y-1.5">
                <label className="text-sm font-medium">参数（JSON 数组）</label>
                <Input
                  className={cn("font-mono", argsErr && "border-destructive")}
                  value={args}
                  onChange={(e) => {
                    setArgs(e.target.value);
                    setArgsErr("");
                  }}
                  placeholder='["-y","@modelcontextprotocol/server-filesystem"]'
                />
                {argsErr && (
                  <p className="text-xs text-destructive" role="alert">
                    {argsErr}
                  </p>
                )}
              </div>
              <div className="space-y-1.5 md:col-span-2">
                <label className="text-sm font-medium">环境变量（JSON 对象，可选）</label>
                <Input
                  className={cn("font-mono", envErr && "border-destructive")}
                  value={env}
                  onChange={(e) => {
                    setEnv(e.target.value);
                    setEnvErr("");
                  }}
                  placeholder='{"API_KEY":"xxx"}'
                />
                {envErr && (
                  <p className="text-xs text-destructive" role="alert">
                    {envErr}
                  </p>
                )}
              </div>
            </>
          ) : (
            <div className="space-y-1.5 md:col-span-2">
              <label className="text-sm font-medium">URL</label>
              <Input
                className="font-mono"
                value={url}
                onChange={(e) => setUrl(e.target.value)}
                placeholder="https://mcp.example.com/sse"
              />
            </div>
          )}
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
