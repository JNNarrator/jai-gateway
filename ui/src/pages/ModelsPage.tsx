import { useEffect, useState } from "react";
import { ArrowUpDown, Boxes, Copy, Search } from "lucide-react";
import { api } from "../api";
import type { ModelRow, ProviderDto } from "../types";
import { toast } from "../lib/toast";
import { copyText } from "../lib/clipboard";
import { PageHeader } from "@/components/common/PageHeader";
import { EmptyState } from "@/components/common/EmptyState";
import { SkeletonList } from "@/components/common/SkeletonList";
import { Button } from "@/components/ui/button";
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
        description="定义每个模型的上下文窗口、最大输出与上游映射；关闭的模型不参与路由。"
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
        <div className="rounded-lg border">
          <Table>
            <TableHeader>
              <TableRow className="bg-muted/50 hover:bg-muted/50">
                <TableHead className="w-1/4">模型名</TableHead>
                <TableHead className="w-1/5">上游模型 ID</TableHead>
                <TableHead className="w-1/6">上下文</TableHead>
                <TableHead className="w-1/6">最大输出</TableHead>
                <TableHead className="w-1/6 text-center">启用</TableHead>
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
        </>
      )}
    </div>
  );
}

function ModelRowEditor({
  m,
  onSave,
  onToggle,
}: {
  m: ModelRow;
  onSave: (ctx: number | null, out: number, alias: string | null) => Promise<void>;
  onToggle: (enabled: boolean) => Promise<void>;
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
          <button
            className="rounded p-0.5 text-muted-foreground/60 hover:bg-muted hover:text-foreground"
            title="复制模型名"
            aria-label={`复制模型名 ${m.modelName}`}
            onClick={() => copyText(m.modelName)}
          >
            <Copy className="size-3" aria-hidden />
          </button>
        </span>
      </TableCell>
      <TableCell>
        <Input
          className="h-8 w-32 text-xs"
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
          className="h-8 w-28 text-xs"
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
          className="h-8 w-24 text-xs"
          type="number"
          step={1024}
          value={out}
          onChange={(e) => {
            setSaved(false);
            setOut(Number(e.target.value));
          }}
        />
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
