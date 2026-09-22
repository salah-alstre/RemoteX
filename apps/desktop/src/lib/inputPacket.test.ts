import { describe, expect, it } from "vitest";
import { encodeInput } from "./inputPacket";

const bytes = (u: Uint8Array) => Array.from(u);

describe("encodeInput", () => {
  it("packs a mouse move into 9 little-endian bytes", () => {
    expect(bytes(encodeInput(7, { type: "mouseMove", x: 0x1234, y: 0xffff }))).toEqual([7, 0, 0, 0, 0, 0x34, 0x12, 0xff, 0xff]);
  });
  it("packs buttons, wheel and keys", () => {
    expect(bytes(encodeInput(1, { type: "mouseButton", button: 2, down: true }))).toEqual([1, 0, 0, 0, 1, 2, 1]);
    expect(bytes(encodeInput(1, { type: "wheel", dx: -2, dy: 10 }))).toEqual([1, 0, 0, 0, 2, 0xfe, 0xff, 10, 0]);
    expect(bytes(encodeInput(1, { type: "key", scancode: 0x1e, extended: false, down: true }))).toEqual([1, 0, 0, 0, 3, 0x1e, 0, 0, 1]);
  });
  it("carries text as utf-8 and release-all with no payload", () => {
    const t = encodeInput(1, { type: "text", text: "م" });
    expect(bytes(t.subarray(5))).toEqual(Array.from(new TextEncoder().encode("م")));
    expect(bytes(encodeInput(1, { type: "releaseAll" }))).toEqual([1, 0, 0, 0, 5]);
  });
});
