import { getCurrentWindow } from "@tauri-apps/api/window";
import { Activity, ClipboardCheck, ClipboardX, Eye, EyeOff, FileUp, Home, Maximize2, MessageSquare, Minimize2, Monitor, PowerOff, ShieldAlert, SlidersHorizontal } from "lucide-react";
import { useCallback, useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { Button, IconButton, cx } from "../components/ui";
import { api } from "../lib/api";
import { computeCanvasSize, type ScaleMode } from "../lib/layout";
import { useApp } from "../lib/store";
import CursorOverlay from "./CursorOverlay";
import { useRemoteInput } from "./input";
import { ChatPanel, FilesPanel, InfoPanel, QualityPanel } from "./panels";
import type { PipelineMetrics } from "./pipeline";

type Panel = "files" | "chat" | "info" | "quality" | null;

function Overlay({ children }: { children: React.ReactNode }) {
  return (
    <div className="anim-fade absolute inset-0 z-20 grid place-items-center bg-bg/85 p-6 backdrop-blur-sm">
      <div role="status" className="anim-pop flex max-w-md flex-col items-center gap-4 rounded-2xl border border-line bg-surface p-8 text-center shadow-card">{children}</div>
    </div>
  );
}

function Spinner() {
  return <span aria-hidden className="size-8 animate-spin rounded-full border-[3px] border-line border-t-accent" />;
}

export default function Viewer() {
  const { t } = useTranslation();
  const { state, act } = useApp();
  const session = state.session;
  const settings = state.settings;
  const canvas = useRef<HTMLCanvasElement>(null);
  const container = useRef<HTMLDivElement>(null);
  const [box, setBox] = useState({ w: 0, h: 0 });
  const [video, setVideo] = useState({ w: 0, h: 0 });
  const [mode, setMode] = useState<ScaleMode>(settings?.scaleMode ?? "fit");
  const [scalePct, setScalePct] = useState(100);
  const [panel, setPanel] = useState<Panel>(null);
  const [fullscreen, setFullscreen] = useState(false);
  const [revealed, setRevealed] = useState(false);
  const [cursorShown, setCursorShown] = useState(settings?.showRemoteCursor ?? true);
  const [clipboardOn, setClipboardOn] = useState(settings?.clipboardSync ?? true);
  const [monitorMenu, setMonitorMenu] = useState(false);
  const [viewMenu, setViewMenu] = useState(false);
  const [metrics, setMetrics] = useState<PipelineMetrics>(() => act.pipeline.getMetrics());
  const hideTimer = useRef<number | undefined>(undefined);

  const perms = session?.permissions;
  const connected = session?.state.state === "connected";
  const sessionId = session?.id ?? 0;

  // Attach the decoder to this canvas for the lifetime of the view.
  useEffect(() => {
    const p = act.pipeline;
    p.onSize = (w, h) => setVideo({ w, h });
    p.attach(canvas.current);
    return () => p.attach(null);
  }, [act.pipeline]);

  useEffect(() => {
    const id = window.setInterval(() => setMetrics({ ...act.pipeline.getMetrics() }), 1000);
    return () => window.clearInterval(id);
  }, [act.pipeline]);

  useEffect(() => {
    if (!container.current) return;
    const ro = new ResizeObserver(([e]) => e && setBox({ w: Math.floor(e.contentRect.width), h: Math.floor(e.contentRect.height) }));
    ro.observe(container.current);
    return () => ro.disconnect();
  }, []);

  useEffect(() => {
    void getCurrentWindow().setFullscreen(fullscreen);
    if (!fullscreen) setRevealed(false);
  }, [fullscreen]);
  useEffect(() => () => void getCurrentWindow().setFullscreen(false), []);

  useRemoteInput(canvas, { session: sessionId, mouse: Boolean(perms?.mouse), keyboard: Boolean(perms?.keyboard), active: connected });

  const unread = session?.unreadChat ?? 0;
  useEffect(() => {
    if (panel === "chat" && unread > 0) act.clearUnread();
  }, [panel, unread, act]);

  const reveal = useCallback(() => {
    setRevealed(true);
    window.clearTimeout(hideTimer.current);
    hideTimer.current = window.setTimeout(() => setRevealed(false), 2500);
  }, []);

  if (!session || !settings) return null;
  const display = session.displays[session.active];
  const size = computeCanvasSize(box, video, mode, scalePct);
  const toolbarHidden = fullscreen && !revealed;
  const status = session.state;
  const showCanvasCursor = !(cursorShown && perms?.view);
  // Prediction: always on/off when chosen; "auto" turns it on when the round trip is long enough to feel
  // (or in performance mode), because on a fast link the host's own report is already immediate.
  const pref = settings.cursorPrediction;
  const inputRtt = session.stats?.latency.inputRttMs || session.stats?.rttMs || 0;
  const predictCursor = pref === "on" || (pref === "auto" && (settings.performanceMode || inputRtt >= 30));

  const toggle = (p: Exclude<Panel, null>) => {
    setPanel((cur) => (cur === p ? null : p));
    setMonitorMenu(false);
    setViewMenu(false);
  };

  return (
    <div className="relative flex h-full flex-col bg-black" onMouseMove={fullscreen ? (e) => e.clientY < 12 && reveal() : undefined}>
      {fullscreen && <div className="absolute inset-x-0 top-0 z-30 h-2" onMouseEnter={reveal} aria-hidden />}

      <header
        className={cx(
          "z-30 flex items-center gap-1 border-b border-line bg-surface/95 px-2 py-1 shadow-card backdrop-blur transition-transform duration-200",
          fullscreen && "absolute inset-x-0 top-0",
          toolbarHidden && "-translate-y-full",
        )}
        onMouseEnter={fullscreen ? reveal : undefined}
        role="toolbar"
        aria-label={t("session.connectedTo", { name: session.ready?.hostName ?? "" })}
      >
        <IconButton label={t("toolbar.home")} onClick={() => act.navigate("home")}><Home size={18} /></IconButton>
        {perms?.elevated && (
          <span className="mx-1 inline-flex items-center gap-1 rounded-full bg-danger/15 px-2 py-0.5 text-[12px] font-semibold text-danger" role="status">
            <ShieldAlert size={13} aria-hidden />{t("elevated.indicator")}
          </span>
        )}

        <div className="relative">
          <IconButton label={t("toolbar.monitor")} active={monitorMenu} onClick={() => { setMonitorMenu((v) => !v); setViewMenu(false); }} disabled={session.displays.length === 0}>
            <Monitor size={18} />
          </IconButton>
          {monitorMenu && (
            <ul role="menu" className="anim-pop absolute start-0 top-full z-40 mt-1 min-w-56 rounded-xl border border-line bg-surface p-1 shadow-card">
              {session.displays.map((d) => (
                <li key={d.index}>
                  <button
                    role="menuitemradio"
                    aria-checked={d.index === session.active}
                    className={cx("flex w-full items-center justify-between gap-4 rounded-lg px-3 py-1.5 text-start hover:bg-surface-2", d.index === session.active && "bg-accent-soft text-accent")}
                    onClick={() => { void api.selectDisplay(session.id, d.index); setMonitorMenu(false); }}
                  >
                    <span>{d.name}</span>
                    <span className="ltr tabular text-[12px] text-muted">{d.width} × {d.height}</span>
                  </button>
                </li>
              ))}
            </ul>
          )}
        </div>

        <IconButton label={t("toolbar.quality")} active={panel === "quality"} onClick={() => toggle("quality")}><SlidersHorizontal size={18} /></IconButton>

        <div className="relative">
          <IconButton label={t("toolbar.view")} active={viewMenu} onClick={() => { setViewMenu((v) => !v); setMonitorMenu(false); }}><Eye size={18} /></IconButton>
          {viewMenu && (
            <div role="menu" className="anim-pop absolute start-0 top-full z-40 mt-1 w-56 rounded-xl border border-line bg-surface p-1 shadow-card">
              {(["fit", "original", "stretch", "scale"] as const).map((m) => (
                <button key={m} role="menuitemradio" aria-checked={mode === m} onClick={() => setMode(m)} className={cx("block w-full rounded-lg px-3 py-1.5 text-start hover:bg-surface-2", mode === m && "bg-accent-soft text-accent")}>
                  {t(`toolbar.${m}`)}
                </button>
              ))}
              {mode === "scale" && (
                <div className="px-3 py-2">
                  <input type="range" min={25} max={300} step={5} value={scalePct} onChange={(e) => setScalePct(Number(e.target.value))} aria-label={t("toolbar.scale")} className="w-full accent-[var(--accent)]" />
                  <div className="tabular text-center text-[12px] text-muted">{scalePct}%</div>
                </div>
              )}
              <hr className="my-1 border-line" />
              <button role="menuitemcheckbox" aria-checked={cursorShown} onClick={() => setCursorShown((v) => !v)} className="flex w-full items-center gap-2 rounded-lg px-3 py-1.5 text-start hover:bg-surface-2">
                {cursorShown ? <EyeOff size={15} /> : <Eye size={15} />}
                {cursorShown ? t("toolbar.hideCursor") : t("toolbar.showCursor")}
              </button>
            </div>
          )}
        </div>

        <IconButton label={fullscreen ? t("toolbar.exitFullscreen") : t("toolbar.fullscreen")} active={fullscreen} onClick={() => setFullscreen((v) => !v)}>
          {fullscreen ? <Minimize2 size={18} /> : <Maximize2 size={18} />}
        </IconButton>
        <IconButton
          label={t("toolbar.clipboard")}
          active={clipboardOn && Boolean(perms?.clipboard)}
          disabled={!perms?.clipboard}
          onClick={() => { setClipboardOn((v) => !v); void api.clipboardSync(session.id, !clipboardOn); }}
        >
          {clipboardOn && perms?.clipboard ? <ClipboardCheck size={18} /> : <ClipboardX size={18} />}
        </IconButton>
        <IconButton label={t("toolbar.files")} active={panel === "files"} onClick={() => toggle("files")} disabled={!perms?.files}><FileUp size={18} /></IconButton>
        <div className="relative">
          <IconButton label={t("toolbar.chat")} active={panel === "chat"} onClick={() => toggle("chat")}><MessageSquare size={18} /></IconButton>
          {session.unreadChat > 0 && panel !== "chat" && <span className="absolute -end-0.5 -top-0.5 size-2.5 rounded-full bg-danger" aria-label={String(session.unreadChat)} />}
        </div>
        <IconButton label={t("toolbar.info")} active={panel === "info"} onClick={() => toggle("info")}><Activity size={18} /></IconButton>

        <div className="ms-2 hidden min-w-0 flex-1 truncate text-[13px] text-muted md:block" dir="auto">
          {session.ready ? t("session.connectedTo", { name: session.ready.hostName }) : ""}
          {perms && !perms.mouse && !perms.keyboard && ` · ${t("session.viewOnly")}`}
        </div>
        <div className="flex-1 md:hidden" />
        <Button size="sm" variant="danger" icon={<PowerOff size={15} />} onClick={() => void act.disconnect()}>{t("toolbar.disconnect")}</Button>
      </header>

      <div className="flex min-h-0 flex-1">
        <div ref={container} className={cx("relative min-w-0 flex-1 overflow-auto", mode === "original" || mode === "scale" ? "" : "grid place-items-center overflow-hidden")}>
          <div className="relative m-auto" style={{ width: size.w || undefined, height: size.h || undefined }}>
            <canvas
              ref={canvas}
              aria-label={t("session.connectedTo", { name: session.ready?.hostName ?? "" })}
              className={cx("block size-full bg-black", showCanvasCursor ? "cursor-default" : "cursor-none")}
              style={{ width: size.w || undefined, height: size.h || undefined, touchAction: "none" }}
              tabIndex={0}
            />
            {cursorShown && perms?.view && display && size.w > 0 && (
              <CursorOverlay predict={predictCursor} session={session.id} displayWidth={display.width} displayHeight={display.height} cssWidth={size.w} cssHeight={size.h} />
            )}
          </div>
          {connected && video.w === 0 && <div className="absolute inset-0 grid place-items-center text-muted"><span>{t("session.noVideo")}</span></div>}
          {connected && perms && !perms.view && <Overlay><p>{t("session.viewOnly")}</p></Overlay>}

          {(status.state === "connecting" || status.state === "authenticating" || status.state === "waitingForApproval") && (
            <Overlay>
              <Spinner />
              <p className="font-medium">{t(status.state === "connecting" ? "session.connecting" : status.state === "authenticating" ? "session.authenticating" : "session.waiting")}</p>
              <Button onClick={() => void act.disconnect()}>{t("common.cancel")}</Button>
            </Overlay>
          )}
          {status.state === "reconnecting" && (
            <Overlay>
              <Spinner />
              <p className="font-semibold">{t("session.lost")}</p>
              <p className="text-muted">{t("session.reconnecting", { attempt: status.attempt, seconds: status.nextInSecs })}</p>
              <Button onClick={() => void act.disconnect()}>{t("session.closeSession")}</Button>
            </Overlay>
          )}
          {status.state === "failed" && (
            <Overlay>
              <p className="font-semibold">{t("session.lost")}</p>
              <p className="text-muted">{t("session.reconnectFailed")}</p>
              <div className="flex gap-2">
                <Button variant="primary" onClick={act.reconnect}>{t("common.reconnect")}</Button>
                <Button onClick={() => void act.disconnect()}>{t("session.closeSession")}</Button>
              </div>
            </Overlay>
          )}
        </div>

        {panel && (
          <aside className="anim-fade z-10 w-80 shrink-0 overflow-auto border-s border-line bg-surface" aria-label={t(`toolbar.${panel}`)}>
            {panel === "info" && <InfoPanel session={session} metrics={metrics} />}
            {panel === "quality" && <QualityPanel session={session} settings={settings} onApplied={() => setPanel(null)} />}
            {panel === "chat" && (
              <ChatPanel
                messages={session.chat}
                onSend={(text) => void api.chat(session.id, text)}
                onClear={act.clearChat}
              />
            )}
            {panel === "files" && <FilesPanel session={session} />}
          </aside>
        )}
      </div>
    </div>
  );
}
