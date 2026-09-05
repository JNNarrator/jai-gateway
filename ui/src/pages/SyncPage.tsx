import { useEffect, useMemo, useState } from "react";
import {
  ArrowLeftRight,
  ClipboardPaste,
  CloudUpload,
  Download,
  RotateCcw,
} from "lucide-react";
import { api } from "../api";
import type {
  WebDavAutoPushStatus,
  WebDavBackupItem,
  WebDavSnapshotInfo,
} from "../types";
import { CopyField } from "@/components/common/CopyField";
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
  const [autoPull, setAutoPull] = useState(false);
  const [lastAuto, setLastAuto] = useState<WebDavAutoPushStatus | null>(null);
  const [lastAutoPull, setLastAutoPull] = useState<WebDavAutoPushStatus | null>(null);
  const [snapInfo, setSnapInfo] = useState<WebDavSnapshotInfo | null>(null);
  const [confirmRestore, setConfirmRestore] = useState(false);
  const [backups, setBackups] = useState<WebDavBackupItem[] | null>(null);
  const [loadingBackups, setLoadingBackups] = useState(false);
  const [confirmRestoreBackup, setConfirmRestoreBackup] =
    useState<WebDavBackupItem | null>(null);
  const [confirmDeleteBackup, setConfirmDeleteBackup] =
    useState<WebDavBackupItem | null>(null);
  const [pushBlockInfo, setPushBlockInfo] = useState("");
  const [msg, setMsg] = useState("");
  const [err, setErr] = useState("");
  const [busy, setBusy] = useState("");
  const [confirmImport, setConfirmImport] = useState(false);
  const [overwriteInfo, setOverwriteInfo] = useState("");

  // 远端配置文件地址实时预览（与后端 join_remote_file 语义一致：目录各路径段编码）
  const remoteConfigUrl = useMemo(() => {
    const base = cfg.url.trim().replace(/\/+$/, "");
    if (!base) return "";
    const dir = cfg.directory.trim().replace(/^\/+|\/+$/g, "");
    const segments = dir
      .split("/")
      .filter(Boolean)
      .map((s) => encodeURIComponent(s))
      .join("/");
    return segments ? `${base}/${segments}/jai-config.json` : `${base}/jai-config.json`;
  }, [cfg.url, cfg.directory]);

  useEffect(() => {
    api
      .webdavConfigGet()
      .then((c) => {
        if (c) {
          setCfg(c);
          setPw(c.password ?? "");
          setAutoPush({ enabled: c.autoPushEnabled, intervalMin: c.autoPushIntervalMin });
          setAutoPull(c.autoPullEnabled);
        }
      })
      .catch(() => {});
    api.webdavAutopushStatus().then(setLastAuto).catch(() => {});
    api.webdavAutopullStatus().then(setLastAutoPull).catch(() => {});
    api.webdavSnapshotInfo().then(setSnapInfo).catch(() => {});
    loadBackups();
    const t = setInterval(() => {
      api.webdavAutopushStatus().then(setLastAuto).catch(() => {});
      api.webdavAutopullStatus().then(setLastAutoPull).catch(() => {});
    }, 60_000);
    return () => clearInterval(t);
  }, []);

  async function loadBackups() {
    setLoadingBackups(true);
    try {
      setBackups(await api.webdavBackupsList());
    } catch (e) {
      setErr(String(e));
      setBackups([]);
    } finally {
      setLoadingBackups(false);
    }
  }

  async function doBackupRestore(b: WebDavBackupItem) {
    setBusy("backup-restore");
    setErr("");
    setConfirmRestoreBackup(null);
    try {
      const r = await api.webdavBackupRestore(b.name);
      setMsg(
        `已从远端备份 ${b.name} 恢复：供应商 ${r.providersImported}，模型 ${r.modelsImported}；待补密钥：${r.missingKeys.join("、") || "无"}`,
      );
      void loadBackups();
    } catch (e) {
      setErr(String(e));
    } finally {
      setBusy("");
    }
  }

  async function doBackupDelete(b: WebDavBackupItem) {
    setBusy("backup-delete");
    setErr("");
    setConfirmDeleteBackup(null);
    try {
      await api.webdavBackupDelete(b.name);
      setMsg(`已删除远端备份 ${b.name}`);
      void loadBackups();
    } catch (e) {
      setErr(String(e));
    } finally {
      setBusy("");
    }
  }

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

  async function saveAutoPull(enabled: boolean) {
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
        autoPullEnabled: enabled,
      });
      setAutoPull(enabled);
      setMsg(enabled ? "自动拉取已开启" : "自动拉取已关闭");
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
      const saved = await api.webdavConfigGet().catch(() => null);
      if (saved) setPw(saved.password ?? "");
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
      await api.webdavPush(false);
      setMsg("已推送到 WebDAV，推送前本地快照与远端旧版备份均已留存");
      void loadBackups();
      void api.webdavSnapshotInfo().then(setSnapInfo).catch(() => {});
    } catch (e) {
      const text = String(e);
      if (text.includes("仍然推送")) {
        // 差异预警：远端有本机没有的内容，弹确认框由用户决定
        setPushBlockInfo(text);
      } else {
        setErr(text);
      }
    } finally {
      setBusy("");
    }
  }

  async function doPushForce() {
    setPushBlockInfo("");
    setBusy("push");
    setErr("");
    try {
      await api.webdavPush(true);
      setMsg("已推送到 WebDAV（以本机为准覆盖），推送前远端旧版已留存为备份");
      void loadBackups();
      void api.webdavSnapshotInfo().then(setSnapInfo).catch(() => {});
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
      void loadBackups();
    } catch (e) {
      setErr(String(e));
    } finally {
      setBusy("");
    }
  }

  async function doRestore() {
    setBusy("restore");
    setErr("");
    setConfirmRestore(false);
    try {
      const r = await api.webdavSnapshotRestore();
      setMsg(
        `已从快照恢复：新增供应商 ${r.providersImported}，重复跳过 ${r.providersSkippedDuplicate}，模型 ${r.modelsImported}；待补密钥：${r.missingKeys.join("、") || "无"}`,
      );
      api.webdavSnapshotInfo().then(setSnapInfo).catch(() => {});
      void loadBackups();
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
            推送前会自动把远端上一版留存为 <code className="font-mono text-xs">jai-config.&lt;时间戳&gt;.json</code> 备份，
            并在本地留存一份快照。
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
              label="远端配置文件地址"
              htmlFor="dav-remote-url"
              className="md:col-span-2"
            >
              <CopyField
                value={remoteConfigUrl}
                display={remoteConfigUrl || "尚未填写 WebDAV 根地址"}
                copyDisabled={!remoteConfigUrl}
              />
            </FormField>
            <FormField
              label="密码（明文入库并随配置同步，留空保持原密码）"
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
                  护栏：本机配置为空而远端有内容时自动跳过（避免覆盖远端备份），需手动推送。
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
            <div className="flex items-center justify-between gap-3">
              <div>
                <div className="text-sm font-medium">自动拉取</div>
                <div className="text-xs leading-relaxed text-muted-foreground">
                  按所选间隔定时拉取远端更新（last-write-wins：仅当远端比上次成功同步更新时导入；
                  空远端不拉取，防远端空配置清空本机）。
                </div>
              </div>
              <Switch
                checked={autoPull}
                disabled={!cfg.url.trim()}
                aria-label="自动拉取开关"
                onCheckedChange={(v) => void saveAutoPull(v)}
              />
            </div>
            <div className="flex items-center gap-2">
              <span className="text-xs text-muted-foreground">定时间隔</span>
              <Select
                value={String(autoPush.intervalMin)}
                disabled={!autoPush.enabled && !autoPull}
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
            {lastAutoPull && (
              <div className="text-xs text-muted-foreground">
                上次自动拉取：
                {new Date(lastAutoPull.atMs).toLocaleString("zh-CN", { hour12: false })} ·{" "}
                <span className={lastAutoPull.ok ? "text-emerald-600" : "text-destructive"}>
                  {lastAutoPull.ok ? "成功" : lastAutoPull.message}
                </span>
              </div>
            )}
          </div>

          <div className="space-y-2 rounded-md border bg-muted/30 p-3">
            <div className="text-sm font-medium">本地推送前快照</div>
            <div className="text-xs leading-relaxed text-muted-foreground">
              {snapInfo?.exists
                ? `上次推送前的完整配置快照（${snapInfo.atMs ? new Date(snapInfo.atMs).toLocaleString("zh-CN", { hour12: false }) : "时间未知"} · ${(snapInfo.chars / 1024).toFixed(1)} KB），可从快照恢复被覆盖前的配置。`
                : "暂无快照：执行过至少一次 WebDAV 推送后才会留存。"}
            </div>
            <Button
              variant="outline"
              size="sm"
              disabled={!snapInfo?.exists || busy === "restore"}
              onClick={() => setConfirmRestore(true)}
            >
              <RotateCcw aria-hidden />
              {busy === "restore" ? "恢复中…" : "从快照恢复"}
            </Button>
          </div>

          <div className="space-y-2 rounded-md border bg-muted/30 p-3">
            <div className="flex items-center justify-between gap-2">
              <div className="text-sm font-medium">远端备份（WebDAV 目录）</div>
              <Button
                variant="ghost"
                size="sm"
                className="h-7"
                disabled={loadingBackups || !!busy}
                onClick={() => void loadBackups()}
              >
                {loadingBackups ? "加载中…" : "刷新"}
              </Button>
            </div>
            <div className="text-xs leading-relaxed text-muted-foreground">
              每次推送前会把远端上一版留存为时间戳备份（同目录
              <code className="font-mono">jai-config.&lt;时间戳&gt;.json</code>），可在此恢复
              指定版本到本地或删除。
            </div>
            {backups === null ? (
              <div className="text-xs text-muted-foreground">未加载（请先配置 WebDAV）</div>
            ) : backups.length === 0 ? (
              <div className="text-xs text-muted-foreground">
                远端目录暂无文件——首次推送后此处会出现备份列表。
              </div>
            ) : (
              <ul className="divide-y rounded-md border bg-background/60 text-xs">
                {backups.map((b) => (
                  <li
                    key={b.name}
                    className="flex items-center justify-between gap-2 px-3 py-2"
                  >
                    <div className="min-w-0">
                      <div className="truncate font-mono">
                        {b.name}
                        {b.isCurrent && (
                          <span className="ml-2 rounded bg-muted px-1 py-0.5 text-[10px] text-muted-foreground">
                            当前
                          </span>
                        )}
                      </div>
                      <div className="text-muted-foreground">
                        {b.ts
                          ? new Date(b.ts).toLocaleString("zh-CN", { hour12: false })
                          : "—"}
                        {b.size != null && ` · ${(b.size / 1024).toFixed(1)} KB`}
                      </div>
                    </div>
                    {!b.isCurrent && (
                      <div className="flex shrink-0 gap-1">
                        <Button
                          variant="outline"
                          size="sm"
                          className="h-7"
                          disabled={busy === "backup-restore"}
                          onClick={() => setConfirmRestoreBackup(b)}
                        >
                          恢复
                        </Button>
                        <Button
                          variant="ghost"
                          size="sm"
                          className="h-7 text-destructive"
                          disabled={busy === "backup-delete"}
                          onClick={() => setConfirmDeleteBackup(b)}
                        >
                          删除
                        </Button>
                      </div>
                    )}
                  </li>
                ))}
              </ul>
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
      <ConfirmDialog
        open={confirmRestore}
        onOpenChange={setConfirmRestore}
        title="从快照恢复本地配置？"
        description="将用上次推送前的完整快照覆盖本地当前配置（含供应商、模型、网关 Key 与 WebDAV 配置），用于误操作回退。若已开启自动推送，恢复结果随后会防抖同步回远端。"
        confirmText="恢复"
        onConfirm={doRestore}
      />
      <ConfirmDialog
        open={!!pushBlockInfo}
        onOpenChange={(o) => !o && setPushBlockInfo("")}
        title="推送将覆盖远端内容"
        description={pushBlockInfo}
        confirmText="仍然推送"
        onConfirm={doPushForce}
      />
      <ConfirmDialog
        open={!!confirmRestoreBackup}
        onOpenChange={(o) => !o && setConfirmRestoreBackup(null)}
        title={`从远端备份恢复本地配置？`}
        description={
          confirmRestoreBackup
            ? `将用远端备份 ${confirmRestoreBackup.name}（${confirmRestoreBackup.ts ? new Date(confirmRestoreBackup.ts).toLocaleString("zh-CN", { hour12: false }) : "时间未知"}）覆盖本地当前配置（含供应商、模型、网关 Key 与 WebDAV 配置）。`
            : ""
        }
        confirmText="恢复"
        onConfirm={() => confirmRestoreBackup && void doBackupRestore(confirmRestoreBackup)}
      />
      <ConfirmDialog
        open={!!confirmDeleteBackup}
        onOpenChange={(o) => !o && setConfirmDeleteBackup(null)}
        title="删除远端备份？"
        description={
          confirmDeleteBackup
            ? `将从 WebDAV 永久删除备份 ${confirmDeleteBackup.name}（本地不受影响）。`
            : ""
        }
        confirmText="删除"
        onConfirm={() => confirmDeleteBackup && void doBackupDelete(confirmDeleteBackup)}
      />
    </div>
  );
}
