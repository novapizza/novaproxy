import { beforeEach, describe, expect, it, vi } from "vitest";

// Mock the Tauri boundary so session.ts runs in plain Node.
vi.mock("@tauri-apps/plugin-dialog", () => ({
  save: vi.fn(),
  open: vi.fn(),
}));
vi.mock("./api", () => ({
  api: { writeFile: vi.fn(), readFile: vi.fn() },
}));

import { open, save } from "@tauri-apps/plugin-dialog";
import { api, type Flow } from "./api";
import { exportHar, exportSession, importSession } from "./session";

const saveMock = save as unknown as ReturnType<typeof vi.fn>;
const openMock = open as unknown as ReturnType<typeof vi.fn>;
const writeMock = api.writeFile as unknown as ReturnType<typeof vi.fn>;
const readMock = api.readFile as unknown as ReturnType<typeof vi.fn>;

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

/** Parse the JSON handed to api.writeFile on its most recent call. */
function lastWritten(): any {
  const [, contents] = writeMock.mock.calls.at(-1)!;
  return JSON.parse(contents as string);
}

beforeEach(() => {
  vi.clearAllMocks();
});

describe("exportSession", () => {
  it("writes a versioned session document and returns true", async () => {
    saveMock.mockResolvedValue("/tmp/s.nova");
    const ok = await exportSession([mkFlow()]);
    expect(ok).toBe(true);
    const doc = lastWritten();
    expect(doc.format).toBe("novaproxy-session");
    expect(doc.version).toBe(1);
    expect(doc.flows).toHaveLength(1);
  });

  it("returns false and writes nothing when the dialog is cancelled", async () => {
    saveMock.mockResolvedValue(null);
    expect(await exportSession([mkFlow()])).toBe(false);
    expect(writeMock).not.toHaveBeenCalled();
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
  beforeEach(() => saveMock.mockResolvedValue("/tmp/out.har"));

  it("produces a HAR 1.2 log with one entry per flow", async () => {
    const ok = await exportHar([mkFlow()]);
    expect(ok).toBe(true);
    const har = lastWritten();
    expect(har.log.version).toBe("1.2");
    expect(har.log.creator.name).toBe("NovaProxy");
    expect(har.log.entries).toHaveLength(1);
  });

  it("parses the URL query string into HAR queryString pairs", async () => {
    await exportHar([mkFlow()]);
    const entry = lastWritten().log.entries[0];
    expect(entry.request.queryString).toEqual([
      { name: "q", value: "hello" },
      { name: "n", value: "2" },
    ]);
  });

  it("tolerates an unparseable URL (empty query string, no throw)", async () => {
    await exportHar([mkFlow({ url: "::::not a url::::" })]);
    const entry = lastWritten().log.entries[0];
    expect(entry.request.queryString).toEqual([]);
  });

  it("includes postData only when the request has a text body", async () => {
    await exportHar([
      mkFlow({ id: "a" }),
      mkFlow({
        id: "b",
        request_body: { size: 3, truncated: false, media_type: "application/json", decoded_from: null, text: "{}", base64: null },
      }),
    ]);
    const [a, b] = lastWritten().log.entries;
    expect(a.request.postData).toBeUndefined();
    expect(b.request.postData).toEqual({ mimeType: "application/json", text: "{}" });
  });

  it("maps duration into timings.wait and response content", async () => {
    await exportHar([
      mkFlow({
        response_body: { size: 42, truncated: false, media_type: "application/json", decoded_from: null, text: "{\"ok\":true}", base64: null },
      }),
    ]);
    const entry = lastWritten().log.entries[0];
    expect(entry.time).toBe(123);
    expect(entry.timings.wait).toBe(123);
    expect(entry.response.content).toMatchObject({ size: 42, mimeType: "application/json", text: "{\"ok\":true}" });
  });

  it("returns false without writing when cancelled", async () => {
    saveMock.mockResolvedValue(null);
    expect(await exportHar([mkFlow()])).toBe(false);
    expect(writeMock).not.toHaveBeenCalled();
  });
});
