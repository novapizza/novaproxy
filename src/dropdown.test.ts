import { describe, expect, it } from "vitest";
import { nextIndex } from "./Dropdown";

describe("nextIndex", () => {
  it("opens onto the first row going down and the last going up", () => {
    expect(nextIndex("down", -1, 4)).toBe(0);
    expect(nextIndex("up", -1, 4)).toBe(3);
  });

  it("steps through the rows", () => {
    expect(nextIndex("down", 0, 4)).toBe(1);
    expect(nextIndex("up", 2, 4)).toBe(1);
  });

  it("wraps at both ends", () => {
    expect(nextIndex("down", 3, 4)).toBe(0);
    expect(nextIndex("up", 0, 4)).toBe(3);
  });

  it("jumps to the ends with Home and End", () => {
    expect(nextIndex("home", 2, 4)).toBe(0);
    expect(nextIndex("end", 2, 4)).toBe(3);
    expect(nextIndex("home", -1, 4)).toBe(0);
    expect(nextIndex("end", -1, 4)).toBe(3);
  });

  it("highlights nothing when there is nothing to highlight", () => {
    for (const key of ["up", "down", "home", "end"] as const) {
      expect(nextIndex(key, -1, 0)).toBe(-1);
    }
  });

  it("stays put on a single row", () => {
    expect(nextIndex("down", 0, 1)).toBe(0);
    expect(nextIndex("up", 0, 1)).toBe(0);
  });

  it("recovers from an index left over from a longer list", () => {
    // The clear row can disappear (nothing left to clear) while the panel is
    // open; a stale highlight past the end must not read out of bounds.
    expect(nextIndex("down", 9, 3)).toBe(0);
    expect(nextIndex("up", 9, 3)).toBe(2);
  });
});
