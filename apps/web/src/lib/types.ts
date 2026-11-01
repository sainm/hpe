export type JsonValue =
  | null
  | boolean
  | number
  | string
  | JsonValue[]
  | { [key: string]: JsonValue };

export interface WorkflowIO {
  name: string;
  type: string;
  required?: boolean;
  default?: JsonValue;
  description?: string;
}

export interface WorkflowVariable {
  name: string;
  type: string;
  initial?: JsonValue;
  description?: string;
}

export interface WorkflowEdge {
  id: string;
  source: string;
  sourcePort?: string;
  target: string;
  targetPort?: string;
}

export interface WorkflowGraph {
  nodes: WorkflowNode[];
  edges: WorkflowEdge[];
}

export interface RetryConfig {
  maxAttempts?: number;
  backoff?: {
    kind?: "constant" | "linear" | "exponential";
    baseMs?: number;
    maxMs?: number;
    jitter?: boolean;
  };
  retryOn?: string[];
  nonRetryable?: string[];
}

export interface ErrorPolicy {
  mode: "fail" | "skip" | "continue" | "fallback" | "compensate";
  fallbackPort?: string | null;
  compensationNodeId?: string | null;
}

export interface WorkflowNode {
  id: string;
  type: string;
  typeVersion: string;
  name?: string;
  description?: string;
  config: Record<string, JsonValue>;
  inputMapping?: Record<string, JsonValue>;
  outputSchema?: Record<string, JsonValue>;
  retry?: RetryConfig;
  timeoutMs?: number;
  errorPolicy?: ErrorPolicy;
  permissions?: Record<string, JsonValue>;
  ui?: Record<string, JsonValue> & { x?: number; y?: number; group?: string };
  body?: WorkflowGraph;
  branches?: { id: string; graph: WorkflowGraph }[];
}

export interface WorkflowSubflow {
  id: string;
  name: string;
  inputs?: WorkflowIO[];
  outputs?: WorkflowIO[];
  graph: WorkflowGraph;
}

export interface WorkflowIR {
  $schema?: string;
  schemaVersion: "hpa.dev/v1";
  id: string;
  name: string;
  description?: string;
  workflowVersion: string;
  inputs: WorkflowIO[];
  outputs: WorkflowIO[];
  variables: WorkflowVariable[];
  nodes: WorkflowNode[];
  edges: WorkflowEdge[];
  triggers?: Record<string, JsonValue>[];
  policies?: Record<string, JsonValue>;
  errorHandling?: Record<string, JsonValue>;
  retry?: RetryConfig;
  timeout?: { executionMs?: number };
  subflows?: WorkflowSubflow[];
  layout?: Record<string, JsonValue>;
  metadata?: Record<string, JsonValue>;
}

export type ExecutionStatus =
  | "pending"
  | "running"
  | "waiting_approval"
  | "succeeded"
  | "failed"
  | "cancelled";

export interface Brand {
  productName?: string;
  shortName?: string;
  tagline?: string;
  internalId?: string;
}

export interface Health {
  status: string;
  internalId: string;
  version: string;
  brand: Brand;
  runtime: string;
}

export interface NodeVersionDefinition {
  version: string;
  deprecated: boolean;
  configSchema: JsonValue;
}

export interface NodeDefinition {
  type: string;
  versions: NodeVersionDefinition[];
  defaultVersion: string;
  outputPorts: string[];
  permissionKinds: string[];
}

export interface NodeCatalog {
  registryVersion: string;
  nodes: NodeDefinition[];
}

export interface Stats {
  workflows: number;
  executions: number;
  running: number;
  waitingApproval: number;
  succeeded: number;
  failed: number;
}

export interface Issue {
  code: string;
  path: string;
  message: string;
  severity: "error" | "warning";
  fixHint?: string;
}

export interface ValidationReport {
  issues: Issue[];
}

export interface ValidationResult {
  valid: boolean;
  report: ValidationReport;
  canonical?: JsonValue;
  yaml?: string;
  nodeCount: number;
  edgeCount: number;
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

export interface WorkflowDetail {
  summary: WorkflowSummary;
  ir: WorkflowIR;
  yaml: string;
}

export interface VersionSummary {
  version: string;
  createdAt: string;
}

export interface ApprovalRequest {
  id: string;
  nodeId: string;
  graphId: string;
  title: string;
  formSchema?: JsonValue;
  assignees: JsonValue[];
  slaMs?: number | null;
  onTimeout: string;
  requestedAt: string;
}

export interface ExecutionError {
  code: string;
  message: string;
  retryable: boolean;
  details?: JsonValue;
}

export interface ExecutionSummary {
  id: string;
  workflowId: string;
  workflowVersion: string;
  status: ExecutionStatus;
  startedAt: string;
  updatedAt: string;
  waiting?: ApprovalRequest | null;
  output?: JsonValue;
  error?: ExecutionError | null;
}

export interface ExecutionFrame {
  frame: "graph" | "loop" | "parallel" | "subflow";
  [key: string]: unknown;
}

export interface ExecutionState {
  id: string;
  workflowId: string;
  workflowVersion: string;
  status: ExecutionStatus;
  input: JsonValue;
  variables: Record<string, JsonValue>;
  nodeOutputs: Record<string, JsonValue>;
  frames: ExecutionFrame[];
  waiting?: ApprovalRequest | null;
  output?: JsonValue;
  error?: ExecutionError | null;
  startedAt: string;
  updatedAt: string;
  stepCount: number;
  tenantId: string;
  env: Record<string, string>;
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

export type SkillEntryKind = "instructions" | "script" | "both";

export interface SkillSummary {
  id: string;
  name: string;
  latestVersion: string;
  license: string | null;
  status: string;
  updatedAt: string;
}

export interface SkillVersionSummary {
  version: string;
  license: string | null;
  status: string;
  contentHash: string;
  importedAt: string;
  publishedAt: string | null;
  scripts: number;
  tests: number;
  entry: SkillEntryKind;
  licenseAllowed: boolean;
}

export interface SkillScriptDefinition {
  id: string;
  path: string;
  runtime?: string;
  args: string[];
  timeoutMs?: number;
  network?: string;
  inputSchema?: JsonValue;
  outputSchema?: JsonValue;
  env?: Record<string, string>;
}

export interface SkillTestExpectation {
  exitCode?: number;
  outputSchema?: JsonValue;
  artifacts?: string[];
}

export interface SkillTestDefinition {
  name: string;
  script?: string;
  input: JsonValue;
  expect: SkillTestExpectation;
}

export interface SkillFileMeta {
  path: string;
  size: number;
  sha256: string;
}

export interface SkillReport {
  issues: Issue[];
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
  summary: SkillVersionSummary;
  manifest: JsonValue;
  skillMd: string;
  report: SkillReport;
  scripts: SkillScriptDefinition[];
  tests: SkillTestDefinition[];
  permissions: JsonValue;
  entry: SkillEntryKind;
  dependencies: JsonValue[];
  files: SkillFileMeta[];
  sourcePath?: string | null;
}

export interface SkillList {
  skills: SkillSummary[];
  runtimes: Record<string, string | null>;
  sandbox: string;
}

export interface SkillShow {
  summary: SkillSummary;
  versions: SkillVersionSummary[];
  runtimes: Record<string, string | null>;
  sandbox: string;
}

export interface ImportedSkill {
  inserted: boolean;
  warnings: Issue[];
  skill: SkillDetail;
}

export interface SkillArtifact {
  id: string;
  name: string;
  sha256: string;
  size: number;
  mimeType: string;
  createdAt?: string;
}

export interface SkillRunFailure {
  code: string;
  message: string;
}

export interface SkillRunResult {
  id: string;
  skillId: string;
  version: string;
  scriptId: string;
  status: "succeeded" | "failed" | "timed_out";
  exitCode: number | null;
  stdout: string;
  stderr: string;
  output: JsonValue | null;
  error?: SkillRunFailure | null;
  artifacts: SkillArtifact[];
  durationMs: number;
  sandbox: string;
  startedAt: string;
  finishedAt: string;
}

export interface SkillRunSummary {
  id: string;
  skillId: string;
  version: string;
  scriptId: string;
  status: string;
  exitCode: number | null;
  sandbox: string;
  durationMs: number;
  startedAt: string;
  finishedAt: string;
  error?: SkillRunFailure | null;
}

export interface SkillRunDetail {
  run: SkillRunSummary & {
    input?: JsonValue;
    output?: JsonValue | null;
    stdout?: string | null;
    stderr?: string | null;
  };
  artifacts: SkillArtifact[];
}

export interface SkillTestResult {
  name: string;
  passed: boolean;
  failures: string[];
  run?: SkillRunResult;
  error?: string;
}

export interface McpStats {
  pendingApprovals: number;
  publishedSkills: number;
  requests: number;
  tokens: number;
  servers: number;
  tools: number;
  resources: number;
  prompts: number;
}

export interface McpOverview {
  endpoint: string;
  resourceMetadata: string;
  protocols: string[];
  sandbox: string;
  runtimes: Record<string, string | null>;
  stats: McpStats;
}

export interface McpTool {
  name: string;
  title?: string;
  description?: string;
  inputSchema?: JsonValue;
  outputSchema?: JsonValue;
  annotations?: Record<string, boolean>;
  _meta?: Record<string, JsonValue>;
}

export interface McpResource {
  uri: string;
  name: string;
  description?: string;
  mimeType?: string;
}

export interface McpPrompt {
  name: string;
  title?: string;
  description?: string;
  arguments?: { name: string; description?: string; required?: boolean }[];
}

export interface McpCatalog {
  tools: McpTool[];
  resources: McpResource[];
  prompts: McpPrompt[];
}

export interface McpToken {
  id: string;
  name: string;
  scopes: string[];
  tenantId: string;
  createdAt: string;
  lastUsedAt?: string | null;
  revokedAt?: string | null;
}

export interface McpApproval {
  id: string;
  tokenId: string;
  toolName: string;
  arguments: JsonValue;
  risk: string;
  status: string;
  createdAt: string;
  expiresAt: string;
  decidedAt?: string | null;
  usedAt?: string | null;
}

export interface McpAuditEntry {
  id: string;
  at: string;
  tokenId?: string | null;
  method: string;
  toolName?: string | null;
  status: string;
  latencyMs: number;
  detail?: JsonValue;
}

export interface McpServer {
  id: string;
  name: string;
  transport: "stdio" | "http";
  command: string[];
  url?: string | null;
  allowedTools: string[];
  network: string[];
  credentialEnv?: string | null;
  createdAt: string;
  updatedAt: string;
}

export interface McpProbe {
  serverId: string;
  name: string;
  transport: string;
  era: string;
  serverInfo: JsonValue;
  tools: { name: string; description?: string }[];
  resources: McpResource[];
  prompts: McpPrompt[];
}

export type AgentStatus =
  | "running"
  | "interrupted"
  | "waiting_approval"
  | "succeeded"
  | "failed"
  | "budget_exhausted"
  | "timeout"
  | "cancelled";

export interface AgentSummary {
  id: string;
  name: string;
  latestVersion: string;
  status: string;
  createdAt: string;
  updatedAt: string;
}

export interface AgentVersionSummary {
  agentId: string;
  version: string;
  name: string;
  description?: string | null;
  contentHash: string;
  status: string;
  createdAt: string;
  publishedAt?: string | null;
}

export interface AgentDetail {
  summary: AgentSummary;
  versions: AgentVersionSummary[];
  latestConfig?: JsonValue | null;
  publishedVersion?: AgentVersionSummary | null;
}

export interface AgentVersionDetail extends AgentVersionSummary {
  config: JsonValue;
}

export interface AgentLimits {
  maxSteps: number;
  maxToolCalls: number;
  timeoutMs: number;
  maxTokens: number;
  maxCostUsd: number;
  maxHandoffs: number;
  maxDepth: number;
}

export interface AgentEvent {
  sessionId: string;
  sequence: number;
  at: string;
  kind: string;
  data: JsonValue;
}

export interface AgentSessionSummary {
  id: string;
  agentId: string;
  agentVersion: string;
  status: AgentStatus;
  depth: number;
  parentSessionId?: string | null;
  steps: number;
  toolCalls: number;
  handoffs: number;
  costUsd: number;
  createdAt: string;
  updatedAt: string;
  finishedAt?: string | null;
}

export interface AgentSessionDetail extends AgentSessionSummary {
  input: JsonValue;
  output?: JsonValue | null;
  error?: string | null;
  stopReason?: string | null;
  tokensIn: number;
  tokensOut: number;
  pendingApproval?: string | null;
  childSessionId?: string | null;
  planIndex: number;
  lastOutput?: JsonValue | null;
  events: AgentEvent[];
}

export interface AgentApproval {
  id: string;
  sessionId: string;
  toolName: string;
  arguments: JsonValue;
  risk: string;
  status: string;
  createdAt: string;
  expiresAt: string;
  decidedAt?: string | null;
  usedAt?: string | null;
}

export interface AgentToolEntry {
  tool: string;
  risk: string;
  kind: string;
  label?: string;
  inputSchema?: JsonValue;
  allowedTools?: string[];
  [key: string]: unknown;
}

export interface AgentToolCatalog {
  builtIn: AgentToolEntry[];
  skills: AgentToolEntry[];
  workflows: AgentToolEntry[];
  mcpServers: AgentToolEntry[];
  connectors: AgentToolEntry[];
}

export interface AgentRuntimeInfo {
  schemaVersion: string;
  sessionSchemaVersion: string;
  approvalTtlSeconds: number;
  defaultLimits: AgentLimits;
  planners: string[];
  risk: Record<string, string>;
  deterministicReplay: string;
}

export interface AgentReplayResult {
  deterministic: boolean;
  replayMode: string;
  matches: boolean;
  originalSessionId: string;
  originalStatus: AgentStatus;
  replayStatus?: AgentStatus;
  originalOutput?: JsonValue;
  replayOutput?: JsonValue;
  actions?: number;
  toolOutputs?: number;
  diffs?: JsonValue[];
  reason?: string;
}

export interface McpToolResult {
  resultType?: string;
  isError?: boolean;
  content?: { type: string; text?: string }[];
  structuredContent?: JsonValue;
  inputRequests?: JsonValue;
  requestState?: string;
  _meta?: Record<string, JsonValue>;
  error?: { code: number; message: string };
  [key: string]: unknown;
}

export type PluginTrust = "verified" | "community" | "untrusted";

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

export interface PluginPackage {
  pluginId: string;
  version: string;
  name: string;
  description?: string | null;
  license: string;
  publisher?: string | null;
  contentHash: string;
  packagePath: string;
  signature?: JsonValue;
  signerKeyId?: string | null;
  signerPublicKey?: string | null;
  trust: string;
  source: string;
  capabilities: JsonValue;
  createdAt: string;
}

export interface PluginCapability {
  pluginId: string;
  version: string;
  kind: string;
  capabilityId: string;
  capabilityVersion: string;
  path: string;
  status: string;
  config: JsonValue;
  permissions: JsonValue;
}

export interface SbomFile {
  path: string;
  size: number;
  sha256: string;
}

export interface SbomWarning {
  path: string;
  pattern: string;
}

export interface PluginSbom {
  schemaVersion: string;
  pluginId: string;
  version: string;
  license: string;
  contentHash: string;
  trust: string;
  signerKeyId?: string | null;
  publicKey?: string | null;
  localKeyId?: string | null;
  files: SbomFile[];
  capabilities: JsonValue;
  dependencies: JsonValue;
  permissions: JsonValue;
  gates: {
    license: boolean;
    signature: boolean;
    staticScan: string;
  };
  warnings: SbomWarning[];
}

export interface PluginEvent {
  id: string;
  at: string;
  pluginId?: string | null;
  version?: string | null;
  kind: string;
  detail: JsonValue;
}

export interface PluginDetail {
  plugin: PluginSummary;
  packages: PluginPackage[];
  installedPackage?: PluginPackage | null;
  capabilities: PluginCapability[];
  sbom?: PluginSbom | null;
  events: PluginEvent[];
}

export interface PluginVersionDetail {
  package: PluginPackage;
  manifest: JsonValue;
  capabilities: PluginCapability[];
  sbom?: PluginSbom | null;
}

export interface PluginInspection {
  valid: boolean;
  manifest: JsonValue;
  contentHash: string;
  trust: string;
  sbom: PluginSbom;
}

export interface PluginSigningKey {
  keyId: string;
  publicKey: string;
  algorithm: string;
  trust: string;
}

export interface PluginTrustedKey {
  keyId: string;
  publicKey: string;
  label: string;
  trust: string;
  createdAt: string;
}

export interface MarketplaceRating {
  average: number | null;
  count: number;
}

export interface MarketSummary {
  pluginId: string;
  version: string;
  name: string;
  description?: string | null;
  license: string;
  publisher?: string | null;
  trust: string;
  contentHash: string;
  reviewStatus: string;
  reviewNote?: string | null;
  submittedAt: string;
  reviewedAt?: string | null;
  withdrawnAt?: string | null;
  downloads: number;
  rating: MarketplaceRating;
}

export interface MarketplaceReview {
  id: string;
  pluginId: string;
  version: string;
  rating: number;
  comment?: string | null;
  createdAt: string;
}

export interface MarketDetail {
  pluginId: string;
  version: string;
  manifest: JsonValue;
  package: PluginPackage;
  reviewStatus: string;
  reviewNote?: string | null;
  submittedAt: string;
  reviewedAt?: string | null;
  withdrawnAt?: string | null;
  downloads: number;
  rating: MarketplaceRating;
  reviews: MarketplaceReview[];
  sbom?: PluginSbom | null;
}

export type ConnectorKind = "http" | "file";
export type ConnectorRisk = "read" | "write" | "destructive";

export interface ConnectorOperation {
  name: string;
  description?: string | null;
  risk: ConnectorRisk;
  inputSchema: JsonValue;
  outputSchema?: JsonValue | null;
  rateLimitPerMinute?: number | null;
  maxAttempts?: number | null;
  idempotency?: string | null;
  http?: { method: string; path: string } | null;
}

export interface ConnectorManifest {
  schemaVersion: string;
  id: string;
  version: string;
  name: string;
  kind: ConnectorKind;
  description?: string | null;
  docs?: string | null;
  auth: { kind: string; env?: string | null }[];
  operations: ConnectorOperation[];
  permissions: JsonValue[];
}

export interface ConnectorDefinition {
  connectorId: string;
  version: string;
  name: string;
  kind: ConnectorKind;
  status: string;
  pluginId?: string | null;
  pluginVersion?: string | null;
  createdAt: string;
  manifest: ConnectorManifest;
  configShape: JsonValue;
}

export interface Connection {
  id: string;
  name: string;
  connectorId: string;
  connectorVersion: string;
  config: JsonValue;
  credentialEnv?: string | null;
  hasCredential: boolean;
  status: string;
  operations: {
    name: string;
    description?: string | null;
    risk: string;
    inputSchema: JsonValue;
  }[];
  createdAt: string;
  updatedAt: string;
}

export interface ConnectorAuditEntry {
  id: string;
  at: string;
  connectionId: string;
  operation: string;
  status: string;
  latencyMs: number;
  detail: JsonValue;
}

export type EnterpriseRole =
  | "owner"
  | "admin"
  | "developer"
  | "publisher"
  | "operator"
  | "approver"
  | "auditor"
  | "viewer";

export interface EnterpriseTenant {
  id: string;
  name: string;
  status: string;
  createdAt: string;
  updatedAt: string;
}

export interface EnterpriseToken {
  id: string;
  name: string;
  tenantId: string;
  role: EnterpriseRole;
  platformAdmin: boolean;
  createdAt: string;
  lastUsedAt: string | null;
  revokedAt: string | null;
  expiresAt: string | null;
}

export type EnterpriseMemberStatus = "active" | "disabled";

export interface EnterpriseMember {
  id: string;
  tenantId: string;
  subject: string;
  displayName: string;
  email: string | null;
  role: EnterpriseRole;
  status: EnterpriseMemberStatus;
  createdAt: string;
  updatedAt: string;
}

export interface EnterpriseQuota {
  maxWorkflows: number;
  maxExecutionsPerDay: number;
  maxSkills: number;
  maxAgents: number;
  maxPlugins: number;
  maxConnections: number;
  maxConcurrentRuns: number;
}

export interface EnterpriseWorker {
  id: string;
  tenantId: string;
  name: string;
  kind: string;
  status: string;
  lastHeartbeatAt: string | null;
  registeredAt: string;
  metadata: JsonValue;
  capabilities: string[];
  maxConcurrency: number;
  runtimeVersion: string | null;
  workerToken?: string;
}

export type EnterpriseWorkerJobStatus =
  | "queued"
  | "leased"
  | "running"
  | "waiting_approval"
  | "succeeded"
  | "failed"
  | "cancelled";

export interface EnterpriseWorkerJob {
  id: string;
  tenantId: string;
  workerId: string | null;
  executionId: string;
  workflowId: string;
  workflowVersion: string;
  status: EnterpriseWorkerJobStatus;
  attempts: number;
  leaseExpiresAt: number | null;
  cancelRequested: boolean;
  error: string | null;
  createdAt: number;
  updatedAt: number;
}

export interface EnterpriseWorkerJobsResponse {
  jobs: EnterpriseWorkerJob[];
}

export interface EnterpriseWorkerTokenResponse {
  workerToken: string;
  tokenId: string;
}

export interface EnterpriseTenantWorkflowsResponse {
  workflows: WorkflowSummary[];
}

export interface EnterpriseBackup {
  id: string;
  tenantId: string;
  path: string;
  size: number;
  sha256: string;
  createdAt: string;
  status: string;
}

export interface EnterpriseAuditEntry {
  id: string;
  at: string;
  tenantId: string;
  actorId: string;
  actorType: string;
  action: string;
  resource: string;
  decision: string;
  reason: string;
  detail: JsonValue;
}

export interface EnterpriseOidc {
  issuer: string;
  audience: string;
  jwksUrl: string;
  network: string[];
  clientId?: string;
  authorizationEndpoint?: string;
  tokenEndpoint?: string;
  redirectUri?: string;
  scopes?: string[];
  adminSubjects?: string[];
  cookieSecure?: boolean;
  sessionTtlSeconds?: number;
}

export interface AuthPrincipal {
  tenantId: string;
  role: string;
  platformAdmin: boolean;
  subject: string;
  email?: string;
  displayName?: string;
}

export interface AuthSession {
  authenticated: boolean;
  ssoConfigured: boolean;
  principal?: AuthPrincipal;
  csrfToken: string | null;
  expiresAt?: number;
}

export interface EnterpriseTenantDetail {
  tenant: EnterpriseTenant;
  quotas: EnterpriseQuota;
  workers: EnterpriseWorker[];
  backups: EnterpriseBackup[];
}

export interface EnterpriseRestoreResult {
  restored: boolean;
  requiresRestart: boolean;
  message: string;
}
