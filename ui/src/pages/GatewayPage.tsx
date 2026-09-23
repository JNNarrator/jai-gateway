import { useEffect, useState } from "react";
import {
  AlertTriangle,
  Copy,
  Eye,
  EyeOff,
  HeartPulse,
  MoreHorizontal,
  Play,
  Plus,
  ShieldCheck,
  Square,
  Trash2,
} from "lucide-react";
import { api } from "../api";
import type { GatewayKeyInfo, GwStatus, HealthSummary } from "../types";
import { toast } from "../lib/toast";
import { fmtClock } from "../lib/format";
import { copyText } from "../lib/clipboard";
import { cn } from "@/lib/utils";
import { goTab } from "../lib/nav";
import { PageHeader } from "@/components/common/PageHeader";
import { CopyField } from "@/components/common/CopyField";
import { StatusBadge } from "@/components/common/StatusBadge";
import { ConfirmDialog } from "@/components/common/ConfirmDialog";
import { KeyRulesDialog } from "@/components/common/KeyRulesDialog";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import {
  Card,
  CardContent,
  CardDescription,
  CardHeader,
  CardTitle,
} from "@/components/ui/card";

/** 危险动作的统一样式：**outline + 红字**，只有二次确认框里的按钮才是实心红。
 *
 *  起因（2026-09-23 排版整改）：此前「停止 / 吊销×3 / 轮换全部密钥」五个实心红按钮
 *  同屏抢注意力，而它们严重程度差得很远 —— 停止可恢复、吊销一把是局部不可逆、
 *  轮换是把所有客户端一起踢下线。全实心红等于没有分级。 */
const DANGER_OUTLINE =
  "text-destructive hover:bg-destructive/10 hover:text-destructive";

/** 「最后使用」用相对时间：这是判断「这把密钥还在用吗」的唯一字段，绝对值要心算。 */
function fmtAgo(ms: number | null | undefined): string {
  if (!ms) return "从未使用";
  const d = Date.now() - ms;
  if (d < 60_000) return "刚刚用过";
  if (d < 3_600_000) return `${Math.floor(d / 60_000)} 分钟前用过`;
  if (d < 86_400_000) return `${Math.floor(d / 3_600_000)} 小时前用过`;
  return `${Math.floor(d / 86_400_000)} 天前用过`;
}

export function GatewayPage() {
  const [status, setStatus] = useState<GwStatus | null>(null);
  // 多密钥（D9-T6a）：一把密钥对应一个客户端 / 一个人，可独立吊销与限制。
  const [keys, setKeys] = useState<GatewayKeyInfo[]>([]);
  /** 已「显示全文」的密钥：id → 全文（只在本次会话内存在，不落任何持久化） */
  const [revealed, setRevealed] = useState<Record<string, string>>({});
  const [newLabel, setNewLabel] = useState("");
  const [keyBusy, setKeyBusy] = useState(false);
  const [confirmRevoke, setConfirmRevoke] = useState<GatewayKeyInfo | null>(null);
  /** 正在编辑规则的密钥（null = 弹窗关闭） */
  const [rulesFor, setRulesFor] = useState<GatewayKeyInfo | null>(null);
  /** 已配过规则的密钥 id 集合 —— 列表上给个状态提示，避免用户忘了配过 */
  const [limitedKeys, setLimitedKeys] = useState<Record<string, boolean>>({});
  const [busy, setBusy] = useState(false);
  const [err, setErr] = useState("");
  const [confirmRotate, setConfirmRotate] = useState(false);
  const [health, setHealth] = useState<HealthSummary | null>(null);

  const port = status?.port ?? 1314;
  const origin = `http://127.0.0.1:${port}`;
  const baseUrl = `${origin}/v1`;
  const mcpUrl = `${origin}/mcp`;

  /** 最新一把。`gw_keys_active` 按「新建在前」返回，所以它就是接入客户端该用的那把；
   *  「接入信息」里的复制动作也一律以它为准（界面上写明了是哪一把，不再有歧义）。 */
  const latest = keys[0];
  const latestFull = latest ? revealed[latest.id] : undefined;

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

  const loadKeys = async () => {
    try {
      const list = await api.gatewayKeyList();
      setKeys(list);
      // 规则摘要（列表上显示「已限制 / 不限」状态）：失败不影响密钥列表本身
      const flags: Record<string, boolean> = {};
      await Promise.all(
        list.map(async (k) => {
          try {
            const r = await api.gatewayKeyRulesGet(k.id);
            flags[k.id] =
              r.providerAllow.length +
                r.providerDeny.length +
                r.modelAllow.length +
                r.modelDeny.length >
              0;
          } catch {
            flags[k.id] = false;
          }
        }),
      );
      setLimitedKeys(flags);
    } catch (e) {
      setErr(String(e));
    }
  };

  useEffect(() => {
    void loadKeys();
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

  /** 取「最新一把」的全文：已在界面上显示过就用现成的，否则现取一次。
   *  **始终按 keys[0] 走**，不复用「用户上次点开过哪一把」—— 那会让复制到的密钥
   *  与界面上标着的那把不一致。 */
  async function latestKeyText(): Promise<string> {
    if (latestFull) return latestFull;
    return (await api.gatewayKeyReveal()).key;
  }

  async function doRegen() {
    setKeyBusy(true);
    try {
      const k = await api.gatewayKeyRegenerate();
      setRevealed({ [k.id]: k.key });
      await loadKeys();
      toast("已轮换：旧密钥全部失效，新密钥已显示在列表最前");
    } catch (e) {
      toast(String(e), "err");
    } finally {
      setKeyBusy(false);
    }
  }

  /** 新建一把密钥（不动旧密钥）：返回值带全文，这是唯一一次能拿到全文的时机 */
  async function doCreateKey() {
    setKeyBusy(true);
    try {
      const k = await api.gatewayKeyCreate(newLabel || null);
      setNewLabel("");
      setRevealed({ [k.id]: k.key });
      await loadKeys();
      toast("已新建密钥：请当场复制，之后列表只显示前缀");
    } catch (e) {
      toast(String(e), "err");
    } finally {
      setKeyBusy(false);
    }
  }

  /** 显示 / 隐藏某一把密钥的全文（入口：行内前缀本身，或「⋯」菜单） */
  async function doToggleReveal(k: GatewayKeyInfo) {
    if (revealed[k.id]) {
      setRevealed((prev) => {
        const next = { ...prev };
        delete next[k.id];
        return next;
      });
      return;
    }
    try {
      const full = (await api.gatewayKeyReveal(k.id)).key;
      setRevealed((prev) => ({ ...prev, [k.id]: full }));
    } catch (e) {
      toast(String(e), "err");
    }
  }

  async function doCopyOne(k: GatewayKeyInfo) {
    try {
      const full = revealed[k.id] || (await api.gatewayKeyReveal(k.id)).key;
      await navigator.clipboard.writeText(full);
      toast(`已复制「${k.label ?? k.prefix}」的密钥`);
    } catch {
      toast("复制失败", "err");
    }
  }

  async function doRevoke() {
    const k = confirmRevoke;
    setConfirmRevoke(null);
    if (!k) return;
    setKeyBusy(true);
    try {
      const changed = await api.gatewayKeyRevoke(k.id);
      setRevealed((prev) => {
        const next = { ...prev };
        delete next[k.id];
        return next;
      });
      await loadKeys();
      toast(changed ? "已吊销：该密钥立即失效" : "该密钥已不存在");
    } catch (e) {
      toast(String(e), "err");
    } finally {
      setKeyBusy(false);
    }
  }

  /** 复制 mcpServers 配置 JSON：展示用占位符，复制时填入真实密钥 */
  async function doCopyMcpConfig() {
    try {
      const real = await latestKeyText();
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
      const real = await latestKeyText();
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
      toast("已复制接入信息（含真实密钥）");
    } catch {
      toast("复制失败", "err");
    }
  }

  return (
    <div className="mx-auto max-w-4xl space-y-4">
      <PageHeader
        title="网关"
        description="本机回环地址上的 OpenAI 兼容入口。上面是「接入信息」（客户端要什么），下面是「密钥管理」（多把密钥怎么管）。"
      />

      {/* 主操作条：吸顶常驻。启停与复制是本页高频操作，且滚动时不该被折叠线切掉。 */}
      {/* data-slot=page-actions：探针契约，fold.mjs 据此认定「本页主操作」并断言首屏可达
          （页级主操作 + 吸顶条内按钮都算；不靠类名/文本猜）。 */}
      <div
        data-slot="page-actions"
        className="sticky top-0 z-10 -mx-2 flex flex-wrap items-center gap-2 border-b border-border/60 bg-card px-2 py-3"
      >
        {status?.running ? (
          // 「停止」是可恢复动作，不该和「吊销 / 轮换」抢同一个实心红（见 DANGER_OUTLINE）
          <Button
            variant="outline"
            className={DANGER_OUTLINE}
            disabled={busy}
            onClick={() => toggle(false)}
          >
            <Square aria-hidden />
            停止
          </Button>
        ) : (
          <Button disabled={busy} onClick={() => toggle(true)}>
            <Play aria-hidden />
            启动
          </Button>
        )}
        <Button
          variant="outline"
          disabled={busy}
          onClick={() => void doCopyEndpoints()}
          data-testid="gateway-copy-endpoints"
        >
          <Copy aria-hidden />
          复制接入信息
        </Button>
        <Button variant="outline" disabled={busy} onClick={() => void doCopyMcpConfig()}>
          <Copy aria-hidden />
          复制 MCP 配置
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
              className="ml-auto"
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
        <CardContent className="space-y-2">
          <span className="block font-mono text-lg font-bold text-foreground">
            127.0.0.1:{port}
          </span>
          <p className="text-xs leading-relaxed text-muted-foreground">
            所有业务端点强制鉴权：Host 仅接受本机回环地址，浏览器跨域需在「设置」中添加白名单。
          </p>
        </CardContent>
      </Card>

      {/* ── 接入信息：客户端要填的东西（消费侧） ─────────────────────────────
          与「密钥管理」分开的理由：此前两者同在一张「客户端接入」卡里，于是密钥行
          的 4 个按钮（规则 / 显示全文 / 复制 / 吊销）和接入用的复制字段挤在一起，
          分不清哪些是「拿去用」、哪些是「管起来」。 */}
      <Card>
        <CardHeader>
          <CardTitle>接入信息</CardTitle>
          <CardDescription>
            Base URL 与 API Key 是所有客户端都要的两项；要求「精确请求地址」的客户端改用下面的完整地址。
          </CardDescription>
        </CardHeader>
        <CardContent className="space-y-4 text-sm">
          <div className="space-y-1.5" data-testid="gateway-base-url-row">
            <div className="text-muted-foreground">Base URL</div>
            <CopyField value={baseUrl} display={baseUrl} />
          </div>

          <div className="space-y-1.5" data-testid="gateway-api-key-row">
            <div className="flex flex-wrap items-baseline gap-x-2 gap-y-1">
              <span className="text-muted-foreground">API Key</span>
              <span className="text-xs text-muted-foreground">
                {latest
                  ? `用最新那一把（${latest.label ?? "未命名"}，${latest.prefix}…）；其余密钥在下面的「密钥管理」`
                  : "当前没有可用密钥"}
              </span>
            </div>
            {latest ? (
              <CopyField
                value={latestFull ?? ""}
                display={latestFull ?? `${latest.prefix}…`}
                onCopy={async () => copyText(await latestKeyText())}
                onToggleReveal={() => void doToggleReveal(latest)}
                revealed={!!latestFull}
              />
            ) : (
              <p className="text-xs text-muted-foreground">
                在下面的「密钥管理」里新建一把即可接入客户端。
              </p>
            )}
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

      {/* ── 密钥管理：多把密钥（管理侧） ─────────────────────────────────── */}
      <Card>
        <CardHeader>
          <CardTitle className="flex items-center gap-2.5">
            密钥管理
            <span className="rounded-full border border-border px-2 py-0.5 text-[11px] font-normal text-muted-foreground">
              {keys.length} 把
            </span>
          </CardTitle>
          <CardDescription>
            每个客户端 / 人一把，可独立吊销与限制。常态只显示前缀 —— 点前缀或「⋯」里的「显示全文」看全文。
          </CardDescription>
        </CardHeader>
        <CardContent className="space-y-4 text-sm">
          {keys.length === 0 ? (
            <p className="text-xs text-muted-foreground" data-testid="gateway-keys-empty">
              当前没有可用密钥。新建一把即可接入客户端。
            </p>
          ) : (
            <ul className="space-y-1.5" data-testid="gateway-key-list">
              {keys.map((k) => {
                const full = revealed[k.id];
                const limited = !!limitedKeys[k.id];
                return (
                  <li
                    key={k.id}
                    className="flex flex-wrap items-center gap-x-3 gap-y-1.5 rounded-md border border-border/60 px-3 py-2"
                    data-testid="gateway-key-row"
                    data-prefix={k.prefix}
                  >
                    {/* 前缀本身就是「显示全文」的入口：要看的和要看的东西是同一个，
                        再单列一个按钮只是多一个要读的控件。 */}
                    <button
                      type="button"
                      data-testid="gateway-key-prefix"
                      aria-label={`${full ? "隐藏" : "显示"}全文 ${k.prefix}`}
                      title={full ? "点一下隐藏全文" : `${k.prefix}…（点一下显示全文）`}
                      // `min-h-6` + inline-flex：真实盒子 ≥24px（UI 门禁硬要求）。
                      // 文字本身只有 16px 高，纯文本按钮的命中区会不达标。
                      className="inline-flex min-h-6 shrink-0 items-center rounded font-mono text-xs text-foreground underline decoration-dotted underline-offset-2 hover:decoration-solid"
                      onClick={() => void doToggleReveal(k)}
                    >
                      {full ?? `${k.prefix}…`}
                    </button>
                    <span
                      data-testid="gateway-key-label"
                      className="w-32 shrink-0 truncate text-xs text-muted-foreground"
                      title={k.label ?? "未命名"}
                    >
                      {k.label ?? "未命名"}
                    </span>
                    <span
                      data-testid="gateway-key-used"
                      className="w-28 shrink-0 truncate text-xs text-muted-foreground"
                      title={`创建于 ${fmtClock(k.createdAt)}`}
                    >
                      {fmtAgo(k.lastUsedAt)}
                    </span>
                    <span className="ml-auto flex shrink-0 items-center gap-2">
                      {/* 规则既是状态也是入口：直接写清「这把密钥能用到什么」，
                          比第 4 个叫「规则」的按钮信息量大。 */}
                      <button
                        type="button"
                        data-testid="gateway-key-rules"
                        data-limited={limited ? "1" : "0"}
                        aria-label={`设置规则 ${k.prefix}`}
                        title={
                          limited
                            ? "这把密钥配了渠道 / 模型规则，点一下修改"
                            : "未限制：能用全部渠道与模型，点一下设置规则"
                        }
                        className={cn(
                          // `min-h-6`：同上，chip 也是可点控件，真实盒子必须 ≥24px
                          "inline-flex min-h-6 shrink-0 items-center gap-1 rounded-full border px-2 text-[11px] transition-colors",
                          limited
                            ? "border-amber-500/50 bg-amber-500/10 text-amber-700 dark:text-amber-300"
                            : "border-border text-muted-foreground hover:bg-muted hover:text-foreground",
                        )}
                        onClick={() => setRulesFor(k)}
                      >
                        <ShieldCheck className="size-3" aria-hidden />
                        {limited ? "已限制" : "不限"}
                      </button>
                      <Button
                        variant="outline"
                        size="sm"
                        aria-label={`复制密钥 ${k.prefix}`}
                        onClick={() => void doCopyOne(k)}
                      >
                        <Copy aria-hidden />
                        复制
                      </Button>
                      <DropdownMenu>
                        <DropdownMenuTrigger asChild>
                          <Button
                            variant="outline"
                            size="icon-sm"
                            aria-label={`更多操作 ${k.prefix}`}
                          >
                            <MoreHorizontal aria-hidden />
                          </Button>
                        </DropdownMenuTrigger>
                        <DropdownMenuContent align="end">
                          <DropdownMenuItem onSelect={() => void doToggleReveal(k)}>
                            {full ? <EyeOff aria-hidden /> : <Eye aria-hidden />}
                            {full ? "隐藏全文" : "显示全文"}
                          </DropdownMenuItem>
                          <DropdownMenuSeparator />
                          <DropdownMenuItem
                            variant="destructive"
                            disabled={keyBusy}
                            onSelect={() => setConfirmRevoke(k)}
                          >
                            <Trash2 aria-hidden />
                            吊销这把密钥
                          </DropdownMenuItem>
                        </DropdownMenuContent>
                      </DropdownMenu>
                    </span>
                  </li>
                );
              })}
            </ul>
          )}

          <div className="flex flex-wrap items-center gap-2 border-t border-border/60 pt-4">
            <Input
              className="h-9 w-56 text-sm"
              placeholder="备注（发给谁，可空）"
              value={newLabel}
              aria-label="新密钥备注"
              onChange={(e) => setNewLabel(e.target.value)}
            />
            <Button size="sm" disabled={keyBusy} onClick={() => void doCreateKey()}>
              <Plus aria-hidden />
              新建密钥
            </Button>
            <span className="text-xs text-muted-foreground">
              新建不会影响已有密钥，也不会把任何人踢下线
            </span>
          </div>

          {/* 危险区：单独一块、带说明。轮换是「把所有客户端一起换掉」，
              与「新建一把」完全不是一类操作，不该并排放在同一行里。 */}
          <div className="flex flex-wrap items-center gap-x-4 gap-y-2 rounded-md border border-destructive/30 bg-destructive/5 px-3 py-2.5">
            <div className="min-w-0 flex-1 text-xs leading-relaxed text-muted-foreground">
              <b className="text-foreground">轮换全部密钥</b>：把所有密钥一起换掉，
              <b className="text-foreground">全部旧密钥立即失效</b>，所有已配置的客户端都要更新。
              只想换某一个客户端，请用上面的「新建密钥」+ 单独吊销旧的那把。
            </div>
            <Button
              variant="outline"
              size="sm"
              className={DANGER_OUTLINE}
              disabled={keyBusy}
              onClick={() => setConfirmRotate(true)}
            >
              轮换全部密钥
            </Button>
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
            用顶部常驻操作条的「复制 MCP 配置」复制：自动填入最新那一把真实密钥，粘贴即用；
            接入后 Agent 可用 list_mcp_servers /
            get_mcp_server_detail / get_tool_schemas / list_skills / get_skill_detail 查询台账。
          </p>
        </CardContent>
      </Card>

      <KeyRulesDialog
        keyInfo={rulesFor}
        onOpenChange={(o) => !o && setRulesFor(null)}
        onSaved={() => void loadKeys()}
      />

      <ConfirmDialog
        open={confirmRotate}
        onOpenChange={setConfirmRotate}
        title="轮换全部网关密钥？"
        description="这是「一键换掉所有密钥」：全部旧密钥立即失效，所有已配置的客户端都需要更新。只想给某个客户端换一把，请用「新建密钥」+ 单独吊销旧的那把。"
        confirmText="全部轮换"
        destructive
        onConfirm={doRegen}
      />

      <ConfirmDialog
        open={!!confirmRevoke}
        onOpenChange={(o) => !o && setConfirmRevoke(null)}
        title={`吊销「${confirmRevoke?.label ?? confirmRevoke?.prefix ?? ""}」？`}
        description="该密钥立即失效，使用它的客户端会收到 401；其他密钥不受影响。吊销是软删，会保留记录。"
        confirmText="吊销"
        destructive
        onConfirm={doRevoke}
      />
    </div>
  );
}
