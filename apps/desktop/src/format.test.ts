import { describe, expect, it } from "vitest";

import { formatBytes, formatDuration } from "./format";

describe("formatBytes", () => {
  it("formats byte-sized amounts", () => {
    expect(formatBytes(0)).toBe("0 B");
    expect(formatBytes(512)).toBe("512 B");
  });

  it("formats larger units with one decimal place", () => {
    expect(formatBytes(1536)).toBe("1.5 KB");
    expect(formatBytes(148_897_792)).toBe("142.0 MB");
    expect(formatBytes(1_073_741_824)).toBe("1.0 GB");
  });

  it("treats invalid input as zero", () => {
    expect(formatBytes(-5)).toBe("0 B");
    expect(formatBytes(Number.NaN)).toBe("0 B");
    expect(formatBytes(Number.POSITIVE_INFINITY)).toBe("0 B");
  });
});

describe("formatDuration", () => {
  it("formats sub-minute durations", () => {
    expect(formatDuration(5)).toBe("00:05");
    expect(formatDuration(65)).toBe("01:05");
  });

  it("formats hour-long durations", () => {
    expect(formatDuration(3661)).toBe("01:01:01");
  });

  it("treats invalid input as zero", () => {
    expect(formatDuration(-3)).toBe("00:00");
    expect(formatDuration(Number.NaN)).toBe("00:00");
    expect(formatDuration(Number.POSITIVE_INFINITY)).toBe("00:00");
  });
});