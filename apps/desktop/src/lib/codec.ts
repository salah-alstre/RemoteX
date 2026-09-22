/** Derives WebCodecs codec strings from the parameter sets inside an Annex-B keyframe. */

export function splitNals(data: Uint8Array): Uint8Array[] {
  const starts: number[] = [];
  for (let i = 0; i + 2 < data.length; i++) {
    if (data[i] === 0 && data[i + 1] === 0 && data[i + 2] === 1) {
      starts.push(i + 3);
      i += 2;
    }
  }
  return starts.map((s, n) => {
    let e = n + 1 < starts.length ? (starts[n + 1] as number) - 3 : data.length;
    while (e > s && data[e - 1] === 0) e--;
    return data.subarray(s, e);
  });
}

/** Removes emulation-prevention bytes (00 00 03 -> 00 00). */
function unescape(nal: Uint8Array): Uint8Array {
  const out: number[] = [];
  for (let i = 0; i < nal.length; i++) {
    if (i >= 2 && nal[i] === 3 && nal[i - 1] === 0 && nal[i - 2] === 0) continue;
    out.push(nal[i] as number);
  }
  return Uint8Array.from(out);
}

const hex = (n: number, width = 2) => n.toString(16).padStart(width, "0");

export function h264CodecString(annexB: Uint8Array): string | null {
  const sps = splitNals(annexB).find((n) => n.length > 3 && ((n[0] as number) & 0x1f) === 7);
  if (!sps) return null;
  return `avc1.${hex(sps[1] as number)}${hex(sps[2] as number)}${hex(sps[3] as number)}`;
}

function reverseBits32(v: number): number {
  let r = 0;
  for (let i = 0; i < 32; i++) r = (r << 1) | ((v >>> i) & 1);
  return r >>> 0;
}

export function h265CodecString(annexB: Uint8Array): string | null {
  const nal = splitNals(annexB).find((n) => n.length > 15 && (((n[0] as number) >> 1) & 0x3f) === 33);
  if (!nal) return null;
  const p = unescape(nal);
  // 2-byte NAL header, 1 byte (vps id/sub-layers/nesting), then profile_tier_level.
  const b = p[3] as number;
  const space = b >> 6;
  const tier = (b >> 5) & 1;
  const profile = b & 0x1f;
  const compat = (((p[4] as number) << 24) | ((p[5] as number) << 16) | ((p[6] as number) << 8) | (p[7] as number)) >>> 0;
  const constraints = Array.from(p.subarray(8, 14));
  while (constraints.length > 0 && constraints[constraints.length - 1] === 0) constraints.pop();
  const level = p[14] as number;
  const prefix = ["", "A", "B", "C"][space] ?? "";
  const parts = [`hev1.${prefix}${profile}`, reverseBits32(compat).toString(16), `${tier ? "H" : "L"}${level}`, ...constraints.map((c) => hex(c))];
  return parts.join(".");
}

export interface VideoHeader {
  keyframe: boolean;
  codec: "h264" | "h265" | "av1";
  width: number;
  height: number;
  capturedMs: number;
  seq: number;
  data: Uint8Array;
}

/** Layout written by the shell: flag, codec, w, h (u32 LE), captured_ms, seq (u64 LE), payload. */
export const HEADER_BYTES = 26;

export function parsePacket(buf: ArrayBuffer): VideoHeader | null {
  if (buf.byteLength <= HEADER_BYTES) return null;
  const v = new DataView(buf);
  const codec = (["h264", "h265", "av1"] as const)[v.getUint8(1)];
  if (!codec) return null;
  return {
    keyframe: v.getUint8(0) === 1,
    codec,
    width: v.getUint32(2, true),
    height: v.getUint32(6, true),
    capturedMs: Number(v.getBigUint64(10, true)),
    seq: Number(v.getBigUint64(18, true)),
    data: new Uint8Array(buf, HEADER_BYTES),
  };
}
