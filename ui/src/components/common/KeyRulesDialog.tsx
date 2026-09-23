import { useEffect, useMemo, useState } from "react";
import { api } from "@/api";
import type { GatewayKeyInfo, KeyRules, RuleOption } from "@/types";
import { toast } from "@/lib/toast";
import { cn } from "@/lib/utils";
import { Button } from "@/components/ui/button";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";

/** 每个条目的三态。用「三态」而不是两个独立勾选框：同一条目既允许又拒绝是无意义的状态。 */
type Tri = "default" | "allow" | "deny";

const TRI_LABEL: Record<Tri, string> = {
  allow: "允许",
  deny: "拒绝",
  default: "默认",
};

/**
 * 与后端 `store::keyrules::axis_allows` **逐字同构**的判定：
 * deny 命中 → 不可用；allow 非空 → 只允许列表内的；都为空 → 不限制。
 *
 * 这里刻意重写一遍而不是调后端：预览要跟着**未保存的草稿**实时变，
 * 每敲一下都问一次后端没有意义。两边的语义靠这一条注释和测试对齐。
 */
function axisAllows(state: Record<string, Tri>, item: string): boolean {
  if (state[item] === "deny") return false;
  const hasAllow = Object.values(state).some((v) => v === "allow");
  if (hasAllow) return state[item] === "allow";
  return true;
}

function stateOf(
  ids: string[],
  allow: string[],
  deny: string[],
): Record<string, Tri> {
  const out: Record<string, Tri> = {};
  for (const id of ids) out[id] = "default";
  // 先写 allow 再写 deny：规则里理论上不会同时出现（后端落库时已按 deny 摘掉 allow），
  // 但万一历史数据里有，按 deny 展示 —— 与后端的判定一致。
  for (const id of allow) out[id] = "allow";
  for (const id of deny) out[id] = "deny";
  return out;
}

function picked(state: Record<string, Tri>, want: Tri): string[] {
  return Object.entries(state)
    .filter(([, v]) => v === want)
    .map(([k]) => k)
    .sort();
}

/** 三态按钮组（允许 / 拒绝 / 默认） */
function TriPicker({
  value,
  onChange,
  label,
  testId,
}: {
  value: Tri;
  onChange: (v: Tri) => void;
  label: string;
  testId: string;
}) {
  return (
    <span className="flex shrink-0 items-center gap-1" role="group" aria-label={label}>
      {(["allow", "deny", "default"] as Tri[]).map((t) => (
        <button
          key={t}
          type="button"
          aria-pressed={value === t}
          data-testid={`${testId}-${t}`}
          onClick={() => onChange(t)}
          className={cn(
            "h-6 rounded border px-2 text-xs transition-colors",
            value === t
              ? t === "deny"
                ? // 「拒绝」选中态用**实心**底 + 白字：浅色主题下 `text-destructive`
                  // 压在 `bg-destructive/10` 上实测对比度只有 3.87（AA 要求 4.5），
                  // 而这是本弹窗里最需要一眼看清的状态（选错就等于放行/封禁反了）。
                  // 与 ConfirmDialog 的危险按钮同一套色，视觉上也自洽。
                  "border-destructive bg-destructive text-white dark:bg-destructive/60 dark:text-white"
                : t === "allow"
                  ? "border-emerald-500/50 bg-emerald-500/10 text-emerald-700 dark:text-emerald-300"
                  : "border-border bg-muted text-foreground"
              : "border-transparent text-muted-foreground hover:bg-muted",
          )}
        >
          {TRI_LABEL[t]}
        </button>
      ))}
    </span>
  );
}

/**
 * 密钥白/黑名单编辑框（D9-T6b）。
 *
 * 形态选择的理由：规则是**按密钥**配的（「这把密钥给谁用、能用什么」），
 * 所以入口在密钥行上，而不是一个全局的规则页 —— 否则每次都要先在规则页里
 * 挑密钥，两个页面来回跳。
 *
 * 同一条目用三态（允许 / 拒绝 / 默认）而不是「两个列表各勾一次」：
 * 既允许又拒绝没有意义，后端也只允许存一个（主键 `(key_id, 条目)`）。
 */
export function KeyRulesDialog({
  keyInfo,
  onOpenChange,
  onSaved,
}: {
  keyInfo: GatewayKeyInfo | null;
  onOpenChange: (open: boolean) => void;
  onSaved?: () => void;
}) {
  const [options, setOptions] = useState<RuleOption[]>([]);
  const [providers, setProviders] = useState<Record<string, Tri>>({});
  const [models, setModels] = useState<Record<string, Tri>>({});
  const [loading, setLoading] = useState(false);
  const [busy, setBusy] = useState(false);

  const keyId = keyInfo?.id ?? null;

  useEffect(() => {
    if (!keyId) return;
    let alive = true;
    setLoading(true);
    Promise.all([api.gatewayKeyRulesGet(keyId), api.gatewayKeyRulesOptions()])
      .then(([rules, opts]) => {
        if (!alive) return;
        setOptions(opts);
        // 候选清单 + 规则里出现过但**当前不在清单**里的条目（渠道被停用 / 模型下架）。
        // 必须带上后者，否则一保存就会把看不见的旧规则静默抹掉。
        const pids = new Set(opts.map((o) => o.providerId));
        const pnames = new Map(opts.map((o) => [o.providerId, o.providerName]));
        const mnames = new Set(opts.map((o) => o.modelName));
        for (const id of [...rules.providerAllow, ...rules.providerDeny]) {
          if (!pids.has(id)) {
            pids.add(id);
            pnames.set(id, id);
          }
        }
        for (const m of [...rules.modelAllow, ...rules.modelDeny]) mnames.add(m);
        setProviders(
          stateOf([...pids], rules.providerAllow, rules.providerDeny),
        );
        setModels(stateOf([...mnames], rules.modelAllow, rules.modelDeny));
      })
      .catch((e) => toast(String(e), "err"))
      .finally(() => {
        if (alive) setLoading(false);
      });
    return () => {
      alive = false;
    };
  }, [keyId]);

  const providerNames = useMemo(() => {
    const m = new Map<string, string>();
    for (const o of options) m.set(o.providerId, o.providerName);
    return m;
  }, [options]);

  const modelNames = useMemo(
    () => [...new Set(options.map((o) => o.modelName))].sort(),
    [options],
  );

  /** 「当前密钥可访问的模型」预览 —— 未保存的草稿也实时反映 */
  const accessible = useMemo(
    () =>
      [
        ...new Set(
          options
            .filter(
              (o) =>
                axisAllows(providers, o.providerId) &&
                axisAllows(models, o.modelName),
            )
            .map((o) => `${o.providerName}/${o.modelName}`),
        ),
      ].sort(),
    [options, providers, models],
  );

  const providerAllow = picked(providers, "allow");
  const providerDeny = picked(providers, "deny");
  const modelAllow = picked(models, "allow");
  const modelDeny = picked(models, "deny");
  const limited =
    providerAllow.length + providerDeny.length + modelAllow.length + modelDeny.length > 0;

  async function save() {
    if (!keyId) return;
    setBusy(true);
    try {
      // 显式标注成 `KeyRules`：DTO 形状一旦和后端对不上，这里编译期就报
      const payload: KeyRules = {
        providerAllow,
        providerDeny,
        modelAllow,
        modelDeny,
      };
      await api.gatewayKeyRulesSet(keyId, payload);
      toast("规则已保存：对该密钥立即生效");
      onSaved?.();
      onOpenChange(false);
    } catch (e) {
      toast(String(e), "err");
    } finally {
      setBusy(false);
    }
  }

  return (
    <Dialog open={!!keyInfo} onOpenChange={onOpenChange}>
      <DialogContent
        className="sm:max-w-2xl"
        data-testid="key-rules-dialog"
        aria-describedby={undefined}
      >
        <DialogHeader>
          <DialogTitle>
            密钥规则 · {keyInfo?.label ?? keyInfo?.prefix ?? ""}
          </DialogTitle>
          <DialogDescription>
            限制这把密钥能用哪些渠道 / 模型。规则**按密钥生效**，其他密钥不受影响。
          </DialogDescription>
        </DialogHeader>

        {/* 说明语义：这三条是本功能的全部规则，用户不需要猜 */}
        <ul
          className="space-y-1 rounded-md border bg-muted/40 px-3 py-2 text-xs leading-relaxed text-muted-foreground"
          data-testid="key-rules-legend"
        >
          <li>
            · <b className="text-foreground">拒绝</b>命中 → 该渠道 / 模型不可用（优先于「允许」）
          </li>
          <li>
            · 一旦有<b className="text-foreground">允许</b>项 → 未列出的渠道 / 模型一律不可用
          </li>
          <li>· 全部「默认」 → 不限制（与未配置规则的密钥行为一致）</li>
        </ul>

        <div className="min-h-0 flex-1 space-y-4 overflow-y-auto pr-1 text-sm">
          {loading ? (
            <p className="text-xs text-muted-foreground">读取规则中…</p>
          ) : (
            <>
              <section className="space-y-1.5">
                <div className="text-xs font-medium text-muted-foreground">渠道</div>
                {providers && Object.keys(providers).length === 0 ? (
                  <p className="text-xs text-muted-foreground">还没有可用的渠道。</p>
                ) : (
                  <ul className="space-y-1" data-testid="key-rules-providers">
                    {Object.keys(providers).map((id) => (
                      <li
                        key={id}
                        className="flex flex-wrap items-center gap-2"
                        data-testid="key-rules-provider-row"
                        data-id={id}
                        data-state={providers[id]}
                      >
                        <span
                          className="min-w-0 flex-1 truncate"
                          title={providerNames.get(id) || id}
                        >
                          {providerNames.get(id) || id}
                        </span>
                        <TriPicker
                          value={providers[id]}
                          label={`渠道 ${providerNames.get(id) || id}`}
                          testId="key-rules-provider"
                          onChange={(v) =>
                            setProviders((prev) => ({ ...prev, [id]: v }))
                          }
                        />
                      </li>
                    ))}
                  </ul>
                )}
              </section>

              <section className="space-y-1.5">
                <div className="text-xs font-medium text-muted-foreground">
                  模型
                  <span className="ml-1 font-normal">
                    （按模型名匹配，同名模型在所有渠道上一起生效）
                  </span>
                </div>
                {modelNames.length === 0 ? (
                  <p className="text-xs text-muted-foreground">还没有可用的模型。</p>
                ) : (
                  <ul className="space-y-1" data-testid="key-rules-models">
                    {modelNames.map((m) => (
                      <li
                        key={m}
                        className="flex flex-wrap items-center gap-2"
                        data-testid="key-rules-model-row"
                        data-id={m}
                        data-state={models[m]}
                      >
                        <span className="min-w-0 flex-1 truncate font-mono text-xs" title={m}>
                          {m}
                        </span>
                        <TriPicker
                          value={models[m]}
                          label={`模型 ${m}`}
                          testId="key-rules-model"
                          onChange={(v) =>
                            setModels((prev) => ({ ...prev, [m]: v }))
                          }
                        />
                      </li>
                    ))}
                  </ul>
                )}
              </section>

              <section className="space-y-1.5">
                <div className="text-xs font-medium text-muted-foreground">
                  当前密钥可访问的模型
                  <span className="ml-1 font-normal">（预览，未保存也实时更新）</span>
                </div>
                <div
                  className="rounded-md border bg-muted/40 px-3 py-2"
                  data-testid="key-rules-preview"
                  data-count={accessible.length}
                >
                  {accessible.length === 0 ? (
                    <span className="text-xs text-destructive">
                      没有可访问的模型 —— 保存后这把密钥调任何模型都会返回 403。
                    </span>
                  ) : (
                    <span className="flex flex-wrap gap-1">
                      {accessible.slice(0, 8).map((n) => (
                        <span
                          key={n}
                          className="rounded border bg-background px-1.5 py-0.5 font-mono text-[11px]"
                          title={n}
                        >
                          {n}
                        </span>
                      ))}
                      {accessible.length > 8 && (
                        <span className="text-xs text-muted-foreground">
                          还有 {accessible.length - 8} 个
                        </span>
                      )}
                    </span>
                  )}
                </div>
                {!limited && (
                  <p className="text-xs text-muted-foreground">
                    当前不限制：这把密钥能用全部渠道与模型。
                  </p>
                )}
              </section>
            </>
          )}
        </div>

        <DialogFooter>
          <Button variant="outline" disabled={busy} onClick={() => onOpenChange(false)}>
            取消
          </Button>
          <Button
            disabled={busy || loading}
            onClick={() => void save()}
            data-testid="key-rules-save"
          >
            保存规则
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}
