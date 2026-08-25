import { describe, expect, it } from "vitest";
import {
  clampColumnWidth,
  COLUMNS,
  COLUMN_ORDER,
  DEFAULT_COLUMNS,
  gridTemplate,
  MAX_COLUMN_W,
  MIN_COLUMN_W,
  minTableWidth,
  normalizeColumns,
  normalizeWidths,
} from "./columns";

describe("column declarations", () => {
  it("every id in the display order has a declaration, and vice versa", () => {
    expect([...COLUMN_ORDER].sort()).toEqual(Object.keys(COLUMNS).sort());
  });

  it("exactly one column is flexible — two would fight over the leftover width", () => {
    const flexible = COLUMN_ORDER.filter((id) => COLUMNS[id].track.includes("fr"));
    expect(flexible).toEqual(["url"]);
  });

  it("the default set fits the smallest window once the tree is hidden", () => {
    // The numbers, so the default set is a measurement and not a taste: the
    // window's minWidth is 940 and the rail takes 78, leaving 862 with the tree
    // hidden (⌘0) and 610 with it open. Seven columns need 746 — they fit the
    // first case, and the table scrolls sideways in the second. Eleven fit
    // neither, which is why there is a default set at all.
    expect(minTableWidth(DEFAULT_COLUMNS)).toBeLessThanOrEqual(940 - 78);
    expect(minTableWidth(DEFAULT_COLUMNS)).toBeGreaterThan(940 - 78 - 252);
    expect(minTableWidth(COLUMN_ORDER)).toBeGreaterThan(940 - 78);
  });
});

describe("gridTemplate", () => {
  it("emits one track per column, in the given order", () => {
    expect(gridTemplate(["method", "url"])).toBe("62px minmax(320px, 1fr)");
  });
});

describe("normalizeColumns", () => {
  it("keeps known ids and puts them back into display order", () => {
    expect(normalizeColumns(["status", "url", "seq"])).toEqual(["seq", "url", "status"]);
  });

  it("drops ids it does not know rather than rendering a hole", () => {
    expect(normalizeColumns(["url", "seq", "made-up"])).toEqual(["seq", "url"]);
  });

  it("always keeps URL: a table of sizes and statuses identifies nothing", () => {
    expect(normalizeColumns(["status", "duration"])).toContain("url");
  });

  it("falls back to the default set on nothing, or on a single-column list", () => {
    expect(normalizeColumns(null)).toEqual(DEFAULT_COLUMNS);
    expect(normalizeColumns([])).toEqual(DEFAULT_COLUMNS);
    expect(normalizeColumns(["nope"])).toEqual(DEFAULT_COLUMNS);
  });
});

describe("column widths", () => {
  it("a dragged width replaces the declared track, URL's 1fr included", () => {
    expect(gridTemplate(["method", "url"], { url: 400 })).toBe("62px 400px");
    expect(gridTemplate(["method", "url"], { method: 90 })).toBe("90px minmax(320px, 1fr)");
  });

  it("counts dragged widths when working out whether the table scrolls", () => {
    const wide = minTableWidth(["method", "url"], { url: 900 });
    expect(wide).toBe(62 + 900);
  });

  it("clamps a width to something a cell can still show", () => {
    expect(clampColumnWidth(2)).toBe(MIN_COLUMN_W);
    expect(clampColumnWidth(9999)).toBe(MAX_COLUMN_W);
    expect(clampColumnWidth(120.6)).toBe(121);
    // NaN survives Math.min/Math.max, so it is rejected explicitly.
    expect(clampColumnWidth(Number.NaN)).toBe(MIN_COLUMN_W);
  });

  it("drops stored widths for columns that no longer exist, and out-of-range ones", () => {
    expect(normalizeWidths({ url: 400, madeUp: 100, method: "wide", status: 1 })).toEqual({
      url: 400,
      status: MIN_COLUMN_W,
    });
    expect(normalizeWidths(null)).toEqual({});
    expect(normalizeWidths("nope")).toEqual({});
  });
});
