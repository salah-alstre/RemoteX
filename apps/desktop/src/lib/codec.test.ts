import { describe, expect, it } from "vitest";
import { HEADER_BYTES, h264CodecString, h265CodecString, parsePacket, splitNals } from "./codec";

describe("codec strings", () => {
  it("derives the avc1 string from the SPS", () => {
    const annexB = Uint8Array.from([0, 0, 0, 1, 0x67, 0x4d, 0x00, 0x34, 0xac, 0, 0, 1, 0x68, 0xee, 0x3c, 0x80, 0, 0, 0, 1, 0x65, 0x88]);
    expect(h264CodecString(annexB)).toBe("avc1.4d0034");
  });

  it("returns null when there is no SPS", () => {
    expect(h264CodecString(Uint8Array.from([0, 0, 0, 1, 0x41, 0x9a]))).toBeNull();
  });

  it("derives an HEVC main-profile string", () => {
    // NAL header 0x42 0x01 (SPS), vps-id byte, PTL: profile_idc 1, compat 0x60000000, level 93 (3.1)
    const sps = [0x42, 0x01, 0x01, 0x01, 0x60, 0x00, 0x00, 0x00, 0xb0, 0x00, 0x00, 0x00, 0x00, 0x00, 0x5d];
    const s = h265CodecString(Uint8Array.from([0, 0, 0, 1, ...sps, 0xa0]));
    expect(s).toBe("hev1.1.6.L93.b0");
  });

  it("splits three- and four-byte start codes", () => {
    const nals = splitNals(Uint8Array.from([0, 0, 0, 1, 0x67, 1, 2, 0, 0, 1, 0x68, 3]));
    expect(nals.map((n) => n[0])).toEqual([0x67, 0x68]);
  });
});

describe("video packets", () => {
  it("parses the header written by the shell", () => {
    const payload = [0, 0, 0, 1, 0x65, 9];
    const buf = new ArrayBuffer(HEADER_BYTES + payload.length);
    const v = new DataView(buf);
    v.setUint8(0, 1);
    v.setUint8(1, 0);
    v.setUint32(2, 1920, true);
    v.setUint32(6, 1080, true);
    v.setBigUint64(10, 1234n, true);
    v.setBigUint64(18, 77n, true);
    new Uint8Array(buf, HEADER_BYTES).set(payload);
    const p = parsePacket(buf)!;
    expect([p.keyframe, p.codec, p.width, p.height, p.capturedMs, p.seq]).toEqual([true, "h264", 1920, 1080, 1234, 77]);
    expect(Array.from(p.data)).toEqual(payload);
  });

  it("rejects truncated or malformed packets", () => {
    expect(parsePacket(new ArrayBuffer(10))).toBeNull();
    const bad = new ArrayBuffer(HEADER_BYTES + 4);
    new DataView(bad).setUint8(1, 9);
    expect(parsePacket(bad)).toBeNull();
  });
});
