import { useEffect, useState } from "react";
import { api } from "../api";
import type { GatewayKeyInfo } from "../types";
import { toast } from "../lib/toast";
import { copyText } from "../lib/clipboard";
import { Card, btnDanger, btnPrimary, btnGhost } from "../components/common/legacy";

export function GatewayPage() {
  const [status, setStatus] = useState<import("../types").GwStatus | null>(null);
  const [key, setKey] = useState<GatewayKeyInfo | null>(null);
  const [revealKey, setRevealKey] = useState<string>("");
  const [busy, setBusy] = useState(false);
  const [err, setErr] = useState("");
  const [sdk, setSdk] = useState<"deepseek" | "openai" | "curl">("deepseek");

  const port = status?.port ?? 1314;
  const baseUrl = `http://127.0.0.1:${port}/v1`;
  const fullKey = revealKey || (key ? `${key.prefix}…` : "");

  async function refresh() {
    try {
      setStatus(await api.status());
    } catch (e) {
      setErr(String(e));
    }
  }

  useEffect(() => {
    refresh();
    const t = setInterval(refresh, 2000);
    return () => clearInterval(t);
  }, []);

  useEffect(() => {
    api.gatewayKeyInfo().then((k) => setKey(k)).catch(() => {});
  }, []);

  async function toggle(run: boolean) {
    setBusy(true);
    setErr("");
    try {
      setStatus(run ? await api.start() : await api.stop());
    } catch (e) {
      setErr(String(e));
    } finally {
      setBusy(false);
    }
  }

  async function doReveal() {
    if (revealKey) {
      setRevealKey("");
      return;
    }
    const full = await api.gatewayKeyReveal();
    setRevealKey(full.key);
  }

  async function doRegen() {
    if (!confirm("轮换密钥：旧密钥将立即失效，已配置的客户端需要更新。继续？")) return;
    const k = await api.gatewayKeyRegenerate();
    setKey(k);
    setRevealKey(k.key);
    toast("密钥已轮换");
  }

  async function doExport() {
    const path = await api.exportConfigToFile();
    toast("已导出（不含 API Key）");
    try {
      await api.revealInFolder(path);
    } catch {
      // 平台不支持打开目录时忽略
    }
  }

  const snippet =
    sdk === "deepseek"
      ? `# DeepSeek Harness（dsh）
export DEEPSEEK_API_KEY=${fullKey || "sk-jai-…"}
# dsh 的 provider 配置里 baseURL 填 ${baseUrl}`
      : sdk === "openai"
        ? `# OpenAI SDK
OPENAI_API_BASE=${baseUrl}
OPENAI_API_KEY=${fullKey || "sk-jai-…"}`
        : `curl ${baseUrl}/chat/completions \\
  -H "Authorization: Bearer ${fullKey || "sk-jai-…"}" \\
  -H "Content-Type: application/json" \\
  -d '{"model":"deepseek-v4-flash","messages":[{"role":"user","content":"hi"}]}'`;

  return (
    <div className="mx-auto max-w-2xl space-y-4">
      {err && (
        <div className="rounded border border-red-900 bg-red-950/40 px-3 py-2 text-sm text-red-300">
          {err}
        </div>
      )}

      <Card title="网关状态">
        <div className="flex flex-wrap items-center gap-3">
          <span
            className={`inline-block h-3 w-3 rounded-full ${
              status?.running ? "bg-emerald-500" : "bg-neutral-600"
            }`}
          />
          {status?.running ? (
            <>
              <span className="rounded bg-emerald-950 px-2 py-0.5 text-xs font-semibold text-emerald-400">
                运行中
              </span>
              <span className="font-mono text-lg font-bold text-white">
                127.0.0.1:{port}
              </span>
            </>
          ) : (
            <>
              <span className="rounded bg-neutral-800 px-2 py-0.5 text-xs font-semibold text-neutral-400">
                已停止
              </span>
              <span className="text-lg font-semibold text-neutral-500">
                未运行
              </span>
            </>
          )}
          <div className="ml-auto flex gap-2">
            {status?.running ? (
              <button
                className={btnDanger}
                disabled={busy}
                onClick={() => toggle(false)}
              >
                停止
              </button>
            ) : (
              <button
                className={btnPrimary}
                disabled={busy}
                onClick={() => toggle(true)}
              >
                启动
              </button>
            )}
          </div>
        </div>
        <p className="mt-3 text-xs leading-relaxed text-neutral-600" title="端口被占用时从 1314 起自动顺延">
          端口被占用会自动顺延（1314 起）。所有业务端点强制鉴权：Host 仅接受本机回环地址，浏览器跨域需在「设置」中添加白名单。
        </p>
      </Card>

      <Card title="客户端接入">
        <div className="space-y-3 text-sm">
          <div>
            <div className="mb-1 text-neutral-400">Base URL</div>
            <div className="flex items-center gap-2">
              <code className="flex-1 rounded bg-black/60 px-3 py-2 font-mono text-emerald-400">
                {baseUrl}
              </code>
              <button className={btnGhost} onClick={() => copyText(baseUrl)} title="复制 Base URL">
                复制
              </button>
            </div>
          </div>
          <div>
            <div className="mb-1 text-neutral-400">API Key</div>
            <div className="flex items-center gap-2">
              <code className="flex-1 truncate rounded bg-black/60 px-3 py-2 font-mono text-emerald-400">
                {revealKey
                  ? fullKey
                  : key
                    ? `${key.prefix}…（点击右侧显示全文）`
                    : "加载中"}
              </code>
              <button className={btnGhost} onClick={() => revealKey && copyText(revealKey)} disabled={!revealKey} title="复制 API Key">
                复制
              </button>
              <button className={btnGhost} onClick={doReveal}>
                {revealKey ? "隐藏" : "显示"}
              </button>
              <button className={btnDanger} onClick={doRegen}>
                轮换密钥
              </button>
            </div>
          </div>
          <div>
            <div className="mb-1 flex items-center justify-between">
              <span className="text-neutral-400">接入示例</span>
              <select
                className="rounded border border-neutral-700 bg-neutral-950 px-2 py-1 text-xs text-neutral-300 outline-none focus:border-amber-500"
                value={sdk}
                onChange={(e) => setSdk(e.target.value as typeof sdk)}
              >
                <option value="deepseek">DeepSeek Harness</option>
                <option value="openai">OpenAI SDK</option>
                <option value="curl">curl</option>
              </select>
            </div>
            <pre className="relative mt-1 overflow-x-auto rounded bg-black/60 p-3 font-mono text-xs leading-relaxed text-neutral-300">
              <button
                className="absolute right-2 top-2 rounded border border-neutral-700 px-2 py-0.5 text-[10px] text-neutral-400 hover:text-neutral-200"
                onClick={() => copyText(snippet)}
              >
                复制
              </button>
              {snippet}
            </pre>
          </div>
        </div>
      </Card>

      <Card title="配置迁移">
        <div className="flex flex-wrap items-center gap-2">
          <button className={btnGhost} onClick={doExport}>
            导出配置（无敏感字段）
          </button>
          <span className="text-xs text-neutral-500">
            密钥不会包含在导出文件中；导入后需在「供应商」页补录 API Key。
          </span>
        </div>
      </Card>
    </div>
  );
}
