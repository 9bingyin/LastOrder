import { useEffect, useState } from "react";
import { Button } from "@cloudflare/kumo/components/button";
import { Surface } from "@cloudflare/kumo/components/surface";
import { clearDebugEvents, redactDebug } from "./diagnostics";
import type { DebugMedia } from "./diagnostics";
import { startSampling } from "./sampling";
import type { Sample } from "./sampling";

type Props = {
  token: string | null;
  readMedia: () => DebugMedia[];
  readRuntime: () => unknown;
};

function Raw({
  title,
  value,
  open = false,
}: {
  title: string;
  value: unknown;
  open?: boolean;
}) {
  return (
    <Surface className="min-w-0 rounded-xl px-5 py-4 ring ring-kumo-line">
      <details open={open}>
        <summary className="cursor-pointer text-sm font-medium text-kumo-strong">
          {title}
        </summary>
        <pre
          className="mt-3 max-h-96 overflow-auto whitespace-pre text-sm text-kumo-default"
          tabIndex={0}
        >
          {typeof value === "string" ? value : JSON.stringify(value, null, 2)}
        </pre>
      </details>
    </Surface>
  );
}

export function DebugPage({ token, readMedia, readRuntime }: Props) {
  const [paused, setPaused] = useState(false);
  const [sample, setSample] = useState<Sample | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [copied, setCopied] = useState(false);
  useEffect(() => {
    if (!token || paused) return;
    return startSampling(
      token,
      readMedia,
      () => ({
        userAgent: navigator.userAgent,
        language: navigator.language,
        online: navigator.onLine,
        secureContext: window.isSecureContext,
        visibilityState: document.visibilityState,
        viewport: { width: innerWidth, height: innerHeight, devicePixelRatio },
        runtime: readRuntime(),
      }),
      (value) => {
        setSample(value);
        setError(value.failure);
      },
    );
  }, [token, paused, readMedia, readRuntime]);

  const safe = sample ? redactDebug(sample, token ? [token] : []) : null;
  return (
    <section
      aria-label="Debug"
      className="mx-auto grid max-w-6xl min-w-0 gap-5 px-5 py-7"
    >
      <div className="flex flex-wrap items-center justify-between gap-3">
        <div className="grid gap-1.5">
          <h2 className="text-xl font-semibold text-kumo-strong">Debug</h2>
          <p className="text-sm text-kumo-subtle">
            每秒采样，事件保留最近 100
            条。凭证已脱敏；仍含网络地址和媒体信息，分享前请检查。
          </p>
        </div>
        <div className="flex flex-wrap gap-2">
          <Button onClick={() => setPaused(!paused)}>
            {paused ? "继续采样" : "暂停采样"}
          </Button>
          <Button
            disabled={!sample}
            onClick={() =>
              void navigator.clipboard
                .writeText(JSON.stringify(safe, null, 2))
                .then(() => setCopied(true))
                .catch(() => setError("复制失败"))
            }
          >
            {copied ? "已复制" : "复制诊断"}
          </Button>
          <Button
            onClick={() => {
              clearDebugEvents();
              setSample((value) => (value ? { ...value, events: [] } : value));
            }}
          >
            清空事件
          </Button>
        </div>
      </div>
      {!token && (
        <p role="alert" className="text-sm text-kumo-danger">
          请使用 CLI 输出的地址打开页面。
        </p>
      )}
      {error && (
        <p role="alert" className="text-sm text-kumo-danger">
          {String(redactDebug(error, token ? [token] : []))}
        </p>
      )}
      <p className="text-sm text-kumo-subtle">
        采样时间：{sample?.time ?? "—"}
        {paused ? "（已暂停）" : ""}
      </p>
      <Raw
        title="CLI 与房间原始快照"
        value={redactDebug(sample?.backend ?? null, token ? [token] : [])}
        open
      />
      {sample?.backend?.connections.map((connection) => (
        <div key={connection.remoteId} className="grid min-w-0 gap-5">
          <Raw
            title={`Iroh 完整连接统计 · ${connection.remoteId}`}
            value={connection.statsRaw}
          />
          {connection.paths.map((path, index) => (
            <Raw
              key={index}
              title={`Iroh 路径统计 · ${index + 1}`}
              value={path.statsRaw}
            />
          ))}
        </div>
      ))}
      <Raw
        title="浏览器与页面运行状态"
        value={redactDebug(sample?.browser ?? null, token ? [token] : [])}
      />
      <Raw
        title="WebRTC 原始统计、SDP 与媒体轨道"
        value={redactDebug(sample?.media ?? [], token ? [token] : [])}
        open
      />
      <Raw
        title="WebSocket、API 与页面事件"
        value={redactDebug(sample?.events ?? [], token ? [token] : [])}
      />
    </section>
  );
}
