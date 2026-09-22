import { describe, expect, it } from "vitest";
import { formatBytes, formatDuration, formatMbps, groupId, normalizeIdInput } from "./format";

describe("format", () => {
  it("formats bytes", () => {
    expect(formatBytes(0, "en")).toBe("0 B");
    expect(formatBytes(1536, "en")).toBe("1.5 KB");
    expect(formatBytes(5 * 1024 * 1024, "en")).toBe("5 MB");
  });

  it("formats durations", () => {
    expect(formatDuration(65)).toBe("1:05");
    expect(formatDuration(3725)).toBe("1:02:05");
    expect(formatDuration(-3)).toBe("0:00");
  });

  it("groups ids and normalises Arabic digits", () => {
    expect(groupId("583294814")).toBe("583 294 814");
    expect(groupId("5832")).toBe("583 2");
    expect(normalizeIdInput("٥٨٣ ٢٩٤ ٨١٤")).toBe("583294814");
    expect(normalizeIdInput("583-294-814-999")).toBe("583294814");
  });

  it("formats bitrate", () => {
    expect(formatMbps(8500, "en")).toBe("8.5");
  });
});
