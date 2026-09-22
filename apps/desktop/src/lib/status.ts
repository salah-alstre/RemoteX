import type { ServerState } from "./types";

export type Tone = "ok" | "warn" | "danger" | "muted";
export type StatusIcon = "online" | "busy" | "offline";

export interface ServiceStatus {
  tone: Tone;
  icon: StatusIcon;
  /** i18n key under `status.` for the headline. */
  key: "starting" | "connecting" | "ready" | "incomingOff" | "reconnecting" | "noInternet" | "serviceUnavailable" | "busy";
  /** i18n key for a plain "Connected" style label (used where incoming state is irrelevant). */
  short: "starting" | "connecting" | "connected" | "reconnecting" | "noInternet" | "serviceUnavailable" | "busy";
  online: boolean;
}

/**
 * Maps the real backend connection state to what the user sees. Status is conveyed by an icon and a
 * text label as well as colour, and never mentions the service address or any technical detail.
 */
export function describeService(server: ServerState, incomingEnabled: boolean): ServiceStatus {
  switch (server.state) {
    case "online":
      return incomingEnabled
        ? { tone: "ok", icon: "online", key: "ready", short: "connected", online: true }
        : { tone: "warn", icon: "online", key: "incomingOff", short: "connected", online: true };
    case "starting":
      return { tone: "warn", icon: "busy", key: "starting", short: "starting", online: false };
    case "connecting":
      return { tone: "warn", icon: "busy", key: "connecting", short: "connecting", online: false };
    case "reconnecting":
      return { tone: "warn", icon: "busy", key: "reconnecting", short: "reconnecting", online: false };
    case "noInternet":
      return { tone: "danger", icon: "offline", key: "noInternet", short: "noInternet", online: false };
    case "serviceUnavailable":
      return { tone: "danger", icon: "offline", key: "serviceUnavailable", short: "serviceUnavailable", online: false };
    case "rejected":
      return { tone: "warn", icon: "busy", key: "busy", short: "busy", online: false };
  }
}
