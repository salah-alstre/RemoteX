import { useEffect, useRef } from "react";
import { localPointer, pointerBus } from "../lib/pointer";
import { cursorBus } from "../lib/store";
import type { CursorUpdateView } from "../lib/types";

function decodeBase64(b64: string): Uint8ClampedArray {
  const bin = atob(b64);
  const out = new Uint8ClampedArray(bin.length);
  for (let i = 0; i < bin.length; i++) out[i] = bin.charCodeAt(i);
  return out;
}

interface Props {
  session: number;
  /** Native size of the remote display; cursor coordinates are in those pixels. */
  displayWidth: number;
  displayHeight: number;
  /** Size the picture currently occupies on screen. */
  cssWidth: number;
  cssHeight: number;
  /**
   * Draw the cursor at the local pointer position immediately instead of waiting for the host to report it.
   * There is still only one cursor: the predicted one takes over while the pointer is moving and hands back
   * to the host-reported position as soon as the pointer rests, so any difference is corrected.
   */
  predict?: boolean;
}

/** After the pointer has been still this long, the host's position wins again. */
const RECONCILE_MS = 160;

/** Draws the remote cursor over the picture, updated straight from events to avoid re-rendering React. */
export default function CursorOverlay({ session, displayWidth, displayHeight, cssWidth, cssHeight, predict = false }: Props) {
  const canvas = useRef<HTMLCanvasElement>(null);
  const last = useRef<{ update: CursorUpdateView; hotX: number; hotY: number; w: number; h: number } | null>(null);
  const geometry = useRef({ displayWidth, displayHeight, cssWidth, cssHeight });
  geometry.current = { displayWidth, displayHeight, cssWidth, cssHeight };
  const predictRef = useRef(predict);
  predictRef.current = predict;

  useEffect(() => {
    const place = () => {
      const el = canvas.current;
      const cur = last.current;
      if (!el || !cur) return;
      const g = geometry.current;
      if (!cur.update.visible || g.displayWidth === 0) {
        el.style.visibility = "hidden";
        return;
      }
      const k = g.cssWidth / g.displayWidth;
      el.style.visibility = "visible";
      el.style.width = `${cur.w * k}px`;
      el.style.height = `${cur.h * k}px`;
      let x = cur.update.x;
      let y = cur.update.y;
      if (predictRef.current && localPointer.inside && performance.now() - localPointer.at < RECONCILE_MS) {
        x = (localPointer.x / 65535) * g.displayWidth;
        y = (localPointer.y / 65535) * g.displayHeight;
      }
      el.style.transform = `translate(${(x - cur.hotX) * k}px, ${(y - cur.hotY) * k}px)`;
    };
    let settle = 0;
    const onLocalMove = () => {
      if (!predictRef.current) return;
      place();
      window.clearTimeout(settle);
      settle = window.setTimeout(place, RECONCILE_MS + 10);
    };

    const onCursor = (ev: Event) => {
      const { session: s, update } = (ev as CustomEvent<{ session: number; update: CursorUpdateView }>).detail;
      const el = canvas.current;
      if (s !== session || !el) return;
      const prev = last.current;
      let hotX = prev?.hotX ?? 0;
      let hotY = prev?.hotY ?? 0;
      let w = prev?.w ?? 0;
      let h = prev?.h ?? 0;
      if (update.shape) {
        const sh = update.shape;
        el.width = sh.width;
        el.height = sh.height;
        const img = new ImageData(decodeBase64(sh.rgbaBase64) as unknown as Uint8ClampedArray<ArrayBuffer>, sh.width, sh.height);
        el.getContext("2d")?.putImageData(img, 0, 0);
        [hotX, hotY, w, h] = [sh.hotX, sh.hotY, sh.width, sh.height];
      }
      last.current = { update, hotX, hotY, w, h };
      place();
    };
    cursorBus.addEventListener("cursor", onCursor);
    pointerBus.addEventListener("move", onLocalMove);
    place();
    return () => {
      cursorBus.removeEventListener("cursor", onCursor);
      pointerBus.removeEventListener("move", onLocalMove);
      window.clearTimeout(settle);
    };
  }, [session, cssWidth, cssHeight, displayWidth, displayHeight]);

  return <canvas ref={canvas} aria-hidden className="pointer-events-none absolute start-0 top-0 origin-top-left" style={{ visibility: "hidden", imageRendering: "auto" }} />;
}
