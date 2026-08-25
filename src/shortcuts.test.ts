import { describe, expect, it } from "vitest";
import {
  formatChord,
  isMac,
  matchChord,
  parseChord,
  RESERVED,
  SHORTCUTS,
  shortcut,
  shortcutGroups,
  suppressedWhileTyping,
} from "./shortcuts";

const ev = (over: Partial<KeyboardEvent> & { key: string }) =>
  ({ metaKey: false, ctrlKey: false, shiftKey: false, altKey: false, ...over }) as KeyboardEvent;

describe("the registry", () => {
  it("has no duplicate ids", () => {
    const ids = SHORTCUTS.map((s) => s.id);
    expect(new Set(ids).size).toBe(ids.length);
  });

  it("has no duplicate chord within a scope", () => {
    const seen = new Map<string, string>();
    for (const s of SHORTCUTS) {
      const key = `${s.scope}:${s.chord}`;
      expect(seen.has(key), `${s.chord} is claimed twice in ${s.scope}: ${seen.get(key)} and ${s.id}`).toBe(false);
      seen.set(key, s.id);
    }
  });

  it("claims nothing the OS or the webview already owns", () => {
    for (const s of SHORTCUTS) {
      expect(RESERVED, `${s.id} takes a reserved chord`).not.toContain(s.chord);
    }
  });

  it("uses no Alt chord — on macOS Option+letter is a different character", () => {
    for (const s of SHORTCUTS) expect(s.chord).not.toContain("Alt");
  });

  it("groups rows in declaration order, keeping each group whole", () => {
    const groups = shortcutGroups();
    expect(groups.map((g) => g.group)).toEqual(["Global", "Filter", "Flows table", "Inspector", "Dialogs"]);
    expect(groups.flatMap((g) => g.items).length).toBe(SHORTCUTS.length);
  });

  it("names the chord that destroys data, so the dialog can warn", () => {
    expect(shortcut("clear").chord).toBe("Mod+K");
    expect(shortcut("clear").when).toMatch(/cannot be undone/);
  });

  it("marks the one chord the native menu owns", () => {
    expect(shortcut("shortcuts").native).toBe(true);
    expect(SHORTCUTS.filter((s) => s.native).map((s) => s.id)).toEqual(["shortcuts"]);
  });
});

describe("parseChord", () => {
  it("reads the modifiers and the key", () => {
    expect(parseChord("Mod+Shift+F")).toEqual({ mod: true, shift: true, key: "F" });
    expect(parseChord("ArrowDown")).toEqual({ mod: false, shift: false, key: "ArrowDown" });
    expect(parseChord("Mod+/")).toEqual({ mod: true, shift: false, key: "/" });
  });
});

describe("formatChord", () => {
  it("renders one badge per key, platform-appropriate", () => {
    expect(formatChord("Mod+Shift+F", true)).toEqual(["⌘", "⇧", "F"]);
    expect(formatChord("Mod+Shift+F", false)).toEqual(["Ctrl", "Shift", "F"]);
  });

  it("uses glyphs for the keys that have one", () => {
    expect(formatChord("Enter", true)).toEqual(["↵"]);
    expect(formatChord("Escape", true)).toEqual(["Esc"]);
    expect(formatChord("ArrowUp", true)).toEqual(["↑"]);
    expect(formatChord("PageDown", true)).toEqual(["PgDn"]);
  });

  it("keeps punctuation as itself and letters uppercase", () => {
    expect(formatChord("Mod+/", true)).toEqual(["⌘", "/"]);
    expect(formatChord("Mod+,", true)).toEqual(["⌘", ","]);
    expect(formatChord("Mod+0", true)).toEqual(["⌘", "0"]);
  });
});

describe("matchChord", () => {
  it("takes ⌘ on macOS and Ctrl elsewhere", () => {
    expect(matchChord(ev({ key: "p", metaKey: true }), "Mod+P", true)).toBe(true);
    expect(matchChord(ev({ key: "p", ctrlKey: true }), "Mod+P", true)).toBe(false);
    expect(matchChord(ev({ key: "p", ctrlKey: true }), "Mod+P", false)).toBe(true);
    expect(matchChord(ev({ key: "p", metaKey: true }), "Mod+P", false)).toBe(false);
  });

  it("never fires on macOS Ctrl chords — Ctrl+K is kill-line in every text field", () => {
    // The bug this whole module replaced: `metaKey || ctrlKey`.
    expect(matchChord(ev({ key: "k", ctrlKey: true }), "Mod+K", true)).toBe(false);
  });

  it("requires Shift exactly, in both directions", () => {
    expect(matchChord(ev({ key: "f", metaKey: true, shiftKey: true }), "Mod+Shift+F", true)).toBe(true);
    expect(matchChord(ev({ key: "f", metaKey: true }), "Mod+Shift+F", true)).toBe(false);
    expect(matchChord(ev({ key: "f", metaKey: true, shiftKey: true }), "Mod+F", true)).toBe(false);
  });

  it("refuses any event carrying Alt, so ⌥⌘F does not fire ⌘F", () => {
    expect(matchChord(ev({ key: "f", metaKey: true, altKey: true }), "Mod+F", true)).toBe(false);
  });

  it("compares letters case-insensitively — Shift changes e.key", () => {
    expect(matchChord(ev({ key: "C", metaKey: true, shiftKey: true }), "Mod+Shift+C", true)).toBe(true);
  });

  it("compares named keys exactly", () => {
    expect(matchChord(ev({ key: "ArrowDown" }), "ArrowDown", true)).toBe(true);
    expect(matchChord(ev({ key: "Down" }), "ArrowDown", true)).toBe(false);
  });
});

describe("suppressedWhileTyping", () => {
  const input = { tagName: "INPUT" } as unknown as EventTarget;
  const div = { tagName: "DIV" } as unknown as EventTarget;
  const editable = { tagName: "DIV", isContentEditable: true } as unknown as EventTarget;

  it("swallows single-key chords inside a field, so typing does not navigate", () => {
    expect(suppressedWhileTyping("j", input)).toBe(true);
    expect(suppressedWhileTyping("j", editable)).toBe(true);
    expect(suppressedWhileTyping("j", div)).toBe(false);
  });

  it("lets Escape and the arrows through — that is how you leave a field", () => {
    expect(suppressedWhileTyping("Escape", input)).toBe(false);
    expect(suppressedWhileTyping("ArrowDown", input)).toBe(false);
    expect(suppressedWhileTyping("Enter", input)).toBe(false);
  });

  it("never suppresses a modifier chord", () => {
    expect(suppressedWhileTyping("Mod+K", input)).toBe(false);
  });
});

describe("isMac", () => {
  it("reads the UA, so a test can hand it one", () => {
    expect(isMac("Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7)")).toBe(true);
    expect(isMac("Mozilla/5.0 (Windows NT 10.0; Win64; x64)")).toBe(false);
  });
});
