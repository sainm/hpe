#!/usr/bin/env node
// Two control-plane nodes sharing one PostgreSQL database. Proves the HA
// primitives that Phase 13 introduced: shared tokens, tenants, quotas,
// workflows/executions, worker leases and revocation.
//
// Usage:
//   HPA_POSTGRES_URL=postgresql://user@host:5432/empty_db node scripts/multi-node-e2e.mjs
//
// The database must be empty (the script applies migrations itself) and must not
// be shared with another run.

import { spawn } from "node:child_process";
import fs from "node:fs/promises";
import net from "node:net";
import os from "node:os";
import path from "node:path";

const root = path.resolve(new URL("..", import.meta.url).pathname);
const binary = process.env.HPA_BINARY || path.join(root, "target/debug/hpa");
const databaseUrl = process.env.HPA_POSTGRES_URL || process.env.DATABASE_URL;

if (!databaseUrl) {
  console.error("HPA_POSTGRES_URL or DATABASE_URL is required (point it at an empty database)");
  process.exit(2);
}

const summary = {
  database: databaseUrl.replace(/:[^:@/]+@/, ":***@"),
  nodes: {},
  checks: {},
  errors: [],
};

function fail(message) {
  summary.errors.push(message);
  console.error(`FAIL: ${message}`);
}

function freePort() {
  return new Promise((resolve, reject) => {
    const server = net.createServer();
    server.once("error", reject);
    server.listen(0, "127.0.0.1", () => {
      const { port } = server.address();
      server.close(() => resolve(port));
    });
  });
}

function run(command, args, { env = {}, capture = true } = {}) {
  return new Promise((resolve, reject) => {
    const child = spawn(command, args, {
      cwd: root,
      env: { ...process.env, ...env },
      stdio: capture ? ["ignore", "pipe", "pipe"] : ["ignore", "inherit", "inherit"],
    });
    let stdout = "";
    let stderr = "";
    child.stdout?.on("data", (chunk) => {
      stdout += chunk;
    });
    child.stderr?.on("data", (chunk) => {
      stderr += chunk;
    });
    child.on("error", reject);
    child.on("close", (code) => {
      if (code !== 0) reject(new Error(`${command} ${args.join(" ")} failed (${code})\n${stderr}`));
      else resolve({ stdout, stderr });
    });
  });
}

async function node(label, dir) {
  const port = await freePort();
  const child = spawn(binary, ["--data-dir", dir, "serve", "--port", String(port)], {
    cwd: root,
    env: {
      ...process.env,
      HPA_POSTGRES_URL: databaseUrl,
      HPA_POSTGRES_RUNTIME: "1",
      HPA_ENTERPRISE_MODE: "required",
      HPA_SCHEDULER_TICK_MS: "200",
      HPA_NODE_ID: label,
    },
    stdio: ["ignore", "pipe", "pipe"],
  });
  let logs = "";
  child.stdout.on("data", (chunk) => {
    logs += chunk;
  });
  child.stderr.on("data", (chunk) => {
    logs += chunk;
  });
  const client = { base: `http://127.0.0.1:${port}` };
  const ready = await waitHealthy(client.base, () => child.exitCode, () => logs);
  if (!ready) {
    child.kill();
    throw new Error(`node ${label} did not become healthy\n${logs}`);
  }
  summary.nodes[label] = client.base;
  return { label, base: client.base, child, logs: () => logs };
}

async function waitHealthy(base, exitCode, logs) {
  for (let attempt = 0; attempt < 400; attempt += 1) {
    if (exitCode() !== null) return false;
    try {
      const response = await fetch(`${base}/api/health`);
      if (response.ok) return true;
    } catch {
      // still starting
    }
    await new Promise((resolve) => setTimeout(resolve, 50));
  }
  void logs;
  return false;
}

async function api(base, method, path, { token, body } = {}) {
  const headers = {};
  if (token) headers.authorization = `Bearer ${token}`;
  if (body !== undefined) headers["content-type"] = "application/json";
  const response = await fetch(`${base}${path}`, {
    method,
    headers,
    body: body === undefined ? undefined : JSON.stringify(body),
  });
  const text = await response.text();
  let parsed = null;
  try {
    parsed = text ? JSON.parse(text) : null;
  } catch {
    parsed = text;
  }
  return { status: response.status, body: parsed };
}

function check(name, condition, detail) {
  summary.checks[name] = { ok: Boolean(condition), detail };
  if (!condition) fail(`${name}: ${JSON.stringify(detail)}`);
}

const workdir = await fs.mkdtemp(path.join(os.tmpdir(), "hpa-multinode-"));
const dirA = path.join(workdir, "node-a");
const dirB = path.join(workdir, "node-b");
await fs.mkdir(dirA, { recursive: true });
await fs.mkdir(dirB, { recursive: true });

let nodeA;
let nodeB;
try {
  const migrated = await run(binary, ["--data-dir", dirA, "db", "migrate"], {
    env: { HPA_POSTGRES_URL: databaseUrl },
  });
  summary.migration = migrated.stdout.trim();
  const init = await run(binary, ["--data-dir", dirA, "enterprise", "init"], {
    env: { HPA_POSTGRES_URL: databaseUrl },
  });
  const adminToken = JSON.parse(init.stdout).token;

  nodeA = await node("node-a", dirA);
  nodeB = await node("node-b", dirB);

  // 1. The same platform token authenticates on both nodes.
  const tenantsA = await api(nodeA.base, "GET", "/api/enterprise/tenants", { token: adminToken });
  const tenantsB = await api(nodeB.base, "GET", "/api/enterprise/tenants", { token: adminToken });
  check("shared-admin-token", tenantsA.status === 200 && tenantsB.status === 200, {
    nodeA: tenantsA.status,
    nodeB: tenantsB.status,
  });

  // 2. Tenants and quotas created on A are visible on B.
  const created = await api(nodeA.base, "POST", "/api/enterprise/tenants", {
    token: adminToken,
    body: { name: `multi_${Date.now()}` },
  });
  const tenantId = created.body?.id;
  check("tenant-created", created.status === 200 && Boolean(tenantId), created.status);

  const quotaSet = await api(nodeA.base, "PUT", `/api/enterprise/quotas/${tenantId}`, {
    token: adminToken,
    body: {
      maxWorkflows: 9,
      maxExecutionsPerDay: 13,
      maxSkills: 3,
      maxAgents: 2,
      maxPlugins: 1,
      maxConnections: 4,
      maxConcurrentRuns: 2,
    },
  });
  const quotaRead = await api(nodeB.base, "GET", `/api/enterprise/quotas/${tenantId}`, {
    token: adminToken,
  });
  check(
    "shared-quota",
    quotaSet.status === 200 && quotaRead.status === 200 && quotaRead.body?.maxExecutionsPerDay === 13,
    { set: quotaSet.status, read: quotaRead.status, quota: quotaRead.body },
  );

  // 3. A tenant token issued on A works on B.
  const tenantToken = await api(nodeA.base, "POST", "/api/enterprise/tokens", {
    token: adminToken,
    body: { tenantId, name: "multi-node", role: "owner" },
  });
  const tenantTokenValue = tenantToken.body?.token;
  check("tenant-token", tenantToken.status === 200 && Boolean(tenantTokenValue), tenantToken.status);

  // 4. Workflows published on A run and are readable on B (PostgreSQL runtime).
  const workflow = await api(nodeA.base, "POST", "/api/workflows", {
    token: tenantTokenValue,
    body: {
      ir: {
        $schema: "https://hpa.dev/schema/ir/v0.1/workflow.schema.json",
        schemaVersion: "hpa.dev/v1",
        id: "wf_multi",
        name: "Multi node",
        workflowVersion: "1.0.0",
        nodes: [
          { id: "start", type: "start", typeVersion: "1.0.0", name: "Start", config: {} },
          { id: "end", type: "end", typeVersion: "1.0.0", name: "End", config: {} },
        ],
        edges: [
          { id: "e1", source: "start", sourcePort: "out", target: "end", targetPort: "in" },
        ],
      },
    },
  });
  const published = await api(nodeA.base, "POST", "/api/workflows/wf_multi/publish", {
    token: tenantTokenValue,
  });
  const execution = await api(nodeA.base, "POST", "/api/workflows/wf_multi/executions", {
    token: tenantTokenValue,
    body: { input: {} },
  });
  const executionId = execution.body?.summary?.id;
  check(
    "execution-started",
    workflow.status === 200 && published.status === 200 && Boolean(executionId),
    { create: workflow.status, publish: published.status, run: execution.status },
  );

  let executionStatus = "";
  for (let attempt = 0; attempt < 200; attempt += 1) {
    const detail = await api(nodeB.base, "GET", `/api/executions/${executionId}`, {
      token: tenantTokenValue,
    });
    executionStatus = detail.body?.summary?.status ?? "";
    if (executionStatus === "succeeded" || executionStatus === "failed") break;
    await new Promise((resolve) => setTimeout(resolve, 100));
  }
  check("execution-visible-on-b", executionStatus === "succeeded", { status: executionStatus });

  // 5. Worker registered on A is leased and executed by a daemon against B.
  const worker = await api(nodeA.base, "POST", "/api/enterprise/workers", {
    token: adminToken,
    body: { tenantId, name: "multi-worker", kind: "runner", capabilities: ["workflow"] },
  });
  const workerToken = worker.body?.workerToken;
  const workerId = worker.body?.id;
  const job = await api(nodeA.base, "POST", "/api/enterprise/worker-jobs", {
    token: adminToken,
    body: { tenantId, workflowId: "wf_multi", workerId, input: {} },
  });
  const jobId = job.body?.id;
  check("worker-job-created", worker.status === 200 && job.status === 200, {
    worker: worker.status,
    job: job.status,
  });

  const daemonDir = path.join(workdir, "daemon");
  await fs.mkdir(daemonDir, { recursive: true });
  const daemon = spawn(
    binary,
    ["--data-dir", daemonDir, "worker", "--control-url", nodeB.base, "--max-concurrency", "1"],
    {
      cwd: root,
      env: { ...process.env, HPA_WORKER_TOKEN: workerToken },
      stdio: ["ignore", "pipe", "pipe"],
    },
  );
  let daemonLogs = "";
  daemon.stdout.on("data", (chunk) => {
    daemonLogs += chunk;
  });
  daemon.stderr.on("data", (chunk) => {
    daemonLogs += chunk;
  });

  let jobStatus = "";
  for (let attempt = 0; attempt < 300; attempt += 1) {
    const listed = await api(nodeA.base, "GET", `/api/enterprise/worker-jobs?tenantId=${tenantId}`, {
      token: adminToken,
    });
    const entry = listed.body?.jobs?.find((candidate) => candidate.id === jobId);
    jobStatus = entry?.status ?? "";
    if (jobStatus === "succeeded" || jobStatus === "failed" || jobStatus === "cancelled") break;
    await new Promise((resolve) => setTimeout(resolve, 100));
  }
  daemon.kill();
  check("worker-executes-across-nodes", jobStatus === "succeeded", {
    status: jobStatus,
    daemon: daemonLogs.slice(-400),
  });

  // 6. Revocation on A is honoured by B.
  const revoked = await api(nodeA.base, "DELETE", `/api/enterprise/tokens/${tenantToken.body?.id}`, {
    token: adminToken,
  });
  const afterRevoke = await api(nodeB.base, "GET", "/api/workflows", {
    token: tenantTokenValue,
  });
  check("shared-revocation", revoked.status === 200 && afterRevoke.status === 401, {
    revoke: revoked.status,
    after: afterRevoke.status,
  });

  // 7. A node started later sees the same control plane.
  const late = await node("node-c", path.join(workdir, "node-c"));
  const lateTenants = await api(late.base, "GET", "/api/enterprise/tenants", { token: adminToken });
  check("late-node-joins", lateTenants.status === 200, { status: lateTenants.status });
  late.child.kill();

  await api(nodeA.base, "DELETE", `/api/enterprise/tenants/${tenantId}`, {
    token: adminToken,
    body: { confirm: true },
  });
} catch (error) {
  fail(error instanceof Error ? error.message : String(error));
} finally {
  nodeA?.child.kill();
  nodeB?.child.kill();
  await fs.rm(workdir, { recursive: true, force: true });
}

console.log(JSON.stringify(summary, null, 2));
if (summary.errors.length > 0) process.exit(1);
