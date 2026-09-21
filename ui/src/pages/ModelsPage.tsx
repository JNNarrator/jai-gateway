import { useEffect, useState, type ReactNode } from "react";
import { ArrowUpDown, Boxes, Copy, Search } from "lucide-react";
import { api } from "../api";
import type { Modality, ModelRow, ProviderDto } from "../types";
import { toast } from "../lib/toast";
import { copyText } from "../lib/clipboard";
import { PageHeader } from "@/components/common/PageHeader";
import { EmptyState } from "@/components/common/EmptyState";
import { EffortLevelsEditor } from "@/components/common/EffortLevelsEditor";
import { MaxToolsEditor } from "@/components/common/MaxToolsEditor";
import { SkeletonList } from "@/components/common/SkeletonList";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import {
  Sheet,
  SheetBody,
  SheetContent,
  SheetDescription,
  SheetFooter,
  SheetHeader,
  SheetTitle,
} from "@/components/ui/sheet";
import { Switch } from "@/components/ui/switch";
import {
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow,
} from "@/components/ui/table";

/**
 * 模型默认值页 —— **只读列表 + 右侧详情抽屉**。
 *
 * 为什么不再做「行内编辑表格」（2026-09-21 UI 优化专题）：
 * 旧实现每行 9 个控件（2 个数字 Input + 别名 Input + 模态下拉 + 推理档位芯片 +
 * 工具上限芯片 + 启用开关 + 复制 + 保存），21 行 = **194 个可交互控件、117 个在折叠线下**，
 * 首屏只有 44%；900×600 下 7 列表宽 732 > 容器 724（溢出 8px），行内输入还被折叠线切 28px。
 * 而且新增的「推理档位值域 / 工具声明数上限」两个芯片把「模型名」列撑宽，
 * 正是 §3 第 5 条自己记下的教训（降级 badge 的文案长度直接决定列宽）。
 *
 * 现在：表格只读（每行仅「复制 / 启用 / 详情」三个控件），全部编辑项收进抽屉一次保存；
 * 表格加**吸顶表头**（与日志页一致），并给 Table 加 `table-fixed`，
 * 让列宽由表头声明决定、不再被单元格内容撑破容器。
 */
export function ModelsPage() {
  const [providers, setProviders] = useState<ProviderDto[]>([]);
  const [selId, setSelId] = useState<string>("");
  const [models, setModels] = useState<ModelRow[]>([]);
  const [q, setQ] = useState("");
  const [sortAsc, setSortAsc] = useState(true);
  const [loading, setLoading] = useState(true);
  /** 详情抽屉：只存 id，数据始终从 models 里取最新的那份，避免抽屉里拿着陈旧快照 */
  const [detailId, setDetailId] = useState<string | null>(null);

  useEffect(() => {
    api.providerList().then(setProviders).catch(() => {});
  }, []);

  useEffect(() => {
    if (providers.length && !selId) setSelId(providers[0].id);
  }, [providers, selId]);

  const reload = async (providerId: string) => {
    const next = await api.modelList(providerId);
    setModels(next);
    return next;
  };

  useEffect(() => {
    if (!selId) return;
    setLoading(true);
    setDetailId(null);
    api
      .modelList(selId)
      .then(setModels)
      .catch(() => {})
      .finally(() => setLoading(false));
  }, [selId]);

  const filtered = models
    .filter((m) => m.modelName.toLowerCase().includes(q.trim().toLowerCase()))
    .sort((a, b) =>
      sortAsc
        ? a.modelName.localeCompare(b.modelName)
        : b.modelName.localeCompare(a.modelName)
    );

  async function setAll(enabled: boolean) {
    try {
      for (const m of filtered) {
        if (m.enabled !== enabled) {
          await api.modelToggle(m.id, enabled);
        }
      }
      if (selId) await reload(selId);
      toast(enabled ? "已全部启用" : "已全部禁用");
    } catch (e) {
      toast(String(e), "err");
    }
  }

  const detail = detailId ? models.find((m) => m.id === detailId) ?? null : null;

  return (
    <div className="mx-auto max-w-4xl space-y-4">
      <PageHeader
        title="模型默认值"
        description="定义每个模型的上下文窗口、最大输出、上游映射与多模态标注；关闭的模型不参与路由。"
        actions={
          <div className="flex items-center gap-2">
            <Button variant="outline" size="sm" onClick={() => void setAll(true)}>
              全部启用
            </Button>
            <Button variant="outline" size="sm" onClick={() => void setAll(false)}>
              全部禁用
            </Button>
          </div>
        }
      />

      <div className="flex flex-wrap items-center gap-2">
        <Select value={selId} onValueChange={setSelId}>
          <SelectTrigger className="w-56" aria-label="选择供应商">
            <SelectValue
              placeholder={providers.length === 0 ? "先在「供应商」页添加并拉取模型" : "选择供应商"}
            />
          </SelectTrigger>
          <SelectContent>
            {providers.map((p) => (
              <SelectItem key={p.id} value={p.id}>
                {p.name}
              </SelectItem>
            ))}
          </SelectContent>
        </Select>
        <div className="relative">
          <Search
            className="pointer-events-none absolute left-2.5 top-1/2 size-4 -translate-y-1/2 text-muted-foreground"
            aria-hidden
          />
          <Input
            className="w-56 pl-8"
            placeholder="搜索模型名…"
            aria-label="搜索模型名"
            value={q}
            onChange={(e) => setQ(e.target.value)}
          />
        </div>
        <Button
          variant="outline"
          size="sm"
          onClick={() => setSortAsc(!sortAsc)}
          title="按名称排序"
        >
          <ArrowUpDown aria-hidden />
          名称 {sortAsc ? "升序" : "降序"}
        </Button>
      </div>
      <p className="text-xs text-muted-foreground">
        这些值是跨协议转换与调用方提示的基础；仅本机模型名对齐时直通也会透传。
        点行内「详情」可编辑该模型的全部配置。
      </p>

      {loading ? (
        <SkeletonList rows={6} itemClassName="h-12" />
      ) : models.length === 0 && selId ? (
        <EmptyState
          icon={Boxes}
          title="该供应商还没有模型"
          description="在「供应商」页点击「拉取模型」自动发现入库，然后回到这里配置默认值。"
        />
      ) : (
        <>
          {/* `table-fixed`：列宽由表头声明决定。旧实现是自动布局，单元格内容（行内输入、
              推理档位芯片）会把表撑得比容器宽 —— 900×600 下实测溢出 8px。 */}
          {/* 吸顶表头的**必要**条件（踩坑记录 2026-09-21，与日志页同因）：
              `Table` 基座自带一层 `div[data-slot=table-container].overflow-x-auto`，
              它的 `overflow-y` 会随之计算成 `auto` → 它是 thead 的**最近可滚动祖先**。
              而它自己没有纵向溢出，于是 `sticky top-0` 完全失效（实测滚动 596px 后
              `th.top` 213 → -383，表头照旧滚走）。把 `max-height` + `overflow:auto` 加到
              **这一层**（而不是外面再套一个 div），它才成为真正的纵向滚动容器，sticky 才生效。
              注：`border-collapse` 与「sticky 放 thead 还是 th」都不是原因（都试过，无效）。 */}
          <div className="rounded-lg border [&_[data-slot=table-container]]:max-h-[calc(100dvh-18rem)] [&_[data-slot=table-container]]:overflow-auto">
            <Table className="table-fixed">
              <TableHeader className="sticky top-0 z-10 bg-muted/50 backdrop-blur">
                <TableRow className="hover:bg-muted/50">
                  <TableHead className="w-[34%]">模型名</TableHead>
                  <TableHead className="w-[22%]">上游模型 ID</TableHead>
                  <TableHead className="w-[13%]">上下文</TableHead>
                  <TableHead className="w-[13%]">最大输出</TableHead>
                  <TableHead className="hidden w-[12%] text-center lg:table-cell">
                    模态（入/出）
                  </TableHead>
                  <TableHead className="w-16 text-center">启用</TableHead>
                  <TableHead className="w-16 text-right">操作</TableHead>
                </TableRow>
              </TableHeader>
              <TableBody className="divide-y">
                {filtered.map((m) => (
                  <TableRow key={m.id} className={m.enabled ? "" : "opacity-50"}>
                    <TableCell className="font-mono text-xs">
                      <span className="inline-flex min-w-0 items-center gap-1">
                        <span className="truncate" title={m.modelName}>
                          {m.modelName}
                        </span>
                        {/* 视觉仍是 16×16 图标，但**真实盒子**做成 24×24（WCAG 2.2 AA 最小目标尺寸），
                            用 `-m-1` 抵消多出来的 8px，表格行高不变。
                            为什么不用 `after:-inset-2` 伪元素外扩（旧写法）：probe-hits 逐点实测有效命中区
                            只有 **6×30** —— 右侧紧邻的元素也有伪元素外扩、且 DOM 在后、paint 在上，
                            把那一侧的 8px 全抢走了。`z-10` 让本按钮赢下重叠区。 */}
                        <button
                          className="relative z-10 -m-1 grid size-6 shrink-0 place-items-center rounded text-muted-foreground/60 hover:bg-muted hover:text-foreground"
                          title="复制模型名"
                          aria-label={`复制模型名 ${m.modelName}`}
                          onClick={() => copyText(m.modelName)}
                        >
                          <Copy className="size-3" aria-hidden />
                        </button>
                        {/* <lg 时「模态（入/出）」列被隐藏（列降级），模态信息并回本列，避免信息凭空消失 */}
                        <ModalityCompact m={m} />
                      </span>
                    </TableCell>
                    <TableCell className="font-mono text-xs text-muted-foreground">
                      <span className="block truncate" title={m.upstreamModelId ?? "（同名）"}>
                        {m.upstreamModelId ?? "（同名）"}
                      </span>
                    </TableCell>
                    <TableCell className="text-xs tabular-nums text-muted-foreground">
                      {fmtNum(m.contextWindow)}
                    </TableCell>
                    <TableCell className="text-xs tabular-nums text-muted-foreground">
                      {fmtNum(m.maxOutputTokens)}
                    </TableCell>
                    <TableCell className="hidden text-center lg:table-cell">
                      <span className="flex flex-col items-center gap-0.5">
                        <span className="flex items-center gap-1">
                          <span className="text-[11px] text-muted-foreground">入</span>
                          <ModalityBadges list={m.inputModalities} />
                        </span>
                        <span className="flex items-center gap-1">
                          <span className="text-[11px] text-muted-foreground">出</span>
                          <ModalityBadges list={m.outputModalities} />
                        </span>
                      </span>
                    </TableCell>
                    <TableCell className="text-center">
                      <Switch
                        checked={m.enabled}
                        onCheckedChange={(v) => {
                          void api
                            .modelToggle(m.id, v)
                            .then(() => reload(selId))
                            .catch((e) => toast(String(e), "err"));
                        }}
                        aria-label={`启用 ${m.modelName}`}
                        className="mx-auto"
                      />
                    </TableCell>
                    <TableCell className="text-right">
                      <Button
                        variant="ghost"
                        size="sm"
                        className="h-7 px-2"
                        aria-label={`编辑详情 ${m.modelName}`}
                        onClick={() => setDetailId(m.id)}
                      >
                        详情
                      </Button>
                    </TableCell>
                  </TableRow>
                ))}
              </TableBody>
            </Table>
            {filtered.length === 0 && (
              <div className="px-4 py-8 text-center text-sm text-muted-foreground">
                没有匹配的模型
              </div>
            )}
          </div>
          {/* 窄窗（<1024px）下「模态（入/出）」列隐藏、该信息并回「模型名」列的 badge */}
          <p className="hidden text-[11px] text-muted-foreground max-lg:block">
            窗口较窄：模态标注已并入「模型名」列（悬停可见完整集合），其余列可左右滑动。
          </p>
        </>
      )}

      {/* key={detail.id}：换模型时强制重挂载，抽屉一律以该模型的最新数据起草，
          不会沿用上一个模型的残留（与「添加/编辑」弹窗的 onOpenChange 重起草同一思路）。 */}
      {detail && (
        <ModelDetailSheet
          key={detail.id}
          m={detail}
          onClose={() => setDetailId(null)}
          onSaved={async () => {
            await reload(selId);
          }}
        />
      )}
    </div>
  );
}

/** 数字列的展示格式：未知显示「—」，千位分隔 */
function fmtNum(n: number | null | undefined) {
  if (n == null) return "—";
  return n.toLocaleString("en-US");
}

// ---------------------------------------------------------------- 详情抽屉

/**
 * 单个模型的全部编辑项，**一次保存**（与「添加/编辑」弹窗同一套交互形态：
 * 三段式 Header/Body/Footer、Footer 常驻、保存中禁用、成功 toast、失败 toast）。
 */
function ModelDetailSheet({
  m,
  onClose,
  onSaved,
}: {
  m: ModelRow;
  onClose: () => void;
  onSaved: () => Promise<void>;
}) {
  const [alias, setAlias] = useState(m.upstreamModelId ?? "");
  const [ctx, setCtx] = useState(String(m.contextWindow ?? 128000));
  const [out, setOut] = useState(String(m.maxOutputTokens));
  const [inputModalities, setInputModalities] = useState<Modality[]>(m.inputModalities ?? []);
  const [outputModalities, setOutputModalities] = useState<Modality[]>(m.outputModalities ?? []);
  const [levels, setLevels] = useState<string[] | null>(m.reasoningEffortLevels ?? null);
  const [maxTools, setMaxTools] = useState<number | null>(m.maxTools ?? null);
  const [enabled, setEnabled] = useState(m.enabled);
  const [saving, setSaving] = useState(false);

  async function save() {
    setSaving(true);
    try {
      const ctxNum = Number(ctx);
      const outNum = Number(out);
      if (!Number.isFinite(outNum) || outNum <= 0) {
        toast("最大输出必须是正整数", "err");
        return;
      }
      await api.modelSetLimits({
        modelId: m.id,
        contextWindow: Number.isFinite(ctxNum) && ctxNum > 0 ? ctxNum : null,
        maxOutputTokens: outNum,
      });
      await api.modelSetAlias(m.id, alias.trim() || null);
      await api.modelSetModalities(
        m.id,
        inputModalities.length ? inputModalities : null,
        outputModalities.length ? outputModalities : null
      );
      await api.modelSetReasoningLevels(m.id, levels);
      await api.modelSetMaxTools(m.id, maxTools);
      if (enabled !== m.enabled) await api.modelToggle(m.id, enabled);
      await onSaved();
      toast("已保存");
      onClose();
    } catch (e) {
      toast(String(e), "err");
    } finally {
      setSaving(false);
    }
  }

  const field = (
    id: string,
    label: string,
    hint: string,
    control: ReactNode
  ) => (
    <div className="space-y-1.5">
      <label htmlFor={id} className="text-xs font-medium text-foreground">
        {label}
      </label>
      {control}
      <p className="text-[11px] text-muted-foreground">{hint}</p>
    </div>
  );

  return (
    <Sheet open onOpenChange={(open) => { if (!open) onClose(); }}>
      <SheetContent aria-describedby={undefined}>
        <SheetHeader>
          <SheetTitle className="pr-8 font-mono">{m.modelName}</SheetTitle>
          <SheetDescription>
            该模型的全部配置。改完点「保存」一次性写入；关闭或「取消」不会保存。
          </SheetDescription>
        </SheetHeader>

        <SheetBody className="space-y-4">
          {field(
            "md-alias",
            "上游模型 ID",
            "发给上游时使用的真实模型 ID；留空表示与模型名同名。",
            <Input
              id="md-alias"
              className="h-9 font-mono text-xs"
              value={alias}
              placeholder="（同名）"
              onChange={(e) => setAlias(e.target.value)}
            />
          )}

          <div className="grid grid-cols-2 gap-3">
            {field(
              "md-ctx",
              "上下文窗口",
              "tokens；留空 = 未知（由上游声明兜底）。",
              <Input
                id="md-ctx"
                className="h-9 text-xs"
                type="number"
                step={1024}
                min={0}
                value={ctx}
                onChange={(e) => setCtx(e.target.value)}
              />
            )}
            {field(
              "md-out",
              "最大输出",
              "tokens；必填。",
              <Input
                id="md-out"
                className="h-9 text-xs"
                type="number"
                step={1024}
                min={1}
                value={out}
                onChange={(e) => setOut(e.target.value)}
              />
            )}
          </div>

          <div className="space-y-2">
            <span className="text-xs font-medium text-foreground">模态标注</span>
            <ModalityChips
              label="输入类型"
              value={inputModalities}
              onChange={setInputModalities}
            />
            <ModalityChips
              label="输出类型"
              value={outputModalities}
              onChange={setOutputModalities}
            />
            <p className="text-[11px] text-muted-foreground">
              全部不选 = 未知（不臆断）；上游未声明时保持未知即可。
            </p>
          </div>

          <div className="space-y-1.5">
            <span className="text-xs font-medium text-foreground">推理档位值域</span>
            <div>
              <EffortLevelsEditor
                verbose
                levels={levels}
                scopeLabel={`模型 ${m.modelName}`}
                onChange={async (next) => setLevels(next)}
              />
            </div>
            <p className="text-[11px] text-muted-foreground">
              留空 = 继承供应商级；供应商级也为空则原样透传。
            </p>
          </div>

          <div className="space-y-1.5">
            <span className="text-xs font-medium text-foreground">工具声明数上限</span>
            <div>
              <MaxToolsEditor
                verbose
                maxTools={maxTools}
                scopeLabel={`模型 ${m.modelName}`}
                onChange={async (n) => setMaxTools(n)}
              />
            </div>
            <p className="text-[11px] text-muted-foreground">
              留空 = 继承供应商级；都不声明则不拦（由上游裁决）。
            </p>
          </div>

          <div className="flex items-center justify-between rounded-md border border-border/60 px-3 py-2">
            <div className="space-y-0.5">
              <div className="text-xs font-medium text-foreground">启用</div>
              <p className="text-[11px] text-muted-foreground">关闭的模型不参与路由。</p>
            </div>
            <Switch
              checked={enabled}
              onCheckedChange={setEnabled}
              aria-label={`启用 ${m.modelName}`}
            />
          </div>
        </SheetBody>

        <SheetFooter>
          <Button variant="outline" onClick={onClose} disabled={saving}>
            取消
          </Button>
          <Button onClick={() => void save()} disabled={saving}>
            {saving ? "保存中…" : "保存"}
          </Button>
        </SheetFooter>
      </SheetContent>
    </Sheet>
  );
}

// ---------------------------------------------------------------- 模态标注（0010）

/** 规范序，与后端 gateway_core::modality::CANONICAL_ORDER 一致。 */
const MODALITY_ORDER: Modality[] = ["text", "image", "audio", "video"];
const MODALITY_LABEL: Record<Modality, string> = {
  text: "文本",
  image: "图像",
  audio: "音频",
  video: "视频",
};
/** 窄窗口降级 badge 用的单字缩写（见 `ModalityCompact`）。 */
const MODALITY_SHORT: Record<Modality, string> = {
  text: "文",
  image: "图",
  audio: "音",
  video: "视",
};

/**
 * 窄窗口下的模态降级展示（`lg` 以下替代整列）。
 *
 * 必须**极紧凑**：badge 挂在「模型名」单元内，文案一长就把该列撑宽、
 * 把隐藏整列省下的宽度又吃回去（实测「模态 文本/图像→文本」这种完整句式
 * 反而让表宽从 801 涨到 804）。故只显示非文本模态的单字缩写
 * （图/音/视），完整集合放 `title`；入/出都只有文本时不显示（默认情形，显示等于噪音）。
 */
function ModalityCompact({ m }: { m: ModelRow }) {
  const inList = m.inputModalities;
  const outList = m.outputModalities;
  const label = (l: Modality[] | null) =>
    !l || l.length === 0 ? "未知" : l.map((x) => MODALITY_LABEL[x]).join("/");
  const plainTextOnly =
    !!inList?.length &&
    !!outList?.length &&
    inList.every((x) => x === "text") &&
    outList.every((x) => x === "text");
  if (plainTextOnly) return null;
  const unknown = !inList?.length && !outList?.length;
  const short = (inList ?? []).filter((x) => x !== "text").map((x) => MODALITY_SHORT[x]).join("");
  return (
    <Badge
      variant={unknown ? "outline" : "secondary"}
      className="shrink-0 px-1 py-0 text-[11px] font-normal lg:hidden"
      title={`模态标注 — 输入：${label(inList)}；输出：${label(outList)}（宽窗口下可直接编辑）`}
    >
      {unknown ? "模态?" : short || "非文本"}
    </Badge>
  );
}

/** 只读徽标：一维模态集合（null/空 = 未知，不臆断）。 */
function ModalityBadges({ list }: { list: Modality[] | null }) {
  if (!list || list.length === 0) {
    return <span className="text-[11px] text-muted-foreground">未知</span>;
  }
  return (
    <span className="inline-flex flex-wrap justify-center gap-0.5">
      {list.map((x) => (
        <Badge key={x} variant="secondary" className="px-1 py-0 text-[11px] font-normal">
          {MODALITY_LABEL[x]}
        </Badge>
      ))}
    </span>
  );
}

/**
 * 抽屉里的模态选择器：**本地 state + 芯片切换**，随「保存」一起提交
 * （旧实现是下拉菜单「勾选即存」，与抽屉的「一次保存」语义冲突）。
 * 全部取消勾选 = 未知（不引入「已知为空」这一无意义状态）。
 */
function ModalityChips({
  label,
  value,
  onChange,
}: {
  label: string;
  value: Modality[];
  onChange: (next: Modality[]) => void;
}) {
  const toggle = (x: Modality) => {
    const picked = value.includes(x) ? value.filter((y) => y !== x) : [...value, x];
    onChange(MODALITY_ORDER.filter((y) => picked.includes(y)));
  };
  return (
    <div className="flex flex-wrap items-center gap-2">
      <span className="w-16 shrink-0 text-[11px] text-muted-foreground">{label}</span>
      {MODALITY_ORDER.map((x) => {
        const on = value.includes(x);
        return (
          <button
            key={x}
            type="button"
            aria-pressed={on}
            aria-label={`${label} ${MODALITY_LABEL[x]}`}
            onClick={() => toggle(x)}
            className={
              "rounded-md border px-2 py-1 text-[11px] transition-colors " +
              (on
                ? "border-primary/40 bg-primary/10 text-foreground"
                : "border-border/60 text-muted-foreground hover:bg-muted hover:text-foreground")
            }
          >
            {MODALITY_LABEL[x]}
          </button>
        );
      })}
    </div>
  );
}
