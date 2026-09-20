import { useEffect, useState } from "react";
import { AlertTriangle, Copy, HeartPulse, Play, Square } from "lucide-react";
import { api } from "../api";
import type { GatewayKeyInfo, GwStatus, HealthSummary } from "../types";
import { toast } from "../lib/toast";
import { goTab } from "../lib/nav";
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
  const [status, setStatus] = useState<GwStatus | null>(null);
  const [key, setKey] = useState<GatewayKeyInfo | null>(null);
  const [revealKey, setRevealKey] = useState<string>("");
  const [busy, setBusy] = useState(false);
  const [err, setErr] = useState("");
  const [confirmRotate, setConfirmRotate] = useState(false);
  const [health, setHealth] = useState<HealthSummary | null>(null);

  const port = status?.port ?? 1314;
  const origin = `http://127.0.0.1:${port}`;
  const baseUrl = `${origin}/v1`;
  const mcpUrl = `${origin}/mcp`;

  /** 完整请求地址：部分客户端把配置项当**精确请求地址**用、不会再补路径
   *  （典型：Reasonix 的「API 地址」即 `request_url`，填 `…/v1` 会打到 `/v1` 上得到 404）。
   *  逐条列出真实端点供这类客户端整条复制；只填 Base URL 的客户端不受影响。 */
  const endpoints = [
    { label: "Chat Completions", method: "POST", url: `${origin}/v1/chat/completions` },
    { label: "Anthropic Messages", method: "POST", url: `${origin}/v1/messages` },
    { label: "Responses", method: "POST", url: `${origin}/v1/responses` },
    { label: "模型列表", method: "GET", url: `${origin}/v1/models` },
    { label: "MCP 元数据", method: "POST", url: mcpUrl },
  ];

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

  useEffect(() => {
    const load = () => api.healthSummary().then(setHealth).catch(() => {});
    load();
    const t = setInterval(load, 30_000);
    return () => clearInterval(t);
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

  /** 复制 mcpServers 配置 JSON：展示用占位符，复制时填入真实密钥 */
  async function doCopyMcpConfig() {
    try {
      const real = revealKey || (await api.gatewayKeyReveal()).key;
      const config = JSON.stringify(
        {
          mcpServers: {
            "jai-registry": {
              type: "http",
              url: mcpUrl,
              headers: { Authorization: `Bearer ${real}` },
            },
          },
        },
        null,
        2,
      );
      await navigator.clipboard.writeText(config);
      toast("已复制（含真实密钥）");
    } catch {
      toast("复制失败", "err");
    }
  }

  /** 复制全部接入地址：一份可直接粘进 Agent 配置的清单（含真实密钥，toast 已注明） */
  async function doCopyEndpoints() {
    try {
      const real = revealKey || (await api.gatewayKeyReveal()).key;
      const text = [
        `JAI 网关接入信息（127.0.0.1:${port}）`,
        "",
        `Base URL  ${baseUrl}`,
        `API Key   ${real}`,
        "",
        "完整请求地址（要求精确地址的客户端用这些，不会再自动补路径）",
        ...endpoints.map((e) => `- ${e.label}（${e.method}）：${e.url}`),
      ].join("\n");
      await navigator.clipboard.writeText(text);
      toast("已复制接入地址（含真实密钥）");
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

      {/* 主操作条：吸顶常驻。此页内容高 ~1093px（默认窗口 764 可视），
          「复制配置」原在 MCP 卡片里，滚动时会被折叠线切掉一截；
          启停与复制是本页高频操作，因此上移到常驻条（状态卡内不再重复放一份）。 */}
      <div className="sticky top-0 z-10 -mx-2 flex flex-wrap items-center gap-2 border-b border-border/60 bg-card px-2 py-3">
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
        <Button variant="outline" disabled={busy} onClick={() => void doCopyMcpConfig()}>
          <Copy aria-hidden />
          复制 MCP 配置
        </Button>
        <Button
          variant="outline"
          disabled={busy}
          onClick={() => void doCopyEndpoints()}
          data-testid="gateway-copy-endpoints"
        >
          <Copy aria-hidden />
          复制完整地址
        </Button>
        <span className="ml-auto font-mono text-xs text-muted-foreground">
          127.0.0.1:{port}
        </span>
      </div>

      {err && (
        <div
          role="alert"
          className="rounded-md border border-destructive/50 bg-destructive/10 px-3 py-2 text-sm text-destructive"
        >
          {err}
        </div>
      )}

      {health?.checkedAtMs != null &&
        (health.down.length > 0 ? (
          <div
            role="alert"
            className="flex flex-wrap items-center gap-2 rounded-md border border-amber-500/40 bg-amber-500/10 px-3 py-2 text-sm"
          >
            <AlertTriangle className="size-4 shrink-0 text-amber-700 dark:text-amber-400" aria-hidden />
            <span className="text-amber-700 dark:text-amber-300">
              上次健康检查 {new Date(health.checkedAtMs).toLocaleTimeString("zh-CN", { hour12: false })} ·
              {health.down.length} 个供应商不可用：
              {health.down.map((d) => d.name).join("、")}
            </span>
            <Button
              variant="outline"
              size="sm"
              className="ml-auto h-7"
              onClick={() => goTab("providers")}
            >
              查看供应商
            </Button>
          </div>
        ) : (
          <div className="flex items-center gap-2 rounded-md border border-emerald-500/30 bg-emerald-500/5 px-3 py-2 text-xs text-emerald-700 dark:text-emerald-300">
            <HeartPulse className="size-4" aria-hidden />
            健康检查正常 · {new Date(health.checkedAtMs).toLocaleTimeString("zh-CN", { hour12: false })}
            （{health.down.length} 个不可用）
          </div>
        ))}

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
            <span className="ml-auto text-xs text-muted-foreground">
              启停与复制配置在顶部常驻操作条
            </span>
          </div>
          <p className="text-xs leading-relaxed text-muted-foreground">
            所有业务端点强制鉴权：Host 仅接受本机回环地址，浏览器跨域需在「设置」中添加白名单。
          </p>
        </CardContent>
      </Card>

      <Card>
        <CardHeader>
          <CardTitle>客户端接入</CardTitle>
          <CardDescription>
            Base URL 与 API Key 是所有客户端都要的两项；要求「精确请求地址」的客户端改用下面的完整地址。
          </CardDescription>
        </CardHeader>
        <CardContent className="space-y-4 text-sm">
          <div className="space-y-1.5" data-testid="gateway-base-url-row">
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

          {/* 完整请求地址与 Base URL 并存：Base URL 供「自己拼路径」的客户端，
              完整地址供「配置项即精确地址」的客户端（典型：Reasonix 的「API 地址」）。 */}
          <div className="space-y-2">
            <div className="flex flex-wrap items-baseline gap-x-2 gap-y-1">
              <span className="text-muted-foreground">完整请求地址</span>
              <span className="text-xs text-muted-foreground">
                客户端要求精确地址时整条复制，不会再自动补路径
              </span>
            </div>
            <ul className="space-y-1.5" data-testid="gateway-endpoints">
              {endpoints.map((e) => (
                <li
                  key={e.url}
                  className="flex items-center gap-2"
                  data-testid="gateway-endpoint"
                  data-url={e.url}
                >
                  <span
                    className="w-36 shrink-0 truncate text-xs text-muted-foreground"
                    title={`${e.label}（${e.method}）`}
                  >
                    {e.label}
                  </span>
                  <span className="w-9 shrink-0 font-mono text-xs text-muted-foreground">
                    {e.method}
                  </span>
                  <CopyField value={e.url} display={e.url} className="min-w-0 flex-1" />
                </li>
              ))}
            </ul>
            <p className="text-xs leading-relaxed text-muted-foreground">
              例：Reasonix 的「API 地址」是<b>精确请求地址</b>（不会再补路径）—— 填{" "}
              <code className="font-mono">{baseUrl}</code> 会打到{" "}
              <code className="font-mono">/v1</code> 上返回 404，应改用 Responses 那一整条。
              多数客户端（OpenAI SDK、dsh 等）只填 Base URL 即可。
            </p>
          </div>
        </CardContent>
      </Card>

      <Card>
        <CardHeader>
          <CardTitle>MCP 元数据服务</CardTitle>
          <CardDescription>
            与网关共用端口和密钥，提供 MCP Server / Skill 台账查询，不代执行工具。
            在客户端的 mcpServers 配置中加入：
          </CardDescription>
        </CardHeader>
        <CardContent className="space-y-3 text-sm">
          <div className="relative">
            <pre className="overflow-x-auto rounded-md border bg-muted/50 p-3 font-mono text-xs leading-relaxed text-foreground">
{`{
  "mcpServers": {
    "jai-registry": {
      "type": "http",
      "url": "${mcpUrl}",
      "headers": {
        "Authorization": "Bearer <网关密钥>"
      }
    }
  }
}`}
            </pre>
          </div>
          <p className="text-xs leading-relaxed text-muted-foreground">
            用顶部常驻操作条的「复制 MCP 配置」复制：自动填入真实密钥，粘贴即用；
            接入后 Agent 可用 list_mcp_servers /
            get_mcp_server_detail / get_tool_schemas / list_skills / get_skill_detail 查询台账。
          </p>
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
