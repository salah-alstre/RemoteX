import { describe, expect, it } from "vitest";
import { computeCanvasSize, normalizePointer, wheelDelta } from "./layout";

describe("layout", () => {
  const video = { w: 1920, h: 1080 };

  it("fits without distorting the aspect ratio", () => {
    expect(computeCanvasSize({ w: 960, h: 800 }, video, "fit", 100)).toEqual({ w: 960, h: 540 });
    expect(computeCanvasSize({ w: 2000, h: 500 }, video, "fit", 100)).toEqual({ w: 888, h: 500 });
  });

  it("supports original, stretch and custom scale", () => {
    expect(computeCanvasSize({ w: 500, h: 500 }, video, "original", 100)).toEqual(video);
    expect(computeCanvasSize({ w: 500, h: 300 }, video, "stretch", 100)).toEqual({ w: 500, h: 300 });
    expect(computeCanvasSize({ w: 500, h: 300 }, video, "scale", 50)).toEqual({ w: 960, h: 540 });
  });

  it("handles unknown sizes", () => {
    expect(computeCanvasSize({ w: 0, h: 0 }, video, "fit", 100)).toEqual({ w: 0, h: 0 });
    expect(computeCanvasSize({ w: 100, h: 100 }, { w: 0, h: 0 }, "fit", 100)).toEqual({ w: 0, h: 0 });
  });

  it("normalises pointer positions and clamps outside the picture", () => {
    const rect = { left: 100, top: 50, width: 200, height: 100 };
    expect(normalizePointer(100, 50, rect)).toEqual({ x: 0, y: 0 });
    expect(normalizePointer(300, 150, rect)).toEqual({ x: 65535, y: 65535 });
    expect(normalizePointer(200, 100, rect)).toEqual({ x: 32768, y: 32768 });
    expect(normalizePointer(-50, 999, rect)).toEqual({ x: 0, y: 65535 });
  });

  it("converts wheel deltas in every unit", () => {
    expect(wheelDelta(100, 0)).toBe(-120); // 100 px down = one notch down
    expect(wheelDelta(-100, 0)).toBe(120);
    expect(wheelDelta(3, 1)).toBe(-120); // 3 lines
    expect(wheelDelta(1, 2)).toBe(-120); // 1 page
    expect(wheelDelta(1e9, 0)).toBe(-32000);
  });
});
