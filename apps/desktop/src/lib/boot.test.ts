import { describe, expect, it } from "vitest";
import { mergeBootLive } from "./boot";

describe("mergeBootLive", () => {
  it("keeps an id delivered by an event when an older snapshot has none", () => {
    const live = { id: "725 719 732", password: "ABC123", server: { state: "online" } as const };
    const merged = mergeBootLive(live, { id: null, password: "", serverOnline: false });
    expect(merged.id).toBe("725 719 732");
    expect(merged.password).toBe("ABC123");
    expect(merged.server.state).toBe("online");
  });

  it("fills the gaps from the snapshot when no event has arrived yet", () => {
    const merged = mergeBootLive(
      { id: null, password: "", server: { state: "starting" } },
      { id: "111 222 333", password: "XYZ789", serverOnline: true },
    );
    expect(merged).toEqual({ id: "111 222 333", password: "XYZ789", server: { state: "online" } });
  });

  it("does not claim online from the snapshot when the service is not online", () => {
    const merged = mergeBootLive(
      { id: null, password: "", server: { state: "connecting" } },
      { id: null, password: "P", serverOnline: false },
    );
    expect(merged.server.state).toBe("connecting");
  });
});
