import { useEffect, useState } from "react";
import { Copy, Play, Square } from "lucide-react";
import { api } from "../api";
import type { GatewayKeyInfo } from "../types";
import { toast } from "../lib/toast";
import { copyText } from "../lib/clipboard";
import { PageHeader } from "@/components/common/PageHeader";
import { CopyField } from "@/components/common/CopyField";
import { StatusBadge } from "@/components/common/StatusBadge";
import { ConfirmDialog } from "@/components/common/ConfirmDialog";
import { Button } from "@/components/ui/button";
import {
  Card,
  CardContent,
  CardDescription,
  CardHeader,
  CardTitle,
} from "@/components/ui/card";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";

const SNIPPETS: { id: "deepseek" | "openai" | "curl"; label: string }[] = [
  { id: "deepseek", label: "DeepSeek Harness" },
  { id: "openai", label: "OpenAI SDK" },
  { id: "curl", label: "curl" },
];

export function GatewayPage() {
  const [status, setStatus] = useState<import("../types").GwStatus | null>(null);
  const [key, setKey] = useState<GatewayKeyInfo | null>(null);
  const [revealKey, setRevealKey] = useState<string>("");
  const [busy, setBusy] = useState(false);
  const [err, setErr] = useState("");
  const [sdk, setSdk] = useState<"deepseek" | "openai" | "curl">("deepseek");
  const [confirmRotate, setConfirmRotate] = useState(false);

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
      <PageHeader
        title="网关"
        description="本机回环地址上的 OpenAI 兼容入口，启动后即可接入各类 Agent 客户端。"
      />

      {err && (
        <div
          role="alert"
          className="rounded-md border border-destructive/50 bg-destructive/10 px-3 py-2 text-sm text-destructive"
        >
          {err}
        </div>
      )}

      <Card>
        <CardHeader>
          <CardTitle className="flex items-center gap-2.5">
            网关状态
            <StatusBadge tone={status?.running ? "ok" : "idle"}>
              {status?.running ? "运行中" : "已停止"}
            </StatusBadge>
          </CardTitle>
          <CardDescription>
            {status?.running
              ? `监听 127.0.0.1:${port}，端口被占用会自动顺延（1314 起）。`
              : "网关未运行。端口被占用会自动顺延（1314 起）。"}
          </CardDescription>
        </CardHeader>
        <CardContent className="space-y-3">
          <div className="flex flex-wrap items-center gap-3">
            <span className="font-mono text-lg font-bold text-foreground">
              127.0.0.1:{port}
            </span>
            <div className="ml-auto">
              {status?.running ? (
                <Button variant="destructive" disabled={busy} onClick={() => toggle(false)}>
                  <Square aria-hidden />
                  停止
                </Button>
              ) : (
                <Button disabled={busy} onClick={() => toggle(true)}>
                  <Play aria-hidden />
                  启动
                </Button>
              )}
            </div>
          </div>
          <p className="text-xs leading-relaxed text-muted-foreground">
            所有业务端点强制鉴权：Host 仅接受本机回环地址，浏览器跨域需在「设置」中添加白名单。
          </p>
        </CardContent>
      </Card>

      <Card>
        <CardHeader>
          <CardTitle>客户端接入</CardTitle>
          <CardDescription>把下面两个字段填进任意 OpenAI 兼容客户端即可。</CardDescription>
        </CardHeader>
        <CardContent className="space-y-4 text-sm">
          <div className="space-y-1.5">
            <div className="text-muted-foreground">Base URL</div>
            <CopyField value={baseUrl} display={baseUrl} />
          </div>
          <div className="space-y-1.5">
            <div className="text-muted-foreground">API Key</div>
            <CopyField
              value={revealKey}
              display={
                revealKey
                  ? revealKey
                  : key
                    ? `${key.prefix}…（点击右侧显示全文）`
                    : "加载中"
              }
              copyDisabled={!revealKey}
              onToggleReveal={doReveal}
              revealed={!!revealKey}
            >
              <Button
                variant="destructive"
                size="sm"
                className="h-9 shrink-0"
                onClick={() => setConfirmRotate(true)}
              >
                轮换密钥
              </Button>
            </CopyField>
          </div>
          <div className="space-y-1.5">
            <div className="flex items-center justify-between">
              <span className="text-muted-foreground">接入示例</span>
              <Select value={sdk} onValueChange={(v) => setSdk(v as typeof sdk)}>
                <SelectTrigger size="sm" className="w-44">
                  <SelectValue />
                </SelectTrigger>
                <SelectContent>
                  {SNIPPETS.map((s) => (
                    <SelectItem key={s.id} value={s.id}>
                      {s.label}
                    </SelectItem>
                  ))}
                </SelectContent>
              </Select>
            </div>
            <div className="relative">
              <pre className="overflow-x-auto rounded-md bg-neutral-950 p-3 pr-12 font-mono text-xs leading-relaxed text-zinc-300 dark:bg-black/60">
                {snippet}
              </pre>
              <button
                className="absolute right-2 top-2 rounded-md border border-neutral-700 p-1.5 text-zinc-400 hover:text-zinc-100"
                onClick={() => copyText(snippet)}
                aria-label="复制接入示例"
              >
                <Copy className="size-3.5" aria-hidden />
              </button>
            </div>
          </div>
        </CardContent>
      </Card>

      <Card>
        <CardHeader>
          <CardTitle>配置迁移</CardTitle>
          <CardDescription>
            密钥不会包含在导出文件中；导入后需在「供应商」页补录 API Key。
          </CardDescription>
        </CardHeader>
        <CardContent>
          <Button variant="outline" onClick={doExport}>
            导出配置（无敏感字段）
          </Button>
        </CardContent>
      </Card>

      <ConfirmDialog
        open={confirmRotate}
        onOpenChange={setConfirmRotate}
        title="轮换网关密钥？"
        description="旧密钥将立即失效，已配置的客户端需要更新为新密钥。"
        confirmText="轮换"
        destructive
        onConfirm={doRegen}
      />
    </div>
  );
}
