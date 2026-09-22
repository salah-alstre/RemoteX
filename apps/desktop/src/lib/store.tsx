import { isPermissionGranted, sendNotification } from "@tauri-apps/plugin-notification";
import { check } from "@tauri-apps/plugin-updater";
import { createContext, useContext, useEffect, useMemo, useReducer, useRef, type ReactNode } from "react";
import { useTranslation } from "react-i18next";
import { applyLanguage, resolveLanguage } from "../i18n";
import { VideoPipeline } from "../session/pipeline";
import { api, onAppEvent, onEngineEvent } from "./api";
import { mergeBootLive } from "./boot";
import type {
  Bootstrap,
  CursorUpdateView,
  DeviceBook,
  DirEntry,
  DisplayInfo,
  EngineEvent,
  IncomingRequest,
  LinkInfo,
  Permissions,
  ServerState,
  Settings,
  TransferInfo,
  ViewerReady,
  ViewerState,
  ViewerStats,
} from "./types";

export interface ChatMsg {
  id: number;
  text: string;
  tsMs: number;
  mine: boolean;
}

export interface SessionState {
  id: number;
  target: string;
  state: ViewerState;
  everConnected: boolean;
  ready: ViewerReady | null;
  stats: ViewerStats | null;
  displays: DisplayInfo[];
  active: number;
  permissions: Permissions | null;
  chat: ChatMsg[];
  unreadChat: number;
  transfers: Record<number, TransferInfo>;
  dir: { path: string; entries: DirEntry[]; error: string | null } | null;
  endedReason: "peerClosed" | null;
}

export interface HostSessionState {
  viewerId: string;
  viewerName: string;
  permissions: Permissions;
  link: LinkInfo;
  chat: ChatMsg[];
  transfers: Record<number, TransferInfo>;
  fileRequests: { id: number; name: string; size: number }[];
}

export type Route = "home" | "devices" | "settings" | "session";

interface AppState {
  ready: boolean;
  boot: Bootstrap | null;
  id: string | null;
  password: string;
  passwordTtl: number | null;
  server: ServerState;
  settings: Settings | null;
  book: DeviceBook;
  presence: Record<string, boolean>;
  incoming: IncomingRequest[];
  host: HostSessionState | null;
  session: SessionState | null;
  route: Route;
  settingsSection: string;
  connectError: string | null;
  toasts: { key: number; message: string; tone: "info" | "error" }[];
}

const initial: AppState = {
  ready: false,
  boot: null,
  id: null,
  password: "",
  passwordTtl: null,
  server: { state: "starting" },
  settings: null,
  book: { trusted: [], entries: [], recents: [] },
  presence: {},
  incoming: [],
  host: null,
  session: null,
  route: "home",
  settingsSection: "general",
  connectError: null,
  toasts: [],
};

type Action =
  | { t: "boot"; boot: Bootstrap }
  | { t: "event"; e: EngineEvent }
  | { t: "settings"; settings: Settings }
  | { t: "book"; patch: Partial<DeviceBook> }
  | { t: "password"; password: string }
  | { t: "route"; route: Route; section?: string }
  | { t: "startSession"; id: number; target: string }
  | { t: "clearSession" }
  | { t: "connectError"; key: string | null }
  | { t: "toast"; message: string; tone: "info" | "error" }
  | { t: "dropToast"; key: number }
  | { t: "unreadReset" }
  | { t: "clearChat" };

let toastKey = 0;

function mapTransfer(map: Record<number, TransferInfo>, info: TransferInfo) {
  return { ...map, [info.id]: info };
}

function reduceEvent(s: AppState, e: EngineEvent): AppState {
  switch (e.type) {
    case "server":
      return { ...s, server: e.data };
    case "identity": {
      const d = e.data.id.replace(/\D/g, "");
      return { ...s, id: `${d.slice(0, 3)} ${d.slice(3, 6)} ${d.slice(6, 9)}` };
    }
    case "tempPassword":
      return { ...s, password: e.data.password, passwordTtl: e.data.expiresInSecs };
    case "incoming":
      return { ...s, incoming: [...s.incoming.filter((r) => r.requestId !== e.data.requestId), e.data] };
    case "incomingCancelled":
      return { ...s, incoming: s.incoming.filter((r) => r.requestId !== e.data.requestId) };
    case "hostStarted":
      return { ...s, host: { ...e.data, chat: [], transfers: {}, fileRequests: [] } };
    case "hostPermissions":
      return s.host ? { ...s, host: { ...s.host, permissions: e.data } } : s;
    case "hostEnded":
      return { ...s, host: null };
    case "presence":
      return { ...s, presence: { ...s.presence, ...Object.fromEntries(e.data.statuses) } };
    case "chat": {
      const msg = { id: e.data.id, text: e.data.text, tsMs: e.data.tsMs, mine: e.data.mine };
      if (s.session && s.session.id === e.data.session) {
        return { ...s, session: { ...s.session, chat: [...s.session.chat, msg], unreadChat: e.data.mine ? s.session.unreadChat : s.session.unreadChat + 1 } };
      }
      return s.host ? { ...s, host: { ...s.host, chat: [...s.host.chat, msg] } } : s;
    }
    case "transfer": {
      if (s.session && s.session.id === e.data.session) return { ...s, session: { ...s.session, transfers: mapTransfer(s.session.transfers, e.data.info) } };
      return s.host ? { ...s, host: { ...s.host, transfers: mapTransfer(s.host.transfers, e.data.info) } } : s;
    }
    case "fileRequest":
      return s.host ? { ...s, host: { ...s.host, fileRequests: [...s.host.fileRequests, { id: e.data.id, name: e.data.name, size: e.data.size }] } } : s;
    case "dirListing":
      return s.session ? { ...s, session: { ...s.session, dir: { path: e.data.path, entries: e.data.entries, error: e.data.error } } } : s;
    case "viewer": {
      if (!s.session || s.session.id !== e.data.session) return s;
      const st = e.data.state;
      const ever = s.session.everConnected || st.state === "connected";
      const session = { ...s.session, state: st, everConnected: ever };
      if (st.state === "failed" && !ever) return { ...s, session: null, route: "home", connectError: st.reason };
      if (st.state === "closed") return { ...s, session: null, route: "home" };
      return { ...s, session };
    }
    case "viewerReady":
      return s.session && s.session.id === e.data.session
        ? { ...s, session: { ...s.session, ready: e.data.info, displays: e.data.info.displays, active: e.data.info.activeDisplay, permissions: e.data.info.permissions } }
        : s;
    case "viewerDisplays":
      return s.session && s.session.id === e.data.session ? { ...s, session: { ...s.session, displays: e.data.displays, active: e.data.active } } : s;
    case "viewerPermissions":
      return s.session && s.session.id === e.data.session ? { ...s, session: { ...s.session, permissions: e.data.permissions } } : s;
    case "viewerStats":
      return s.session && s.session.id === e.data.session ? { ...s, session: { ...s.session, stats: e.data.stats } } : s;
    default:
      return s;
  }
}

function reducer(s: AppState, a: Action): AppState {
  switch (a.t) {
    case "boot":
      return {
        ...s,
        ready: true,
        boot: a.boot,
        ...mergeBootLive(s, a.boot),
        settings: a.boot.settings,
        book: a.boot.book,
      };
    case "event":
      return reduceEvent(s, a.e);
    case "settings":
      return { ...s, settings: a.settings };
    case "book":
      return { ...s, book: { ...s.book, ...a.patch } };
    case "password":
      return { ...s, password: a.password };
    case "route":
      return { ...s, route: a.route, settingsSection: a.section ?? s.settingsSection };
    case "startSession":
      return {
        ...s,
        route: "session",
        connectError: null,
        session: {
          id: a.id, target: a.target, state: { state: "connecting" }, everConnected: false, ready: null, stats: null, displays: [], active: 0,
          permissions: null, chat: [], unreadChat: 0, transfers: {}, dir: null, endedReason: null,
        },
      };
    case "clearSession":
      return { ...s, session: null, route: "home" };
    case "connectError":
      return { ...s, connectError: a.key };
    case "toast":
      // Identical messages are shown once until dismissed, so a flapping condition cannot flood the screen.
      if (s.toasts.some((t) => t.message === a.message)) return s;
      return { ...s, toasts: [...s.toasts.slice(-3), { key: ++toastKey, message: a.message, tone: a.tone }] };
    case "dropToast":
      return { ...s, toasts: s.toasts.filter((t) => t.key !== a.key) };
    case "unreadReset":
      return s.session ? { ...s, session: { ...s.session, unreadChat: 0 } } : s;
    case "clearChat":
      return { ...s, session: s.session ? { ...s.session, chat: [], unreadChat: 0 } : null, host: s.host ? { ...s.host, chat: [] } : null };
  }
}

export const cursorBus = new EventTarget();
export const emitCursor = (session: number, update: CursorUpdateView) => cursorBus.dispatchEvent(new CustomEvent("cursor", { detail: { session, update } }));

interface Actions {
  connect: (target: string, password: string, unattended: boolean, elevated?: boolean) => Promise<void>;
  disconnect: () => Promise<void>;
  reconnect: () => void;
  saveSettings: (patch: Partial<Settings>) => Promise<void>;
  regeneratePassword: () => Promise<void>;
  respondIncoming: (r: IncomingRequest, permissions: Permissions | null, trust: boolean) => Promise<void>;
  navigate: (route: Route, section?: string) => void;
  toast: (message: string, tone?: "info" | "error") => void;
  dismissToast: (key: number) => void;
  clearUnread: () => void;
  clearChat: () => void;
  setBook: (patch: Partial<DeviceBook>) => void;
  pipeline: VideoPipeline;
}

const Ctx = createContext<{ state: AppState; act: Actions } | null>(null);

export function useApp() {
  const v = useContext(Ctx);
  if (!v) throw new Error("useApp outside provider");
  return v;
}

async function notify(title: string, body: string) {
  try {
    if (document.hasFocus()) return;
    if (await isPermissionGranted()) sendNotification({ title, body });
  } catch {
    /* notifications are best-effort */
  }
}

export function AppProvider({ children }: { children: ReactNode }) {
  const { t } = useTranslation();
  const [state, dispatch] = useReducer(reducer, initial);
  const stateRef = useRef(state);
  stateRef.current = state;
  const tRef = useRef(t);
  tRef.current = t;

  const pipeline = useMemo(
    () =>
      new VideoPipeline({
        needKeyframe: () => {
          const s = stateRef.current.session;
          if (s) void api.keyframe(s.id);
        },
        unsupported: () => {
          const s = stateRef.current.session;
          dispatch({ t: "toast", message: tRef.current("session.decoderUnsupported"), tone: "error" });
          if (s) void api.setQuality(s.id, { codec: "h264" });
        },
        ack: (n) => void api.ackVideo(n),
      }),
    [],
  );

  useEffect(() => {
    void api.subscribeVideo((buf) => void pipeline.onPacket(buf));
    const unsubs: Promise<() => void>[] = [
      onEngineEvent((e) => {
        if (e.type === "cursor") return emitCursor(e.data.session, e.data.update);
        dispatch({ t: "event", e });
        const tr = tRef.current;
        if (e.type === "incoming") void notify(tr("notify.incomingTitle"), tr("notify.incomingBody", { name: e.data.viewerName }));
        if (e.type === "hostStarted") void notify(tr("notify.acceptedTitle"), tr("notify.acceptedBody", { name: e.data.viewerName }));
        if (e.type === "hostEnded") void notify(tr("notify.disconnectedTitle"), tr("notify.disconnectedBody"));
        if (e.type === "hostNotice" && e.data.notice.notice === "declined") void notify(tr("notify.rejectedTitle"), tr("notify.rejectedBody", { name: e.data.notice.viewerId }));
        if (e.type === "transfer" && e.data.info.status === "completed" && !e.data.info.upload) {
          void notify(tr("notify.fileTitle"), tr("files.received", { name: e.data.info.name }));
          if (stateRef.current.settings?.openDirAfterTransfer) void api.openFolder("downloads");
        }
        if (e.type === "hostNotice" && e.data.notice.notice === "elevationUnavailable") dispatch({ t: "toast", message: tr("elevated.unavailableHost"), tone: "error" });
        if (e.type === "clipboard") dispatch({ t: "toast", message: tr("session.clipboardReceived"), tone: "info" });
        if (e.type === "security") dispatch({ t: "toast", message: e.data.message, tone: "error" });
        if (e.type === "viewer" && e.data.state.state === "closed") dispatch({ t: "toast", message: tr("session.endedByPeer"), tone: "info" });
      }),
      onAppEvent("navigate", () => dispatch({ t: "route", route: "settings" })),
      onAppEvent("settings-changed", () => void api.bootstrap().then((boot) => dispatch({ t: "settings", settings: boot.settings }))),
    ];
    // Listen first, then take the snapshot, so no engine event can fall between the two.
    void Promise.all(unsubs)
      .then(() => api.bootstrap())
      .then((boot) => dispatch({ t: "boot", boot }));
    return () => unsubs.forEach((p) => void p.then((u) => u()));
  }, [pipeline]);

  // Safety net: if the service reports online but the id never reached the UI, ask the engine directly.
  useEffect(() => {
    if (state.server.state !== "online" || state.id) return;
    const t = window.setTimeout(() => void api.bootstrap().then((boot) => dispatch({ t: "boot", boot })), 500);
    return () => window.clearTimeout(t);
  }, [state.server.state, state.id]);

  // Presence for saved and recent devices.
  useEffect(() => {
    if (state.server.state !== "online") return;
    const ids = [...new Set([...state.book.entries.map((e) => e.id), ...state.book.recents.map((r) => r.id)])].slice(0, 64);
    if (ids.length === 0) return;
    void api.queryPresence(ids);
    const timer = window.setInterval(() => void api.queryPresence(ids), 30_000);
    return () => window.clearInterval(timer);
  }, [state.server.state, state.book.entries, state.book.recents]);

  // Automatic update check once per launch, when enabled and the server is reachable.
  const checkUpdates = state.settings?.checkUpdates;
  const serverOnline = state.server.state === "online";
  const checked = useRef(false);
  useEffect(() => {
    if (!checkUpdates || !serverOnline || checked.current) return;
    checked.current = true;
    void check()
      .then((update) => {
        if (!update) return;
        dispatch({ t: "toast", message: tRef.current("about.updateAvailable", { version: update.version }), tone: "info" });
        void notify(tRef.current("notify.updateTitle"), tRef.current("notify.updateBody", { version: update.version }));
      })
      .catch(() => {
        /* offline or update endpoint not configured: stay silent, the About page reports failures on demand */
      });
  }, [checkUpdates, serverOnline]);

  // Theme, language, direction, scale, performance mode.
  const s = state.settings;
  const osLocale = state.boot?.osLanguage ?? "en";
  useEffect(() => {
    if (!s) return;
    applyLanguage(resolveLanguage(s.language, osLocale));
    const dark = window.matchMedia("(prefers-color-scheme: dark)");
    const apply = () => (document.documentElement.dataset.theme = s.theme === "system" ? (dark.matches ? "dark" : "light") : s.theme);
    apply();
    document.documentElement.dataset.perf = String(s.performanceMode);
    document.documentElement.style.fontSize = `${(14 * s.uiScale) / 100}px`;
    dark.addEventListener("change", apply);
    return () => dark.removeEventListener("change", apply);
  }, [s, osLocale]);

  const act = useMemo<Actions>(
    () => ({
      connect: async (target, password, unattended, elevated = false) => {
        dispatch({ t: "connectError", key: null });
        try {
          const id = await api.connect(target, password, unattended, elevated);
          dispatch({ t: "startSession", id, target });
          const boot = await api.bootstrap();
          dispatch({ t: "book", patch: boot.book });
        } catch (e) {
          dispatch({ t: "connectError", key: typeof e === "string" ? lowerFirst(e) : "protocol" });
        }
      },
      disconnect: async () => {
        const sess = stateRef.current.session;
        if (sess) await api.disconnect(sess.id);
        dispatch({ t: "clearSession" });
      },
      reconnect: () => {
        const sess = stateRef.current.session;
        if (sess) void api.keyframe(sess.id);
      },
      saveSettings: async (patch) => {
        const cur = stateRef.current.settings;
        if (!cur) return;
        const saved = await api.saveSettings({ ...cur, ...patch });
        dispatch({ t: "settings", settings: saved });
      },
      regeneratePassword: async () => dispatch({ t: "password", password: await api.regeneratePassword() }),
      respondIncoming: async (r, permissions, trust) => {
        await api.respondIncoming(r.requestId, permissions, trust, r.viewerId, r.viewerName);
        if (trust && permissions) dispatch({ t: "book", patch: { trusted: (await api.bootstrap()).book.trusted } });
      },
      navigate: (route, section) => dispatch({ t: "route", route, section }),
      toast: (message, tone = "info") => dispatch({ t: "toast", message, tone }),
      dismissToast: (key) => dispatch({ t: "dropToast", key }),
      clearUnread: () => dispatch({ t: "unreadReset" }),
      clearChat: () => dispatch({ t: "clearChat" }),
      setBook: (patch) => dispatch({ t: "book", patch }),
      pipeline,
    }),
    [pipeline],
  );

  return <Ctx.Provider value={{ state, act }}>{children}</Ctx.Provider>;
}

function lowerFirst(s: string): string {
  return s.charAt(0).toLowerCase() + s.slice(1);
}
