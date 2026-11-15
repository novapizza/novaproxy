import { beforeEach, describe, expect, it, vi } from "vitest";

// Mock the Tauri boundary so session.ts runs in plain Node.
vi.mock("@tauri-apps/plugin-dialog", () => ({
  save: vi.fn(),
  open: vi.fn(),
}));
vi.mock("./api", () => ({
  api: { writeFile: vi.fn(), readFile: vi.fn(), exportFlows: vi.fn() },
}));

import { open, save } from "@tauri-apps/plugin-dialog";
import { api, type Flow } from "./api";
import { exportHar, exportSession, importSession } from "./session";

const saveMock = save as unknown as ReturnType<typeof vi.fn>;
const openMock = open as unknown as ReturnType<typeof vi.fn>;
const writeMock = api.writeFile as unknown as ReturnType<typeof vi.fn>;
const readMock = api.readFile as unknown as ReturnType<typeof vi.fn>;
const exportMock = api.exportFlows as unknown as ReturnType<typeof vi.fn>;

function mkFlow(over: Partial<Flow> = {}): Flow {
  return {
    id: "f0",
    seq: 0,
    method: "GET",
    scheme: "https",
    host: "example.com",
    path: "/",
    url: "https://example.com/search?q=hello&n=2",
    client_addr: "127.0.0.1:1",
    pid: null,
    process: null,
    http_version: "HTTP/1.1",
    state: "Completed",
    status: 200,
    request_headers: [{ name: "accept", value: "*/*" }],
    response_headers: [{ name: "content-type", value: "application/json" }],
    request_body: null,
    response_body: null,
    request_size: 0,
    response_size: 42,
    content_type: "application/json",
    started_at: 0,
    duration_ms: 123,
    error: null,
    resent: false,
    mapped_from: null,
    ...over,
  } as Flow;
}

beforeEach(() => {
  vi.clearAllMocks();
});

describe("exportSession", () => {
  it("hands the ids and the UI-only flows to the backend, never redacted", async () => {
    saveMock.mockResolvedValue("/tmp/s.nova");
    exportMock.mockResolvedValue(2);
    const imported = mkFlow({ id: "i1" });
    expect(await exportSession({ ids: ["f1", "i1"], extra: [imported] })).toBe(2);
    expect(exportMock).toHaveBeenCalledWith("/tmp/s.nova", "nova", ["f1", "i1"], [imported], false);
    expect(writeMock).not.toHaveBeenCalled();
  });

  it("returns null and exports nothing when the dialog is cancelled", async () => {
    saveMock.mockResolvedValue(null);
    expect(await exportSession({ ids: ["f1"], extra: [] })).toBeNull();
    expect(exportMock).not.toHaveBeenCalled();
  });
});

describe("importSession", () => {
  it("reads flows from a { flows: [...] } document", async () => {
    openMock.mockResolvedValue("/tmp/s.nova");
    readMock.mockResolvedValue(JSON.stringify({ flows: [mkFlow({ id: "x" })] }));
    const flows = await importSession();
    expect(flows?.map((f) => f.id)).toEqual(["x"]);
  });

  it("accepts a bare array document", async () => {
    openMock.mockResolvedValue("/tmp/s.nova");
    readMock.mockResolvedValue(JSON.stringify([mkFlow({ id: "y" })]));
    const flows = await importSession();
    expect(flows?.map((f) => f.id)).toEqual(["y"]);
  });

  it("unwraps the first path when the dialog returns an array", async () => {
    openMock.mockResolvedValue(["/tmp/a.nova", "/tmp/b.nova"]);
    readMock.mockResolvedValue(JSON.stringify({ flows: [] }));
    await importSession();
    expect(readMock).toHaveBeenCalledWith("/tmp/a.nova");
  });

  it("returns null when cancelled", async () => {
    openMock.mockResolvedValue(null);
    expect(await importSession()).toBeNull();
    expect(readMock).not.toHaveBeenCalled();
  });
});

describe("exportHar", () => {
  it("passes the redaction choice through", async () => {
    saveMock.mockResolvedValue("/tmp/out.har");
    exportMock.mockResolvedValue(1);
    expect(await exportHar({ ids: ["f0"], extra: [] }, true)).toBe(1);
    expect(exportMock).toHaveBeenCalledWith("/tmp/out.har", "har", ["f0"], [], true);
    await exportHar({ ids: ["f0"], extra: [] }, false);
    expect(exportMock).toHaveBeenLastCalledWith("/tmp/out.har", "har", ["f0"], [], false);
  });

  it("returns null without exporting when cancelled", async () => {
    saveMock.mockResolvedValue(null);
    expect(await exportHar({ ids: ["f0"], extra: [] }, true)).toBeNull();
    expect(exportMock).not.toHaveBeenCalled();
  });
});
