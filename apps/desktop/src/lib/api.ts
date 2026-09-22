import { Channel, invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import { encodeInput } from "./inputPacket";
import type { AddressEntry, Bootstrap, DeviceBook, EngineEvent, Permissions, Settings, TrustedDevice } from "./types";

export type UiInput =
  | { type: "mouseMove"; x: number; y: number }
  | { type: "mouseButton"; button: number; down: boolean }
  | { type: "wheel"; dx: number; dy: number }
  | { type: "key"; scancode: number; extended: boolean; down: boolean }
  | { type: "text"; text: string }
  | { type: "releaseAll" };

export const api = {
  bootstrap: () => invoke<Bootstrap>("get_bootstrap"),
  saveSettings: (settings: Settings) => invoke<Settings>("settings_save", { settings }),
  regeneratePassword: () => invoke<string>("regenerate_password"),
  setUnattendedPassword: (password: string) => invoke<void>("unattended_set_password", { password }),
  clearUnattendedPassword: () => invoke<void>("unattended_clear_password"),

  connect: (target: string, password: string, unattended: boolean, elevated: boolean) =>
    invoke<number>("connect", { args: { target, password, unattended, elevated } }),
  elevationAvailable: () => invoke<boolean>("elevation_available"),
  /** High-rate path: a raw binary body, no JSON. */
  input: (session: number, event: UiInput) => invoke<void>("viewer_input_bin", encodeInput(session, event)),
  selectDisplay: (session: number, index: number) => invoke<void>("viewer_select_display", { session, index }),
  setQuality: (session: number, overrides: Partial<Settings>) => invoke<void>("viewer_set_quality", { session, overrides }),
  keyframe: (session: number) => invoke<void>("viewer_keyframe", { session }),
  clipboardSync: (session: number, on: boolean) => invoke<void>("viewer_clipboard_sync", { session, on }),
  chat: (session: number, text: string) => invoke<void>("viewer_chat", { session, text }),
  sendFiles: (session: number, paths: string[]) => invoke<void>("viewer_send_files", { session, paths }),
  download: (session: number, path: string) => invoke<void>("viewer_download", { session, path }),
  listDir: (session: number, path: string) => invoke<void>("viewer_list_dir", { session, path }),
  fileAction: (session: number, action: "pause" | "resume" | "cancel" | "retry", id: number) => invoke<void>("viewer_file", { session, action, id }),
  disconnect: (session: number) => invoke<void>("viewer_disconnect", { session }),

  respondIncoming: (requestId: number, permissions: Permissions | null, trust: boolean, viewerId: string, viewerName: string) =>
    invoke<void>("respond_incoming", { decision: { requestId, permissions, trust, viewerId, viewerName } }),
  hostSetPermissions: (permissions: Permissions) => invoke<void>("host_set_permissions", { permissions }),
  hostChat: (text: string) => invoke<void>("host_chat", { text }),
  hostKick: () => invoke<void>("host_kick"),
  hostApproveFile: (id: number, accept: boolean) => invoke<void>("host_approve_file", { id, accept }),

  trust: (id: string, name: string) => invoke<TrustedDevice[]>("book_trust", { id, name }),
  revoke: (id: string) => invoke<TrustedDevice[]>("book_revoke", { id }),
  revokeAll: () => invoke<TrustedDevice[]>("book_revoke_all"),
  renameTrusted: (id: string, name: string) => invoke<TrustedDevice[]>("book_rename_trusted", { id, name }),
  upsertEntry: (entry: Pick<AddressEntry, "id" | "name" | "favorite" | "group">) => invoke<AddressEntry[]>("book_upsert", { entry }),
  removeEntry: (id: string) => invoke<AddressEntry[]>("book_remove", { id }),
  queryPresence: (ids: string[]) => invoke<void>("presence_query", { ids }),

  openFolder: (kind: "logs" | "downloads" | "data") => invoke<void>("open_folder", { kind }),
  securityLog: (limit: number) => invoke<string[]>("read_security_log", { limit }),

  ackVideo: (count: number) => invoke<void>("video_ack", { count }),
  subscribeVideo: (onPacket: (data: ArrayBuffer) => void) => {
    const channel = new Channel<ArrayBuffer>();
    channel.onmessage = onPacket;
    return invoke<void>("video_subscribe", { channel });
  },
};

export type { DeviceBook };

export function onEngineEvent(handler: (e: EngineEvent) => void): Promise<UnlistenFn> {
  return listen<EngineEvent>("engine-event", (e) => handler(e.payload));
}

export function onAppEvent(name: string, handler: () => void): Promise<UnlistenFn> {
  return listen(name, handler);
}
