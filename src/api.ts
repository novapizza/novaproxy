import { invoke, Channel } from "@tauri-apps/api/core";
import type { Flow } from "./bindings/Flow";
import type { ProxyStatus } from "./bindings/ProxyStatus";
import type { CaStatus } from "./bindings/CaStatus";
import type { Rule } from "./bindings/Rule";
import type { RuleKind } from "./bindings/RuleKind";
import type { Interception } from "./bindings/Interception";
import type { Header } from "./bindings/Header";
import type { NetworkConditions } from "./bindings/NetworkConditions";
import type { BodyPreview } from "./bindings/BodyPreview";
import type { WsMessage } from "./bindings/WsMessage";
import type { TlsScope } from "./bindings/TlsScope";
import type { McpStatus } from "./bindings/McpStatus";
import type { HelperStatus } from "./bindings/HelperStatus";
import type { UpdateStatus } from "./bindings/UpdateStatus";
import type { UpdateProgress } from "./bindings/UpdateProgress";

/** Thin typed wrappers over the Tauri command surface. */
export const api = {
  /** Register the streaming channel that receives flow snapshots. */
  subscribeFlows: (channel: Channel<Flow>) => invoke<void>("subscribe_flows", { channel }),
  /** Register the streaming channel that receives captured WebSocket frames. */
  subscribeWs: (channel: Channel<WsMessage>) => invoke<void>("subscribe_ws", { channel }),
  proxyStatus: () => invoke<ProxyStatus>("proxy_status"),
  startProxy: (port?: number) => invoke<ProxyStatus>("start_proxy", { port }),
  stopProxy: () => invoke<ProxyStatus>("stop_proxy"),

  getRules: () => invoke<Rule[]>("get_rules"),
  setRules: (rules: Rule[]) => invoke<void>("set_rules", { rules }),

  getScript: () => invoke<string>("get_script"),
  setScript: (source: string, enabled: boolean) => invoke<void>("set_script", { source, enabled }),

  getNetworkConditions: () => invoke<NetworkConditions>("get_network_conditions"),
  setNetworkConditions: (net: NetworkConditions) =>
    invoke<void>("set_network_conditions", { net }),

  /** Drop every retained flow in the engine (paired with the UI's Clear). */
  clearFlows: () => invoke<void>("clear_flows"),

  /**
   * Every flow the engine still retains, newest first — bodies included.
   * The list keeps no body bytes, so exports read them from here.
   */
  retainedFlows: () => invoke<Flow[]>("retained_flows"),

  /** State of the MCP endpoint that exposes captured traffic to AI tooling. */
  mcpStatus: () => invoke<McpStatus>("mcp_status"),
  /** Start or stop that endpoint; the choice is remembered across launches. */
  setMcpEnabled: (enable: boolean, port?: number) =>
    invoke<McpStatus>("set_mcp_enabled", { enable, port }),

  getTlsScope: () => invoke<TlsScope>("get_tls_scope"),
  setTlsScope: (scope: TlsScope) => invoke<void>("set_tls_scope", { scope }),

  /**
   * Fetch a body the list is not holding: from the on-disk body store when the
   * preview reports `spilled`, otherwise from the flow the engine retains.
   * `mediaType`/`encoding` come from that preview so the stored bytes decode the
   * same way they did live.
   */
  readBody: (
    flowId: string,
    side: "request" | "response",
    mediaType: string | null,
    encoding: string | null,
  ) => invoke<BodyPreview>("read_body", { flowId, side, mediaType, encoding }),

  writeFile: (path: string, contents: string) => invoke<void>("write_file", { path, contents }),
  readFile: (path: string) => invoke<string>("read_file", { path }),

  subscribeBreakpoints: (channel: Channel<Interception>) =>
    invoke<void>("subscribe_breakpoints", { channel }),
  setBreakpoint: (armed: boolean, pattern?: string) =>
    invoke<void>("set_breakpoint", { armed, pattern }),
  resumeBreakpoint: (id: string, cont: boolean, headers: Header[]) =>
    invoke<void>("resume_breakpoint", { id, cont, headers }),

  setSystemProxy: (enable: boolean) => invoke<ProxyStatus>("set_system_proxy", { enable }),
  /**
   * Put back proxy settings left over from a session that ended uncleanly.
   * Offered rather than automatic: without the helper it raises the OS password
   * prompt, which the app must not do unasked.
   */
  restoreSystemProxy: () => invoke<ProxyStatus>("restore_system_proxy"),

  /** State of the macOS helper that applies proxy changes without a password. */
  helperStatus: () => invoke<HelperStatus>("helper_status"),
  /** Install it — one administrator prompt, then none. */
  installHelper: () => invoke<HelperStatus>("install_helper"),
  uninstallHelper: () => invoke<HelperStatus>("uninstall_helper"),

  resendFlow: (flow: Flow) => invoke<void>("resend_flow", { flow }),

  caStatus: () => invoke<CaStatus>("ca_status"),
  /**
   * Install & trust the CA. Defaults to the current user's trust domain, which
   * needs no admin password; `allUsers` opts into the machine-wide store.
   */
  installCa: (allUsers = false) => invoke<CaStatus>("install_ca", { allUsers }),
  uninstallCa: () => invoke<CaStatus>("uninstall_ca"),
  regenerateCa: () => invoke<CaStatus>("regenerate_ca"),

  /**
   * Ask the release endpoint whether a newer build exists. Resolves with
   * `configured: false` in a build that has no updater endpoint rather than
   * rejecting, since that is a normal state for a development tree.
   */
  checkUpdate: () => invoke<UpdateStatus>("check_update"),
  /**
   * Download and install the update the last check found, streaming progress.
   * On success the app relaunches, so nothing after this resolves.
   */
  installUpdate: (channel: Channel<UpdateProgress>) =>
    invoke<void>("install_update", { channel }),
};

export { Channel };
export type {
  BodyPreview,
  Flow,
  ProxyStatus,
  CaStatus,
  Rule,
  RuleKind,
  Interception,
  Header,
  NetworkConditions,
  WsMessage,
  TlsScope,
  McpStatus,
  HelperStatus,
  UpdateStatus,
  UpdateProgress,
};
