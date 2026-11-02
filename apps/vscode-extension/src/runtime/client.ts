import type {
  AgentSessionSummary,
  AgentSummary,
  ApprovalSummary,
  Artifact,
  ArtifactDetail,
  ExecutionDetail,
  ExecutionEvent,
  ExecutionSummary,
  Health,
  JsonValue,
  MarketplaceEvent,
  MarketplacePublisher,
  MarketplaceSource,
  McpCatalog,
  McpOverview,
  McpServer,
  McpTokenCreateResult,
  PluginSummary,
  RemoteMarketplacePackage,
  SkillDetail,
  SkillSummary,
  Stats,
  VulnerabilityReport,
  WorkflowSummary,
} from "./types";

export class RuntimeApiError extends Error {
  readonly status: number;
  readonly code: string;
  readonly details?: JsonValue;

  constructor(status: number, code: string, message: string, details?: JsonValue) {
    super(message);
    this.name = "RuntimeApiError";
    this.status = status;
    this.code = code;
    this.details = details;
  }
}

export type TokenProvider = () => Promise<string | undefined> | string | undefined;

export interface RuntimeClientOptions {
  baseUrl: string;
  tokenProvider?: TokenProvider;
  fetchImpl?: typeof fetch;
}

export class RuntimeClient {
  private baseUrl: string;
  private tokenProvider?: TokenProvider;
  private readonly fetchImpl: typeof fetch;

  constructor(options: RuntimeClientOptions) {
    this.baseUrl = normalizeBaseUrl(options.baseUrl);
    this.tokenProvider = options.tokenProvider;
    this.fetchImpl = options.fetchImpl ?? fetch;
  }

  get endpoint(): string {
    return this.baseUrl;
  }

  setBaseUrl(baseUrl: string): void {
    this.baseUrl = normalizeBaseUrl(baseUrl);
  }

  setTokenProvider(provider: TokenProvider | undefined): void {
    this.tokenProvider = provider;
  }

  url(path: string): string {
    if (/^https?:\/\//i.test(path)) return path;
    return `${this.baseUrl}${path.startsWith("/") ? path : `/${path}`}`;
  }

  health(): Promise<Health> {
    return this.request<Health>("/api/health", {}, 4000);
  }

  stats(): Promise<Stats> {
    return this.request<Stats>("/api/stats");
  }

  listWorkflows(): Promise<WorkflowSummary[]> {
    return this.request<WorkflowSummary[]>("/api/workflows");
  }

  runWorkflow(
    workflowId: string,
    input: JsonValue = {},
    version?: string,
  ): Promise<ExecutionDetail> {
    return this.request<ExecutionDetail>(
      `/api/workflows/${encodeURIComponent(workflowId)}/executions`,
      {
        method: "POST",
        body: JSON.stringify({ input, version }),
      },
    );
  }

  listExecutions(): Promise<ExecutionSummary[]> {
    return this.request<ExecutionSummary[]>("/api/executions");
  }

  getExecution(executionId: string): Promise<ExecutionDetail> {
    return this.request<ExecutionDetail>(`/api/executions/${encodeURIComponent(executionId)}`);
  }

  listEvents(executionId: string): Promise<ExecutionEvent[]> {
    return this.request<ExecutionEvent[]>(
      `/api/executions/${encodeURIComponent(executionId)}/events`,
    );
  }

  cancelExecution(executionId: string): Promise<ExecutionDetail> {
    return this.request<ExecutionDetail>(
      `/api/executions/${encodeURIComponent(executionId)}/cancel`,
      { method: "POST", body: "{}" },
    );
  }

  decideApproval(
    executionId: string,
    nodeId: string,
    decision: "approved" | "rejected",
    form: JsonValue = {},
  ): Promise<ExecutionDetail> {
    return this.request<ExecutionDetail>(
      `/api/executions/${encodeURIComponent(executionId)}/approvals`,
      {
        method: "POST",
        body: JSON.stringify({ nodeId, decision, form }),
      },
    );
  }

  listApprovals(): Promise<ApprovalSummary[]> {
    return this.request<ApprovalSummary[]>("/api/approvals");
  }

  listArtifacts(executionId?: string): Promise<Artifact[]> {
    const query = executionId ? `?executionId=${encodeURIComponent(executionId)}` : "";
    return this.request<Artifact[]>(`/api/artifacts${query}`);
  }

  getArtifact(artifactId: string): Promise<ArtifactDetail> {
    return this.request<ArtifactDetail>(`/api/artifacts/${encodeURIComponent(artifactId)}`);
  }

  listAgents(): Promise<AgentSummary[]> {
    return this.request<{ agents: AgentSummary[] }>("/api/agents").then((value) => value.agents);
  }

  listAgentSessions(): Promise<AgentSessionSummary[]> {
    return this.request<{ sessions: AgentSessionSummary[] }>("/api/agents/sessions").then(
      (value) => value.sessions,
    );
  }

  listSkills(): Promise<SkillSummary[]> {
    return this.request<{ skills: SkillSummary[] }>("/api/skills").then((value) => value.skills);
  }

  getSkill(skillId: string, version: string): Promise<SkillDetail> {
    return this.request<SkillDetail>(
      `/api/skills/${encodeURIComponent(skillId)}/versions/${encodeURIComponent(version)}`,
    );
  }

  exportSkill(skillId: string, version: string, path: string): Promise<{ exported: boolean; path: string }> {
    return this.request(`/api/skills/${encodeURIComponent(skillId)}/versions/${encodeURIComponent(version)}/export`, {
      method: "POST",
      body: JSON.stringify({ path }),
    });
  }

  listPlugins(): Promise<PluginSummary[]> {
    return this.request<{ plugins: PluginSummary[] }>("/api/plugins").then(
      (value) => value.plugins,
    );
  }

  listMarketplaceSources(): Promise<MarketplaceSource[]> {
    return this.request<{ sources: MarketplaceSource[] }>("/api/marketplace/sources").then(
      (value) => value.sources,
    );
  }

  addMarketplaceSource(body: {
    id: string;
    label: string;
    indexUri: string;
    keyId: string;
    publicKey: string;
    keyLabel: string;
  }): Promise<{ sourceId: string; keyId: string; status: string }> {
    return this.request("/api/marketplace/sources", {
      method: "POST",
      body: JSON.stringify(body),
    });
  }

  rotateMarketplaceSourceKey(
    sourceId: string,
    body: { keyId: string; publicKey: string; label: string },
  ): Promise<{ sourceId: string; keyId: string; rotatedAt: string }> {
    return this.request(
      `/api/marketplace/sources/${encodeURIComponent(sourceId)}/rotate-key`,
      { method: "POST", body: JSON.stringify(body) },
    );
  }

  refreshMarketplaceSource(sourceId: string): Promise<JsonValue> {
    return this.request(
      `/api/marketplace/sources/${encodeURIComponent(sourceId)}/refresh`,
      { method: "POST", body: "{}" },
    );
  }

  listMarketplacePublishers(): Promise<MarketplacePublisher[]> {
    return this.request<{ publishers: MarketplacePublisher[] }>(
      "/api/marketplace/publishers",
    ).then((value) => value.publishers);
  }

  addMarketplacePublisher(body: {
    id: string;
    name: string;
  }): Promise<{ publisherId: string; name: string; status: string }> {
    return this.request("/api/marketplace/publishers", {
      method: "POST",
      body: JSON.stringify(body),
    });
  }

  addMarketplacePublisherKey(
    publisherId: string,
    body: {
      keyId: string;
      publicKey: string;
      label: string;
      validFrom?: string;
      validUntil?: string;
    },
  ): Promise<JsonValue> {
    return this.request(
      `/api/marketplace/publishers/${encodeURIComponent(publisherId)}/keys`,
      { method: "POST", body: JSON.stringify(body) },
    );
  }

  revokeMarketplacePublisherKey(publisherId: string, keyId: string): Promise<JsonValue> {
    return this.request(
      `/api/marketplace/publishers/${encodeURIComponent(publisherId)}/keys`,
      { method: "DELETE", body: JSON.stringify({ keyId }) },
    );
  }

  listRemoteMarketplace(sourceId?: string): Promise<RemoteMarketplacePackage[]> {
    const query = sourceId ? `?source=${encodeURIComponent(sourceId)}` : "";
    return this.request<{ packages: RemoteMarketplacePackage[] }>(
      `/api/marketplace/remote${query}`,
    ).then((value) => value.packages);
  }

  installRemoteMarketplacePackage(
    sourceId: string,
    pluginId: string,
    version: string,
  ): Promise<JsonValue> {
    return this.request(
      `/api/marketplace/remote/${encodeURIComponent(sourceId)}/${encodeURIComponent(pluginId)}/versions/${encodeURIComponent(version)}/install`,
      { method: "POST", body: "{}" },
    );
  }

  marketplaceVulnerability(
    pluginId: string,
    version: string,
  ): Promise<VulnerabilityReport> {
    return this.request(
      `/api/marketplace/vulnerabilities/${encodeURIComponent(pluginId)}/${encodeURIComponent(version)}`,
    );
  }

  marketplaceEventCursor(): Promise<number> {
    return this.request<{ sequence: number }>(
      "/api/marketplace/events/latest",
    ).then((value) => value.sequence);
  }

  async streamMarketplaceEvents(
    since: number | null,
    onEvent: (event: MarketplaceEvent) => void,
    signal?: AbortSignal,
    onReady?: (sequence: number) => void,
  ): Promise<void> {
    const headers = new Headers({ accept: "text/event-stream" });
    if (since !== null) {
      headers.set("last-event-id", String(Math.max(0, since)));
    }
    const token = await this.tokenProvider?.();
    if (token) headers.set("authorization", `Bearer ${token}`);
    const response = await this.fetchImpl(
      this.url(
        since === null
          ? "/api/marketplace/events/stream"
          : `/api/marketplace/events/stream?since=${Math.max(0, since)}`,
      ),
      { headers, signal },
    ).catch((error: unknown) => {
      throw networkError(error);
    });
    if (!response.ok) {
      throw httpError(response.status, await response.text().catch(() => ""));
    }
    const readySequence = Number(response.headers.get("x-hpa-event-sequence"));
    if (Number.isSafeInteger(readySequence) && readySequence >= 0) {
      onReady?.(readySequence);
    }
    if (!response.body) throw new RuntimeApiError(0, "NETWORK", "response body is empty");
    const reader = response.body.getReader();
    const decoder = new TextDecoder();
    let buffer = "";
    try {
      while (true) {
        const { done, value } = await reader.read();
        if (done) break;
        buffer += decoder.decode(value, { stream: true });
        let boundary = buffer.indexOf("\n\n");
        while (boundary >= 0) {
          const frame = buffer.slice(0, boundary);
          buffer = buffer.slice(boundary + 2);
          const data = frame
            .split("\n")
            .filter((line) => line.startsWith("data:"))
            .map((line) => line.slice(5).trimStart())
            .join("\n");
          if (data) {
            try {
              onEvent(JSON.parse(data) as MarketplaceEvent);
            } catch {
              throw new RuntimeApiError(0, "INVALID_JSON", "invalid marketplace event payload");
            }
          }
          boundary = buffer.indexOf("\n\n");
        }
      }
    } finally {
      reader.releaseLock();
    }
  }

  mcpOverview(): Promise<McpOverview> {
    return this.request<McpOverview>("/api/mcp/overview");
  }

  mcpCatalog(): Promise<McpCatalog> {
    return this.request<McpCatalog>("/api/mcp/catalog");
  }

  listMcpServers(): Promise<McpServer[]> {
    return this.request<{ servers: McpServer[] }>("/api/mcp/servers").then(
      (value) => value.servers,
    );
  }

  createMcpToken(name = "vscode-extension"): Promise<McpTokenCreateResult> {
    return this.request<McpTokenCreateResult>("/api/mcp/tokens", {
      method: "POST",
      body: JSON.stringify({ name, scopes: ["mcp:read", "mcp:execute"] }),
    });
  }

  revokeMcpToken(tokenId: string): Promise<{ revoked: boolean }> {
    return this.request<{ revoked: boolean }>(`/api/mcp/tokens/${encodeURIComponent(tokenId)}`, {
      method: "DELETE",
    });
  }

  async streamEvents(
    executionId: string,
    onEvent: (event: ExecutionEvent) => void,
    signal?: AbortSignal,
  ): Promise<void> {
    const headers = new Headers({ accept: "text/event-stream" });
    const token = await this.tokenProvider?.();
    if (token) headers.set("authorization", `Bearer ${token}`);
    const response = await this.fetchImpl(
      this.url(`/api/executions/${encodeURIComponent(executionId)}/events/stream`),
      { headers, signal },
    ).catch((error: unknown) => {
      throw networkError(error);
    });
    if (!response.ok) {
      const payload = await response.text().catch(() => "");
      throw httpError(response.status, payload);
    }
    if (!response.body) throw new RuntimeApiError(0, "NETWORK", "response body is empty");
    const reader = response.body.getReader();
    const decoder = new TextDecoder();
    let buffer = "";
    try {
      while (true) {
        const { done, value } = await reader.read();
        if (done) break;
        buffer += decoder.decode(value, { stream: true });
        let boundary = buffer.indexOf("\n\n");
        while (boundary >= 0) {
          const frame = buffer.slice(0, boundary);
          buffer = buffer.slice(boundary + 2);
          const data = frame
            .split("\n")
            .filter((line) => line.startsWith("data:"))
            .map((line) => line.slice(5).trimStart())
            .join("\n");
          if (data) {
            try {
              onEvent(JSON.parse(data) as ExecutionEvent);
            } catch {
              throw new RuntimeApiError(0, "INVALID_JSON", "invalid event payload");
            }
          }
          boundary = buffer.indexOf("\n\n");
        }
      }
    } finally {
      reader.releaseLock();
    }
  }

  private async request<T>(path: string, init: RequestInit = {}, timeoutMs = 15000): Promise<T> {
    const headers = new Headers(init.headers);
    headers.set("accept", "application/json");
    if (init.body !== undefined && !headers.has("content-type")) {
      headers.set("content-type", "application/json");
    }
    const token = await this.tokenProvider?.();
    if (token) headers.set("authorization", `Bearer ${token}`);
    const response = await this.fetchImpl(this.url(path), {
      ...init,
      headers,
      signal: init.signal ?? AbortSignal.timeout(timeoutMs),
    }).catch((error: unknown) => {
      throw networkError(error);
    });
    const text = await response.text();
    let payload: unknown = null;
    if (text.trim() !== "") {
      try {
        payload = JSON.parse(text);
      } catch {
        if (response.ok) {
          throw new RuntimeApiError(response.status, "INVALID_JSON", "invalid JSON response");
        }
      }
    }
    if (!response.ok) throw httpError(response.status, text, payload);
    return payload as T;
  }
}

function normalizeBaseUrl(value: string): string {
  return value.trim().replace(/\/+$/, "");
}

function networkError(error: unknown): RuntimeApiError {
  if (error instanceof RuntimeApiError) return error;
  const message = error instanceof Error ? error.message : String(error);
  return new RuntimeApiError(0, "NETWORK", message);
}

function httpError(status: number, text: string, payload?: unknown): RuntimeApiError {
  const error = (payload as { error?: { code?: string; message?: string; details?: JsonValue } } | null)
    ?.error;
  if (error?.code && error?.message) {
    return new RuntimeApiError(status, error.code, error.message, error.details);
  }
  return new RuntimeApiError(status, status === 401 ? "UNAUTHORIZED" : "HTTP_ERROR", text || `HTTP ${status}`);
}
