import { invoke, Channel } from "@tauri-apps/api/core";
import type { Flow } from "./bindings/Flow";
import type { ProxyStatus } from "./bindings/ProxyStatus";
import type { CaStatus } from "./bindings/CaStatus";
import type { Rule } from "./bindings/Rule";
import type { RuleKind } from "./bindings/RuleKind";
import type { Interception } from "./bindings/Interception";
import type { Header } from "./bindings/Header";
import type { NetworkConditions } from "./bindings/NetworkConditions";
import type { WsMessage } from "./bindings/WsMessage";
import type { TlsScope } from "./bindings/TlsScope";

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

  getTlsScope: () => invoke<TlsScope>("get_tls_scope"),
  setTlsScope: (scope: TlsScope) => invoke<void>("set_tls_scope", { scope }),

  writeFile: (path: string, contents: string) => invoke<void>("write_file", { path, contents }),
  readFile: (path: string) => invoke<string>("read_file", { path }),

  subscribeBreakpoints: (channel: Channel<Interception>) =>
    invoke<void>("subscribe_breakpoints", { channel }),
  setBreakpoint: (armed: boolean, pattern?: string) =>
    invoke<void>("set_breakpoint", { armed, pattern }),
  resumeBreakpoint: (id: string, cont: boolean, headers: Header[]) =>
    invoke<void>("resume_breakpoint", { id, cont, headers }),

  setSystemProxy: (enable: boolean) => invoke<ProxyStatus>("set_system_proxy", { enable }),
  resendFlow: (flow: Flow) => invoke<void>("resend_flow", { flow }),

  caStatus: () => invoke<CaStatus>("ca_status"),
  installCa: () => invoke<CaStatus>("install_ca"),
  uninstallCa: () => invoke<CaStatus>("uninstall_ca"),
  regenerateCa: () => invoke<CaStatus>("regenerate_ca"),
};

export { Channel };
export type {
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
};
