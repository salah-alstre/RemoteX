import type { ServerState } from "./types";

interface Live {
  id: string | null;
  password: string;
  server: ServerState;
}

interface BootLive {
  id: string | null;
  password: string;
  serverOnline: boolean;
}

/**
 * Merges the bootstrap snapshot into live state. The snapshot is requested at start-up and can arrive
 * *after* engine events that are newer than it (a fast registration finishes while the snapshot is
 * still being assembled). Values already delivered by events therefore win; the snapshot only fills gaps.
 */
export function mergeBootLive(s: Live, boot: BootLive): Live {
  return {
    id: s.id ?? boot.id,
    password: s.password || boot.password,
    server: boot.serverOnline && s.server.state !== "online" ? { state: "online" } : s.server,
  };
}
