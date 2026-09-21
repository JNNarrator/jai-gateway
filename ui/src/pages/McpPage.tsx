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
import { useDirtyGuard } from "@/lib/dirty";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import {
  Dialog,
  DialogBody,
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
import {
  Tooltip,
  TooltipContent,
  TooltipProvider,
  TooltipTrigger,
} from "@/components/ui/tooltip";

/** 两个 per-server 开关的说明文案（顶部提示条 / hover 详情共用，避免两处漂移） */
const SWITCH_HELP = {
  enabled: {
    label: "启用",
    short: "网关是否连接它 —— 关闭后不进入台账与工具列表，也不能被代理执行",
    detail:
      "是否让网关连接这个 MCP Server。关闭后：它不出现在台账与 /mcp 工具列表里，也无法被代理执行（即使「代理执行」仍开着）。",
  },
  proxy: {
    label: "代理执行",
    short:
      "是否让 Agent 能调它的工具 —— 开启后工具以 <server>__<tool> 暴露给 Agent 并经网关转发执行",
    detail:
      "是否把这个 Server 的工具交给 Agent：开启后其工具以 <server>__<tool> 出现在 /mcp 工具列表，Agent 可主动调用、由网关转发给真实 Server 执行。需先「启用」；默认关闭（最小权限），有状态/长时工具建议先评估。",
  },
} as const;

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
    <div className="mx-auto max-w-4xl space-y-4">
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

      {list.length > 0 && (
        <div
          className="flex flex-wrap gap-x-5 gap-y-1 rounded-md border bg-muted/40 px-3 py-2 text-xs text-muted-foreground"
          data-testid="mcp-switch-help"
        >
          <span>
            <span className="font-medium text-foreground">{SWITCH_HELP.enabled.label}</span>
            ：{SWITCH_HELP.enabled.short}
          </span>
          <span>
            <span className="font-medium text-foreground">{SWITCH_HELP.proxy.label}</span>
            ：{SWITCH_HELP.proxy.short}
          </span>
        </div>
      )}

      <div className="space-y-3">
        <TooltipProvider delayDuration={200}>
        {list.map((m) => (
          <div
            key={m.id}
            className="rounded-lg border bg-card p-4 text-card-foreground shadow-sm"
          >
            <div className="flex flex-wrap items-center gap-x-3 gap-y-2">
              <div className="flex shrink-0 items-center gap-3">
                <Tooltip>
                  <TooltipTrigger asChild>
                    <label className="flex cursor-pointer items-center gap-1.5">
                      <Switch
                        checked={m.enabled}
                        onCheckedChange={(v) => act(() => api.mcpSetEnabled(m.id, v))}
                        aria-label={`启用/停用 ${m.name}`}
                      />
                      <span className="text-xs text-muted-foreground">
                        {SWITCH_HELP.enabled.label}
                      </span>
                    </label>
                  </TooltipTrigger>
                  <TooltipContent className="max-w-xs text-left">
                    {SWITCH_HELP.enabled.detail}
                  </TooltipContent>
                </Tooltip>
                <Tooltip>
                  <TooltipTrigger asChild>
                    <label className="flex cursor-pointer items-center gap-1.5">
                      <Switch
                        checked={m.proxyAllowed}
                        onCheckedChange={(v) => act(() => api.mcpSetProxyAllowed(m.id, v))}
                        aria-label={`允许代理执行 ${m.name}`}
                      />
                      <span
                        className={cn(
                          "text-xs",
                          // 停用时该开关虽然还能保留配置，但此刻不生效 —— 标签随之变淡
                          m.enabled ? "text-muted-foreground" : "text-muted-foreground/50",
                        )}
                      >
                        {SWITCH_HELP.proxy.label}
                      </span>
                    </label>
                  </TooltipTrigger>
                  <TooltipContent className="max-w-xs text-left">
                    {SWITCH_HELP.proxy.detail}
                  </TooltipContent>
                </Tooltip>
              </div>
              <div className="min-w-0 flex-1 basis-40">
                <div className="flex flex-wrap items-center gap-2">
                  <span className="font-medium text-foreground">{m.name}</span>
                  <Badge variant="secondary" className="text-[11px] font-normal">
                    {m.kind}
                  </Badge>
                  {!m.enabled && <Badge variant="outline">已停用</Badge>}
                  {/* 「已停用 + 代理执行开」是最容易被误读的组合：光看开关看不出此刻不生效。
                      放在徽标行（而非下方 truncate 的路径行），否则会被裁掉看不见。 */}
                  {!m.enabled && m.proxyAllowed && (
                    <span
                      className="rounded bg-muted px-1.5 py-0.5 text-[11px] text-muted-foreground"
                      data-testid="mcp-proxy-inactive"
                    >
                      停用中，代理执行暂不生效
                    </span>
                  )}
                </div>
                {/* 路径/URL 会被截断（310px 容器放不下），补 title 让 hover 可读全量 */}
                <div
                  className="truncate font-mono text-xs text-muted-foreground"
                  title={
                    m.kind === "stdio"
                      ? `${m.command ?? ""} ${m.args ?? ""}`
                      : m.url ?? ""
                  }
                >
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
                  aria-label={`测试连接 ${m.name}`}
                  title="测试连接"
                  onClick={() =>
                    act(async () => {
                      await api.mcpToolsList(m.id);
                      toast(`MCP「${m.name}」连接正常`);
                    })
                  }
                >
                  <PlugZap aria-hidden />
                  <span className="hidden lg:inline">测试连接</span>
                </Button>
                <Button
                  variant="outline"
                  size="sm"
                  aria-label={`列出工具 ${m.name}`}
                  title="列出工具"
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
                  <span className="hidden lg:inline">列出工具</span>
                </Button>
                <Button
                  variant="outline"
                  size="sm"
                  aria-label={`编辑 ${m.name}`}
                  title="编辑"
                  onClick={() => setDialog({ mode: "edit", row: m })}
                >
                  <Pencil aria-hidden />
                  <span className="hidden lg:inline">编辑</span>
                </Button>
                <Button
                  variant="outline"
                  size="sm"
                  className="text-destructive hover:bg-destructive/10 hover:text-destructive"
                  aria-label={`删除 ${m.name}`}
                  title="删除"
                  onClick={() => setConfirmDelete(m)}
                >
                  <Trash2 aria-hidden />
                  <span className="hidden lg:inline">删除</span>
                </Button>
              </div>
            </div>
          </div>
        ))}
        </TooltipProvider>
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
        <DialogBody>
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
                </DialogBody>

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

  // 未保存改动追踪：本组件只在弹窗打开时挂载（父层是 `{dialog?.mode === ... && <McpDialog/>}`），
  // 所以这里逐字段与「打开时的初始值」比对即可，关闭弹窗 → 卸载 → 自动注销脏源。
  const dirty =
    name !== (initial?.name ?? "") ||
    kind !== (initial?.kind ?? "stdio") ||
    command !== (initial?.command ?? "") ||
    args !== (initial?.args ?? "") ||
    url !== (initial?.url ?? "") ||
    env !== (initial?.env ?? "");
  useDirtyGuard("MCP Server 表单", dirty);

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
        <DialogBody>
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
                </DialogBody>

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
