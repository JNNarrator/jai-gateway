import { useEffect, useState } from "react";
import { KeyRound, Plus, ShieldCheck } from "lucide-react";
import { api } from "../api";
import { toast } from "../lib/toast";
import { goTab } from "../lib/nav";
import { cn } from "@/lib/utils";
import { PageHeader } from "@/components/common/PageHeader";
import { ConfirmDialog } from "@/components/common/ConfirmDialog";
import { Button } from "@/components/ui/button";
import {
  Card,
  CardContent,
  CardDescription,
  CardHeader,
  CardTitle,
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
import { Switch } from "@/components/ui/switch";
import { Textarea } from "@/components/ui/textarea";

export function SettingsPage() {
  const [raw, setRaw] = useState("");
  const [saved, setSaved] = useState(false);
  const [port, setPort] = useState(1314);
  const [logsEnabled, setLogsEnabled] = useState(true);
  const [retentionDays, setRetentionDays] = useState(30);
  const [logRowCap, setLogRowCap] = useState(50000);
  const [portMsg, setPortMsg] = useState("");
  const [logMsg, setLogMsg] = useState("");
  const [vaultKind, setVaultKind] = useState("…");
  const [portBusyWarn, setPortBusyWarn] = useState("");
  const [retentionOpen, setRetentionOpen] = useState(false);
  const [originOpen, setOriginOpen] = useState(false);
  const [originValue, setOriginValue] = useState("");
  const corsHasWildcard = raw.split("\n").some((s) => s.trim() === "*");

  useEffect(() => {
    api
      .corsAllowGet()
      .then((l) => setRaw(l.join("\n")))
      .catch(() => {});
    api
      .settingsGet()
      .then((s) => {
        setPort(s.preferredPort);
        setLogsEnabled(s.logsEnabled);
        setRetentionDays(s.retentionDays);
        setLogRowCap(s.logRowCap);
      })
      .catch(() => {});
    api.vaultStorageKind().then(setVaultKind).catch(() => setVaultKind("unknown"));
  }, []);

  async function save() {
    const lines = raw.split("\n").map((s) => s.trim()).filter(Boolean);
    await api.corsAllowSet(lines);
    setRaw(lines.join("\n"));
    setSaved(true);
    setTimeout(() => setSaved(false), 1500);
  }

  async function savePort() {
    setPortMsg("");
    const n = Number(port);
    if (!Number.isInteger(n) || n < 1 || n > 65535) {
      setPortMsg("端口需为 1–65535 的整数");
      return;
    }
    if (await api.portInUse(n)) {
      setPortBusyWarn(`端口 ${n} 当前被占用，保存后网关会自动顺延到可用端口。`);
      return; // 由 ConfirmDialog 决定是否继续
    }
    await api.settingsSetPort(n);
    setPortMsg("已保存：重启网关后生效（端口占用自动顺延）");
    toast("端口已保存");
  }

  async function toggleLogs(on: boolean) {
    await api.settingsSetLogsEnabled(on);
    setLogsEnabled(on);
    setLogMsg(on ? "日志记录已开启" : "日志记录已关闭（新请求不再落库）");
    setTimeout(() => setLogMsg(""), 2500);
  }

  function addOrigin() {
    const origin = originValue.trim();
    if (!origin) return;
    const lines = raw.split("\n").map((s) => s.trim()).filter(Boolean);
    if (!lines.includes(origin)) {
      lines.push(origin);
      setRaw(lines.join("\n"));
    }
    setOriginOpen(false);
    setOriginValue("");
  }

  return (
    <div className="mx-auto max-w-2xl space-y-4">
      <PageHeader title="设置" description="端口、日志与安全策略。" />

      <Card>
        <CardHeader>
          <CardTitle>网关端口</CardTitle>
          <CardDescription>默认 1314；被占用时顺延，实际端口见「网关」页。</CardDescription>
        </CardHeader>
        <CardContent className="space-y-2">
          <div className="flex items-center gap-2">
            <Input
              className="w-32 font-mono"
              type="number"
              value={port}
              onChange={(e) => setPort(Number(e.target.value))}
              aria-label="网关端口"
            />
            <Button onClick={savePort}>保存</Button>
          </div>
          {portMsg && <div className="text-xs text-emerald-600 dark:text-emerald-400">{portMsg}</div>}
        </CardContent>
      </Card>

      <Card>
        <CardHeader>
          <CardTitle className="flex items-center gap-2.5">
            请求日志
            <Switch
              checked={logsEnabled}
              onCheckedChange={(v) => void toggleLogs(v)}
              aria-label="日志记录开关"
            />
            <span className="text-sm font-normal text-muted-foreground">
              {logsEnabled ? "记录中" : "已关闭"}
            </span>
          </CardTitle>
          <CardDescription>
            保留策略：{retentionDays} 天 / 上限 {logRowCap.toLocaleString("zh-CN")} 行
            （每日自动清理）。日志仅含元数据，不含 prompt 与响应明文。
          </CardDescription>
        </CardHeader>
        <CardContent>
          <Button variant="outline" size="sm" onClick={() => setRetentionOpen(true)}>
            编辑保留策略
          </Button>
          {logMsg && <div className="mt-2 text-xs text-emerald-600 dark:text-emerald-400">{logMsg}</div>}
        </CardContent>
      </Card>

      <Card>
        <CardHeader>
          <CardTitle>浏览器跨域白名单（CORS）</CardTitle>
          <CardDescription>
            默认拒绝一切远程网页来源访问网关（安全基线）。如果你使用网页版聊天应用（如自部署的
            NextChat 等），把它的 Origin 加进白名单，每行一条；通配符 * 表示放行全部（不推荐）。
            本机来源（localhost / 127.0.0.1）始终放行。
          </CardDescription>
        </CardHeader>
        <CardContent className="space-y-3">
          {corsHasWildcard && (
            <div className="rounded-md border border-primary/40 bg-primary/10 px-3 py-2 text-xs text-primary">
              ⚠ 通配符 * 会放行所有来源，生产环境不建议使用。
            </div>
          )}
          <Textarea
            className="h-32 font-mono text-xs"
            value={raw}
            onChange={(e) => setRaw(e.target.value)}
            placeholder={"https://chat.example.com\nhttp://192.168.1.5:3000"}
          />
          <div className="flex flex-wrap items-center gap-2">
            <Button onClick={save}>保存</Button>
            <Button
              variant="outline"
              size="sm"
              onClick={() => {
                setOriginValue("");
                setOriginOpen(true);
              }}
            >
              <Plus aria-hidden />
              添加域名
            </Button>
            <Button
              variant="ghost"
              size="sm"
              onClick={() => setRaw("https://chat.example.com\nhttp://192.168.1.5:3000")}
            >
              填入示例
            </Button>
            {saved && <span className="text-xs text-emerald-600 dark:text-emerald-400">已生效</span>}
          </div>
        </CardContent>
      </Card>

      <Card>
        <CardHeader>
          <CardTitle className="flex items-center gap-2">
            <KeyRound className="size-4" aria-hidden />
            关于密钥存储
          </CardTitle>
          <CardDescription>
            当前凭据存储：{" "}
            <span
              className={cn(
                "font-medium",
                vaultKind === "file"
                  ? "text-amber-600 dark:text-amber-400"
                  : "text-emerald-600 dark:text-emerald-400",
              )}
            >
              {vaultKind === "keyring"
                ? "系统钥匙串"
                : vaultKind === "file"
                  ? "文件降级（0600）"
                  : vaultKind}
            </span>
          </CardDescription>
        </CardHeader>
        <CardContent className="space-y-3">
          <ul className="list-disc space-y-1.5 pl-5 text-xs leading-relaxed text-muted-foreground">
            <li>上游 API Key：优先保存在 macOS 钥匙串 / Windows 凭据管理器，数据库只存引用。</li>
            <li>若系统凭据存储不可用（如沙箱/CI 权限受限），自动降级为数据目录下的 vault_fallback.json（Unix 0600）。</li>
            <li>网关 Key sk-jai-*：按设计决策明文存放本地 SQLite（非敏感级别），前缀展示、轮换采用吊销+新建保留审计痕迹、永不导出。</li>
            <li>删除供应商会同时清除对应凭据。</li>
          </ul>
          <Button variant="outline" size="sm" onClick={() => goTab("providers")}>
            <ShieldCheck aria-hidden />
            前往供应商页补录/查看凭据 →
          </Button>
        </CardContent>
      </Card>

      <Card>
        <CardHeader>
          <CardTitle>数据位置</CardTitle>
        </CardHeader>
        <CardContent>
          <code className="font-mono text-xs text-muted-foreground">
            jai.db 位于系统应用数据目录（Tauri appDataDir）/jai.db
          </code>
        </CardContent>
      </Card>

      <ConfirmDialog
        open={!!portBusyWarn}
        onOpenChange={(o) => !o && setPortBusyWarn("")}
        title="端口当前被占用"
        description={portBusyWarn}
        confirmText="仍然保存"
        onConfirm={async () => {
          await api.settingsSetPort(Number(port));
          setPortBusyWarn("");
          setPortMsg("已保存：重启网关后生效（端口占用自动顺延）");
          toast("端口已保存");
        }}
      />

      <Dialog open={retentionOpen} onOpenChange={setRetentionOpen}>
        <DialogContent className="sm:max-w-sm">
          <DialogHeader>
            <DialogTitle>编辑日志保留策略</DialogTitle>
            <DialogDescription>保留天数至少 1 天，行数上限至少 1000。</DialogDescription>
          </DialogHeader>
          <div className="space-y-3">
            <div className="space-y-1.5">
              <label className="text-sm font-medium">保留天数</label>
              <Input
                type="number"
                value={retentionDays}
                onChange={(e) => setRetentionDays(Number(e.target.value))}
              />
            </div>
            <div className="space-y-1.5">
              <label className="text-sm font-medium">日志行数上限</label>
              <Input
                type="number"
                value={logRowCap}
                onChange={(e) => setLogRowCap(Number(e.target.value))}
              />
            </div>
          </div>
          <DialogFooter className="gap-2">
            <Button variant="ghost" onClick={() => setRetentionOpen(false)}>
              取消
            </Button>
            <Button
              onClick={async () => {
                try {
                  await api.settingsSetRetention(retentionDays, logRowCap);
                  setRetentionOpen(false);
                  toast("保留策略已更新");
                } catch (e) {
                  setLogMsg(String(e));
                }
              }}
            >
              保存
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>

      <Dialog open={originOpen} onOpenChange={setOriginOpen}>
        <DialogContent className="sm:max-w-sm">
          <DialogHeader>
            <DialogTitle>添加允许的来源</DialogTitle>
            <DialogDescription>例如 https://chat.example.com</DialogDescription>
          </DialogHeader>
          <Input
            value={originValue}
            onChange={(e) => setOriginValue(e.target.value)}
            onKeyDown={(e) => e.key === "Enter" && addOrigin()}
            placeholder="https://chat.example.com"
            autoFocus
          />
          <DialogFooter className="gap-2">
            <Button variant="ghost" onClick={() => setOriginOpen(false)}>
              取消
            </Button>
            <Button onClick={addOrigin}>添加</Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>
    </div>
  );
}
