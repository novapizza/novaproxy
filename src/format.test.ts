import { describe, expect, it } from "vitest";
import { formatBytes, formatCellBytes, formatClock } from "./format";

describe("formatBytes", () => {
  it("keeps bytes whole and scales with one decimal", () => {
    expect(formatBytes(0)).toBe("0 B");
    expect(formatBytes(812)).toBe("812 B");
    expect(formatBytes(1024)).toBe("1.0 KB");
    expect(formatBytes(1536)).toBe("1.5 KB");
    expect(formatBytes(24_700)).toBe("24.1 KB");
    expect(formatBytes(5 * 1024 ** 3)).toBe("5.0 GB");
  });

  it("takes bigint, since wire sizes arrive as one", () => {
    expect(formatBytes(2048n)).toBe("2.0 KB");
  });

  it("never renders a negative or a NaN size", () => {
    expect(formatBytes(-1)).toBe("0 B");
    expect(formatBytes(Number.NaN)).toBe("0 B");
  });
});

describe("formatCellBytes", () => {
  it("distinguishes `no body` from `an empty body`", () => {
    // The table has to keep these apart: a 204 sent nothing at all.
    expect(formatCellBytes(0)).toBe("–");
    expect(formatCellBytes(null)).toBe("–");
    expect(formatCellBytes(undefined)).toBe("–");
    expect(formatCellBytes(2)).toBe("2 B");
  });
});

describe("formatClock", () => {
  it("renders local wall-clock time to the millisecond", () => {
    const d = new Date(2026, 6, 4, 22, 40, 3, 296);
    expect(formatClock(d.getTime())).toBe("22:40:03.296");
  });

  it("pads every field, so the column stays a column", () => {
    const d = new Date(2026, 0, 1, 9, 5, 7, 40);
    expect(formatClock(d.getTime())).toBe("09:05:07.040");
  });

  it("does not print `Invalid Date` into a cell", () => {
    expect(formatClock(Number.NaN)).toBe("—");
  });
});
