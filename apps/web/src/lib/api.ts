import type {
  AgentApproval,
  AuthSession,
  AgentDetail,
  AgentReplayResult,
  AgentRuntimeInfo,
  AgentSessionDetail,
  AgentSessionSummary,
  AgentSummary,
  AgentToolCatalog,
  AgentVersionDetail,
  AgentVersionSummary,
  Artifact,
  ArtifactDetail,
  ApprovalSummary,
  Connection,
  ConnectorAuditEntry,
  ConnectorDefinition,
  EnterpriseAuditEntry,
  EnterpriseBackup,
  EnterpriseMember,
  EnterpriseOauthOverview,
  EnterpriseOidc,
  EnterpriseQuota,
  EnterpriseRestoreResult,
  EnterpriseRole,
  EnterpriseTenant,
  EnterpriseTenantDetail,
  EnterpriseTenantWorkflowsResponse,
  EnterpriseToken,
  EnterpriseWorker,
  EnterpriseWorkerJob,
  EnterpriseWorkerJobsResponse,
  EnterpriseWorkerTokenResponse,
  ExecutionDetail,
  ExecutionEvent,
  ExecutionSummary,
  Health,
  ImportedSkill,
  McpApproval,
  McpAuditEntry,
  McpCatalog,
  McpOverview,
  McpProbe,
  McpServer,
  McpToken,
  McpToolResult,
  OauthClient,
  OauthConsentRequest,
  OauthTokenSummary,
  MarketDetail,
  MarketSummary,
  NodeCatalog,
  PluginDetail,
  PluginInspection,
  PluginSigningKey,
  PluginSummary,
  PluginTrustedKey,
  PluginVersionDetail,
  SkillDetail,
  SkillList,
  SkillRunDetail,
  SkillRunResult,
  SkillRunSummary,
  SkillShow,
  SkillTestResult,
  Stats,
  ValidationResult,
  VersionSummary,
  WorkflowDetail,
  WorkflowIR,
  WorkflowSummary,
  JsonValue,
} from "./types";

const API_BASE = "/api";
export const ENTERPRISE_TOKEN_KEY = "hpa.enterprise.token";

export function readEnterpriseToken(): string {
  try {
    return window.localStorage.getItem(ENTERPRISE_TOKEN_KEY)?.trim() ?? "";
  } catch {
    return "";
  }
}

export function writeEnterpriseToken(token: string) {
  const value = token.trim();
  if (value) {
    window.localStorage.setItem(ENTERPRISE_TOKEN_KEY, value);
  } else {
    window.localStorage.removeItem(ENTERPRISE_TOKEN_KEY);
  }
}

export function clearEnterpriseToken() {
  window.localStorage.removeItem(ENTERPRISE_TOKEN_KEY);
}

function applyEnterpriseAuth(headers: Headers) {
  const token = readEnterpriseToken();
  if (token) {
    headers.set("authorization", `Bearer ${token}`);
  }
}

let csrfToken: string | null = null;
let unauthorizedHandler: (() => void) | null = null;

export function setCsrfToken(token: string | null) {
  csrfToken = token && token.trim() ? token.trim() : null;
}

export function setUnauthorizedHandler(handler: (() => void) | null) {
  unauthorizedHandler = handler;
}

function applyCsrf(headers: Headers, method: string) {
  if (!csrfToken) return;
  const verb = method.toUpperCase();
  if (verb === "GET" || verb === "HEAD") return;
  headers.set("X-HPA-CSRF", csrfToken);
}

export class ApiError extends Error {
  status: number;
  code: string;
  details?: unknown;

  constructor(status: number, code: string, message: string, details?: unknown) {
    super(message);
    this.name = "ApiError";
    this.status = status;
    this.code = code;
    this.details = details;
  }
}

interface ErrorBody {
  error?: {
    code?: string;
    message?: string;
    details?: unknown;
  };
}

async function request<T>(path: string, init?: RequestInit): Promise<T> {
  const headers = new Headers(init?.headers);
  if (init?.body && !headers.has("content-type")) {
    headers.set("content-type", "application/json");
  }
  applyEnterpriseAuth(headers);
  applyCsrf(headers, init?.method ?? "GET");
  let response: Response;
  try {
    response = await fetch(`${API_BASE}${path}`, { ...init, headers });
  } catch (error) {
    throw new ApiError(0, "NETWORK", error instanceof Error ? error.message : String(error));
  }
  const text = await response.text();
  let body: unknown = undefined;
  if (text) {
    try {
      body = JSON.parse(text) as unknown;
    } catch {
      body = text;
    }
  }
  if (!response.ok) {
    if (response.status === 401) {
      unauthorizedHandler?.();
    }
    const errorBody = body as ErrorBody | undefined;
    throw new ApiError(
      response.status,
      errorBody?.error?.code ?? "HTTP_ERROR",
      errorBody?.error?.message ?? response.statusText,
      errorBody?.error?.details,
    );
  }
  return body as T;
}

async function requestBlob(path: string): Promise<Blob> {
  const headers = new Headers();
  applyEnterpriseAuth(headers);
  let response: Response;
  try {
    response = await fetch(`${API_BASE}${path}`, { headers });
  } catch (error) {
    throw new ApiError(0, "NETWORK", error instanceof Error ? error.message : String(error));
  }
  if (!response.ok) {
    const text = await response.text();
    let errorBody: ErrorBody | undefined;
    if (text) {
      try {
        errorBody = JSON.parse(text) as ErrorBody;
      } catch {
        errorBody = undefined;
      }
    }
    throw new ApiError(
      response.status,
      errorBody?.error?.code ?? "HTTP_ERROR",
      errorBody?.error?.message ?? response.statusText,
      errorBody?.error?.details,
    );
  }
  return response.blob();
}

export const api = {
  health: () => request<Health>("/health"),
  stats: () => request<Stats>("/stats"),
  nodes: () => request<NodeCatalog>("/nodes"),

  authSession: () => request<AuthSession>("/auth/session"),
  authLogout: () =>
    request<{ loggedOut: boolean }>("/auth/logout", { method: "POST" }),

  listAgents: () => request<{ agents: AgentSummary[] }>("/agents"),
  createAgent: (config: JsonValue) =>
    request<{ inserted: boolean; summary: AgentSummary; version: AgentVersionSummary }>(
      "/agents",
      { method: "POST", body: JSON.stringify({ config }) },
    ),
  getAgent: (id: string) =>
    request<AgentDetail>(`/agents/${encodeURIComponent(id)}`),
  getAgentVersion: (id: string, version: string) =>
    request<AgentVersionDetail>(
      `/agents/${encodeURIComponent(id)}/versions/${encodeURIComponent(version)}`,
    ),
  publishAgent: (id: string, version?: string) =>
    request<{ published: boolean; version: AgentVersionSummary }>(
      `/agents/${encodeURIComponent(id)}/publish`,
      { method: "POST", body: JSON.stringify({ version }) },
    ),
  runAgent: (id: string, input: JsonValue, version?: string) =>
    request<AgentSessionDetail>(`/agents/${encodeURIComponent(id)}/run`, {
      method: "POST",
      body: JSON.stringify({ input, version }),
    }),
  listAgentSessions: (agentId?: string, limit = 30) =>
    request<{ sessions: AgentSessionSummary[] }>(
      `/agents/sessions?limit=${limit}${agentId ? `&agentId=${encodeURIComponent(agentId)}` : ""}`,
    ),
  getAgentSession: (id: string) =>
    request<AgentSessionDetail>(`/agents/sessions/${encodeURIComponent(id)}`),
  continueAgentSession: (id: string) =>
    request<AgentSessionDetail>(
      `/agents/sessions/${encodeURIComponent(id)}/continue`,
      { method: "POST" },
    ),
  cancelAgentSession: (id: string) =>
    request<AgentSessionDetail>(
      `/agents/sessions/${encodeURIComponent(id)}/cancel`,
      { method: "POST" },
    ),
  replayAgentSession: (id: string) =>
    request<AgentReplayResult>(
      `/agents/sessions/${encodeURIComponent(id)}/replay`,
      { method: "POST" },
    ),
  listAgentApprovals: (status = "pending") =>
    request<{ approvals: AgentApproval[] }>(
      `/agents/approvals?status=${encodeURIComponent(status)}`,
    ),
  decideAgentApproval: (id: string, decision: "approved" | "rejected") =>
    request<{ approval: AgentApproval; session: AgentSessionDetail }>(
      `/agents/approvals/${encodeURIComponent(id)}/decide`,
      { method: "POST", body: JSON.stringify({ decision }) },
    ),
  agentTools: () => request<AgentToolCatalog>("/agents/tools"),
  agentRuntime: () => request<AgentRuntimeInfo>("/agents/runtime"),

  listPlugins: () => request<{ plugins: PluginSummary[] }>("/plugins"),
  pluginKey: () => request<PluginSigningKey>("/plugins/key"),
  pluginKeys: () => request<{ keys: PluginTrustedKey[] }>("/plugins/keys"),
  trustPluginKey: (body: {
    keyId: string;
    publicKey: string;
    label?: string;
    trust?: "verified" | "community";
  }) =>
    request<{ keyId: string; trust: string }>("/plugins/keys", {
      method: "POST",
      body: JSON.stringify(body),
    }),
  inspectPlugin: (path: string, allowUnsigned?: boolean, allowUntrusted?: boolean) =>
    request<PluginInspection>("/plugins/inspect", {
      method: "POST",
      body: JSON.stringify({ path, allowUnsigned, allowUntrusted }),
    }),
  installPlugin: (path: string, allowUnsigned?: boolean, allowUntrusted?: boolean) =>
    request<PluginDetail>("/plugins/install", {
      method: "POST",
      body: JSON.stringify({ path, allowUnsigned, allowUntrusted }),
    }),
  samplePlugin: () =>
    request<PluginDetail>("/plugins/sample", { method: "POST" }),
  getPlugin: (id: string) =>
    request<PluginDetail>(`/plugins/${encodeURIComponent(id)}`),
  getPluginVersion: (id: string, version: string) =>
    request<PluginVersionDetail>(
      `/plugins/${encodeURIComponent(id)}/versions/${encodeURIComponent(version)}`,
    ),
  enablePlugin: (id: string) =>
    request<PluginDetail>(`/plugins/${encodeURIComponent(id)}/enable`, {
      method: "POST",
    }),
  disablePlugin: (id: string) =>
    request<PluginDetail>(`/plugins/${encodeURIComponent(id)}/disable`, {
      method: "POST",
    }),
  rollbackPlugin: (id: string) =>
    request<PluginDetail>(`/plugins/${encodeURIComponent(id)}/rollback`, {
      method: "POST",
    }),
  uninstallPlugin: (id: string, force = false) =>
    request<PluginDetail>(`/plugins/${encodeURIComponent(id)}/uninstall`, {
      method: "POST",
      body: JSON.stringify({ force }),
    }),

  listMarketplace: () =>
    request<{ plugins: MarketSummary[] }>("/marketplace"),
  publishMarketplace: (path: string) =>
    request<MarketDetail>("/marketplace/publish", {
      method: "POST",
      body: JSON.stringify({ path }),
    }),
  getMarketVersion: (id: string, version: string) =>
    request<MarketDetail>(
      `/marketplace/${encodeURIComponent(id)}/versions/${encodeURIComponent(version)}`,
    ),
  reviewMarketVersion: (
    id: string,
    version: string,
    status: "approved" | "rejected",
    note?: string,
  ) =>
    request<MarketDetail>(
      `/marketplace/${encodeURIComponent(id)}/versions/${encodeURIComponent(version)}/review`,
      { method: "POST", body: JSON.stringify({ status, note }) },
    ),
  withdrawMarketVersion: (id: string, version: string) =>
    request<MarketDetail>(
      `/marketplace/${encodeURIComponent(id)}/versions/${encodeURIComponent(version)}/withdraw`,
      { method: "POST" },
    ),
  rateMarketVersion: (id: string, version: string, rating: number, comment?: string) =>
    request<MarketDetail>(
      `/marketplace/${encodeURIComponent(id)}/versions/${encodeURIComponent(version)}/rate`,
      { method: "POST", body: JSON.stringify({ rating, comment }) },
    ),
  installMarketVersion: (id: string, version: string) =>
    request<PluginDetail>(
      `/marketplace/${encodeURIComponent(id)}/versions/${encodeURIComponent(version)}/install`,
      { method: "POST" },
    ),

  listConnectors: () =>
    request<{ connectors: ConnectorDefinition[] }>("/connectors"),
  getConnector: (id: string) =>
    request<ConnectorDefinition>(`/connectors/${encodeURIComponent(id)}`),
  sampleConnection: () =>
    request<Connection>("/connectors/sample", { method: "POST" }),
  listConnections: () => request<{ connections: Connection[] }>("/connections"),
  saveConnection: (body: {
    name: string;
    connectorId: string;
    config: JsonValue;
    credentialEnv?: string;
  }) =>
    request<Connection>("/connections", {
      method: "POST",
      body: JSON.stringify(body),
    }),
  deleteConnection: (id: string) =>
    request<{ deleted: boolean }>(`/connections/${encodeURIComponent(id)}`, {
      method: "DELETE",
    }),
  testConnection: (id: string) =>
    request<{ connectionId: string; test: JsonValue }>(
      `/connections/${encodeURIComponent(id)}/test`,
      { method: "POST" },
    ),
  invokeConnection: (id: string, operation: string, args: JsonValue) =>
    request<{ connectionId: string; operation: string; result: JsonValue }>(
      `/connections/${encodeURIComponent(id)}/invoke`,
      { method: "POST", body: JSON.stringify({ operation, arguments: args }) },
    ),
  connectorAudit: (limit = 50) =>
    request<{ entries: ConnectorAuditEntry[] }>(
      `/connector-audit?limit=${limit}`,
    ),

  listWorkflows: () => request<WorkflowSummary[]>("/workflows"),
  getWorkflow: (id: string) =>
    request<WorkflowDetail>(`/workflows/${encodeURIComponent(id)}`),
  saveWorkflow: (ir: WorkflowIR) =>
    request<WorkflowSummary>("/workflows", {
      method: "POST",
      body: JSON.stringify({ ir }),
    }),
  publishWorkflow: (id: string) =>
    request<WorkflowSummary>(`/workflows/${encodeURIComponent(id)}/publish`, {
      method: "POST",
    }),
  listVersions: (id: string) =>
    request<VersionSummary[]>(`/workflows/${encodeURIComponent(id)}/versions`),

  listSkills: () => request<SkillList>("/skills"),
  getSkill: (id: string) =>
    request<SkillShow>(`/skills/${encodeURIComponent(id)}`),
  importSkill: (path: string) =>
    request<ImportedSkill>("/skills/import", {
      method: "POST",
      body: JSON.stringify({ path }),
    }),
  getSkillVersion: (id: string, version: string) =>
    request<SkillDetail>(
      `/skills/${encodeURIComponent(id)}/versions/${encodeURIComponent(version)}`,
    ),
  publishSkill: (id: string, version: string) =>
    request<{ published: boolean }>(
      `/skills/${encodeURIComponent(id)}/versions/${encodeURIComponent(version)}/publish`,
      { method: "POST" },
    ),
  exportSkill: (id: string, version: string, path: string) =>
    request<{ exported: boolean; path: string }>(
      `/skills/${encodeURIComponent(id)}/versions/${encodeURIComponent(version)}/export`,
      { method: "POST", body: JSON.stringify({ path }) },
    ),
  runSkill: (
    id: string,
    version: string,
    input: JsonValue,
    script?: string,
  ) =>
    request<{ run: SkillRunResult; artifacts: unknown[] }>(
      `/skills/${encodeURIComponent(id)}/versions/${encodeURIComponent(version)}/run`,
      {
        method: "POST",
        body: JSON.stringify({ input, script }),
      },
    ),
  testSkill: (id: string, version: string, testCase?: string) =>
    request<{ passed: boolean; results: SkillTestResult[] }>(
      `/skills/${encodeURIComponent(id)}/versions/${encodeURIComponent(version)}/tests`,
      {
        method: "POST",
        body: JSON.stringify({ case: testCase }),
      },
    ),
  listSkillRuns: (id: string, limit = 20) =>
    request<{ runs: SkillRunSummary[] }>(
      `/skills/${encodeURIComponent(id)}/runs?limit=${limit}`,
    ),
  getSkillRun: (id: string) =>
    request<SkillRunDetail>(`/skill-runs/${encodeURIComponent(id)}`),

  mcpOverview: () => request<McpOverview>("/mcp/overview"),
  mcpCatalog: () => request<McpCatalog>("/mcp/catalog"),
  mcpCall: (name: string, args: JsonValue, requestState?: string) =>
    request<McpToolResult>("/mcp/call", {
      method: "POST",
      body: JSON.stringify({ name, arguments: args, requestState }),
    }),
  mcpReadResource: (uri: string) =>
    request<JsonValue>("/mcp/resource", {
      method: "POST",
      body: JSON.stringify({ uri }),
    }),
  mcpGetPrompt: (name: string, args: JsonValue) =>
    request<JsonValue>("/mcp/prompt", {
      method: "POST",
      body: JSON.stringify({ name, arguments: args }),
    }),
  mcpTokens: () => request<{ tokens: McpToken[] }>("/mcp/tokens"),
  mcpCreateToken: (name: string, scopes: string[]) =>
    request<{ token: string; record: McpToken }>("/mcp/tokens", {
      method: "POST",
      body: JSON.stringify({ name, scopes }),
    }),
  mcpRevokeToken: (id: string) =>
    request<{ revoked: boolean }>(`/mcp/tokens/${encodeURIComponent(id)}`, {
      method: "DELETE",
    }),
  mcpApprovals: (status?: string) =>
    request<{ approvals: McpApproval[] }>(
      `/mcp/approvals${status ? `?status=${encodeURIComponent(status)}` : ""}`,
    ),
  mcpDecideApproval: (id: string, decision: "approved" | "rejected") =>
    request<{ decided: boolean; status: string }>(
      `/mcp/approvals/${encodeURIComponent(id)}/decide`,
      { method: "POST", body: JSON.stringify({ decision }) },
    ),
  mcpAudit: (limit = 50) =>
    request<{ audit: McpAuditEntry[] }>(`/mcp/audit?limit=${limit}`),
  mcpServers: () => request<{ servers: McpServer[] }>("/mcp/servers"),
  mcpSaveServer: (server: {
    id?: string;
    name: string;
    transport: "stdio" | "http";
    command: string[];
    url?: string;
    allowedTools: string[];
    network: string[];
    credentialEnv?: string;
  }) =>
    request<{ server: McpServer }>("/mcp/servers", {
      method: "POST",
      body: JSON.stringify(server),
    }),
  mcpDeleteServer: (id: string) =>
    request<{ deleted: boolean }>(`/mcp/servers/${encodeURIComponent(id)}`, {
      method: "DELETE",
    }),
  mcpProbeServer: (id: string) =>
    request<McpProbe>(`/mcp/servers/${encodeURIComponent(id)}/probe`, {
      method: "POST",
    }),
  mcpCallServer: (id: string, tool: string, args: JsonValue) =>
    request<McpToolResult>(`/mcp/servers/${encodeURIComponent(id)}/call`, {
      method: "POST",
      body: JSON.stringify({ tool, arguments: args }),
    }),
  validate: (format: "yaml" | "json", content: string) =>
    request<ValidationResult>("/workflows/validate", {
      method: "POST",
      body: JSON.stringify({ format, content }),
    }),

  listExecutions: (limit = 50) =>
    request<ExecutionSummary[]>(`/executions?limit=${limit}`),
  getExecution: (id: string) =>
    request<ExecutionDetail>(`/executions/${encodeURIComponent(id)}`),
  listEvents: (id: string, after = 0) =>
    request<ExecutionEvent[]>(
      `/executions/${encodeURIComponent(id)}/events?after=${after}`,
    ),
  startExecution: (
    workflowId: string,
    input: JsonValue,
    env?: Record<string, string>,
  ) =>
    request<ExecutionDetail>(
      `/workflows/${encodeURIComponent(workflowId)}/executions`,
      {
        method: "POST",
        body: JSON.stringify({ input, env }),
      },
    ),
  decideApproval: (
    executionId: string,
    nodeId: string,
    decision: "approved" | "rejected" | "timeout",
    form: JsonValue,
  ) =>
    request<ExecutionDetail>(
      `/executions/${encodeURIComponent(executionId)}/approvals`,
      {
        method: "POST",
        body: JSON.stringify({ nodeId, decision, form }),
      },
    ),
  cancelExecution: (executionId: string) =>
    request<ExecutionDetail>(
      `/executions/${encodeURIComponent(executionId)}/cancel`,
      { method: "POST" },
    ),

  listApprovals: () => request<ApprovalSummary[]>("/approvals"),
  listArtifacts: (executionId?: string) =>
    request<Artifact[]>(
      `/artifacts${executionId ? `?executionId=${encodeURIComponent(executionId)}` : ""}`,
    ),
  getArtifact: (id: string) =>
    request<ArtifactDetail>(`/artifacts/${encodeURIComponent(id)}`),

  enterpriseTenants: () =>
    request<{ tenants: EnterpriseTenant[] }>("/enterprise/tenants"),
  enterpriseCreateTenant: (name: string) =>
    request<EnterpriseTenant>("/enterprise/tenants", {
      method: "POST",
      body: JSON.stringify({ name }),
    }),
  enterpriseTenant: (id: string) =>
    request<EnterpriseTenantDetail>(`/enterprise/tenants/${encodeURIComponent(id)}`),
  enterpriseDeleteTenant: (id: string) =>
    request<{ deleted: boolean }>(`/enterprise/tenants/${encodeURIComponent(id)}`, {
      method: "DELETE",
      body: JSON.stringify({ confirm: true }),
    }),
  enterpriseTokens: (tenantId?: string) =>
    request<{ tokens: EnterpriseToken[] }>(
      `/enterprise/tokens${tenantId ? `?tenantId=${encodeURIComponent(tenantId)}` : ""}`,
    ),
  enterpriseCreateToken: (body: {
    tenantId: string;
    name: string;
    role: EnterpriseRole;
    platformAdmin?: boolean;
    expiresInDays?: number;
  }) =>
    request<{ token: string; tokenInfo: EnterpriseToken }>("/enterprise/tokens", {
      method: "POST",
      body: JSON.stringify(body),
    }),
  enterpriseRevokeToken: (id: string) =>
    request<{ revoked: boolean }>(`/enterprise/tokens/${encodeURIComponent(id)}`, {
      method: "DELETE",
    }),
  enterpriseMembers: (tenantId?: string) =>
    request<{ members: EnterpriseMember[] }>(
      `/enterprise/members${tenantId ? `?tenantId=${encodeURIComponent(tenantId)}` : ""}`,
    ),
  enterpriseCreateMember: (body: {
    tenantId: string;
    subject: string;
    displayName?: string;
    email?: string;
    role: EnterpriseRole;
  }) =>
    request<EnterpriseMember>("/enterprise/members", {
      method: "POST",
      body: JSON.stringify(body),
    }),
  enterpriseUpdateMember: (
    id: string,
    body: {
      displayName?: string;
      email?: string;
      role?: EnterpriseRole;
      status?: "active" | "disabled";
    },
  ) =>
    request<EnterpriseMember>(`/enterprise/members/${encodeURIComponent(id)}`, {
      method: "PUT",
      body: JSON.stringify(body),
    }),
  enterpriseDeleteMember: (id: string) =>
    request<{ deleted: boolean }>(`/enterprise/members/${encodeURIComponent(id)}`, {
      method: "DELETE",
    }),
  enterpriseQuota: (tenantId: string) =>
    request<EnterpriseQuota>(`/enterprise/quotas/${encodeURIComponent(tenantId)}`),
  enterpriseSaveQuota: (tenantId: string, body: EnterpriseQuota) =>
    request<EnterpriseQuota>(`/enterprise/quotas/${encodeURIComponent(tenantId)}`, {
      method: "PUT",
      body: JSON.stringify(body),
    }),
  enterpriseWorkers: (tenantId?: string) =>
    request<{ workers: EnterpriseWorker[] }>(
      `/enterprise/workers${tenantId ? `?tenantId=${encodeURIComponent(tenantId)}` : ""}`,
    ),
  enterpriseRegisterWorker: (body: {
    tenantId: string;
    name: string;
    kind: string;
    metadata?: JsonValue;
    capabilities?: string[];
    maxConcurrency?: number;
  }) =>
    request<EnterpriseWorker>("/enterprise/workers", {
      method: "POST",
      body: JSON.stringify(body),
    }),
  enterpriseRotateWorkerToken: (id: string) =>
    request<EnterpriseWorkerTokenResponse>(
      `/enterprise/workers/${encodeURIComponent(id)}/token`,
      { method: "POST" },
    ),
  enterpriseWorkerJobs: (tenantId?: string, limit = 50) => {
    const params = new URLSearchParams();
    if (tenantId) params.set("tenantId", tenantId);
    params.set("limit", String(limit));
    return request<EnterpriseWorkerJobsResponse>(
      `/enterprise/worker-jobs?${params.toString()}`,
    );
  },
  enterpriseCreateWorkerJob: (body: {
    tenantId: string;
    workflowId: string;
    workerId?: string;
    input?: JsonValue;
    version?: string;
  }) =>
    request<EnterpriseWorkerJob>("/enterprise/worker-jobs", {
      method: "POST",
      body: JSON.stringify(body),
    }),
  enterpriseCancelWorkerJob: (id: string) =>
    request<EnterpriseWorkerJob>(
      `/enterprise/worker-jobs/${encodeURIComponent(id)}/cancel`,
      { method: "POST" },
    ),
  enterpriseTenantWorkflows: (tenantId: string) =>
    request<EnterpriseTenantWorkflowsResponse>(
      `/enterprise/tenants/${encodeURIComponent(tenantId)}/workflows`,
    ),
  enterpriseWorkerHeartbeat: (id: string) =>
    request<EnterpriseWorker>(`/enterprise/workers/${encodeURIComponent(id)}/heartbeat`, {
      method: "POST",
    }),
  enterpriseDisableWorker: (id: string) =>
    request<EnterpriseWorker>(`/enterprise/workers/${encodeURIComponent(id)}/disable`, {
      method: "POST",
    }),
  enterpriseBackups: (tenantId?: string) =>
    request<{ backups: EnterpriseBackup[] }>(
      `/enterprise/backups${tenantId ? `?tenantId=${encodeURIComponent(tenantId)}` : ""}`,
    ),
  enterpriseCreateBackup: (tenantId: string) =>
    request<EnterpriseBackup>(`/enterprise/tenants/${encodeURIComponent(tenantId)}/backups`, {
      method: "POST",
    }),
  enterpriseDeleteBackup: (id: string) =>
    request<{ deleted: boolean }>(`/enterprise/backups/${encodeURIComponent(id)}`, {
      method: "DELETE",
    }),
  enterpriseRestoreBackup: (id: string) =>
    request<EnterpriseRestoreResult>(`/enterprise/backups/${encodeURIComponent(id)}/restore`, {
      method: "POST",
    }),
  enterprisePurgeBackups: (retainDays: number, retainCount: number) =>
    request<{ deleted: number }>("/enterprise/backups/purge", {
      method: "POST",
      body: JSON.stringify({ retainDays, retainCount }),
    }),
  enterpriseAudit: (tenantId?: string, limit = 50) =>
    request<{ entries: EnterpriseAuditEntry[] }>(
      `/enterprise/audit?limit=${limit}${tenantId ? `&tenantId=${encodeURIComponent(tenantId)}` : ""}`,
    ),
  enterpriseExportAudit: (tenantId: string | undefined, format: "json" | "csv") =>
    requestBlob(
      `/enterprise/audit/export?format=${format}${tenantId ? `&tenantId=${encodeURIComponent(tenantId)}` : ""}`,
    ),
  enterpriseOauth: () => request<EnterpriseOauthOverview>("/enterprise/oauth/overview"),
  enterpriseOauthClients: () => request<{ clients: OauthClient[] }>("/enterprise/oauth/clients"),
  enterpriseOauthCreateClient: (body: {
    name: string;
    redirect_uris: string[];
    token_endpoint_auth_method: string;
    scope: string;
  }) =>
    request<OauthClient>("/enterprise/oauth/clients", {
      method: "POST",
      body: JSON.stringify(body),
    }),
  enterpriseOauthDisableClient: (id: string) =>
    request<{ disabled: string }>(`/enterprise/oauth/clients/${encodeURIComponent(id)}`, {
      method: "DELETE",
    }),
  enterpriseOauthRotateSecret: (id: string) =>
    request<{ clientId: string; clientSecret: string }>(
      `/enterprise/oauth/clients/${encodeURIComponent(id)}/secret`,
      { method: "POST" },
    ),
  enterpriseOauthTokens: () => request<{ tokens: OauthTokenSummary[] }>("/enterprise/oauth/tokens"),
  enterpriseOauthRevokeToken: (id: string) =>
    request<{ revoked: string }>(`/enterprise/oauth/tokens/${encodeURIComponent(id)}`, {
      method: "DELETE",
    }),
  enterpriseOauthRotateKey: () =>
    request<{ kid: string; alg: string; rotatedAt: number }>("/enterprise/oauth/keys/rotate", {
      method: "POST",
    }),
  oauthConsentRequest: (id: string) =>
    request<OauthConsentRequest>(`/oauth/requests/${encodeURIComponent(id)}`),
  oauthConsentDecision: (id: string, decision: "allow" | "deny", remember: boolean) =>
    request<{ redirect: string }>(`/oauth/requests/${encodeURIComponent(id)}/decision`, {
      method: "POST",
      body: JSON.stringify({ decision, remember }),
    }),
  enterpriseOidc: () => request<EnterpriseOidc>("/enterprise/oidc"),
  enterpriseSaveOidc: (body: EnterpriseOidc) =>
    request<EnterpriseOidc>("/enterprise/oidc", {
      method: "PUT",
      body: JSON.stringify(body),
    }),
};
