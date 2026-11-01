import type { WorkspaceConfiguration } from "vscode";

export type ConnectionMode = "auto" | "local" | "remote";
export type LocaleSetting = "auto" | "ja" | "en" | "zh";
export type SkillTarget = "project" | "personal";

export interface HpaConfig {
  mode: ConnectionMode;
  localEndpoint: string;
  mcpEndpoint: string;
  remoteEndpoint: string;
  remoteMcpEndpoint: string;
  locale: LocaleSetting;
  skillTarget: SkillTarget;
  personalSkillsPath: string;
  binary: string;
  startArgs: string[];
}

export const DEFAULT_LOCAL_ENDPOINT = "http://127.0.0.1:8787";

export function readConfig(config: WorkspaceConfiguration): HpaConfig {
  return {
    mode: normalizeMode(config.get<string>("mode")),
    localEndpoint: normalizeEndpoint(config.get<string>("local.endpoint")) || DEFAULT_LOCAL_ENDPOINT,
    mcpEndpoint: normalizeEndpoint(config.get<string>("mcp.endpoint")),
    remoteEndpoint: normalizeEndpoint(config.get<string>("remote.endpoint")),
    remoteMcpEndpoint: normalizeEndpoint(config.get<string>("remote.mcpEndpoint")),
    locale: normalizeLocale(config.get<string>("locale")),
    skillTarget: config.get<string>("skills.installTarget") === "personal" ? "personal" : "project",
    personalSkillsPath: config.get<string>("skills.personalPath")?.trim() ?? "",
    binary: config.get<string>("runtime.binary")?.trim() ?? "",
    startArgs: normalizeArgs(config.get<string[]>("runtime.startArgs")),
  };
}

export function normalizeMode(value: string | undefined): ConnectionMode {
  return value === "local" || value === "remote" ? value : "auto";
}

export function normalizeLocale(value: string | undefined): LocaleSetting {
  return value === "ja" || value === "en" || value === "zh" ? value : "auto";
}

export function normalizeEndpoint(value: string | undefined): string {
  return (value ?? "").trim().replace(/\/+$/, "");
}

export function normalizeArgs(value: string[] | undefined): string[] {
  if (!Array.isArray(value)) return ["serve"];
  const args = value.filter(
    (entry): entry is string => typeof entry === "string" && entry.trim() !== "",
  );
  return args.length > 0 ? args : ["serve"];
}

export function deriveMcpEndpoint(apiEndpoint: string): string {
  try {
    const url = new URL(apiEndpoint);
    return `${url.origin}/mcp`;
  } catch {
    return "";
  }
}

export interface ResolvedEndpoints {
  apiEndpoint: string;
  mcpEndpoint: string;
  isRemote: boolean;
  problem?: string;
}

export function resolveEndpoints(config: HpaConfig): ResolvedEndpoints {
  const prefersRemote =
    config.mode === "remote" ||
    (config.mode === "auto" && config.remoteEndpoint !== "" && config.localEndpoint === "");
  const apiEndpoint = prefersRemote ? config.remoteEndpoint : config.localEndpoint;
  const isRemote = prefersRemote;
  if (apiEndpoint === "") {
    return {
      apiEndpoint,
      mcpEndpoint: "",
      isRemote,
      problem: "endpoint-not-configured",
    };
  }
  const problem = validateEndpoint(apiEndpoint, isRemote);
  if (problem) {
    return { apiEndpoint, mcpEndpoint: "", isRemote, problem };
  }
  const configuredMcp = isRemote ? config.remoteMcpEndpoint : config.mcpEndpoint;
  const mcpEndpoint = configuredMcp || deriveMcpEndpoint(apiEndpoint);
  const mcpProblem = validateEndpoint(mcpEndpoint, isRemote);
  return {
    apiEndpoint,
    mcpEndpoint: mcpProblem ? "" : mcpEndpoint,
    isRemote,
    problem: mcpProblem,
  };
}

export function validateEndpoint(value: string, remote: boolean): string | undefined {
  let url: URL;
  try {
    url = new URL(value);
  } catch {
    return "endpoint-invalid";
  }
  if (url.protocol !== "http:" && url.protocol !== "https:") return "endpoint-invalid";
  if (url.username !== "" || url.password !== "") return "endpoint-userinfo";
  if (remote && url.protocol !== "https:") return "remote-requires-https";
  if (!remote && !isLoopbackHost(url.hostname)) return "local-requires-loopback";
  return undefined;
}

export function isLoopbackHost(hostname: string): boolean {
  const host = hostname.toLowerCase();
  if (host === "localhost" || host === "::1" || host === "[::1]") return true;
  if (host === "0.0.0.0") return false;
  return /^127(?:\.\d{1,3}){3}$/.test(host);
}
