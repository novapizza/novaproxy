import { describe, expect, it } from "vitest";
import { renderedRows, sliceGroups, type ListMetrics, sliceFlat } from "./virtual";

/** Flat list: one group, no header. */
const flat: ListMetrics = { rowH: 50, headerH: 0, overscan: 0 };
/** Grouped list: 30px host headers. */
const grouped: ListMetrics = { rowH: 50, headerH: 30, overscan: 0 };

describe("sliceGroups — flat list", () => {
  it("renders only the rows the viewport can show", () => {
    const [s] = sliceGroups([1000], flat, 0, 500);
    expect(s.from).toBe(0);
    expect(s.to).toBe(10);
    expect(s.padTop).toBe(0);
    expect(s.padBottom).toBe(990 * 50);
    // The spacers plus the rendered rows always add up to the full height, so
    // the scrollbar describes the whole list.
    expect(s.padTop + (s.to - s.from) * flat.rowH + s.padBottom).toBe(s.height);
  });

  it("windows to the scrolled position", () => {
    const [s] = sliceGroups([1000], flat, 5_000, 500);
    expect(s.from).toBe(100);
    expect(s.to).toBe(110);
    expect(s.padTop).toBe(100 * 50);
    expect(s.padBottom).toBe(890 * 50);
  });

  it("keeps the row count bounded however long the list gets", () => {
    const short = sliceGroups([100], flat, 0, 500);
    const huge = sliceGroups([100_000], flat, 0, 500);
    expect(renderedRows(huge)).toBe(renderedRows(short));
  });

  it("renders extra rows either side when overscanning", () => {
    const [s] = sliceGroups([1000], { ...flat, overscan: 4 }, 5_000, 500);
    expect(s.from).toBe(96);
    expect(s.to).toBe(114);
  });

  it("stops at the ends rather than overscanning past them", () => {
    const [top] = sliceGroups([1000], { ...flat, overscan: 4 }, 0, 500);
    expect(top.from).toBe(0);
    expect(top.padTop).toBe(0);

    const [end] = sliceGroups([20], flat, 500, 500);
    expect(end.to).toBe(20);
    expect(end.padBottom).toBe(0);
  });

  it("renders nothing until the viewport has been measured", () => {
    const [s] = sliceGroups([1000], flat, 0, 0);
    expect(s.onScreen).toBe(false);
    expect(renderedRows([s])).toBe(0);
    // The height is still reported, so the list does not collapse on the first
    // frame and then jump.
    expect(s.height).toBe(1000 * 50);
  });

  it("survives a zero row height instead of dividing by it", () => {
    const [s] = sliceGroups([1000], { ...flat, rowH: 0 }, 0, 500);
    expect(s.onScreen).toBe(false);
    expect(Number.isFinite(s.padTop)).toBe(true);
  });

  it("handles an empty list", () => {
    expect(sliceGroups([], flat, 0, 500)).toEqual([]);
    const [s] = sliceGroups([0], flat, 0, 500);
    expect(s.from).toBe(0);
    expect(s.to).toBe(0);
  });
});

describe("sliceGroups — grouped by host", () => {
  it("skips groups that are off-screen but keeps their height", () => {
    // Groups of 10 rows each: 30 + 500 = 530px apiece.
    const slices = sliceGroups([10, 10, 10, 10], grouped, 0, 400);
    expect(slices.map((s) => s.onScreen)).toEqual([true, false, false, false]);
    expect(slices.every((s) => s.height === 530)).toBe(true);
    expect(renderedRows(slices)).toBe(8); // 400px of viewport / 50px rows
  });

  it("renders the header of every group it touches", () => {
    // Viewport straddles the boundary between group 1 and group 2.
    const slices = sliceGroups([10, 10, 10], grouped, 400, 300);
    expect(slices.map((s) => s.onScreen)).toEqual([true, true, false]);
    // Group 1's tail and group 2's head, nothing more.
    expect(slices[0].to).toBe(10);
    expect(slices[0].padBottom).toBe(0);
    expect(slices[1].from).toBe(0);
  });

  it("offsets rows by the header above them", () => {
    // Second group starts at 530; its first row starts at 560.
    const slices = sliceGroups([10, 10], grouped, 560, 100);
    expect(slices[1].from).toBe(0);
    expect(slices[1].to).toBe(2);
  });

  it("shows only the header when a group is barely on screen", () => {
    // Group 2 starts at 530; the viewport ends at 550, so 20px of it shows —
    // its header, no rows yet.
    const slices = sliceGroups([10, 10], grouped, 150, 400);
    expect(slices[1].onScreen).toBe(true);
    expect(slices[1].to - slices[1].from).toBe(0);
  });

  it("keeps the rendered row count bounded across many groups", () => {
    const many = sliceGroups(Array.from({ length: 500 }, () => 20), grouped, 0, 600);
    expect(renderedRows(many)).toBeLessThan(20);
  });
});

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
