import { useEffect, useRef, useState } from "react";
import { Globe, KeyRound, Plus, RefreshCw, ShieldCheck } from "lucide-react";
import { check as checkForUpdate, type Update } from "@tauri-apps/plugin-updater";
import { relaunch } from "@tauri-apps/plugin-process";
import { getVersion } from "@tauri-apps/api/app";
import { api } from "../api";
import { toast } from "../lib/toast";
import { goTab } from "../lib/nav";
import { cn } from "@/lib/utils";
import { useDirtyGuard } from "@/lib/dirty";
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

/** 多行文本类设置的归一化：行首尾空白 / 空行 / 末尾换行不算「改动」（保存时后端也会 trim） */
function normLines(s: string): string {
  return s
    .split("\n")
    .map((x) => x.trim())
    .filter(Boolean)
    .join("\n");
}

export function SettingsPage() {
  const [raw, setRaw] = useState("");
  const [saved, setSaved] = useState(false);
  const [port, setPort] = useState(1314);
  const [logsEnabled, setLogsEnabled] = useState(true);
  const [retentionDays, setRetentionDays] = useState(30);
  const [logRowCap, setLogRowCap] = useState(50000);
  const [portMsg, setPortMsg] = useState("");
  const [logMsg, setLogMsg] = useState("");
  const [portBusyWarn, setPortBusyWarn] = useState("");
  const [retentionOpen, setRetentionOpen] = useState(false);
  const [originOpen, setOriginOpen] = useState(false);
  const [originValue, setOriginValue] = useState("");
  const [proxyEnabled, setProxyEnabled] = useState(false);
  const [proxyUrl, setProxyUrl] = useState("");
  const [proxyBypass, setProxyBypass] = useState("");
  const [proxyMsg, setProxyMsg] = useState("");
  const [proxyMsgOk, setProxyMsgOk] = useState(false);
  const [proxyBusy, setProxyBusy] = useState(false);
  const corsHasWildcard = raw.split("\n").some((s) => s.trim() === "*");

  // ── 未保存改动追踪（label = 设置）────────────────────────────────────────────
  // 本页是**页内**表单（端口 / 代理 / CORS / 日志保留各卡片自带保存按钮），
  // 没有弹窗生命周期可借，所以用「加载完成时的快照」比对：
  //   ① 每处 IPC 加载成功后立刻把对应字段写进快照（同时置该字段 loaded=true）；
  //   ② 每个卡片保存成功后把快照更新为刚保存的值 → 立刻回到不脏；
  //   ③ 只有 loaded=true 的字段参与比对 —— 否则「初始默认值 vs 已加载值」会造成假脏。
  const snapRef = useRef({
    port: 1314,
    logsEnabled: true,
    retentionDays: 30,
    logRowCap: 50000,
    cors: "",
    proxyEnabled: false,
    proxyUrl: "",
    proxyBypass: "",
  });
  const [loaded, setLoaded] = useState({ settings: false, cors: false, proxy: false });

  const dirty =
    (loaded.settings &&
      (port !== snapRef.current.port ||
        logsEnabled !== snapRef.current.logsEnabled ||
        retentionDays !== snapRef.current.retentionDays ||
        logRowCap !== snapRef.current.logRowCap)) ||
    (loaded.cors && normLines(raw) !== snapRef.current.cors) ||
    (loaded.proxy &&
      (proxyEnabled !== snapRef.current.proxyEnabled ||
        proxyUrl.trim() !== snapRef.current.proxyUrl ||
        normLines(proxyBypass) !== snapRef.current.proxyBypass));

  useDirtyGuard("设置", dirty);

  useEffect(() => {
    api
      .corsAllowGet()
      .then((l) => {
        const v = l.join("\n");
        setRaw(v);
        snapRef.current.cors = normLines(v);
      })
      .catch(() => {})
      .finally(() => setLoaded((p) => ({ ...p, cors: true })));
    api
      .settingsGet()
      .then((s) => {
        setPort(s.preferredPort);
        setLogsEnabled(s.logsEnabled);
        setRetentionDays(s.retentionDays);
        setLogRowCap(s.logRowCap);
        snapRef.current.port = s.preferredPort;
        snapRef.current.logsEnabled = s.logsEnabled;
        snapRef.current.retentionDays = s.retentionDays;
        snapRef.current.logRowCap = s.logRowCap;
      })
      .catch(() => {})
      .finally(() => setLoaded((p) => ({ ...p, settings: true })));
    api
      .proxyGet()
      .then((p) => {
        const bypass = p.bypass.join("\n");
        setProxyEnabled(p.enabled);
        setProxyUrl(p.url);
        setProxyBypass(bypass);
        snapRef.current.proxyEnabled = p.enabled;
        snapRef.current.proxyUrl = p.url;
        snapRef.current.proxyBypass = normLines(bypass);
      })
      .catch(() => {})
      .finally(() => setLoaded((p) => ({ ...p, proxy: true })));
  }, []);

  async function saveProxy() {
    setProxyMsg("");
    setProxyMsgOk(false);
    if (proxyEnabled && !proxyUrl.trim()) {
      setProxyMsg("启用代理需填写代理地址");
      return;
    }
    const bypass = proxyBypass
      .split("\n")
      .map((s) => s.trim())
      .filter(Boolean);
    setProxyBusy(true);
    try {
      await api.proxySet({ enabled: proxyEnabled, url: proxyUrl.trim(), bypass });
      setProxyMsg("已保存：重启网关后生效（与端口约定一致）");
      setProxyMsgOk(true);
      // 保存成功 → 快照跟进，立即回到「不脏」（否则刚保存完就被拦）
      snapRef.current.proxyEnabled = proxyEnabled;
      snapRef.current.proxyUrl = proxyUrl.trim();
      snapRef.current.proxyBypass = bypass.join("\n");
    } catch (e) {
      setProxyMsg(String(e));
    } finally {
      setProxyBusy(false);
    }
  }

  async function testProxy() {
    setProxyMsg("");
    setProxyMsgOk(false);
    if (proxyEnabled && !proxyUrl.trim()) {
      setProxyMsg("启用代理需填写代理地址");
      return;
    }
    const bypass = proxyBypass
      .split("\n")
      .map((s) => s.trim())
      .filter(Boolean);
    setProxyBusy(true);
    try {
      const r = await api.proxyTest({ enabled: proxyEnabled, url: proxyUrl.trim(), bypass });
      setProxyMsg(r);
      setProxyMsgOk(true);
    } catch (e) {
      setProxyMsg(String(e));
    } finally {
      setProxyBusy(false);
    }
  }

  async function save() {
    const lines = raw.split("\n").map((s) => s.trim()).filter(Boolean);
    await api.corsAllowSet(lines);
    setRaw(lines.join("\n"));
    snapRef.current.cors = lines.join("\n");
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
    snapRef.current.port = n;
    setPortMsg("已保存：重启网关后生效（端口占用自动顺延）");
    toast("端口已保存");
  }

  async function toggleLogs(on: boolean) {
    await api.settingsSetLogsEnabled(on);
    setLogsEnabled(on);
    snapRef.current.logsEnabled = on;
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
      <PageHeader title="设置" description="端口、日志、安全策略与软件更新。" />

      {/* 吸顶锚点：设置页较长（多张卡片），让端口/代理/日志/CORS/凭据/更新随时可跳。
          data-slot=page-actions 是探针契约：本页的「常驻可达入口」就是这条锚点导航
          （各卡片的保存按钮随卡片滚动，故不作为首屏断言对象），fold.mjs 据此断言其可见。 */}
      <nav
        data-slot="page-actions"
        aria-label="设置分区导航"
        className="sticky top-0 z-10 flex flex-wrap gap-1 rounded-md border border-border/60 bg-background/95 px-1.5 py-1 backdrop-blur"
      >
        {([["settings-port", "端口"], ["settings-proxy", "代理"], ["settings-logs", "日志"], ["settings-cors", "跨域"], ["settings-creds", "凭据"], ["settings-data", "数据"], ["settings-update", "更新"]] as Array<[string, string]>).map(
          ([id, label]) => (
            <Button
              key={id}
              variant="ghost"
              size="sm"
              className="h-7 px-2 text-xs"
              onClick={() =>
                document.getElementById(id)?.scrollIntoView({ block: "start", behavior: "smooth" })
              }
            >
              {label}
            </Button>
          ),
        )}
      </nav>


      <Card id="settings-port" className="scroll-mt-16">
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
          {portMsg && <div className="text-xs text-emerald-700 dark:text-emerald-400">{portMsg}</div>}
        </CardContent>
      </Card>

      <Card id="settings-proxy" className="scroll-mt-16">
        <CardHeader>
          <CardTitle className="flex items-center gap-2.5">
            <Globe className="size-4" aria-hidden />
            网络代理
            <Switch
              checked={proxyEnabled}
              onCheckedChange={setProxyEnabled}
              aria-label="网络代理开关"
            />
            <span className="text-sm font-normal text-muted-foreground">
              {proxyEnabled ? "已启用" : "已关闭"}
            </span>
          </CardTitle>
          <CardDescription>
            网关出站（上游模型 / 健康检查 / WebDAV 同步）经代理访问；关闭时与默认行为完全一致。
            保存后<strong>重启网关生效</strong>（与端口约定一致）。
          </CardDescription>
        </CardHeader>
        <CardContent className="space-y-3">
          <div className="space-y-1">
            <div className="text-xs text-muted-foreground">
              代理地址（http / https / socks5，可含 user:pass@ 认证）
            </div>
            <Input
              className="font-mono"
              disabled={!proxyEnabled}
              value={proxyUrl}
              onChange={(e) => setProxyUrl(e.target.value)}
              placeholder="http://127.0.0.1:7890"
              aria-label="代理地址"
            />
          </div>
          <div className="space-y-1">
            <div className="text-xs text-muted-foreground">
              绕过列表（每行一个 host 或 .suffix；* 表示全部绕过）
            </div>
            <Textarea
              className="h-20 font-mono text-xs"
              disabled={!proxyEnabled}
              value={proxyBypass}
              onChange={(e) => setProxyBypass(e.target.value)}
              placeholder={"127.0.0.1\n.internal"}
              aria-label="代理绕过列表"
            />
          </div>
          <div className="flex items-center gap-2">
            <Button onClick={() => void saveProxy()} disabled={proxyBusy}>
              保存
            </Button>
            <Button
              variant="outline"
              size="sm"
              disabled={proxyBusy}
              onClick={() => void testProxy()}
            >
              {proxyBusy ? "测试中…" : "测试连接"}
            </Button>
          </div>
          {proxyMsg && (
            <div
              className={
                proxyMsgOk
                  ? "text-xs text-emerald-700 dark:text-emerald-400"
                  : "text-xs text-destructive"
              }
            >
              {proxyMsg}
            </div>
          )}
        </CardContent>
      </Card>

      <Card id="settings-logs" className="scroll-mt-16">
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
          {logMsg && <div className="mt-2 text-xs text-emerald-700 dark:text-emerald-400">{logMsg}</div>}
        </CardContent>
      </Card>

      <Card id="settings-cors" className="scroll-mt-16">
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
            {saved && <span className="text-xs text-emerald-700 dark:text-emerald-400">已生效</span>}
          </div>
        </CardContent>
      </Card>

      <Card id="settings-creds" className="scroll-mt-16">
        <CardHeader>
          <CardTitle className="flex items-center gap-2">
            <KeyRound className="size-4" aria-hidden />
            关于密钥存储
          </CardTitle>
          <CardDescription>凭据统一入库，随 WebDAV 配置同步</CardDescription>
        </CardHeader>
        <CardContent className="space-y-3">
          <ul className="list-disc space-y-1.5 pl-5 text-xs leading-relaxed text-muted-foreground">
            <li>上游 API Key：明文存放在本地 SQLite，安全性依赖数据目录文件权限（与网关 Key 同级）。</li>
            <li>网关 Key sk-jai-*：明文存放本地 SQLite，前缀展示、轮换采用吊销+新建保留审计痕迹。</li>
            <li>配置 WebDAV 后：供应商 API Key、网关 Key、WebDAV 密码随导出同步，换机拉取即用；手动重新生成网关 Key 会自动更新远端。</li>
            <li>删除供应商会同时清除对应凭据。</li>
          </ul>
          <Button variant="outline" size="sm" onClick={() => goTab("providers")}>
            <ShieldCheck aria-hidden />
            前往供应商页补录/查看凭据 →
          </Button>
        </CardContent>
      </Card>

      <Card id="settings-data" className="scroll-mt-16">
        <CardHeader>
          <CardTitle>数据位置</CardTitle>
        </CardHeader>
        <CardContent>
          <code className="font-mono text-xs text-muted-foreground">
            jai.db 位于系统应用数据目录（Tauri appDataDir）/jai.db
          </code>
        </CardContent>
      </Card>

      <UpdateCard />

      <ConfirmDialog
        open={!!portBusyWarn}
        onOpenChange={(o) => !o && setPortBusyWarn("")}
        title="端口当前被占用"
        description={portBusyWarn}
        confirmText="仍然保存"
        onConfirm={async () => {
          await api.settingsSetPort(Number(port));
          snapRef.current.port = Number(port);
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
                  snapRef.current.retentionDays = retentionDays;
                  snapRef.current.logRowCap = logRowCap;
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

type UpdateStatus = "idle" | "checking" | "latest" | "available" | "downloading" | "ready";

/** 软件更新卡片：检查 GitHub Releases 最新版本 → 下载安装 → 重启生效。 */
function UpdateCard() {
  const [version, setVersion] = useState("");
  const [status, setStatus] = useState<UpdateStatus>("idle");
  const [update, setUpdate] = useState<Update | null>(null);
  const [progress, setProgress] = useState(0);
  const [err, setErr] = useState("");
  // 持有 Update 对象供下载阶段使用；卸载后置空避免悬挂引用
  const updateRef = useRef<Update | null>(null);

  useEffect(() => {
    getVersion()
      .then(setVersion)
      .catch(() => setVersion(""));
    // 打开设置页时静默检查一次；失败不打扰（如离线 / 开发模式）
    void silentCheck();
  }, []);

  useEffect(() => {
    return () => {
      updateRef.current = null;
    };
  }, []);

  async function silentCheck() {
    try {
      const upd = await checkForUpdate();
      if (upd) {
        updateRef.current = upd;
        setUpdate(upd);
        setStatus("available");
      } else {
        setStatus("latest");
      }
    } catch {
      /* 静默失败：保留 idle 态，用户可手动重试 */
    }
  }

  async function check() {
    setStatus("checking");
    setErr("");
    try {
      const upd = await checkForUpdate();
      if (upd) {
        updateRef.current = upd;
        setUpdate(upd);
        setStatus("available");
      } else {
        updateRef.current = null;
        setUpdate(null);
        setStatus("latest");
        toast("当前已是最新版本");
      }
    } catch (e) {
      setErr(String(e));
      setStatus("idle");
    }
  }

  async function download() {
    const upd = updateRef.current ?? update;
    if (!upd) return;
    setStatus("downloading");
    setProgress(0);
    let received = 0;
    let total = 0;
    try {
      await upd.downloadAndInstall((event) => {
        if (event.event === "Started") {
          total = event.data.contentLength ?? 0;
        } else if (event.event === "Progress") {
          received += event.data.chunkLength;
          if (total > 0) {
            setProgress(Math.min(100, Math.round((received / total) * 100)));
          }
        } else if (event.event === "Finished") {
          setProgress(100);
        }
      });
      setStatus("ready");
    } catch (e) {
      setErr(String(e));
      setStatus("available");
    }
  }

  return (
    <Card id="settings-update" className="scroll-mt-16">
      <CardHeader>
        <CardTitle>软件更新</CardTitle>
        <CardDescription>
          更新源：GitHub Releases（JNNarrator/jai-gateway），下载包经 minisign 签名校验。
          {version ? `当前版本 v${version}。` : ""}
        </CardDescription>
      </CardHeader>
      <CardContent className="space-y-3">
        {status === "latest" && (
          <div className="text-sm text-emerald-700 dark:text-emerald-400">
            ✓ 已是最新版本{version ? `（v${version}）` : ""}
          </div>
        )}
        {status === "available" && update && (
          <div className="space-y-2">
            <div className="text-sm">
              发现新版本{" "}
              <span className="font-medium text-primary">v{update.version}</span>
              {version ? `（当前 v${version}）` : ""}
            </div>
            {update.body && (
              <pre className="max-h-40 overflow-y-auto whitespace-pre-wrap rounded-md border bg-muted/40 px-3 py-2 text-xs text-muted-foreground">
                {update.body}
              </pre>
            )}
          </div>
        )}
        {status === "downloading" && (
          <div className="space-y-1.5">
            <div className="h-2 w-full overflow-hidden rounded-full bg-muted">
              <div
                className="h-full rounded-full bg-primary transition-all"
                style={{ width: `${progress}%` }}
              />
            </div>
            <div className="text-xs text-muted-foreground">
              下载并安装中… {progress}%
            </div>
          </div>
        )}
        {status === "ready" && (
          <div className="text-sm text-emerald-700 dark:text-emerald-400">
            ✓ 更新已安装，重启应用后生效
          </div>
        )}
        {err && (
          <p className="text-xs text-destructive" role="alert">
            {err}
          </p>
        )}
        <div className="flex items-center gap-2">
          <Button
            variant="outline"
            size="sm"
            onClick={() => void check()}
            disabled={status === "checking" || status === "downloading"}
          >
            <RefreshCw aria-hidden className={cn(status === "checking" && "animate-spin")} />
            {status === "checking" ? "检查中…" : "检查更新"}
          </Button>
          {status === "available" && (
            <Button size="sm" onClick={() => void download()}>
              下载并安装
            </Button>
          )}
          {status === "ready" && (
            <Button size="sm" onClick={() => void relaunch()}>
              重启应用
            </Button>
          )}
        </div>
      </CardContent>
    </Card>
  );
}
