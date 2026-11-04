import { describe, expect, it } from "vitest";
import { childPath, nodeValueText } from "./jsonCopy";

describe("nodeValueText", () => {
  it("copies a string without its quotes", () => {
    expect(nodeValueText("Bearer abc")).toBe("Bearer abc");
    expect(nodeValueText("")).toBe("");
  });

  it("copies other scalars as JSON writes them", () => {
    expect(nodeValueText(42)).toBe("42");
    expect(nodeValueText(1.5e-7)).toBe("1.5e-7");
    expect(nodeValueText(false)).toBe("false");
    expect(nodeValueText(null)).toBe("null");
  });

  it("copies a branch as indented JSON", () => {
    expect(nodeValueText({ a: [1, "x"] })).toBe('{\n  "a": [\n    1,\n    "x"\n  ]\n}');
    expect(nodeValueText([])).toBe("[]");
  });
});

describe("childPath", () => {
  it("joins identifier keys with dots and starts without one", () => {
    expect(childPath("", "data", false)).toBe("data");
    expect(childPath("data", "user_id", false)).toBe("data.user_id");
  });

  it("indexes array elements", () => {
    expect(childPath("data.items", "0", true)).toBe("data.items[0]");
    expect(childPath("", "3", true)).toBe("[3]");
  });

  it("brackets keys that cannot follow a dot", () => {
    expect(childPath("headers", "content-type", false)).toBe('headers["content-type"]');
    expect(childPath("", "1st", false)).toBe('["1st"]');
    expect(childPath("a", 'say "hi"', false)).toBe('a["say \\"hi\\""]');
  });
});
