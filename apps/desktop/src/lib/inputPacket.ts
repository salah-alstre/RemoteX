import type { UiInput } from "./api";

/**
 * Compact binary form of an input event (see `decode_input` in the shell):
 * `session u32 LE | kind u8 | payload`. No JSON, no per-event object graph.
 */
export function encodeInput(session: number, e: UiInput): Uint8Array {
  const enc = e.type === "text" ? new TextEncoder().encode(e.text) : null;
  const buf = new Uint8Array(5 + (enc ? enc.length : 4));
  const dv = new DataView(buf.buffer);
  dv.setUint32(0, session, true);
  switch (e.type) {
    case "mouseMove":
      buf[4] = 0;
      dv.setUint16(5, e.x, true);
      dv.setUint16(7, e.y, true);
      return buf.subarray(0, 9);
    case "mouseButton":
      buf[4] = 1;
      buf[5] = e.button;
      buf[6] = e.down ? 1 : 0;
      return buf.subarray(0, 7);
    case "wheel":
      buf[4] = 2;
      dv.setInt16(5, e.dx, true);
      dv.setInt16(7, e.dy, true);
      return buf.subarray(0, 9);
    case "key":
      buf[4] = 3;
      dv.setUint16(5, e.scancode, true);
      buf[7] = e.extended ? 1 : 0;
      buf[8] = e.down ? 1 : 0;
      return buf.subarray(0, 9);
    case "text":
      buf[4] = 4;
      buf.set(enc as Uint8Array, 5);
      return buf.subarray(0, 5 + (enc as Uint8Array).length);
    case "releaseAll":
      buf[4] = 5;
      return buf.subarray(0, 5);
  }
}
