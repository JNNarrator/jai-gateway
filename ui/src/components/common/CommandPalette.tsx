import { useEffect, useMemo, useState } from "react";
import { CornerDownLeft, Search } from "lucide-react";
import { api } from "../../api";
import { useNav, type Tab } from "../../lib/nav";
import { toast } from "../../lib/toast";
import { cn } from "@/lib/utils";
import { Dialog, DialogContent } from "@/components/ui/dialog";
import { Input } from "@/components/ui/input";

type Entry =
  | { kind: "page"; tab: Tab; label: string; hint: string }
  | { kind: "action"; id: string; label: string; hint: string; run: () => void }
  | { kind: "provider"; id: string; label: string; hint: string }
  | { kind: "model"; providerId: string; label: string; hint: string };

const PAGES: { tab: Tab; label: string }[] = [
  { tab: "gateway", label: "网关" },
  { tab: "providers", label: "供应商" },
  { tab: "models", label: "模型" },
  { tab: "logs", label: "请求日志" },
  { tab: "stats", label: "用量统计" },
  { tab: "sync", label: "WebDAV 同步" },
  { tab: "mcp", label: "MCP 服务器" },
  { tab: "skills", label: "技能" },
  { tab: "settings", label: "设置" },
];

/** Ctrl+K / Cmd+K 全局命令面板：跳转页面 / 快捷操作 / 搜索供应商与模型。 */
export function CommandPalette() {
  const { setTab } = useNav();
  const [open, setOpen] = useState(false);
  const [q, setQ] = useState("");
  const [idx, setIdx] = useState(0);
  const [extra, setExtra] = useState<Entry[]>([]);

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if ((e.ctrlKey || e.metaKey) && e.key.toLowerCase() === "k") {
        e.preventDefault();
        setOpen((o) => !o);
        setQ("");
        setIdx(0);
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, []);

  // 打开时异步加载供应商/模型（供搜索）
  useEffect(() => {
    if (!open) return;
    let live = true;
    (async () => {
      const entries: Entry[] = [];
      try {
        const ps = await api.providerList();
        for (const p of ps) {
          entries.push({
            kind: "provider",
            id: p.id,
            label: p.name,
            hint: `供应商 · ${p.baseUrl ?? ""}`,
          });
        }
        for (const p of ps) {
          const ms = await api.modelList(p.id).catch(() => []);
          for (const m of ms) {
            entries.push({
              kind: "model",
              providerId: p.id,
              label: m.modelName,
              hint: `模型 · ${p.name}`,
            });
          }
        }
      } catch {
        // 列表加载失败不阻断面板
      }
      if (live) setExtra(entries);
    })();
    return () => {
      live = false;
    };
  }, [open]);

  const all = useMemo<Entry[]>(() => {
    const pages: Entry[] = PAGES.map((p) => ({
      kind: "page",
      tab: p.tab,
      label: p.label,
      hint: "跳转页面",
    }));
    const actions: Entry[] = [
      {
        kind: "action",
        id: "copy-endpoint",
        label: "复制网关端点",
        hint: "http://127.0.0.1:<端口>",
        run: () => {
          void (async () => {
            const s = await api.status().catch(() => null);
            await navigator.clipboard.writeText(
              `http://127.0.0.1:${s?.port ?? 1314}`,
            );
            toast("已复制网关端点");
          })();
        },
      },
      {
        kind: "action",
        id: "sync-push",
        label: "WebDAV 立即推送",
        hint: "推送前差异预警仍生效",
        run: () => {
          void api
            .webdavPush(false)
            .then(() => toast("已推送到 WebDAV"))
            .catch((e) => toast(String(e), "err"));
        },
      },
      {
        kind: "action",
        id: "sync-pull",
        label: "WebDAV 立即拉取",
        hint: "拉取远端配置并导入",
        run: () => {
          void api
            .webdavPull()
            .then(() => toast("已拉取并导入"))
            .catch((e) => toast(String(e), "err"));
        },
      },
    ];
    return [...pages, ...actions, ...extra];
  }, [extra]);

  const ql = q.trim().toLowerCase();
  const matched = useMemo(
    () =>
      ql
        ? all.filter((e) => (e.label + " " + e.hint).toLowerCase().includes(ql))
        : all.slice(0, 12),
    [all, ql],
  );

  useEffect(() => {
    setIdx(0);
  }, [q, open]);

  useEffect(() => {
    if (!open) return;
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "ArrowDown") {
        e.preventDefault();
        setIdx((i) => Math.min(i + 1, Math.max(matched.length - 1, 0)));
      } else if (e.key === "ArrowUp") {
        e.preventDefault();
        setIdx((i) => Math.max(i - 1, 0));
      } else if (e.key === "Enter") {
        e.preventDefault();
        const hit = matched[idx];
        if (hit) run(hit);
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [open, matched, idx]);

  function run(e: Entry) {
    setOpen(false);
    setQ("");
    if (e.kind === "page") {
      setTab(e.tab);
    } else if (e.kind === "action") {
      e.run();
    } else if (e.kind === "provider") {
      setTab("providers");
    } else {
      setTab("models");
    }
  }

  return (
    <Dialog open={open} onOpenChange={setOpen}>
      <DialogContent className="top-[18%] max-w-lg translate-y-0 gap-0 p-0">
        <div className="flex items-center gap-2 border-b px-3">
          <Search className="size-4 shrink-0 text-muted-foreground" aria-hidden />
          <Input
            autoFocus
            className="h-12 border-0 bg-transparent shadow-none focus-visible:ring-0"
            placeholder="搜索页面 / 供应商 / 模型，或输入操作…"
            value={q}
            onChange={(e) => setQ(e.target.value)}
            aria-label="命令面板搜索"
          />
        </div>
        <div className="max-h-80 overflow-y-auto p-1.5">
          {matched.length === 0 && (
            <div className="px-3 py-6 text-center text-xs text-muted-foreground">
              没有匹配项
            </div>
          )}
          {matched.map((e, i) => (
            <button
              key={`${e.kind}-${"tab" in e ? e.tab : "id" in e ? e.id : e.label}`}
              className={cn(
                "flex w-full items-center justify-between gap-3 rounded-md px-3 py-2 text-left text-sm",
                i === idx && "bg-primary/10 text-primary",
              )}
              onMouseEnter={() => setIdx(i)}
              onClick={() => run(e)}
            >
              <span className="truncate">{e.label}</span>
              <span className="flex shrink-0 items-center gap-1 text-[11px] text-muted-foreground">
                {e.hint}
                {i === idx && <CornerDownLeft className="size-3" aria-hidden />}
              </span>
            </button>
          ))}
        </div>
      </DialogContent>
    </Dialog>
  );
}
