import { useEffect, useState } from "react";
import { api } from "../api";
import { toast } from "../lib/toast";
import { goTab } from "../lib/nav";
import { Card, inputCls, btnPrimary, btnGhost } from "../components/common/legacy";

export function SyncPage() {
  const [importText, setImportText] = useState("");
  const [cfg, setCfg] = useState<{ url: string; username: string; directory: string }>({
    url: "",
    username: "",
    directory: "",
  });
  const [pw, setPw] = useState("");
  const [showPw, setShowPw] = useState(false);
  const [msg, setMsg] = useState("");
  const [err, setErr] = useState("");
  const [busy, setBusy] = useState("");

  useEffect(() => {
    api
      .webdavConfigGet()
      .then((c) => {
        if (c) setCfg(c);
      })
      .catch(() => {});
  }, []);

  async function doImport() {
    setErr("");
    if (!importText.trim()) {
      setErr("请先粘贴 JSON 内容");
      return;
    }
    if (!confirm("导入会将 JSON 合并进当前本地配置，确定继续？")) return;
    try {
      const r = await api.configImport(importText, false);
      setMsg(
        `导入完成：新增供应商 ${r.providersImported}，重复跳过 ${r.providersSkippedDuplicate}，模型 ${r.modelsImported}；待补密钥：${r.missingKeys.join("、") || "无"}`,
      );
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
      setMsg(r.message);
      if (r.willOverwrite && !confirm(`${r.message}。确定继续拉取吗？`)) {
        return;
      }
      if (r.willOverwrite) {
        await doPull();
      } else {
        toast("远端与本地一致");
      }
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
      <h1 className="text-lg font-semibold">配置同步</h1>
      {err && (
        <div className="rounded border border-red-900 bg-red-950/40 px-3 py-2 text-sm text-red-300">
          {err}
        </div>
      )}
      {msg && (
        <div className="rounded border border-emerald-900 bg-emerald-950/40 px-3 py-2 text-sm text-emerald-300">
          {msg}
        </div>
      )}

      <Card title="导入导出 JSON">
        <p className="mb-3 text-xs leading-relaxed text-neutral-500">
          导出产物只含供应商与模型定义，不含任何 API Key。导入后请在「供应商」页逐项补录凭据。
        </p>
        <textarea
          className={`${inputCls} h-32 font-mono`}
          value={importText}
          onChange={(e) => setImportText(e.target.value)}
          placeholder="粘贴从另一台设备导出的 jai-export JSON…"
        />
        <div className="mt-3 flex flex-wrap items-center gap-2">
          <button className={btnPrimary} onClick={doImport}>
            导入
          </button>
          <button
            className={btnGhost}
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
            粘贴
          </button>
          <span className="text-xs text-neutral-500">
            导出入口仍在「网关」页。
          </span>
          <button className={btnGhost} onClick={() => goTab("gateway")}>
            前往网关页 →
          </button>
        </div>
      </Card>

      <Card title="WebDAV 同步">
        <p className="mb-3 text-xs leading-relaxed text-neutral-500">
          手动推/拉：推送使用本机当前配置覆盖远端；拉取使用远端配置覆盖本机
          （last-write-wins）。推送前会在本地留存一份快照。
        </p>
        <div className="grid grid-cols-2 gap-3">
          <label className="text-xs text-neutral-400">
            WebDAV 根地址
            <input
              className={`${inputCls} mt-1 font-mono`}
              value={cfg.url}
              onChange={(e) => setCfg({ ...cfg, url: e.target.value })}
              placeholder="https://dav.example.com/remote.php/dav/files/u"
            />
          </label>
          <label className="text-xs text-neutral-400">
            用户名
            <input
              className={`${inputCls} mt-1`}
              value={cfg.username}
              onChange={(e) => setCfg({ ...cfg, username: e.target.value })}
            />
          </label>
          <label className="text-xs text-neutral-400">
            目录（可选）
            <input
              className={`${inputCls} mt-1 font-mono`}
              value={cfg.directory}
              onChange={(e) => setCfg({ ...cfg, directory: e.target.value })}
              placeholder="jai/backups"
            />
          </label>
          <label className="text-xs text-neutral-400">
            <span className="flex items-center justify-between">
              <span>密码（存入钥匙串，留空保持原密码）</span>
              <button
                className={btnGhost}
                type="button"
                onClick={() => setShowPw(!showPw)}
              >
                {showPw ? "隐藏" : "显示"}
              </button>
            </span>
            <input
              className={`${inputCls} mt-1`}
              type={showPw ? "text" : "password"}
              value={pw}
              onChange={(e) => setPw(e.target.value)}
            />
          </label>
        </div>
        <div className="mt-4 flex flex-wrap gap-2">
          <button className={btnPrimary} onClick={saveCfg}>
            保存配置
          </button>
          <button className={btnGhost} onClick={testWebdav}>
            测试连接
          </button>
          <button className={btnGhost} onClick={previewWebdav}>
            预览变更
          </button>
          <button className={btnGhost} disabled={busy === "push"} onClick={doPush}>
            {busy === "push" ? "推送中…" : "推送"}
          </button>
          <button className={btnGhost} disabled={busy === "pull"} onClick={doPull}>
            {busy === "pull" ? "拉取中…" : "拉取"}
          </button>
        </div>
        {busy && (
          <div className="mt-3 h-1 w-full overflow-hidden rounded bg-neutral-800">
            <div className="h-full w-1/3 animate-pulse rounded bg-amber-500" />
          </div>
        )}
      </Card>
    </div>
  );
}
