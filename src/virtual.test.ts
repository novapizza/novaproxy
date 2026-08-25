import { describe, expect, it } from "vitest";
import { sliceFlat } from "./virtual";

describe("sliceFlat", () => {
  it("renders nothing before the viewport is measured", () => {
    // A first frame that mounts 10,000 rows is the crash this module prevents.
    const s = sliceFlat(10_000, 34, 0, 0, 8);
    expect([s.from, s.to]).toEqual([0, 0]);
    expect(s.padBottom).toBe(10_000 * 34);
  });

  it("windows to the viewport plus the overscan on both sides", () => {
    const s = sliceFlat(1000, 10, 500, 100, 2);
    expect(s.from).toBe(48);
    expect(s.to).toBe(62);
    expect(s.padTop).toBe(480);
    expect(s.padBottom).toBe((1000 - 62) * 10);
  });

  it("spacers plus rendered rows always add up to the full height", () => {
    for (const scrollTop of [0, 137, 4000, 33_800]) {
      const s = sliceFlat(1000, 34, scrollTop, 400, 8);
      expect(s.padTop + (s.to - s.from) * 34 + s.padBottom).toBe(1000 * 34);
    }
  });

  it("clamps at both ends rather than running off the list", () => {
    const top = sliceFlat(20, 34, 0, 400, 8);
    expect(top.from).toBe(0);
    const past = sliceFlat(20, 34, 99_999, 400, 8);
    expect(past.from).toBe(20);
    expect(past.to).toBe(20);
    expect(past.padBottom).toBe(0);
  });

  it("an empty list is not a special case", () => {
    expect(sliceFlat(0, 34, 0, 400, 8)).toEqual({ from: 0, to: 0, padTop: 0, padBottom: 0 });
  });
});
