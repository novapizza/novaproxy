import { describe, expect, it } from "vitest";
import {
  COLUMNS,
  COLUMN_ORDER,
  DEFAULT_COLUMNS,
  gridTemplate,
  minTableWidth,
  normalizeColumns,
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
