export type JsonValue =
  | null
  | boolean
  | number
  | string
  | JsonValue[]
  | { [key: string]: JsonValue };

export interface Health {
  status: string;
  internalId: string;
  version: string;
  runtime: string;
  brand?: {
    productName?: string;
    internalId?: string;
    defaultLocale?: string;
  };
}

export interface Stats {
  workflows: number;
  executions: number;
  running: number;
  waitingApproval: number;
  succeeded: number;
  failed: number;
}

export interface WorkflowSummary {
  id: string;
  name: string;
  version: string;
  description?: string | null;
  published: boolean;
  createdAt: string;
  updatedAt: string;
}

export interface ApprovalRequest {
  id: string;
  nodeId: string;
  graphId: string;
  title: string;
  assignees: JsonValue[];
  onTimeout: string;
  requestedAt: string;
}

export interface ExecutionError {
  code?: string;
  message: string;
  nodeId?: string | null;
}

export interface ExecutionSummary {
  id: string;
  workflowId: string;
  workflowVersion: string;
  status: string;
  startedAt: string;
  updatedAt: string;
  waiting?: ApprovalRequest | null;
  output?: JsonValue;
  error?: ExecutionError | null;
}

export interface ExecutionState extends ExecutionSummary {
  input: JsonValue;
  variables: Record<string, JsonValue>;
  nodeOutputs: Record<string, JsonValue>;
  stepCount?: number;
  tenantId?: string;
}

export interface ExecutionDetail {
  summary: ExecutionSummary;
  state: ExecutionState;
}

export interface ExecutionEvent {
  id: string;
  executionId: string;
  sequence: number;
  kind: string;
  at: string;
  nodeId?: string | null;
  graphId?: string | null;
  attempt?: number | null;
  data: JsonValue;
  message?: string | null;
}

export interface ApprovalSummary {
  executionId: string;
  workflowId: string;
  workflowVersion: string;
  startedAt: string;
  request: ApprovalRequest;
}

export interface Artifact {
  id: string;
  executionId: string;
  name: string;
  kind: string;
  mimeType: string;
  size: number;
  contentJson: string;
  createdAt: string;
}

export interface ArtifactDetail {
  artifact: Artifact;
  content: JsonValue;
}

export interface AgentSummary {
  id: string;
  name: string;
  latestVersion: string;
  status: string;
  createdAt: string;
  updatedAt: string;
}

export interface AgentSessionSummary {
  id: string;
  agentId: string;
  agentVersion: string;
  status: string;
  depth: number;
  steps: number;
  toolCalls: number;
  costUsd: number;
  createdAt: string;
  updatedAt: string;
}

export interface SkillSummary {
  id: string;
  name: string;
  latestVersion: string;
  license: string | null;
  status: string;
  updatedAt: string;
}

export interface SkillFileMeta {
  path: string;
  size: number;
  sha256: string;
}

export interface SkillDetail {
  skill: {
    id: string;
    name: string;
    version: string;
    license: string | null;
    status: string;
    contentHash: string;
  };
  summary: {
    version: string;
    license: string | null;
    status: string;
    contentHash: string;
    entry: string;
  };
  files: SkillFileMeta[];
  skillMd: string;
}

export interface PluginSummary {
  id: string;
  name: string;
  latestVersion: string;
  status: string;
  installedVersion?: string | null;
  previousVersion?: string | null;
  trust: string;
  license: string;
  createdAt: string;
  updatedAt: string;
}

export interface MarketplaceSourceKey {
  keyId: string;
  label: string;
  validFrom?: string | null;
  validUntil?: string | null;
}

export interface MarketplaceSource {
  id: string;
  label: string;
  indexUri: string;
  enabled: boolean;
  lastSequence: number;
  lastRefreshAt?: string | null;
  createdAt: string;
  updatedAt: string;
  keys: MarketplaceSourceKey[];
  packageCount: number;
  approvedCount: number;
  withdrawnCount: number;
}

export interface MarketplacePublisherKey extends MarketplaceSourceKey {
  revokedAt?: string | null;
  createdAt: string;
}

export interface MarketplacePublisher {
  id: string;
  name: string;
  status: string;
  createdAt: string;
  updatedAt: string;
  keys: MarketplacePublisherKey[];
}

export interface RemoteMarketplacePackage {
  sourceId: string;
  pluginId: string;
  version: string;
  publisherId: string;
  signerKeyId: string;
  packageSha256: string;
  reviewStatus: string;
  sequence: number;
  publishedAt?: string | null;
  withdrawnAt?: string | null;
  revocationReason?: string | null;
  provenance?: {
    kind: string;
    statementPath: string;
    statementSha256: string;
    bundlePath: string;
    bundleSha256: string;
  } | null;
  oci?: {
    repository: string;
    digest: string;
  } | null;
}

export interface VulnerabilityReport {
  schemaVersion: string;
  status: "disabled" | "clean" | "vulnerable";
  source: string;
  checkedAt?: string;
  databaseGeneratedAt?: string | null;
  package: { ecosystem: string; name: string; version: string };
  query: { ecosystem?: string | null; name: string; version: string };
  vulnerabilities: {
    id: string;
    summary?: string | null;
    modified?: string | null;
    aliases?: string[];
  }[];
}

export interface MarketplaceEvent {
  sequence: number;
  id: string;
  at: string;
  pluginId?: string | null;
  version?: string | null;
  kind: string;
  detail: JsonValue;
}

export interface McpOverview {
  endpoint: string;
  resourceMetadata: string;
  protocols: string[];
  sandbox: string;
  runtimes: Record<string, string | null>;
  stats: {
    pendingApprovals: number;
    prompts: number;
    publishedSkills: number;
    requests: number;
    resources: number;
    servers: number;
    tokens: number;
    tools: number;
  };
}

export interface McpTool {
  name: string;
  title?: string;
  description?: string;
}

export interface McpCatalog {
  tools: McpTool[];
  resources: { uri: string; name: string; description?: string }[];
  prompts: { name: string; title?: string; description?: string }[];
}

export interface McpServer {
  id: string;
  name: string;
  transport: string;
  command: string[];
  url?: string | null;
  allowedTools: string[];
  createdAt: string;
  updatedAt: string;
}

export interface McpToken {
  id: string;
  name: string;
  scopes: string[];
  tenantId: string;
  createdAt: string;
  revokedAt?: string | null;
}

export interface McpTokenCreateResult {
  token: string;
  record: McpToken;
}

export type RuntimeStatus = "connecting" | "online" | "offline" | "degraded";

export interface RuntimeState {
  status: RuntimeStatus;
  health?: Health;
  error?: string;
  checkedAt: string;
}

export interface RuntimeApiErrorShape {
  status: number;
  code: string;
  message: string;
  details?: JsonValue;
}
