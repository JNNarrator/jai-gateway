import { useEffect, useState } from "react";
import { useFieldArray, useForm } from "react-hook-form";
import { zodResolver } from "@hookform/resolvers/zod";
import { z } from "zod";
import {
  KeyRound,
  Pencil,
  PlugZap,
  Plus,
  RefreshCw,
  Trash2,
} from "lucide-react";
import { api } from "../api";
import type { ProviderDto } from "../types";
import { toast } from "../lib/toast";
import { fmtClock } from "../lib/format";
import { cn } from "@/lib/utils";
import { PageHeader } from "@/components/common/PageHeader";
import { SkeletonList } from "@/components/common/SkeletonList";
import { EmptyState } from "@/components/common/EmptyState";
import { FormField } from "@/components/common/FormField";
import { ConfirmDialog } from "@/components/common/ConfirmDialog";
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
  const [msg, setMsg] = useState<{ id: string; ok: boolean; text: string } | null>(null);
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
      .catch((e) => setMsg({ id: "", ok: false, text: String(e) }))
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
        description="按优先级与权重路由；凭据只写入系统钥匙串，数据库不落明文。"
        actions={
          <Button onClick={() => setDialog({ mode: "create" })}>
            <Plus aria-hidden />
            添加供应商
          </Button>
        }
      />

      {msg && !msg.id && (
        <div
          role="alert"
          className="rounded-md border border-destructive/50 bg-destructive/10 px-3 py-2 text-sm text-destructive"
        >
          {msg.text}
        </div>
      )}

      {loading ? (
        <SkeletonList rows={3} />
      ) : (
        <>
      {list.length === 0 && !dialog && (
        <EmptyState
          icon={KeyRound}
          title="还没有供应商"
          description="添加第一个上游渠道 —— API Key 会立即写入系统钥匙串，数据库只保存引用地址，绝不落盘明文。"
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
        description="其模型映射与钥匙串凭据将一并清除，操作不可撤销。"
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
                <Badge variant="outline" className="border-amber-500/40 text-amber-600 dark:text-amber-400">
                  缺少凭据
                </Badge>
              )}
              {p.lastOkAt && !lastFailed && (
                <Badge variant="outline" className="border-emerald-500/30 text-emerald-600 dark:text-emerald-400">
                  最近成功
                </Badge>
              )}
              {lastFailed && (
                <Badge variant="outline" className="border-red-500/30 text-red-600 dark:text-red-400">
                  最近失败
                </Badge>
              )}
            </div>
            <div className="truncate font-mono text-xs text-muted-foreground">
              {p.baseUrl}
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
                    ? "text-emerald-600 dark:text-emerald-400"
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
    formState: { errors, isSubmitting },
  } = useForm<FormValues>({
    resolver: zodResolver(mode === "create" ? createSchema : editSchema),
    defaultValues: {
      name: p?.name ?? "",
      baseUrl: p?.baseUrl ?? "",
      family: (p?.family as FormValues["family"]) ?? "openai_compat",
      apiKey: "",
      priority: p?.priority ?? 100,
      weight: p?.weight ?? 1,
      extraHeaders: initialHeaders,
    },
  });
  const { fields, append, remove } = useFieldArray({ control, name: "extraHeaders" });

  const family = watch("family");
  const meta = FAMILY_HINT[family] ?? FAMILY_HINT.openai_compat;

  const [formErr, setFormErr] = useState("");
  const [testMsg, setTestMsg] = useState<{ ok: boolean; text: string } | null>(null);
  const [envOpen, setEnvOpen] = useState(false);

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
      <DialogContent className="max-h-[85vh] overflow-y-auto sm:max-w-lg">
        <DialogHeader>
          <DialogTitle>
            {mode === "create" ? "添加供应商" : `编辑「${p?.name}」`}
          </DialogTitle>
          <DialogDescription>
            {mode === "create"
              ? "API Key 会立即写入系统钥匙串，数据库只保存引用地址。"
              : "API Key 留空表示保持现有凭据不变。"}
          </DialogDescription>
        </DialogHeader>

        <form className="space-y-4" onSubmit={(e) => void submit(e)}>
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
            label="API Key（凭据存储方式见设置页）"
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
              placeholder={mode === "edit" ? (p?.hasKey ? "•••• 已存于钥匙串" : "尚未录入") : ""}
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
                testMsg.ok ? "text-emerald-600 dark:text-emerald-400" : "text-destructive",
              )}
            >
              {testMsg.text}
            </div>
          )}

          <DialogFooter className="gap-2">
            <Button
              type="button"
              variant="outline"
              disabled={isSubmitting}
              onClick={() => void testConnection()}
            >
              <PlugZap aria-hidden />
              测试连接
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
