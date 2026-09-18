import { useEffect, useState } from "react";
import { ArrowUpDown, Boxes, Copy, Minus, Search } from "lucide-react";
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
import {
  DropdownMenu,
  DropdownMenuCheckboxItem,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuLabel,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
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
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow,
} from "@/components/ui/table";

export function ModelsPage() {
  const [providers, setProviders] = useState<ProviderDto[]>([]);
  const [selId, setSelId] = useState<string>("");
  const [models, setModels] = useState<ModelRow[]>([]);
  const [q, setQ] = useState("");
  const [sortAsc, setSortAsc] = useState(true);
  const [loading, setLoading] = useState(true);

  useEffect(() => {
    api.providerList().then(setProviders).catch(() => {});
  }, []);

  useEffect(() => {
    if (providers.length && !selId) setSelId(providers[0].id);
  }, [providers, selId]);

  useEffect(() => {
    if (!selId) return;
    setLoading(true);
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
      if (selId) setModels(await api.modelList(selId));
      toast(enabled ? "已全部启用" : "已全部禁用");
    } catch (e) {
      toast(String(e), "err");
    }
  }

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
          <SelectTrigger className="w-56">
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
        {/* 窄窗口（最小 760px）下 7 列会横向溢出：表格容器内可横向滚动 */}
        <div className="rounded-lg border">
          <Table>
            <TableHeader>
              <TableRow className="bg-muted/50 hover:bg-muted/50">
                <TableHead className="w-1/5">模型名</TableHead>
                <TableHead className="w-1/5">上游模型 ID</TableHead>
                <TableHead className="w-1/6">上下文</TableHead>
                <TableHead className="w-1/6">最大输出</TableHead>
                <TableHead className="hidden w-28 text-center lg:table-cell">
                  模态（入/出）
                </TableHead>
                <TableHead className="w-1/12 text-center">启用</TableHead>
                <TableHead className="w-20 text-right">操作</TableHead>
              </TableRow>
            </TableHeader>
            <TableBody className="divide-y">
              {filtered.map((m) => (
                <ModelRowEditor
                  key={m.id}
                  m={m}
                  onSave={async (ctx, out, alias) => {
                    await api.modelSetLimits({
                      modelId: m.id,
                      contextWindow: ctx,
                      maxOutputTokens: out,
                    });
                    await api.modelSetAlias(m.id, alias);
                    setModels(await api.modelList(selId));
                  }}
                  onToggle={async (v) => {
                    await api.modelToggle(m.id, v);
                    setModels(await api.modelList(selId));
                  }}
                  onSetModalities={async (input, output) => {
                    await api.modelSetModalities(m.id, input, output);
                    setModels(await api.modelList(selId));
                  }}
                  onSetReasoningLevels={async (levels) => {
                    await api.modelSetReasoningLevels(m.id, levels);
                    setModels(await api.modelList(selId));
                  }}
                  onSetMaxTools={async (maxTools) => {
                    await api.modelSetMaxTools(m.id, maxTools);
                    setModels(await api.modelList(selId));
                  }}
                />
              ))}
            </TableBody>
          </Table>
          {filtered.length === 0 && (
            <div className="px-4 py-8 text-center text-sm text-muted-foreground">
              没有匹配的模型
            </div>
          )}
        </div>
        {/* 窄窗（<1024px）下「模态（入/出）」列隐藏、该信息并回「模型名」列的 badge，
            以收回 112px 宽度（最小 900 窗口实测原溢出 77px）；其余列仍可横向滚动 */}
        <p className="hidden text-[11px] text-muted-foreground max-lg:block">
          窗口较窄：模态标注已并入「模型名」列（悬停可见完整集合），其余列可左右滑动。
        </p>
        </>
      )}
    </div>
  );
}

function ModelRowEditor({
  m,
  onSave,
  onToggle,
  onSetModalities,
  onSetReasoningLevels,
  onSetMaxTools,
}: {
  m: ModelRow;
  onSave: (ctx: number | null, out: number, alias: string | null) => Promise<void>;
  onToggle: (enabled: boolean) => Promise<void>;
  onSetModalities: (input: Modality[] | null, output: Modality[] | null) => Promise<void>;
  onSetReasoningLevels: (levels: string[] | null) => Promise<void>;
  onSetMaxTools: (maxTools: number | null) => Promise<void>;
}) {
  const [ctx, setCtx] = useState(m.contextWindow ?? 128000);
  const [out, setOut] = useState(m.maxOutputTokens);
  const [alias, setAlias] = useState(m.upstreamModelId ?? "");
  const [saved, setSaved] = useState(false);

  async function handleSave() {
    await onSave(
      m.contextWindow == null && ctx === 128000 ? null : ctx,
      out,
      alias.trim() || null
    );
    setSaved(true);
    window.setTimeout(() => setSaved(false), 3000);
  }

  return (
    <TableRow className={m.enabled ? "" : "opacity-50"}>
      <TableCell className="font-mono text-xs">
        <span className="inline-flex items-center gap-1">
          {m.modelName}
          {/* 视觉 16×16，命中区靠伪元素扩到 32×32（与 Switch 同一手法：
              图标按钮不做视觉放大，避免撑开表格行高） */}
          <button
            className="relative rounded p-0.5 text-muted-foreground/60 hover:bg-muted hover:text-foreground after:absolute after:-inset-2 after:content-['']"
            title="复制模型名"
            aria-label={`复制模型名 ${m.modelName}`}
            onClick={() => copyText(m.modelName)}
          >
            <Copy className="size-3" aria-hidden />
          </button>
          {/* <lg 时「模态（入/出）」列被隐藏（列降级，见 §3 第 5 条），
              模态信息降级为一行紧凑 badge，避免信息凭空消失。 */}
          <ModalityCompact m={m} />
          {/* 推理档位值域（0011）：同理不新增列（7 列已吃掉全部宽度），
              以紧凑芯片就地编辑，避免窄窗横向溢出回归。 */}
          <EffortLevelsEditor
            levels={m.reasoningEffortLevels ?? null}
            scopeLabel={`模型 ${m.modelName}`}
            onChange={(levels) =>
              onSetReasoningLevels(levels).catch((e) => toast(String(e), "err"))
            }
          />
          {/* 工具声明数上限（0012）：同上，就地编辑不新增列 */}
          <MaxToolsEditor
            maxTools={m.maxTools ?? null}
            scopeLabel={`模型 ${m.modelName}`}
            onChange={(n) => onSetMaxTools(n).catch((e) => toast(String(e), "err"))}
          />
        </span>
      </TableCell>
      <TableCell>
        <Input
          className="h-8 w-32 max-lg:w-24 text-xs"
          value={alias}
          placeholder="同模型名"
          title="发给上游时使用的真实模型 ID；留空表示同名"
          onChange={(e) => {
            setSaved(false);
            setAlias(e.target.value);
          }}
        />
      </TableCell>
      <TableCell>
        <Input
          className="h-8 w-28 max-lg:w-20 text-xs"
          type="number"
          step={1024}
          value={ctx}
          onChange={(e) => {
            setSaved(false);
            setCtx(Number(e.target.value));
          }}
        />
      </TableCell>
      <TableCell>
        <Input
          className="h-8 w-24 max-lg:w-20 text-xs"
          type="number"
          step={1024}
          value={out}
          onChange={(e) => {
            setSaved(false);
            setOut(Number(e.target.value));
          }}
        />
      </TableCell>
      <TableCell className="hidden text-center lg:table-cell">
        <ModalityEditor m={m} onChange={onSetModalities} />
      </TableCell>
      <TableCell className="text-center">
        <Switch
          checked={m.enabled}
          onCheckedChange={(v) => void onToggle(v)}
          aria-label={`启用 ${m.modelName}`}
          className="mx-auto"
        />
      </TableCell>
      <TableCell className="text-right">
        <Button variant="outline" size="sm" className="h-8" onClick={() => void handleSave()}>
          {saved ? "已保存" : "保存"}
        </Button>
      </TableCell>
    </TableRow>
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
      className="px-1 py-0 text-[11px] font-normal lg:hidden"
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
 * 输入/输出模态编辑器：勾选即存即生效（菜单不关闭，可连续标注）；
 * 「清除标注」= 两维一起回到「未知」（后端同时把 0009 旧布尔列置 NULL）。
 * 取消最后一个勾选亦等价于「未知」——不引入「已知为空」这一无意义状态。
 */
function ModalityEditor({
  m,
  onChange,
}: {
  m: ModelRow;
  onChange: (input: Modality[] | null, output: Modality[] | null) => Promise<void>;
}) {
  const toggle = (dim: "in" | "out", x: Modality) => {
    const cur = (dim === "in" ? m.inputModalities : m.outputModalities) ?? [];
    const picked = cur.includes(x) ? cur.filter((y) => y !== x) : [...cur, x];
    const ordered = MODALITY_ORDER.filter((y) => picked.includes(y));
    const next = ordered.length ? ordered : null;
    void onChange(
      dim === "in" ? next : m.inputModalities,
      dim === "out" ? next : m.outputModalities
    );
  };

  const dimension = (label: string, which: "in" | "out", list: Modality[] | null) => (
    <>
      <DropdownMenuLabel className="text-[11px] text-muted-foreground">
        {label}
      </DropdownMenuLabel>
      {MODALITY_ORDER.map((x) => (
        <DropdownMenuCheckboxItem
          key={`${which}-${x}`}
          checked={(list ?? []).includes(x)}
          onSelect={(e) => e.preventDefault()}
          onCheckedChange={() => toggle(which, x)}
        >
          {MODALITY_LABEL[x]}
        </DropdownMenuCheckboxItem>
      ))}
    </>
  );

  return (
    <DropdownMenu>
      <DropdownMenuTrigger asChild>
        <Button
          variant="ghost"
          size="sm"
          className="mx-auto h-auto flex-col gap-0.5 px-2 py-1"
          title="标注该模型支持的输入/输出模态；未知 = 上游未声明或已清除"
          aria-label={`模态标注 ${m.modelName}`}
        >
          <span className="flex items-center gap-1 text-[11px]">
            <span className="text-muted-foreground">入</span>
            <ModalityBadges list={m.inputModalities} />
          </span>
          <span className="flex items-center gap-1 text-[11px]">
            <span className="text-muted-foreground">出</span>
            <ModalityBadges list={m.outputModalities} />
          </span>
        </Button>
      </DropdownMenuTrigger>
      <DropdownMenuContent align="center" className="w-36">
        {dimension("输入类型", "in", m.inputModalities)}
        <DropdownMenuSeparator />
        {dimension("输出类型", "out", m.outputModalities)}
        <DropdownMenuSeparator />
        <DropdownMenuItem onSelect={() => void onChange(null, null)}>
          <Minus className="size-3.5" aria-hidden />
          清除标注（未知）
        </DropdownMenuItem>
      </DropdownMenuContent>
    </DropdownMenu>
  );
}
