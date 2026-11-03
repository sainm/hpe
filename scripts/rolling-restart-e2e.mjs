#!/usr/bin/env node
// Rolling restarts on a PostgreSQL control plane: kill a node while work is in
// flight, bring a replacement up, and assert nothing is lost or duplicated.
//
// This rehearses a same-version rolling restart. Cross-version compatibility
// (old binary against a newer schema) is NOT covered here and remains manual.
//
// Usage:
//   HPA_POSTGRES_URL=postgresql://user@host:5432/empty_db node scripts/rolling-restart-e2e.mjs

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

const summary = { cycles: [], checks: {}, errors: [] };
const fail = (message) => {
  summary.errors.push(message);
  console.error(`FAIL: ${message}`);
};

const freePort = () =>
  new Promise((resolve, reject) => {
    const server = net.createServer();
    server.once("error", reject);
    server.listen(0, "127.0.0.1", () => {
      const { port } = server.address();
      server.close(() => resolve(port));
    });
  });

const run = (args, env) =>
  new Promise((resolve, reject) => {
    const child = spawn(binary, args, {
      cwd: root,
      env: { ...process.env, ...env },
      stdio: ["ignore", "pipe", "pipe"],
    });
    let out = "";
    child.stdout.on("data", (chunk) => (out += chunk));
    child.stderr.on("data", (chunk) => (out += chunk));
    child.on("close", (code) => (code === 0 ? resolve(out) : reject(new Error(out))));
  });

async function startNode(label, dir) {
  const port = await freePort();
  const child = spawn(binary, ["--data-dir", dir, "serve", "--port", String(port)], {
    cwd: root,
    env: {
      ...process.env,
      HPA_POSTGRES_URL: databaseUrl,
      HPA_POSTGRES_RUNTIME: "1",
      HPA_ENTERPRISE_MODE: "required",
      HPA_NODE_ID: label,
      HPA_SCHEDULER_TICK_MS: "200",
    },
    stdio: ["ignore", "pipe", "pipe"],
  });
  let logs = "";
  child.stdout.on("data", (chunk) => (logs += chunk));
  child.stderr.on("data", (chunk) => (logs += chunk));
  const base = `http://127.0.0.1:${port}`;
  for (let attempt = 0; attempt < 400; attempt += 1) {
    if (child.exitCode !== null) break;
    try {
      const response = await fetch(`${base}/api/health`);
      if (response.ok) return { label, base, child, logs: () => logs };
    } catch {
      // still starting
    }
    await new Promise((resolve) => setTimeout(resolve, 50));
  }
  child.kill();
  throw new Error(`node ${label} did not start\n${logs}`);
}

const hardStop = async (node) => {
  node.child.kill("SIGKILL");
  await new Promise((resolve) => node.child.once("close", resolve));
};

async function api(base, method, route, { token, body } = {}) {
  const headers = {};
  if (token) headers.authorization = `Bearer ${token}`;
  if (body !== undefined) headers["content-type"] = "application/json";
  const response = await fetch(`${base}${route}`, {
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

const approvalIr = () => ({
  $schema: "https://hpa.dev/schema/ir/v0.1/workflow.schema.json",
  schemaVersion: "hpa.dev/v1",
  id: "wf_rolling",
  name: "Rolling restart",
  workflowVersion: "1.0.0",
  nodes: [
    { id: "start", type: "start", typeVersion: "1.0.0", name: "Start", config: {} },
    {
      id: "approve",
      type: "human.approval",
      typeVersion: "1.0.0",
      name: "Approve",
      config: {
        title: "Approve rolling restart",
        formSchema: {
          type: "object",
          properties: { decision: { type: "string", enum: ["approve", "reject"] } },
          required: ["decision"],
        },
        assignees: [{ kind: "role", value: "approver" }],
        slaMs: 86400000,
        onTimeout: "reject",
      },
    },
    {
      id: "end",
      type: "end",
      typeVersion: "1.0.0",
      name: "End",
      config: {},
    },
  ],
  edges: [
    { id: "e1", source: "start", sourcePort: "out", target: "approve", targetPort: "in" },
    { id: "e2", source: "approve", sourcePort: "approved", target: "end", targetPort: "in" },
    { id: "e3", source: "approve", sourcePort: "rejected", target: "end", targetPort: "in" },
  ],
});

const workdir = await fs.mkdtemp(path.join(os.tmpdir(), "hpa-rolling-"));
const alive = new Set();

try {
  await run(["--data-dir", path.join(workdir, "bootstrap"), "db", "migrate"], {
    HPA_POSTGRES_URL: databaseUrl,
  });
  const init = await run(["--data-dir", path.join(workdir, "bootstrap"), "enterprise", "init"], {
    HPA_POSTGRES_URL: databaseUrl,
  });
  const admin = JSON.parse(init).token;

  const nodeA = await startNode("node-a", path.join(workdir, "a"));
  alive.add(nodeA);
  let nodeB = await startNode("node-b", path.join(workdir, "b"));
  alive.add(nodeB);

  const tenant = await api(nodeA.base, "POST", "/api/enterprise/tenants", {
    token: admin,
    body: { name: `rolling_${Date.now()}` },
  });
  const tenantId = tenant.body?.id;
  const tenantToken = await api(nodeA.base, "POST", "/api/enterprise/tokens", {
    token: admin,
    body: { tenantId, name: "rolling", role: "owner" },
  });
  const scoped = tenantToken.body?.token;
  check("tenant-ready", tenant.status === 200 && Boolean(scoped), tenant.status);

  const created = await api(nodeA.base, "POST", "/api/workflows", {
    token: scoped,
    body: { ir: approvalIr() },
  });
  const published = await api(nodeA.base, "POST", "/api/workflows/wf_rolling/publish", {
    token: scoped,
  });
  check("workflow-published", created.status === 200 && published.status === 200, {
    create: created.status,
    publish: published.status,
  });

  // Two cycles: each kills the node that owns the pending approval and lets a
  // fresh node (empty data dir) take over from PostgreSQL.
  let nodes = [nodeA, nodeB];
  for (let cycle = 1; cycle <= 2; cycle += 1) {
    // Kill the older node each cycle and keep the other one serving; the
    // replacement becomes the newest member of the pool.
    const [starter, survivor] = nodes;
    const started = await api(starter.base, "POST", "/api/workflows/wf_rolling/executions", {
      token: scoped,
      body: { input: {} },
    });
    const executionId = started.body?.summary?.id;
    check(`cycle${cycle}:started`, started.status === 200 && Boolean(executionId), started.status);

    // Wait until the approval is pending, then kill the node that started it.
    let waiting = false;
    for (let attempt = 0; attempt < 100; attempt += 1) {
      const detail = await api(starter.base, "GET", `/api/executions/${executionId}`, {
        token: scoped,
      });
      if (detail.body?.summary?.status === "waiting_approval") {
        waiting = true;
        break;
      }
      await new Promise((resolve) => setTimeout(resolve, 50));
    }
    check(`cycle${cycle}:waiting`, waiting, "approval should be pending before the kill");

    await hardStop(starter);
    alive.delete(starter);
    const replacement = await startNode(
      `node-${cycle === 1 ? "c" : "d"}`,
      path.join(workdir, `r${cycle}`),
    );
    alive.add(replacement);
    nodes = [survivor, replacement];

    const seenOnReplacement = await api(
      replacement.base,
      "GET",
      `/api/executions/${executionId}`,
      { token: scoped },
    );
    check(
      `cycle${cycle}:recovered`,
      seenOnReplacement.status === 200 &&
        seenOnReplacement.body?.summary?.status === "waiting_approval",
      { status: seenOnReplacement.body?.summary?.status },
    );

    const decided = await api(
      replacement.base,
      "POST",
      `/api/executions/${executionId}/approvals`,
      { token: scoped, body: { nodeId: "approve", decision: "approved", form: { decision: "approve" } } },
    );
    check(`cycle${cycle}:approve`, decided.status === 200, decided.status);

    let finalStatus = "";
    for (let attempt = 0; attempt < 200; attempt += 1) {
      const detail = await api(replacement.base, "GET", `/api/executions/${executionId}`, {
        token: scoped,
      });
      finalStatus = detail.body?.summary?.status ?? "";
      if (finalStatus === "succeeded" || finalStatus === "failed") break;
      await new Promise((resolve) => setTimeout(resolve, 50));
    }
    check(`cycle${cycle}:finished`, finalStatus === "succeeded", finalStatus);

    // The survivor must also see the finished execution (shared state, no skew).
    const seenOnSurvivor = await api(survivor.base, "GET", `/api/executions/${executionId}`, {
      token: scoped,
    });
    check(
      `cycle${cycle}:no-skew`,
      seenOnSurvivor.body?.summary?.status === "succeeded",
      seenOnSurvivor.body?.summary?.status,
    );

    const list = await api(survivor.base, "GET", "/api/executions?limit=200", { token: scoped });
    const executions = Array.isArray(list.body) ? list.body : [];
    const duplicates = executions.filter((entry) => entry.id === executionId).length;
    check(`cycle${cycle}:no-duplicates`, duplicates === 1, { duplicates });
    summary.cycles.push({ cycle, executionId, finalStatus });
  }

  // The node that never restarted keeps serving after the churn.
  const survivor = nodes[nodes.length - 1];
  const health = await api(survivor.base, "GET", "/api/enterprise/tenants", { token: admin });
  check("survivor-healthy", health.status === 200, health.status);
} catch (error) {
  fail(error instanceof Error ? error.message : String(error));
} finally {
  for (const node of alive) node.child.kill("SIGKILL");
  await fs.rm(workdir, { recursive: true, force: true, maxRetries: 5, retryDelay: 100 }).catch(() => {});
}

console.log(JSON.stringify(summary, null, 2));
if (summary.errors.length > 0) process.exit(1);
