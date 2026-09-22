import { describe, expect, it } from "vitest";
import { mappedKeyCount, scanFor } from "./keymap";

describe("keymap", () => {
  it("maps physical keys independent of layout", () => {
    // KeyA is the same physical key on English and Arabic layouts; the host decides 'a' or 'ش'.
    expect(scanFor("KeyA")).toEqual([0x1e, false]);
    expect(scanFor("KeyQ")).toEqual([0x10, false]);
    expect(scanFor("Space")).toEqual([0x39, false]);
  });

  it("marks extended (E0) keys", () => {
    expect(scanFor("ArrowLeft")).toEqual([0x4b, true]);
    expect(scanFor("NumpadEnter")).toEqual([0x1c, true]);
    expect(scanFor("ControlRight")).toEqual([0x1d, true]);
    expect(scanFor("AltRight")).toEqual([0x38, true]);
    expect(scanFor("Delete")).toEqual([0x53, true]);
  });

  it("distinguishes the numpad from the main block", () => {
    expect(scanFor("Numpad1")).toEqual([0x4f, false]);
    expect(scanFor("End")).toEqual([0x4f, true]);
  });

  it("covers function keys, modifiers and locks", () => {
    for (let i = 1; i <= 12; i++) expect(scanFor(`F${i}`)).toBeDefined();
    for (const c of ["ShiftLeft", "ShiftRight", "ControlLeft", "AltLeft", "CapsLock", "NumLock", "MetaLeft"]) expect(scanFor(c)).toBeDefined();
  });

  it("has no scancode collisions inside the same prefix group", () => {
    const seen = new Set<string>();
    const codes = ["KeyA", "KeyB", "KeyC", "Digit1", "Enter", "NumpadEnter", "Home", "Numpad7"];
    for (const c of codes) {
      const [s, e] = scanFor(c)!;
      const key = `${e ? "E0" : ""}${s}`;
      expect(seen.has(key)).toBe(false);
      seen.add(key);
    }
    expect(mappedKeyCount).toBeGreaterThan(100);
  });

  it("returns undefined for unknown codes", () => {
    expect(scanFor("Unidentified")).toBeUndefined();
  });
});
