import { useEffect, useState } from "react";
import { useFieldArray, useForm } from "react-hook-form";
import { zodResolver } from "@hookform/resolvers/zod";
import { z } from "zod";
import {
  ExternalLink,
  KeyRound,
  Pencil,
  PlugZap,
  Radar,
  Plus,
  RefreshCw,
  Trash2,
} from "lucide-react";
import { api } from "../api";
import type { DraftProbeReport, ProviderDto } from "../types";
import { toast } from "../lib/toast";
import { fmtClock } from "../lib/format";
import { cn } from "@/lib/utils";
import { PageHeader } from "@/components/common/PageHeader";
import { StickyFeedback, useFeedback } from "@/components/common/PageFeedback";
import { SkeletonList } from "@/components/common/SkeletonList";
import { EmptyState } from "@/components/common/EmptyState";
import { EffortLevelsEditor } from "@/components/common/EffortLevelsEditor";
import { MaxToolsEditor } from "@/components/common/MaxToolsEditor";
import { FormField } from "@/components/common/FormField";
import { ProbePanel } from "@/components/common/ProbePanel";
import { ConfirmDialog } from "@/components/common/ConfirmDialog";
import { useDirtyGuard } from "@/lib/dirty";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import {
  Card,
  CardContent,
} from "@/components/ui/card";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogBody,
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

const FAMILY_LABEL: Record<string, string> = {
  openai_compat: "OpenAI 兼容",
  openai_responses: "OpenAI Responses",
  anthropic: "Anthropic",
  gemini: "Gemini",
};

const FAMILY_OPTIONS = [
  { value: "openai_compat", label: "OpenAI 兼容（chat/completions）" },
  { value: "openai_responses", label: "OpenAI Responses（/responses）" },
  { value: "anthropic", label: "Anthropic" },
  { value: "gemini", label: "Gemini" },
] as const;

const FAMILY_HINT: Record<string, { placeholder: string; hint: string }> = {
  openai_compat: {
    placeholder: "https://api.deepseek.com/v1",
    hint: "OpenAI 兼容填到 /v1，网关自动拼 /chat/completions",
  },
  openai_responses: {
    placeholder: "https://one-model.com/v1",
    hint: "OpenAI Responses 填到 /v1，网关自动拼 /responses",
  },
  anthropic: {
    placeholder: "https://api.anthropic.com",
    hint: "Anthropic 只填主机根，网关自动拼 /v1/messages",
  },
  gemini: {
    placeholder: "https://generativelanguage.googleapis.com",
    hint: "Gemini 只填主机根，网关自动拼 /v1beta/models",
  },
};

const headerRowSchema = z
  .object({ key: z.string(), value: z.string() })
  .superRefine((row, ctx) => {
    if (row.value.trim() && !row.key.trim()) {
      ctx.addIssue({
        code: z.ZodIssueCode.custom,
        path: ["key"],
        message: "请求头名称不能为空",
      });
    }
  });

const baseSchema = z.object({
  name: z.string().trim().min(1, "名称不能为空"),
  baseUrl: z
    .string()
    .trim()
    .min(1, "Base URL 不能为空")
    .url("需为合法 URL（含 https:// 前缀）"),
  family: z.enum(["openai_compat", "openai_responses", "anthropic", "gemini"]),
  priority: z
    .number({ message: "需为数字" })
    .int("需为整数")
    .min(0, "需 ≥ 0"),
  weight: z
    .number({ message: "需为数字" })
    .int("需为整数")
    .min(1, "需 ≥ 1"),
  extraHeaders: z.array(headerRowSchema),
  website: z
    .union([z.literal(""), z.string().trim().url("需为合法 URL（含 https:// 前缀）")])
    .optional(),
});

const createSchema = baseSchema.extend({
  apiKey: z.string().trim().min(1, "API Key 不能为空"),
});
const editSchema = baseSchema.extend({
  apiKey: z.string(), // 留空 = 不变
});
type FormValues = z.infer<typeof createSchema>;

export function ProvidersPage() {
  const [list, setList] = useState<ProviderDto[]>([]);
  const [busy, setBusy] = useState("");
  // 卡片内的反馈（`ProvidersPage` 一直是这么做的 —— 反馈落在产生它的那张卡里，
  // 是本轮全局整改的样板）；页级反馈（列表加载失败等）走吸顶条。
  const [msg, setMsg] = useState<{ id: string; ok: boolean; text: string } | null>(null);
  const fb = useFeedback();
  const [dialog, setDialog] = useState<
    { mode: "create" } | { mode: "edit"; p: ProviderDto } | null
  >(null);
  const [confirmDelete, setConfirmDelete] = useState<ProviderDto | null>(null);
  const [loading, setLoading] = useState(true);

  async function refresh() {
    setList(await api.providerList());
  }
  useEffect(() => {
    refresh()
      .catch((e) => fb.pageErr(String(e)))
      .finally(() => setLoading(false));
  }, []);

  async function act(id: string, fn: () => Promise<unknown>) {
    setBusy(id);
    setMsg(null);
    try {
      await fn();
      await refresh();
    } catch (e) {
      setMsg({ id, ok: false, text: String(e) });
    } finally {
      setBusy("");
    }
  }

  return (
    <div className="mx-auto max-w-3xl space-y-4">
      <PageHeader
        title="上游供应商"
        description="按优先级与权重路由；凭据随 WebDAV 配置同步，换机拉取即用。"
        actions={
          <Button onClick={() => setDialog({ mode: "create" })}>
            <Plus aria-hidden />
            添加供应商
          </Button>
        }
      />

      <StickyFeedback feedback={fb.page} onDismiss={fb.clearPage} />

      {loading ? (
        <SkeletonList rows={3} />
      ) : (
        <>
      {list.length === 0 && !dialog && (
        <EmptyState
          icon={KeyRound}
          title="还没有供应商"
          description="添加第一个上游渠道 —— 凭据随配置入库并随 WebDAV 同步，导入/拉取后立即可用。"
          className="py-16"
        />
      )}

      <div className="space-y-3">
        {list.map((p) => (
          <ProviderCard
            key={p.id}
            p={p}
            busy={busy === p.id}
            msg={msg?.id === p.id ? msg.text : null}
            onEdit={() => setDialog({ mode: "edit", p })}
            onTest={() =>
              act(p.id, async () => {
                const t = await api.providerTest(p.id);
                setMsg({ id: p.id, ok: true, text: t });
              })
            }
            onDiscover={() =>
              act(p.id, async () => {
                const [total, added] = await api.providerDiscoverModels(p.id);
                setMsg({
                  id: p.id,
                  ok: true,
                  text: `发现 ${total} 个模型，新增 ${added} 个（已有模型不会被覆盖）`,
                });
              })
            }
            onToggle={(v) => act(p.id, () => api.providerSetEnabled(p.id, v))}
            onSetReasoningLevels={async (levels) => {
              await api.providerSetReasoningLevels(p.id, levels);
              setList(await api.providerList());
            }}
            onSetMaxTools={async (maxTools) => {
              await api.providerSetMaxTools(p.id, maxTools);
              setList(await api.providerList());
            }}
            onDelete={() => setConfirmDelete(p)}
          />
        ))}
      </div>
        </>
      )}

      {dialog && (
        <ProviderDialog
          mode={dialog.mode}
          provider={dialog.mode === "edit" ? dialog.p : null}
          onClose={() => setDialog(null)}
          onDone={async () => {
            setDialog(null);
            await refresh();
          }}
        />
      )}

      <ConfirmDialog
        open={!!confirmDelete}
        onOpenChange={(o) => !o && setConfirmDelete(null)}
        title={`删除供应商「${confirmDelete?.name ?? ""}」？`}
        description="其模型映射与凭据将一并清除，操作不可撤销。"
        confirmText="删除"
        destructive
        onConfirm={() => {
          if (confirmDelete) act("", () => api.providerDelete(confirmDelete.id));
        }}
      />
    </div>
  );
}

function FamilyBadge({ family }: { family: string }) {
  return (
    <Badge variant="secondary" className="text-[11px] font-normal">
      {FAMILY_LABEL[family] ?? family}
    </Badge>
  );
}

function ProviderCard(props: {
  p: ProviderDto;
  busy: boolean;
  msg: string | null;
  onTest: () => void;
  onDiscover: () => void;
  onEdit: () => void;
  onToggle: (enabled: boolean) => void;
  onSetReasoningLevels: (levels: string[] | null) => Promise<void>;
  onSetMaxTools: (maxTools: number | null) => Promise<void>;
  onDelete: () => void;
}) {
  const { p } = props;
  const lastFailed =
    !!p.lastErrAt && (!p.lastOkAt || (p.lastErrAt ?? 0) > (p.lastOkAt ?? 0));

  return (
    <Card>
      <CardContent className="space-y-2 p-4">
        <div className="flex items-start gap-3">
          <Switch
            checked={p.enabled}
            onCheckedChange={props.onToggle}
            aria-label={`启用/禁用 ${p.name}`}
          />
          <div className="min-w-0 flex-1">
            <div className="flex flex-wrap items-center gap-2">
              <span className="font-medium text-foreground">{p.name}</span>
              <FamilyBadge family={p.family} />
              <span className="text-xs text-muted-foreground">
                优先级 {p.priority} · 权重 {p.weight}
              </span>
              {!p.hasKey && (
                <Badge variant="outline" className="border-amber-500/40 text-amber-700 dark:text-amber-400">
                  缺少凭据
                </Badge>
              )}
              {p.lastOkAt && !lastFailed && (
                <Badge variant="outline" className="border-emerald-500/30 text-emerald-700 dark:text-emerald-400">
                  最近成功
                </Badge>
              )}
              {lastFailed && (
                <Badge variant="outline" className="border-red-500/30 text-red-700 dark:text-red-400">
                  最近失败
                </Badge>
              )}
            </div>
            <div className="truncate font-mono text-xs text-muted-foreground" title={p.baseUrl}>
              {p.baseUrl}
            </div>
            {/* 「官网」真实盒子做成 ≥24px 高（`py-1` + `-my-1` 抵消，视觉与行距不变）。
                旧写法只靠 `after:-inset-y-1.5` 外扩，probe-hits 实测有效命中区 54×18
                （高只有 18 < 24）：下方「推理档位值域 / 工具上限」两个触发器也有伪元素外扩
                且 DOM 在后，把本按钮的下半截热区抢走了。`z-10` 让本按钮赢下重叠区。 */}
            {p.website && (
              <button
                type="button"
                className="relative z-10 -my-1 inline-flex items-center gap-1 py-1 text-xs text-muted-foreground underline-offset-2 hover:text-foreground hover:underline"
                onClick={() => {
                  api.openWebsite(p.website!).catch((e) =>
                    toast(`打开官网失败: ${e}`)
                  );
                }}
                title={p.website}
              >
                <ExternalLink className="size-3" aria-hidden />
                官网
              </button>
            )}
            {/* 推理档位值域（0011）：供应商级默认，模型行可覆盖。
                未声明的渠道 JAI 原样透传 reasoning_effort，上游不认就 400。 */}
            <div className="mt-1 flex flex-wrap items-center gap-3">
              <EffortLevelsEditor
                verbose
                levels={p.reasoningEffortLevels ?? null}
                scopeLabel={`供应商 ${p.name}`}
                onChange={(levels) =>
                  props
                    .onSetReasoningLevels(levels)
                    .catch((e) => toast(String(e), "err"))
                }
              />
              <MaxToolsEditor
                verbose
                maxTools={p.maxTools ?? null}
                scopeLabel={`供应商 ${p.name}`}
                onChange={(n) =>
                  props.onSetMaxTools(n).catch((e) => toast(String(e), "err"))
                }
              />
            </div>
            {(p.lastOkAt || p.lastErrAt) && (
              <div className="mt-1 text-xs">
                {lastFailed ? (
                  <span className="text-destructive">
                    最近失败（{fmtClock(p.lastErrAt)}）：{p.lastErrMsg}
                  </span>
                ) : (
                  <span className="text-muted-foreground">
                    最近成功：{fmtClock(p.lastOkAt)}
                  </span>
                )}
              </div>
            )}
            {props.msg && (
              <div
                className={cn(
                  "mt-1 text-xs",
                  props.msg.startsWith("连接成功") || props.msg.startsWith("发现")
                    ? "text-emerald-700 dark:text-emerald-400"
                    : "text-destructive",
                )}
              >
                {props.msg}
              </div>
            )}
          </div>
          <div className="flex shrink-0 gap-2">
            <Button
              variant="outline"
              size="sm"
              disabled={props.busy}
              onClick={props.onTest}
            >
              <PlugZap aria-hidden />
              测试
            </Button>
            <Button
              variant="outline"
              size="sm"
              disabled={props.busy}
              onClick={props.onDiscover}
            >
              <RefreshCw aria-hidden />
              拉取模型
            </Button>
            <Button variant="outline" size="sm" onClick={props.onEdit}>
              <Pencil aria-hidden />
              编辑
            </Button>
            <Button
              variant="outline"
              size="sm"
              className="text-destructive hover:bg-destructive/10 hover:text-destructive"
              onClick={props.onDelete}
            >
              <Trash2 aria-hidden />
              删除
            </Button>
          </div>
        </div>
      </CardContent>
    </Card>
  );
}

function ProviderDialog({
  mode,
  provider,
  onDone,
  onClose,
}: {
  mode: "create" | "edit";
  provider: ProviderDto | null;
  onDone: () => void;
  onClose: () => void;
}) {
  const p = mode === "edit" ? provider : null;
  const initialHeaders: FormValues["extraHeaders"] = (() => {
    if (p?.extraHeaders) {
      try {
        const obj = JSON.parse(p.extraHeaders) as Record<string, string>;
        return Object.entries(obj).map(([key, value]) => ({ key, value }));
      } catch {
        /* 忽略坏数据，回退空行 */
      }
    }
    return [];
  })();

  const {
    register,
    handleSubmit,
    control,
    watch,
    getValues,
    setValue,
    formState: { errors, isDirty, isSubmitting },
  } = useForm<FormValues>({
    resolver: zodResolver(mode === "create" ? createSchema : editSchema),
    defaultValues: {
      name: p?.name ?? "",
      baseUrl: p?.baseUrl ?? "",
      family: (p?.family as FormValues["family"]) ?? "openai_compat",
      apiKey: "",
      website: p?.website ?? "",
      priority: p?.priority ?? 100,
      weight: p?.weight ?? 1,
      extraHeaders: initialHeaders,
    },
  });
  const { fields, append, remove } = useFieldArray({ control, name: "extraHeaders" });

  // 未保存改动追踪：本组件只在弹窗打开时挂载（父层是 `{dialog && <ProviderDialog/>}`），
  // 所以 isDirty 天然只在弹窗打开期间参与判定；关弹窗 → 卸载 → 自动注销脏源。
  useDirtyGuard("供应商表单", isDirty);

  const family = watch("family");
  const meta = FAMILY_HINT[family] ?? FAMILY_HINT.openai_compat;

  const [formErr, setFormErr] = useState("");
  const [testMsg, setTestMsg] = useState<{ ok: boolean; text: string } | null>(null);
  const [envOpen, setEnvOpen] = useState(false);

  // ---- 端点探测（D9-T1）：全是弹窗内临时状态，**不进表单、不影响脏状态判定**
  const [probeModel, setProbeModel] = useState("");
  const [allowLoopback, setAllowLoopback] = useState(false);
  const [probeBusy, setProbeBusy] = useState(false);
  const [probeReport, setProbeReport] = useState<DraftProbeReport | null>(null);
  const [probeErr, setProbeErr] = useState("");
  /** 探测目标候选：编辑态取已入库模型；新建态取「测试连接」拉到的模型名 */
  const [modelOptions, setModelOptions] = useState<string[]>([]);

  useEffect(() => {
    if (!p) return;
    let alive = true;
    api
      .modelList(p.id)
      .then((rows) => {
        if (!alive) return;
        const names = rows.map((r) => r.modelName);
        setModelOptions(names);
        setProbeModel((cur) => cur || names[0] || "");
      })
      .catch(() => {
        /* 列模型失败不影响探测（用户可以手填模型名） */
      });
    return () => {
      alive = false;
    };
  }, [p]);

  async function testConnection() {
    setFormErr("");
    setTestMsg(null);
    const v = getValues();
    try {
      const r = await api.providerTestDraft({
        baseUrl: v.baseUrl,
        family: v.family,
        apiKey: v.apiKey,
      });
      setModelOptions(r.modelNames);
      setProbeModel((cur) => cur || r.modelNames[0] || "");
      const preview = r.modelNames.slice(0, 3).join(", ");
      setTestMsg({
        ok: true,
        text:
          `连接成功 · 发现 ${r.count} 个模型` +
          (r.count ? `：${preview}${r.count > 3 ? "…" : ""}` : ""),
      });
    } catch (e) {
      setTestMsg({
        ok: false,
        text: `连接失败：${e}。请检查网络、Base URL、API Key 是否匹配该协议族。`,
      });
    }
  }

  /** 端点探测：对每个候选端点发一次真实的最小推理请求（max_tokens=1）。 */
  async function runProbe() {
    setProbeErr("");
    setProbeReport(null);
    const v = getValues();
    setProbeBusy(true);
    try {
      const headersObj: Record<string, string> = {};
      for (const row of v.extraHeaders) {
        const k = row.key.trim();
        if (k) headersObj[k] = row.value.trim();
      }
      const report = await api.providerProbeDraft({
        baseUrl: v.baseUrl,
        family: v.family,
        apiKey: v.apiKey,
        // 编辑态没重输 key 时，让后端用库里存的 key（否则会以未鉴权身份打上游）
        providerId: p?.id ?? null,
        model: probeModel,
        extraHeaders: Object.keys(headersObj).length
          ? JSON.stringify(headersObj)
          : null,
        allowLoopback,
      });
      setProbeReport(report);
    } catch (e) {
      setProbeErr(String(e));
    } finally {
      setProbeBusy(false);
    }
  }

  const submit = handleSubmit(async (v) => {
    setFormErr("");
    try {
      const headersObj: Record<string, string> = {};
      for (const row of v.extraHeaders) {
        const k = row.key.trim();
        const val = row.value.trim();
        if (k) headersObj[k] = val;
      }
      const eh = Object.keys(headersObj).length ? JSON.stringify(headersObj) : null;
      if (mode === "create") {
        await api.providerCreate({
          name: v.name,
          baseUrl: v.baseUrl,
          family: v.family,
          priority: v.priority,
          weight: v.weight,
          extraHeaders: eh,
          apiKey: v.apiKey,
          website: v.website || null,
          // 门禁 `require_probe_pass` 开启时后端会校验它（默认关闭）
          probeFingerprint: probeReport?.fingerprint ?? null,
        });
      } else if (p) {
        await api.providerUpdate({
          id: p.id,
          name: v.name,
          baseUrl: v.baseUrl,
          priority: v.priority,
          weight: v.weight,
          // 与新建语义一致：有行则整体覆盖为 JSON 串，全空行传 null 显式清空
          extraHeaders: eh,
          apiKey: v.apiKey || undefined,
          website: v.website || null,
          probeFingerprint: probeReport?.fingerprint ?? null,
        });
      }
      toast(mode === "create" ? "供应商已创建" : "已保存");
      onDone();
    } catch (e) {
      setFormErr(String(e));
    }
  });

  return (
    <Dialog open onOpenChange={(o) => !o && onClose()}>
      <DialogContent className="sm:max-w-lg">
        <DialogHeader>
          <DialogTitle>
            {mode === "create" ? "添加供应商" : `编辑「${p?.name}」`}
          </DialogTitle>
          <DialogDescription>
            {mode === "create"
              ? "API Key 明文入库，配置 WebDAV 后随同步携带。"
              : "API Key 留空表示保持现有凭据不变。"}
          </DialogDescription>
        </DialogHeader>

        <form
          className="flex min-h-0 flex-1 flex-col gap-4"
          onSubmit={(e) => void submit(e)}
        >
          <DialogBody className="space-y-4">
            <div className="grid grid-cols-1 gap-4 md:grid-cols-2">
              <FormField label="名称" htmlFor="pf-name" error={errors.name?.message}>
                <Input id="pf-name" placeholder="官方 / 某中转…" {...register("name")} />
              </FormField>
              <FormField
                label="协议族"
                htmlFor="pf-family"
                error={errors.family?.message}
                hint={mode === "edit" ? "协议族创建后不可修改" : undefined}
              >
                <Select
                  value={watch("family")}
                  disabled={mode === "edit"}
                  onValueChange={(v) => {
                    setValue("family", v as FormValues["family"], {
                      shouldValidate: true,
                    });
                  }}
                >
                  <SelectTrigger id="pf-family" className="w-full">
                    <SelectValue />
                  </SelectTrigger>
                  <SelectContent>
                    {FAMILY_OPTIONS.map((o) => (
                      <SelectItem key={o.value} value={o.value}>
                        {o.label}
                      </SelectItem>
                    ))}
                  </SelectContent>
                </Select>
              </FormField>
            </div>

            <FormField
              label="Base URL"
              htmlFor="pf-url"
              error={errors.baseUrl?.message}
              hint={meta.hint}
            >
              <Input
                id="pf-url"
                placeholder={meta.placeholder}
                {...register("baseUrl")}
              />
            </FormField>

            <FormField
              label="官网（可选）"
              htmlFor="pf-website"
              error={errors.website?.message}
            >
              <Input
                id="pf-website"
                placeholder="https://provider.example.com"
                {...register("website")}
              />
            </FormField>

            <FormField
              label="API Key"
              htmlFor="pf-key"
              error={errors.apiKey?.message}
              labelExtra={
                <Button
                  type="button"
                  variant="outline"
                  size="sm"
                  className="h-6 px-2 text-xs"
                  onClick={() => setEnvOpen(true)}
                >
                  从环境变量导入
                </Button>
              }
            >
              <Input
                id="pf-key"
                type="password"
                placeholder={mode === "edit" ? (p?.hasKey ? "•••• 已保存" : "尚未录入") : ""}
                {...register("apiKey")}
              />
            </FormField>

            <div className="grid grid-cols-1 gap-4 md:grid-cols-2">
              <FormField
                label="路由优先级（数字越小越优先）"
                htmlFor="pf-priority"
                error={errors.priority?.message}
              >
                <Input
                  id="pf-priority"
                  type="number"
                  {...register("priority", { valueAsNumber: true })}
                />
              </FormField>
              <FormField
                label="权重（同优先级按比例分发）"
                htmlFor="pf-weight"
                error={errors.weight?.message}
              >
                <Input
                  id="pf-weight"
                  type="number"
                  {...register("weight", { valueAsNumber: true })}
                />
              </FormField>
            </div>

            <div className="space-y-2">
              <div className="text-sm font-medium">追加请求头（可选）</div>
              {fields.map((f, i) => (
                <div key={f.id} className="space-y-1">
                  <div className="flex gap-2">
                    <Input
                      placeholder="Header 名，如 HTTP-Referer"
                      {...register(`extraHeaders.${i}.key` as const)}
                    />
                    <Input placeholder="值" {...register(`extraHeaders.${i}.value` as const)} />
                    <Button
                      type="button"
                      variant="outline"
                      size="icon"
                      className="size-9 shrink-0"
                      aria-label="删除此行"
                      onClick={() => remove(i)}
                    >
                      <Trash2 className="size-4" aria-hidden />
                    </Button>
                  </div>
                  {errors.extraHeaders?.[i]?.key && (
                    <p className="text-xs text-destructive" role="alert">
                      {errors.extraHeaders[i]?.key?.message}
                    </p>
                  )}
                </div>
              ))}
              <Button
                type="button"
                variant="outline"
                size="sm"
                onClick={() => append({ key: "", value: "" })}
              >
                <Plus aria-hidden />
                添加请求头
              </Button>
            </div>

            {formErr && (
              <div className="text-xs text-destructive" role="alert">
                {formErr}
              </div>
            )}
            {testMsg && (
              <div
                className={cn(
                  "text-xs",
                  testMsg.ok ? "text-emerald-700 dark:text-emerald-400" : "text-destructive",
                )}
              >
                {testMsg.text}
              </div>
            )}

            <div className="space-y-2 rounded-md border p-3">
              <div className="flex items-center gap-1.5 text-sm font-medium">
                <Radar className="size-4" aria-hidden />
                端点探测
              </div>
              <p className="text-xs text-muted-foreground">
                对每个候选端点发一次真实的最小推理请求（max_tokens=1），逐端点给出
                「通过 / 失败 + 原因 + 延迟」。只判 HTTP 200 不够——网关拦截页也是
                200，所以这里会校验响应结构。探测会真的调用模型，可能产生计费。
              </p>
              <FormField
                label="探测用模型"
                htmlFor="pf-probe-model"
                hint={
                  modelOptions.length
                    ? "从该渠道已知模型里选，或直接填一个模型名"
                    : "还没有已知模型：可先点「测试连接」拉取模型列表，或直接填模型名"
                }
              >
                <Input
                  id="pf-probe-model"
                  list="pf-probe-model-options"
                  placeholder="如 gpt-4o-mini"
                  value={probeModel}
                  onChange={(e) => setProbeModel(e.target.value)}
                />
                <datalist id="pf-probe-model-options">
                  {modelOptions.map((m) => (
                    <option key={m} value={m} />
                  ))}
                </datalist>
              </FormField>
              <label className="flex items-center gap-2 text-xs text-muted-foreground">
                <Switch checked={allowLoopback} onCheckedChange={setAllowLoopback} />
                允许访问本机地址（用于 Ollama / LM Studio 等本机部署）
              </label>
              <ProbePanel report={probeReport} busy={probeBusy} error={probeErr} />
            </div>

          </DialogBody>

          <DialogFooter className="gap-2 border-t pt-4">

            <Button
              type="button"
              variant="outline"
              disabled={isSubmitting}
              onClick={() => void testConnection()}
            >
              <PlugZap aria-hidden />
              测试连接
            </Button>
            <Button
              type="button"
              variant="outline"
              disabled={isSubmitting || probeBusy}
              onClick={() => void runProbe()}
            >
              <Radar aria-hidden />
              {probeBusy ? "探测中…" : "端点探测"}
            </Button>
            <Button type="submit" disabled={isSubmitting}>
              {mode === "create" ? "创建" : "保存"}
            </Button>
            <Button type="button" variant="ghost" onClick={onClose}>
              取消
            </Button>
          </DialogFooter>
        </form>

        <EnvVarDialog
          open={envOpen}
          onOpenChange={setEnvOpen}
          onImport={(value) => {
            setValue("apiKey", value, { shouldValidate: true });
            toast("已从环境变量导入");
          }}
        />
      </DialogContent>
    </Dialog>
  );
}

/** 替代 window.prompt：输入环境变量名，读取后回填 API Key */
function EnvVarDialog({
  open,
  onOpenChange,
  onImport,
}: {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  onImport: (value: string) => void;
}) {
  const [name, setName] = useState("");
  const [err, setErr] = useState("");

  async function confirm() {
    if (!name.trim()) {
      setErr("环境变量名不能为空");
      return;
    }
    try {
      onImport(await api.readEnvVar(name.trim()));
      onOpenChange(false);
      setName("");
      setErr("");
    } catch (e) {
      setErr(String(e));
    }
  }

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className="sm:max-w-sm">
        <DialogHeader>
          <DialogTitle>从环境变量导入</DialogTitle>
          <DialogDescription>输入环境变量名，例如 DEEPSEEK_API_KEY。</DialogDescription>
        </DialogHeader>
        <Input
          value={name}
          onChange={(e) => setName(e.target.value)}
          onKeyDown={(e) => e.key === "Enter" && void confirm()}
          placeholder="DEEPSEEK_API_KEY"
          autoFocus
        />
        {err && (
          <p className="text-xs text-destructive" role="alert">
            {err}
          </p>
        )}
        <DialogFooter>
          <Button variant="outline" onClick={() => onOpenChange(false)}>
            取消
          </Button>
          <Button onClick={() => void confirm()}>导入</Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}
