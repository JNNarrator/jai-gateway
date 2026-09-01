import { useEffect, useState } from "react";
import { Play, Square } from "lucide-react";
import { api } from "../api";
import type { GatewayKeyInfo } from "../types";
import { toast } from "../lib/toast";
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

export function GatewayPage() {
  const [status, setStatus] = useState<import("../types").GwStatus | null>(null);
  const [key, setKey] = useState<GatewayKeyInfo | null>(null);
  const [revealKey, setRevealKey] = useState<string>("");
  const [busy, setBusy] = useState(false);
  const [err, setErr] = useState("");
  const [confirmRotate, setConfirmRotate] = useState(false);

  const port = status?.port ?? 1314;
  const baseUrl = `http://127.0.0.1:${port}/v1`;

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

  async function doRegen() {
    const k = await api.gatewayKeyRegenerate();
    setKey(k);
    setRevealKey(k.key);
    toast("密钥已轮换");
  }

  /** 不经「显示全文」直接复制完整密钥：内部取全量值，不在界面上展示 */
  async function doCopyKey() {
    try {
      const full = revealKey || (await api.gatewayKeyReveal()).key;
      await navigator.clipboard.writeText(full);
      toast("已复制");
    } catch {
      toast("复制失败", "err");
    }
  }

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
              onCopy={doCopyKey}
              onToggleReveal={() =>
                void (revealKey
                  ? setRevealKey("")
                  : api
                      .gatewayKeyReveal()
                      .then((r) => setRevealKey(r.key))
                      .catch(() => {}))
              }
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
