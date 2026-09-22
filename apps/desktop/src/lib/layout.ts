export type ScaleMode = "fit" | "original" | "stretch" | "scale";

export interface Size {
  w: number;
  h: number;
}

/** CSS size of the remote picture inside its container for each view mode. */
export function computeCanvasSize(container: Size, video: Size, mode: ScaleMode, scalePercent: number): Size {
  if (video.w <= 0 || video.h <= 0 || container.w <= 0 || container.h <= 0) return { w: 0, h: 0 };
  switch (mode) {
    case "original":
      return { w: video.w, h: video.h };
    case "stretch":
      return { w: container.w, h: container.h };
    case "scale":
      return { w: Math.round((video.w * scalePercent) / 100), h: Math.round((video.h * scalePercent) / 100) };
    case "fit": {
      const k = Math.min(container.w / video.w, container.h / video.h);
      return { w: Math.floor(video.w * k), h: Math.floor(video.h * k) };
    }
  }
}

/** Pointer position inside a rectangle mapped to the protocol's 0..65535 range. */
export function normalizePointer(clientX: number, clientY: number, rect: { left: number; top: number; width: number; height: number }): { x: number; y: number } {
  const clamp = (v: number) => Math.min(1, Math.max(0, v));
  return {
    x: Math.round(clamp((clientX - rect.left) / Math.max(1, rect.width)) * 65535),
    y: Math.round(clamp((clientY - rect.top) / Math.max(1, rect.height)) * 65535),
  };
}

/** Wheel movement to protocol units (120 per notch), whatever unit the browser reports. */
export function wheelDelta(delta: number, mode: number): number {
  const notches = mode === 1 ? delta / 3 : mode === 2 ? delta : delta / 100;
  return Math.max(-32000, Math.min(32000, Math.round(-notches * 120)));
}
