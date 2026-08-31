import { useEffect, useState } from "react";
import { api } from "../api";
import { toast } from "../lib/toast";
import { goTab } from "../lib/nav";
import { Card, inputCls, btnGhost, btnPrimary } from "../components/common/legacy";

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
      if (!confirm(`端口 ${n} 当前被占用，保存后网关会自动顺延到可用端口。继续？`)) {
        return;
      }
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

  return (
    <div className="mx-auto max-w-2xl space-y-4">
      <h1 className="text-lg font-semibold">设置</h1>

      <Card title="网关端口">
        <div className="flex items-center gap-2">
          <input
            className={`${inputCls} w-32 font-mono`}
            type="number"
            value={port}
            onChange={(e) => setPort(Number(e.target.value))}
          />
          <button className={btnPrimary} onClick={savePort}>
            保存
          </button>
          <span className="text-xs text-neutral-500">
            默认 1314；被占用时顺延，实际端口见「网关」页
          </span>
        </div>
        {portMsg && <div className="mt-2 text-xs text-emerald-400">{portMsg}</div>}
      </Card>

      <Card title="请求日志">
        <div className="flex items-center gap-3">
          <button
            role="switch"
            aria-checked={logsEnabled}
            onClick={() => toggleLogs(!logsEnabled)}
            className={`relative h-5 w-9 rounded-full transition-colors ${
              logsEnabled ? "bg-emerald-600" : "bg-neutral-700"
            }`}
          >
            <span
              className={`absolute top-0.5 h-4 w-4 rounded-full bg-white transition-all ${
                logsEnabled ? "left-[18px]" : "left-0.5"
              }`}
            />
          </button>
          <span className="text-sm text-neutral-300">
            日志记录：{logsEnabled ? "记录中" : "已关闭"}
          </span>
        </div>
        <p className="mt-3 text-xs leading-relaxed text-neutral-500">
          保留策略：{retentionDays} 天 / 上限 {logRowCap.toLocaleString("zh-CN")} 行
          （每日自动清理）。日志仅含元数据，不含 prompt 与响应明文。
        </p>
        <button
          className={`${btnGhost} mt-2`}
          onClick={async () => {
            const daysStr = window.prompt("保留天数（至少 1）", String(retentionDays));
            if (!daysStr) return;
            const days = Number(daysStr);
            const capStr = window.prompt("日志行数上限（至少 1000）", String(logRowCap));
            if (!capStr) return;
            const cap = Number(capStr);
            try {
              await api.settingsSetRetention(days, cap);
              setRetentionDays(days);
              setLogRowCap(cap);
              toast("保留策略已更新");
            } catch (e) {
              setLogMsg(String(e));
            }
          }}
        >
          编辑保留策略
        </button>
        {logMsg && <div className="mt-2 text-xs text-emerald-400">{logMsg}</div>}
      </Card>

      <Card title="浏览器跨域白名单（CORS）">
        <p className="mb-3 text-xs leading-relaxed text-neutral-500">
          默认拒绝一切远程网页来源访问网关（安全基线）。
          如果你使用网页版聊天应用（如自部署的 NextChat 等），
          把它的 Origin 加进白名单，每行一条；通配符 * 表示放行全部（不推荐）。
          本机来源（localhost / 127.0.0.1）始终放行。
        </p>
        {corsHasWildcard && (
          <div className="mb-3 rounded border border-amber-800 bg-amber-950/40 px-3 py-2 text-xs text-amber-300">
            ⚠ 通配符 * 会放行所有来源，生产环境不建议使用。
          </div>
        )}
        <textarea
          className={`${inputCls} h-32 font-mono`}
          value={raw}
          onChange={(e) => setRaw(e.target.value)}
          placeholder={"https://chat.example.com\nhttp://192.168.1.5:3000"}
        />
        <div className="mt-3 flex flex-wrap items-center gap-2">
          <button className={btnPrimary} onClick={save}>
            保存
          </button>
          <button
            className={btnGhost}
            onClick={() => {
              const origin = window.prompt("输入允许的来源 Origin，例如 https://chat.example.com");
              if (!origin) return;
              const lines = raw.split("\n").map((s) => s.trim()).filter(Boolean);
              if (!lines.includes(origin)) {
                lines.push(origin);
                setRaw(lines.join("\n"));
              }
            }}
          >
            + 添加域名
          </button>
          <button
            className={btnGhost}
            onClick={() => setRaw("https://chat.example.com\nhttp://192.168.1.5:3000")}
          >
            填入示例
          </button>
          {saved && <span className="text-xs text-emerald-400">已生效</span>}
        </div>
      </Card>

      <Card title="关于密钥存储">
        <p className="mb-2 text-xs text-neutral-500">
          当前凭据存储：{" "}
          <span className={vaultKind === "file" ? "text-amber-400" : "text-emerald-400"}>
            {vaultKind === "keyring"
              ? "🔑 系统钥匙串"
              : vaultKind === "file"
                ? "📁 文件降级（0600）"
                : vaultKind}
          </span>
        </p>
        <ul className="list-disc space-y-1.5 pl-5 text-xs leading-relaxed text-neutral-500">
          <li>🔑 上游 API Key：优先保存在 macOS 钥匙串 / Windows 凭据管理器，数据库只存引用。</li>
          <li>📁 若系统凭据存储不可用（如沙箱/CI 权限受限），自动降级为数据目录下的 vault_fallback.json（Unix 0600）。</li>
          <li>🔐 网关 Key sk-jai-*：按设计决策明文存放本地 SQLite（非敏感级别），前缀展示、轮换采用吊销+新建保留审计痕迹、永不导出。</li>
          <li>🗑 删除供应商会同时清除对应凭据。</li>
        </ul>
        <button className={`${btnGhost} mt-3`} onClick={() => goTab("providers")}>
          前往供应商页补录/查看凭据 →
        </button>
      </Card>

      <Card title="数据位置">
        <code className="font-mono text-xs text-neutral-400">
          jai.db 位于系统应用数据目录（Tauri appDataDir）/jai.db
        </code>
      </Card>
    </div>
  );
}
