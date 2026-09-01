import { useEffect, useState } from "react";
import { ArrowLeftRight, ClipboardPaste, CloudUpload, Download } from "lucide-react";
import { api } from "../api";
import type { WebDavAutoPushStatus } from "../types";
import { toast } from "../lib/toast";
import { PageHeader } from "@/components/common/PageHeader";
import { ConfirmDialog } from "@/components/common/ConfirmDialog";
import { FormField } from "@/components/common/FormField";
import { Button } from "@/components/ui/button";
import {
  Card,
  CardContent,
  CardDescription,
  CardHeader,
  CardTitle,
} from "@/components/ui/card";
import { Input } from "@/components/ui/input";
import { Textarea } from "@/components/ui/textarea";
import { Switch } from "@/components/ui/switch";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";

const INTERVAL_OPTIONS: { value: number; label: string }[] = [
  { value: 30, label: "每 30 分钟" },
  { value: 60, label: "每 1 小时" },
  { value: 360, label: "每 6 小时" },
];

export function SyncPage() {
  const [importText, setImportText] = useState("");
  const [cfg, setCfg] = useState<{ url: string; username: string; directory: string }>({
    url: "",
    username: "",
    directory: "",
  });
  const [pw, setPw] = useState("");
  const [showPw, setShowPw] = useState(false);
  const [autoPush, setAutoPush] = useState({ enabled: false, intervalMin: 60 });
  const [lastAuto, setLastAuto] = useState<WebDavAutoPushStatus | null>(null);
  const [msg, setMsg] = useState("");
  const [err, setErr] = useState("");
  const [busy, setBusy] = useState("");
  const [confirmImport, setConfirmImport] = useState(false);
  const [overwriteInfo, setOverwriteInfo] = useState("");

  useEffect(() => {
    api
      .webdavConfigGet()
      .then((c) => {
        if (c) {
          setCfg(c);
          setAutoPush({ enabled: c.autoPushEnabled, intervalMin: c.autoPushIntervalMin });
        }
      })
      .catch(() => {});
    api.webdavAutopushStatus().then(setLastAuto).catch(() => {});
    const t = setInterval(
      () => api.webdavAutopushStatus().then(setLastAuto).catch(() => {}),
      60_000,
    );
    return () => clearInterval(t);
  }, []);

  async function doImport() {
    setErr("");
    setConfirmImport(false);
    try {
      const r = await api.configImport(importText, false);
      setMsg(
        `导入完成：新增供应商 ${r.providersImported}，重复跳过 ${r.providersSkippedDuplicate}，模型 ${r.modelsImported}；待补密钥：${r.missingKeys.join("、") || "无"}`,
      );
    } catch (e) {
      setErr(String(e));
    }
  }

  async function doExport() {
    try {
      const path = await api.exportConfigToFile();
      toast("已导出（不含 API Key）");
      try {
        await api.revealInFolder(path);
      } catch {
        // 平台不支持打开目录时忽略
      }
    } catch (e) {
      setErr(String(e));
    }
  }

  async function saveAuto(next: { enabled: boolean; intervalMin: number }) {
    setErr("");
    if (!cfg.url.trim()) {
      setErr("请先填写并保存 WebDAV 连接配置");
      return;
    }
    try {
      await api.webdavConfigSet({
        url: cfg.url,
        username: cfg.username,
        directory: cfg.directory,
        password: null,
        autoPushEnabled: next.enabled,
        autoPushIntervalMin: next.intervalMin,
      });
      setAutoPush(next);
      setMsg(next.enabled ? "自动推送已开启" : "自动推送已关闭");
    } catch (e) {
      setErr(String(e));
    }
  }

  async function saveCfg() {
    setErr("");
    const url = cfg.url.trim();
    if (!url) {
      setErr("WebDAV 根地址不能为空");
      return;
    }
    const normalized = url.endsWith("/") ? url : `${url}/`;
    try {
      await api.webdavConfigSet({ ...cfg, url: normalized, password: pw || null });
      setCfg({ ...cfg, url: normalized });
      setPw("");
      setMsg("WebDAV 连接配置已保存");
    } catch (e) {
      setErr(String(e));
    }
  }

  async function testWebdav() {
    setErr("");
    setMsg("");
    const url = cfg.url.trim();
    if (!url) {
      setErr("WebDAV 根地址不能为空");
      return;
    }
    try {
      const result = await api.webdavTest({
        url,
        username: cfg.username,
        password: pw || null,
      });
      setMsg(result);
      toast("WebDAV 连接成功");
    } catch (e) {
      setErr(String(e));
      toast("WebDAV 连接失败", "err");
    }
  }

  async function previewWebdav() {
    setErr("");
    setMsg("");
    try {
      const r = await api.webdavPreview();
      if (r.willOverwrite) {
        setOverwriteInfo(`${r.message}。拉取将用远端配置覆盖本机。`);
        return; // 由 ConfirmDialog 决定是否拉取
      }
      setMsg(r.message);
      toast("远端与本地一致");
    } catch (e) {
      setErr(String(e));
    }
  }

  async function doPush() {
    setBusy("push");
    setErr("");
    try {
      await api.webdavPush();
      setMsg("已推送到 WebDAV，推送前本地快照已留存");
    } catch (e) {
      setErr(String(e));
    } finally {
      setBusy("");
    }
  }

  async function doPull() {
    setBusy("pull");
    setErr("");
    setOverwriteInfo("");
    try {
      const r = await api.webdavPull();
      setMsg(
        `拉取并导入完成：新增供应商 ${r.providersImported}，模型 ${r.modelsImported}；待补密钥：${r.missingKeys.join("、") || "无"}`,
      );
    } catch (e) {
      setErr(String(e));
    } finally {
      setBusy("");
    }
  }

  return (
    <div className="mx-auto max-w-2xl space-y-4">
      <PageHeader
        title="配置同步"
        description="在设备间迁移或备份供应商与模型配置。"
      />

      {err && (
        <div
          role="alert"
          className="rounded-md border border-destructive/50 bg-destructive/10 px-3 py-2 text-sm text-destructive"
        >
          {err}
        </div>
      )}
      {msg && (
        <div className="rounded-md border border-primary/40 bg-primary/10 px-3 py-2 text-sm text-primary">
          {msg}
        </div>
      )}

      <Card>
        <CardHeader>
          <CardTitle>导入导出 JSON</CardTitle>
          <CardDescription>
            导出产物只含供应商与模型定义，不含任何 API Key。导入后请在「供应商」页逐项补录凭据。
          </CardDescription>
        </CardHeader>
        <CardContent className="space-y-3">
          <Textarea
            rows={3}
            className="min-h-0 font-mono text-xs"
            value={importText}
            onChange={(e) => setImportText(e.target.value)}
            placeholder="粘贴从另一台设备导出的 jai-export JSON…"
          />
          <div className="flex flex-wrap items-center gap-2">
            <Button onClick={() => setConfirmImport(true)} disabled={!importText.trim()}>
              导入
            </Button>
            <Button
              variant="outline"
              onClick={async () => {
                try {
                  const text = await navigator.clipboard.readText();
                  setImportText(text);
                  toast("已粘贴");
                } catch {
                  toast("无法读取剪贴板", "err");
                }
              }}
            >
              <ClipboardPaste aria-hidden />
              粘贴
            </Button>
            <Button variant="outline" onClick={doExport}>
              导出到文件
            </Button>
          </div>
        </CardContent>
      </Card>

      <Card>
        <CardHeader>
          <CardTitle className="flex items-center gap-2">
            <ArrowLeftRight className="size-4" aria-hidden />
            WebDAV 同步
          </CardTitle>
          <CardDescription>
            手动推/拉：推送使用本机当前配置覆盖远端；拉取使用远端配置覆盖本机（last-write-wins）。
            推送前会在本地留存一份快照。
          </CardDescription>
        </CardHeader>
        <CardContent className="space-y-4">
          <div className="grid grid-cols-1 gap-4 md:grid-cols-2">
            <FormField label="WebDAV 根地址" htmlFor="dav-url" className="md:col-span-2">
              <Input
                id="dav-url"
                className="font-mono"
                value={cfg.url}
                onChange={(e) => setCfg({ ...cfg, url: e.target.value })}
                placeholder="https://dav.example.com/remote.php/dav/files/u"
              />
            </FormField>
            <FormField label="用户名" htmlFor="dav-user">
              <Input
                id="dav-user"
                value={cfg.username}
                onChange={(e) => setCfg({ ...cfg, username: e.target.value })}
              />
            </FormField>
            <FormField label="目录（可选）" htmlFor="dav-dir">
              <Input
                id="dav-dir"
                className="font-mono"
                value={cfg.directory}
                onChange={(e) => setCfg({ ...cfg, directory: e.target.value })}
                placeholder="jai/backups"
              />
            </FormField>
            <FormField
              label="密码（存入钥匙串，留空保持原密码）"
              htmlFor="dav-pw"
              className="md:col-span-2"
              labelExtra={
                <Button
                  type="button"
                  variant="ghost"
                  size="sm"
                  className="h-6 px-2 text-xs"
                  onClick={() => setShowPw(!showPw)}
                >
                  {showPw ? "隐藏" : "显示"}
                </Button>
              }
            >
              <Input
                id="dav-pw"
                type={showPw ? "text" : "password"}
                value={pw}
                onChange={(e) => setPw(e.target.value)}
              />
            </FormField>
          </div>

          <div className="flex flex-wrap gap-2">
            <Button onClick={saveCfg}>保存配置</Button>
            <Button variant="outline" onClick={testWebdav}>
              测试连接
            </Button>
            <Button variant="outline" onClick={previewWebdav}>
              预览变更
            </Button>
            <Button variant="outline" disabled={busy === "push"} onClick={doPush}>
              <CloudUpload aria-hidden />
              {busy === "push" ? "推送中…" : "推送"}
            </Button>
            <Button variant="outline" disabled={busy === "pull"} onClick={doPull}>
              <Download aria-hidden />
              {busy === "pull" ? "拉取中…" : "拉取"}
            </Button>
          </div>
          {busy && (
            <div className="h-1 w-full overflow-hidden rounded bg-muted">
              <div className="h-full w-1/3 animate-pulse rounded bg-primary" />
            </div>
          )}

          <div className="space-y-3 rounded-md border bg-muted/30 p-3">
            <div className="flex items-center justify-between gap-3">
              <div>
                <div className="text-sm font-medium">自动推送</div>
                <div className="text-xs leading-relaxed text-muted-foreground">
                  配置变更后防抖 30 秒推送一次，并按所选间隔定时推送。以本机为准，直接覆盖远端。
                </div>
              </div>
              <Switch
                checked={autoPush.enabled}
                disabled={!cfg.url.trim()}
                aria-label="自动推送开关"
                onCheckedChange={(v) =>
                  void saveAuto({ enabled: v, intervalMin: autoPush.intervalMin })
                }
              />
            </div>
            <div className="flex items-center gap-2">
              <span className="text-xs text-muted-foreground">定时间隔</span>
              <Select
                value={String(autoPush.intervalMin)}
                disabled={!autoPush.enabled}
                onValueChange={(v) =>
                  void saveAuto({ enabled: autoPush.enabled, intervalMin: Number(v) })
                }
              >
                <SelectTrigger size="sm" className="w-36">
                  <SelectValue />
                </SelectTrigger>
                <SelectContent>
                  {INTERVAL_OPTIONS.map((o) => (
                    <SelectItem key={o.value} value={String(o.value)}>
                      {o.label}
                    </SelectItem>
                  ))}
                </SelectContent>
              </Select>
            </div>
            {lastAuto && (
              <div className="text-xs text-muted-foreground">
                上次自动推送：
                {new Date(lastAuto.atMs).toLocaleString("zh-CN", { hour12: false })} ·{" "}
                <span className={lastAuto.ok ? "text-emerald-600" : "text-destructive"}>
                  {lastAuto.ok ? "成功" : lastAuto.message}
                </span>
              </div>
            )}
          </div>
        </CardContent>
      </Card>

      <ConfirmDialog
        open={confirmImport}
        onOpenChange={setConfirmImport}
        title="导入配置？"
        description="导入会将 JSON 合并进当前本地配置（同名供应商跳过）。"
        confirmText="导入"
        onConfirm={doImport}
      />
      <ConfirmDialog
        open={!!overwriteInfo}
        onOpenChange={(o) => !o && setOverwriteInfo("")}
        title="拉取将覆盖本地配置"
        description={overwriteInfo}
        confirmText="拉取"
        onConfirm={doPull}
      />
    </div>
  );
}
