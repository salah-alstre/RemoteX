export interface Permissions {
  view: boolean;
  mouse: boolean;
  keyboard: boolean;
  clipboard: boolean;
  files: boolean;
  /** Elevated (administrator) control through the RemoteX service. Never implied by the others. */
  elevated: boolean;
}

export const ALL_PERMISSIONS: Permissions = { view: true, mouse: true, keyboard: true, clipboard: true, files: true, elevated: false };

export type PermissionKey = keyof Permissions;

export interface Settings {
  language: "system" | "en" | "ar";
  theme: "system" | "light" | "dark";
  startWithWindows: boolean;
  minimizeToTray: boolean;
  launchMinimized: boolean;
  closeBehavior: "tray" | "exit";
  checkUpdates: boolean;
  firstRunDone: boolean;
  deviceName: string;
  preset: "auto" | "best" | "balanced" | "lowLatency" | "lowBandwidth";
  codec: "auto" | "h264" | "h265";
  fps: 0 | 15 | 30 | 60;
  bitrateKbps: number;
  resolution: string;
  hardwareAcceleration: boolean;
  preferDirect: boolean;
  relayFallback: boolean;
  adaptiveQuality: boolean;
  lowBandwidth: boolean;
  performanceMode: boolean;
  showRemoteCursor: boolean;
  cursorPrediction: "auto" | "on" | "off";
  scaleMode: "fit" | "original" | "stretch" | "scale";
  incomingEnabled: boolean;
  tempPasswordExpiryMins: number;
  rotateAfterSession: boolean;
  defaultPermissions: Permissions;
  clipboardSync: boolean;
  clipboardImages: boolean;
  unattendedEnabled: boolean;
  unattendedAnyDevice: boolean;
  /** Unattended sessions may use elevated control. Off unless the owner turns it on. */
  unattendedElevated: boolean;
  downloadDir: string;
  askBeforeReceiving: boolean;
  maxTransfers: number;
  openDirAfterTransfer: boolean;
  uiScale: number;
}

export interface TrustedDevice {
  id: string;
  name: string;
  addedMs: number;
  lastConnectionMs: number | null;
}

export interface AddressEntry {
  id: string;
  name: string;
  favorite: boolean;
  group: "my" | "other";
  addedMs: number;
  lastConnectedMs: number | null;
}

export interface Recent {
  id: string;
  lastMs: number;
}

export interface DeviceBook {
  trusted: TrustedDevice[];
  entries: AddressEntry[];
  recents: Recent[];
}

export interface Bootstrap {
  id: string | null;
  password: string;
  settings: Settings;
  book: DeviceBook;
  version: string;
  build: string;
  brand: { name: string; company: string; website: string; github: string; supportEmail: string };
  encoders: { codec: string; name: string; hardware: boolean }[];
  serverOnline: boolean;
  computerName: string;
  downloadsDir: string;
  dataDir: string;
  unattendedPasswordSet: boolean;
  osLanguage: string;
}

export interface DisplayInfo {
  index: number;
  name: string;
  width: number;
  height: number;
  x: number;
  y: number;
  primary: boolean;
}

export type ServerState =
  | { state: "starting" }
  | { state: "connecting" }
  | { state: "online" }
  | { state: "reconnecting"; attempt: number }
  | { state: "noInternet" }
  | { state: "serviceUnavailable" }
  | { state: "rejected" };

export type FailReason =
  | "offline"
  | "notAccepting"
  | "rateLimited"
  | "banned"
  | "busy"
  | "invalidId"
  | "serverUnavailable"
  | "wrongPassword"
  | "lockedOut"
  | "declined"
  | "timeout"
  | "unattendedDisabled"
  | "unsupported"
  | "connectionLost"
  | "protocol";

export type ViewerState =
  | { state: "connecting" }
  | { state: "authenticating" }
  | { state: "waitingForApproval" }
  | { state: "connected" }
  | { state: "reconnecting"; attempt: number; nextInSecs: number }
  | { state: "failed"; reason: FailReason }
  | { state: "closed" };

export interface LinkInfo {
  kind: "direct" | "relay";
  peer: string;
  protocol: string;
}

export interface ViewerReady {
  hostName: string;
  displays: DisplayInfo[];
  permissions: Permissions;
  activeDisplay: number;
  link: LinkInfo;
}

export type NetQuality = "excellent" | "good" | "fair" | "poor";

export interface HostStatsView {
  fps: number;
  bitrateKbps: number;
  captureMs: number;
  encodeMs: number;
  dropped: number;
  encoder: string;
  codec: "H264" | "H265" | "Av1" | null;
  width: number;
  height: number;
  inputQueueUs: number;
  inputApplyUs: number;
  staleMovesDropped: number;
}

export interface ViewerStats {
  rttMs: number;
  jitterMs: number;
  recvKbps: number;
  framesPerSec: number;
  droppedPercent: number;
  quality: NetQuality;
  host: HostStatsView;
  latency: LatencyView;
}

/** Measured responsiveness of the control loop; 0 means not measured yet. */
export interface LatencyView {
  inputRttMs: number;
  inputQueueMs: number;
  hostQueueMs: number;
  hostApplyMs: number;
  motionToFrameMs: number;
  staleMovesDropped: number;
  staleVideoDropped: number;
  fileRateKbps: number;
}

export type TransferStatus = "pending" | "active" | "paused" | "completed" | "failed" | "cancelled";

export interface TransferInfo {
  id: number;
  name: string;
  size: number;
  transferred: number;
  upload: boolean;
  status: TransferStatus;
  speedBps: number;
  etaSecs: number | null;
  error: string | null;
}

export interface IncomingRequest {
  requestId: number;
  viewerId: string;
  viewerName: string;
  requested: Permissions;
  sameNetwork: boolean;
  timeoutSecs: number;
}

export interface DirEntry {
  name: string;
  isDir: boolean;
  size: number;
}

export interface CursorShapeView {
  id: number;
  width: number;
  height: number;
  hotX: number;
  hotY: number;
  rgbaBase64: string;
}

export interface CursorUpdateView {
  x: number;
  y: number;
  visible: boolean;
  shape: CursorShapeView | null;
}

export type EndReason = "peerClosed" | "localClosed" | "connectionLost" | "kicked";

export type HostNotice =
  | { notice: "lockedOut" }
  | { notice: "elevationUnavailable" }
  | { notice: "passwordRejected"; viewerId: string }
  | { notice: "fileReceived"; name: string }
  | { notice: "declined"; viewerId: string };

export type EngineEvent =
  | { type: "server"; data: ServerState }
  | { type: "identity"; data: { id: string } }
  | { type: "tempPassword"; data: { password: string; expiresInSecs: number | null } }
  | { type: "incoming"; data: IncomingRequest }
  | { type: "incomingCancelled"; data: { requestId: number } }
  | { type: "hostStarted"; data: { viewerId: string; viewerName: string; permissions: Permissions; link: LinkInfo } }
  | { type: "hostPermissions"; data: Permissions }
  | { type: "hostEnded"; data: { reason: EndReason } }
  | { type: "hostNotice"; data: { notice: HostNotice } }
  | { type: "viewer"; data: { session: number; state: ViewerState } }
  | { type: "viewerReady"; data: { session: number; info: ViewerReady } }
  | { type: "viewerDisplays"; data: { session: number; displays: DisplayInfo[]; active: number } }
  | { type: "viewerPermissions"; data: { session: number; permissions: Permissions } }
  | { type: "viewerStats"; data: { session: number; stats: ViewerStats } }
  | { type: "cursor"; data: { session: number; update: CursorUpdateView } }
  | { type: "clipboard"; data: { session: number; text: string | null; image: boolean } }
  | { type: "chat"; data: { session: number; id: number; text: string; tsMs: number; mine: boolean } }
  | { type: "transfer"; data: { session: number; info: TransferInfo } }
  | { type: "fileRequest"; data: { session: number; id: number; name: string; size: number } }
  | { type: "dirListing"; data: { session: number; path: string; entries: DirEntry[]; error: string | null } }
  | { type: "presence"; data: { statuses: [string, boolean][] } }
  | { type: "security"; data: { message: string } };
