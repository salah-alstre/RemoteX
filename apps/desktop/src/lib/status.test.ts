import { describe, expect, it } from "vitest";
import { describeService } from "./status";
import type { ServerState } from "./types";

const all: ServerState[] = [
  { state: "starting" },
  { state: "connecting" },
  { state: "online" },
  { state: "reconnecting", attempt: 2 },
  { state: "noInternet" },
  { state: "serviceUnavailable" },
  { state: "rejected" },
];

describe("service status", () => {
  it("is ready only when really online with incoming enabled", () => {
    expect(describeService({ state: "online" }, true)).toMatchObject({ key: "ready", tone: "ok", online: true });
    expect(describeService({ state: "online" }, false)).toMatchObject({ key: "incomingOff", tone: "warn", online: true });
  });

  it("uses distinct icons and labels, never colour alone", () => {
    const seen = new Set(all.map((s) => `${describeService(s, true).icon}:${describeService(s, true).key}`));
    expect(seen.size).toBeGreaterThanOrEqual(6);
    for (const s of all) expect(describeService(s, true).key).toBeTruthy();
  });

  it("distinguishes no internet from the service being unavailable", () => {
    expect(describeService({ state: "noInternet" }, true).key).toBe("noInternet");
    expect(describeService({ state: "serviceUnavailable" }, true).key).toBe("serviceUnavailable");
  });

  it("treats a blocked device like an unavailable service without exposing why", () => {
    expect(describeService({ state: "rejected" }, true).key).toBe("busy");
  });

  it("only allows connecting outward when online", () => {
    for (const s of all) expect(describeService(s, true).online).toBe(s.state === "online");
  });
});
