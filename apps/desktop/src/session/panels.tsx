import { open as openDialog } from "@tauri-apps/plugin-dialog";
import { getCurrentWebview } from "@tauri-apps/api/webview";
import { ArrowUp, Copy, File as FileIcon, Folder, Pause, Play, RotateCcw, Send, Trash2, X } from "lucide-react";
import { useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { Button, IconButton, Select, TextInput, Toggle, cx } from "../components/ui";
import { api } from "../lib/api";
import { formatBytes, formatDuration, formatMbps, formatRate } from "../lib/format";
import type { SessionState } from "../lib/store";
import type { NetQuality, Settings, TransferInfo } from "../lib/types";
import type { PipelineMetrics } from "./pipeline";

const qualityTone: Record<NetQuality, string> = { excellent: "text-ok", good: "text-ok", fair: "text-warn", poor: "text-danger" };
const qualityBars: Record<NetQuality, number> = { excellent: 4, good: 3, fair: 2, poor: 1 };

export function InfoPanel({ session, metrics }: { session: SessionState; metrics: PipelineMetrics }) {
  const { t, i18n } = useTranslation();
  const st = session.stats;
  const link = session.ready?.link;
  const num = (v: number, d = 0) => new Intl.NumberFormat(i18n.language, { maximumFractionDigits: d }).format(v);
  const rows: [string, string][] = [
    [t("info.connection"), link ? t(`info.${link.kind}`) : "—"],
    [t("info.protocol"), link ? t(link.protocol === "tcp" ? "info.tcp" : "info.tlsws") : "—"],
    [t("info.latency"), st ? t("info.unit_ms", { value: num(st.rttMs, 0) }) : "—"],
    [t("info.jitter"), st ? t("info.unit_ms", { value: num(st.jitterMs, 1) }) : "—"],
    [t("info.fps"), num(metrics.fps, 0)],
    [t("info.resolution"), metrics.width ? `${metrics.width} × ${metrics.height}` : "—"],
    [t("info.bitrate"), st ? t("info.unit_mbps", { value: formatMbps(st.recvKbps, i18n.language) }) : "—"],
    [t("info.codec"), st?.host.codec === "H264" ? "H.264" : st?.host.codec === "H265" ? "H.265" : "—"],
    [t("info.dropped"), st ? `${num(st.droppedPercent, 1)}%` : "—"],
    [t("info.encoder"), st?.host.encoder || "—"],
    [t("info.capture"), st ? t("info.unit_ms", { value: num(st.host.captureMs, 1) }) : "—"],
    [t("info.encode"), st ? t("info.unit_ms", { value: num(st.host.encodeMs, 1) }) : "—"],
    [t("info.decode"), t("info.unit_ms", { value: num(metrics.decodeMs, 1) })],
    [t("info.render"), t("info.unit_ms", { value: num(metrics.renderMs, 1) })],
  ];
  const ms = (v: number, d = 1) => (v > 0 ? t("info.unit_ms", { value: num(v, d) }) : "—");
  const lat = st?.latency;
  const diagnostics: [string, string][] = lat
    ? [
        [t("info.inputRtt"), ms(lat.inputRttMs)],
        [t("info.inputQueue"), ms(lat.inputQueueMs, 2)],
        [t("info.hostQueue"), ms(lat.hostQueueMs, 2)],
        [t("info.hostApply"), ms(lat.hostApplyMs, 2)],
        [t("info.motionToFrame"), ms(lat.motionToFrameMs)],
        [t("info.staleMoves"), num(lat.staleMovesDropped)],
        [t("info.staleVideo"), num(lat.staleVideoDropped + metrics.droppedByDecoder)],
        [t("info.fileRate"), lat.fileRateKbps > 0 ? `${num(lat.fileRateKbps / 8, 0)} KB/s` : "—"],
      ]
    : [];
  return (
    <div className="p-4">
      <h3 className="mb-3 font-semibold">{t("info.title")}</h3>
      {st && (
        <div className={cx("mb-3 flex items-center gap-2 rounded-lg bg-surface-2 px-3 py-2 font-medium", qualityTone[st.quality])} role="status">
          <span aria-hidden className="flex items-end gap-0.5">
            {[1, 2, 3, 4].map((n) => (
              <span key={n} className={cx("w-1 rounded-sm bg-current", n > qualityBars[st.quality] && "opacity-25")} style={{ height: 4 + n * 3 }} />
            ))}
          </span>
          {t("info.network")}: {t(`info.${st.quality}`)}
        </div>
      )}
      <dl className="grid grid-cols-[auto_1fr] gap-x-4 gap-y-1.5 text-[13px]">
        {rows.map(([k, v]) => (
          <div key={k} className="contents">
            <dt className="text-muted">{k}</dt>
            <dd className="ltr tabular text-end" data-selectable>{v}</dd>
          </div>
        ))}
      </dl>
      {st && (
        <details className="mt-4 rounded-lg border border-border px-3 py-2 text-[13px]">
          <summary className="cursor-pointer select-none font-medium">{t("info.diagnostics")}</summary>
          <dl className="mt-2 grid grid-cols-[auto_1fr] gap-x-4 gap-y-1.5">
            {diagnostics.map(([k, v]) => (
              <div key={k} className="contents">
                <dt className="text-muted">{k}</dt>
                <dd className="ltr tabular text-end" data-selectable>{v}</dd>
              </div>
            ))}
          </dl>
        </details>
      )}
    </div>
  );
}

export function QualityPanel({ session, settings, onApplied }: { session: SessionState; settings: Settings; onApplied: () => void }) {
  const { t } = useTranslation();
  const [draft, setDraft] = useState<Partial<Settings>>({
    preset: settings.preset, fps: settings.fps, resolution: settings.resolution, bitrateKbps: settings.bitrateKbps,
    codec: settings.codec, hardwareAcceleration: settings.hardwareAcceleration, adaptiveQuality: settings.adaptiveQuality,
  });
  const set = (p: Partial<Settings>) => setDraft((d) => ({ ...d, ...p }));
  return (
    <div className="p-4">
      <h3 className="mb-2 font-semibold">{t("quality.title")}</h3>
      <div className="flex flex-col gap-1 divide-y divide-line">
        {(
          [
            ["preset", "quality.preset", <Select key="p" value={draft.preset} onChange={(e) => set({ preset: e.target.value as Settings["preset"] })} className="min-w-0">{(["auto", "best", "balanced", "lowLatency", "lowBandwidth"] as const).map((p) => <option key={p} value={p}>{t(`quality.${p}`)}</option>)}</Select>],
            ["fps", "quality.fps", <Select key="f" value={draft.fps} onChange={(e) => set({ fps: Number(e.target.value) as Settings["fps"] })} className="min-w-0"><option value={0}>{t("common.auto")}</option>{[15, 30, 60].map((f) => <option key={f} value={f}>{f}</option>)}</Select>],
            ["res", "quality.resolution", <Select key="r" value={draft.resolution} onChange={(e) => set({ resolution: e.target.value })} className="min-w-0"><option value="auto">{t("common.auto")}</option><option value="native">{t("quality.native")}</option><option value="1080">1080p</option><option value="720">720p</option><option value="480">480p</option></Select>],
            ["br", "quality.bitrate", <Select key="b" value={draft.bitrateKbps} onChange={(e) => set({ bitrateKbps: Number(e.target.value) })} className="min-w-0"><option value={0}>{t("common.auto")}</option>{[1000, 2500, 5000, 10000, 20000, 40000].map((b) => <option key={b} value={b}>{b / 1000} Mbps</option>)}</Select>],
            ["codec", "quality.codec", <Select key="c" value={draft.codec} onChange={(e) => set({ codec: e.target.value as Settings["codec"] })} className="min-w-0"><option value="auto">{t("common.auto")}</option><option value="h264">{t("quality.h264")}</option><option value="h265">{t("quality.h265")}</option></Select>],
          ] as const
        ).map(([k, label, control]) => (
          <label key={k} className="flex items-center justify-between gap-3 py-2 text-[13px]">
            <span>{t(label)}</span>
            {control}
          </label>
        ))}
        <Toggle label={t("quality.hardware")} checked={draft.hardwareAcceleration ?? true} onChange={(v) => set({ hardwareAcceleration: v })} />
        <Toggle label={t("quality.adaptive")} checked={draft.adaptiveQuality ?? true} onChange={(v) => set({ adaptiveQuality: v })} />
      </div>
      <Button variant="primary" className="mt-3 w-full" onClick={() => void api.setQuality(session.id, draft).then(onApplied)}>{t("quality.apply")}</Button>
    </div>
  );
}

export function ChatPanel({ messages, onSend, onClear }: { messages: { id: number; text: string; tsMs: number; mine: boolean }[]; onSend: (text: string) => void; onClear?: () => void }) {
  const { t, i18n } = useTranslation();
  const [text, setText] = useState("");
  const end = useRef<HTMLDivElement>(null);
  useEffect(() => end.current?.scrollIntoView({ block: "end" }), [messages.length]);
  const time = new Intl.DateTimeFormat(i18n.language, { timeStyle: "short" });
  return (
    <div className="flex h-full min-h-0 flex-col p-4">
      <div className="mb-2 flex items-center justify-between">
        <h3 className="font-semibold">{t("chat.title")}</h3>
        {onClear && <IconButton label={t("chat.clear")} onClick={onClear}><Trash2 size={16} /></IconButton>}
      </div>
      <div className="min-h-0 flex-1 space-y-2 overflow-auto" aria-live="polite">
        {messages.length === 0 && <p className="text-[13px] text-muted">{t("chat.empty")}</p>}
        {messages.map((m) => (
          <div key={m.id + String(m.mine)} className={cx("group flex flex-col", m.mine ? "items-end" : "items-start")}>
            <div className={cx("max-w-[85%] rounded-2xl px-3 py-1.5", m.mine ? "bg-accent text-accent-fg" : "bg-surface-2")} data-selectable dir="auto">{m.text}</div>
            <div className="flex items-center gap-1 text-[11px] text-muted">
              {time.format(m.tsMs)}
              <button aria-label={t("chat.copy")} title={t("chat.copy")} className="opacity-0 transition group-hover:opacity-100 focus:opacity-100" onClick={() => void navigator.clipboard.writeText(m.text)}>
                <Copy size={11} />
              </button>
            </div>
          </div>
        ))}
        <div ref={end} />
      </div>
      <form
        className="mt-2 flex gap-2"
        onSubmit={(e) => {
          e.preventDefault();
          if (text.trim()) onSend(text);
          setText("");
        }}
      >
        <TextInput value={text} onChange={(e) => setText(e.target.value)} placeholder={t("chat.placeholder")} maxLength={4000} dir="auto" aria-label={t("chat.placeholder")} />
        <Button type="submit" variant="primary" aria-label={t("chat.send")} title={t("chat.send")} icon={<Send size={15} />} />
      </form>
    </div>
  );
}

function TransferRow({ tr, onAction }: { tr: TransferInfo; onAction?: (a: "pause" | "resume" | "cancel" | "retry") => void }) {
  const { t, i18n } = useTranslation();
  const pct = tr.size > 0 ? Math.min(100, (tr.transferred / tr.size) * 100) : 100;
  const running = tr.status === "active" || tr.status === "paused" || tr.status === "pending";
  return (
    <li className="rounded-lg border border-line p-2.5">
      <div className="flex items-center justify-between gap-2">
        <span className="min-w-0 truncate font-medium" title={tr.name} dir="auto">{tr.name}</span>
        <span className="shrink-0 text-[12px] text-muted">{t(`files.status_${tr.status}`)}</span>
      </div>
      <div className="my-1.5 h-1.5 overflow-hidden rounded-full bg-surface-2" role="progressbar" aria-valuenow={Math.round(pct)} aria-valuemin={0} aria-valuemax={100} aria-label={tr.name}>
        <div className={cx("h-full rounded-full transition-[width]", tr.status === "failed" ? "bg-danger" : "bg-accent")} style={{ width: `${pct}%` }} />
      </div>
      <div className="flex items-center justify-between text-[12px] text-muted">
        <span className="ltr tabular">{formatBytes(tr.transferred, i18n.language)} / {formatBytes(tr.size, i18n.language)}</span>
        {tr.status === "active" && <span className="ltr tabular">{formatRate(tr.speedBps, i18n.language)}{tr.etaSecs != null && ` · ${formatDuration(tr.etaSecs)}`}</span>}
        {tr.error && <span className="text-danger">{tr.error}</span>}
      </div>
      {onAction && (
        <div className="mt-1 flex gap-1">
          {tr.status === "active" && <IconButton label={t("files.pause")} onClick={() => onAction("pause")}><Pause size={14} /></IconButton>}
          {tr.status === "paused" && <IconButton label={t("files.resume")} onClick={() => onAction("resume")}><Play size={14} /></IconButton>}
          {running && <IconButton label={t("files.cancel")} onClick={() => onAction("cancel")}><X size={14} /></IconButton>}
          {(tr.status === "failed" || tr.status === "cancelled") && tr.upload && <IconButton label={t("files.retry")} onClick={() => onAction("retry")}><RotateCcw size={14} /></IconButton>}
        </div>
      )}
    </li>
  );
}

export function RemoteBrowser({ session }: { session: SessionState }) {
  const { t, i18n } = useTranslation();
  const [path, setPath] = useState("");
  const [picked, setPicked] = useState<string | null>(null);
  useEffect(() => void api.listDir(session.id, path), [session.id, path]);
  const dir = session.dir && session.dir.path === path ? session.dir : null;
  const join = (name: string) => (path === "" ? name : path.replace(/[\\/]+$/, "") + "\\" + name);
  const parent = () => {
    const trimmed = path.replace(/[\\/]+$/, "");
    const idx = trimmed.lastIndexOf("\\");
    setPath(idx <= 2 && /^[A-Za-z]:$/.test(trimmed.slice(0, idx)) ? "" : idx > 0 ? trimmed.slice(0, idx + (trimmed.slice(0, idx).endsWith(":") ? 1 : 0)) : "");
    setPicked(null);
  };
  return (
    <div className="mt-3 rounded-lg border border-line">
      <div className="flex items-center gap-2 border-b border-line p-2">
        <IconButton label={t("files.up")} onClick={parent} disabled={path === ""}><ArrowUp size={15} /></IconButton>
        <span className="ltr min-w-0 flex-1 truncate text-[12px] text-muted" data-selectable>{path || t("files.roots")}</span>
        <Button size="sm" variant="primary" disabled={!picked} onClick={() => picked && void api.download(session.id, picked)}>{t("files.downloadSelected")}</Button>
      </div>
      <ul className="max-h-56 overflow-auto p-1">
        {dir?.error && <li className="p-2 text-[13px] text-danger">{dir.error}</li>}
        {dir && !dir.error && dir.entries.length === 0 && <li className="p-2 text-[13px] text-muted">{t("files.empty")}</li>}
        {dir?.entries.map((e) => {
          const full = join(e.name);
          return (
            <li key={e.name}>
              <button
                className={cx("flex w-full items-center gap-2 rounded px-2 py-1 text-start text-[13px] hover:bg-surface-2", picked === full && "bg-accent-soft")}
                onClick={() => setPicked(full)}
                onDoubleClick={() => { if (e.isDir) { setPath(full); setPicked(null); } }}
              >
                {e.isDir ? <Folder size={14} className="shrink-0 text-accent" /> : <FileIcon size={14} className="shrink-0 text-muted" />}
                <span className="min-w-0 flex-1 truncate" dir="auto">{e.name}</span>
                {!e.isDir && <span className="tabular text-[11px] text-muted">{formatBytes(e.size, i18n.language)}</span>}
              </button>
            </li>
          );
        })}
      </ul>
    </div>
  );
}

export function FilesPanel({ session }: { session: SessionState }) {
  const { t } = useTranslation();
  const allowed = session.permissions?.files ?? false;
  const [over, setOver] = useState(false);
  const transfers = Object.values(session.transfers).sort((a, b) => b.id - a.id);

  useEffect(() => {
    if (!allowed) return;
    let un: (() => void) | undefined;
    void getCurrentWebview()
      .onDragDropEvent((e) => {
        if (e.payload.type === "over") setOver(true);
        else if (e.payload.type === "drop") {
          setOver(false);
          void api.sendFiles(session.id, e.payload.paths);
        } else setOver(false);
      })
      .then((u) => (un = u));
    return () => un?.();
  }, [allowed, session.id]);

  if (!allowed) return <p className="p-4 text-muted">{t("files.disabled")}</p>;
  return (
    <div className="flex h-full min-h-0 flex-col overflow-auto p-4">
      <h3 className="mb-3 font-semibold">{t("files.title")}</h3>
      <div className={cx("mb-3 rounded-xl border-2 border-dashed p-4 text-center text-[13px] transition", over ? "border-accent bg-accent-soft text-accent" : "border-line text-muted")}>
        {t("files.dropHere")}
        <div className="mt-2 flex justify-center gap-2">
          <Button size="sm" onClick={() => void openDialog({ multiple: true }).then((p) => { if (Array.isArray(p) && p.length) void api.sendFiles(session.id, p); })}>{t("files.upload")}</Button>
          <Button size="sm" onClick={() => void openDialog({ directory: true }).then((p) => { if (typeof p === "string") void api.sendFiles(session.id, [p]); })}>{t("files.sendFolder")}</Button>
        </div>
      </div>
      <h4 className="mb-1 text-[13px] font-semibold text-muted">{t("files.browse")}</h4>
      <RemoteBrowser session={session} />
      <ul className="mt-4 space-y-2">
        {transfers.length === 0 && <li className="text-[13px] text-muted">{t("files.noTransfers")}</li>}
        {transfers.map((tr) => (
          <TransferRow key={tr.id} tr={tr} onAction={(a) => void api.fileAction(session.id, a, tr.id)} />
        ))}
      </ul>
    </div>
  );
}

export { TransferRow, formatMbps };
