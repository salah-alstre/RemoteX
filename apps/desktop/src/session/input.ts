import { useEffect, type RefObject } from "react";
import { api, type UiInput } from "../lib/api";
import { normalizePointer, wheelDelta } from "../lib/layout";
import { scanFor } from "../lib/keymap";
import { localPointer, updateLocalPointer } from "../lib/pointer";
const localPointerX = () => localPointer.x;
const localPointerY = () => localPointer.y;

/** Pointer positions are forwarded at most this often; the host also keeps only the newest position. */
const MOVE_INTERVAL_MS = 4;

interface Options {
  session: number;
  mouse: boolean;
  keyboard: boolean;
  active: boolean;
}

const isEditable = (el: Element | null) => el instanceof HTMLInputElement || el instanceof HTMLTextAreaElement || el instanceof HTMLSelectElement || (el as HTMLElement | null)?.isContentEditable;

/**
 * Captures pointer and keyboard input on the remote canvas and forwards it.
 * Keys are sent as physical scancodes so the host's own layout (English, Arabic) produces the character.
 * Everything held is released on blur so nothing can get stuck on the remote machine.
 */
export function useRemoteInput(canvas: RefObject<HTMLCanvasElement | null>, { session, mouse, keyboard, active }: Options): void {
  useEffect(() => {
    const el = canvas.current;
    if (!el || !active) return;
    const send = (event: UiInput) => void api.input(session, event);
    const held = new Set<string>();
    let pending: { x: number; y: number } | null = null;
    let timer = 0;
    let lastSent = 0;
    // The layout rectangle only changes on resize/scroll; reading it on every pointer event forces layout.
    let rect = el.getBoundingClientRect();
    const refreshRect = () => (rect = el.getBoundingClientRect());
    const ro = new ResizeObserver(refreshRect);
    ro.observe(el);
    window.addEventListener("scroll", refreshRect, true);

    const flushMove = () => {
      timer = 0;
      if (!pending) return;
      lastSent = performance.now();
      send({ type: "mouseMove", ...pending });
      pending = null;
    };
    const point = (e: PointerEvent | WheelEvent) => normalizePointer(e.clientX, e.clientY, rect);
    // A click carries its own, newer position; an older queued position must never be sent after it.
    const dropPending = () => {
      pending = null;
      if (timer) window.clearTimeout(timer);
      timer = 0;
    };

    // No animation-frame batching: the position goes out as soon as the rate limit allows, so input is not
    // held back for up to a frame, and nothing here touches React state.
    const onMove = (e: PointerEvent) => {
      const p = point(e);
      updateLocalPointer(p.x, p.y);
      if (!mouse) return;
      pending = p;
      const wait = MOVE_INTERVAL_MS - (performance.now() - lastSent);
      if (wait <= 0) {
        if (timer) window.clearTimeout(timer);
        flushMove();
      } else {
        timer ||= window.setTimeout(flushMove, wait);
      }
    };
    const onLeave = () => updateLocalPointer(localPointerX(), localPointerY(), false);
    const onDown = (e: PointerEvent) => {
      if (!mouse) return;
      el.setPointerCapture(e.pointerId);
      dropPending();
      send({ type: "mouseMove", ...point(e) });
      send({ type: "mouseButton", button: e.button, down: true });
      e.preventDefault();
    };
    const onUp = (e: PointerEvent) => {
      if (!mouse) return;
      dropPending();
      send({ type: "mouseMove", ...point(e) });
      send({ type: "mouseButton", button: e.button, down: false });
    };
    const onWheel = (e: WheelEvent) => {
      if (!mouse) return;
      e.preventDefault();
      const dy = wheelDelta(e.deltaY, e.deltaMode);
      const dx = -wheelDelta(e.deltaX, e.deltaMode);
      if (dx || dy) send({ type: "wheel", dx, dy });
    };
    const noMenu = (e: Event) => e.preventDefault();

    const onKey = (down: boolean) => (e: KeyboardEvent) => {
      if (!keyboard || isEditable(document.activeElement)) return;
      if (e.isComposing || e.keyCode === 229) return;
      const scan = scanFor(e.code);
      if (scan) {
        e.preventDefault();
        if (down) held.add(e.code);
        else held.delete(e.code);
        send({ type: "key", scancode: scan[0], extended: scan[1], down });
      } else if (down && e.key.length === 1 && !e.ctrlKey && !e.metaKey) {
        // A character with no physical key on this keyboard: send it as text.
        e.preventDefault();
        send({ type: "text", text: e.key });
      }
    };
    const onKeyDown = onKey(true);
    const onKeyUp = onKey(false);
    const onComposition = (e: CompositionEvent) => {
      if (keyboard && e.data) send({ type: "text", text: e.data });
    };

    const releaseAll = () => {
      held.clear();
      send({ type: "releaseAll" });
    };
    const onVisibility = () => document.hidden && releaseAll();

    el.addEventListener("pointermove", onMove);
    el.addEventListener("pointerleave", onLeave);
    el.addEventListener("pointerdown", onDown);
    el.addEventListener("pointerup", onUp);
    el.addEventListener("wheel", onWheel, { passive: false });
    el.addEventListener("contextmenu", noMenu);
    window.addEventListener("keydown", onKeyDown, true);
    window.addEventListener("keyup", onKeyUp, true);
    window.addEventListener("compositionend", onComposition);
    window.addEventListener("blur", releaseAll);
    document.addEventListener("visibilitychange", onVisibility);
    return () => {
      el.removeEventListener("pointermove", onMove);
      el.removeEventListener("pointerleave", onLeave);
      ro.disconnect();
      window.removeEventListener("scroll", refreshRect, true);
      el.removeEventListener("pointerdown", onDown);
      el.removeEventListener("pointerup", onUp);
      el.removeEventListener("wheel", onWheel);
      el.removeEventListener("contextmenu", noMenu);
      window.removeEventListener("keydown", onKeyDown, true);
      window.removeEventListener("keyup", onKeyUp, true);
      window.removeEventListener("compositionend", onComposition);
      window.removeEventListener("blur", releaseAll);
      document.removeEventListener("visibilitychange", onVisibility);
      if (timer) window.clearTimeout(timer);
      send({ type: "releaseAll" });
    };
  }, [canvas, session, mouse, keyboard, active]);
}
