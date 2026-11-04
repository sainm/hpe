#!/usr/bin/env node

import { spawn } from "node:child_process";
import fs from "node:fs/promises";
import http from "node:http";
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
  checks: {},
  events: [],
  errors: [],
};

const fail = (message) => {
  summary.errors.push(message);
  console.error(`FAIL: ${message}`);
};

const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms));

function check(name, condition, detail) {
  summary.checks[name] = { ok: Boolean(condition), detail };
  if (!condition) fail(`${name}: ${JSON.stringify(detail)}`);
}

function freePort() {
  return new Promise((resolve, reject) => {
    const server = net.createServer();
    server.once("error", reject);
    server.listen(0, "127.0.0.1", () => {
      const address = server.address();
      server.close(() => resolve(address.port));
    });
  });
}

function run(args, env = {}) {
  return new Promise((resolve, reject) => {
    const child = spawn(binary, args, {
      cwd: root,
      env: { ...process.env, ...env },
      stdio: ["ignore", "pipe", "pipe"],
    });
    let stdout = "";
    let stderr = "";
    child.stdout.on("data", (chunk) => {
      stdout += chunk;
    });
    child.stderr.on("data", (chunk) => {
      stderr += chunk;
    });
    child.on("error", reject);
    child.on("close", (code) => {
      if (code === 0) resolve(stdout);
      else reject(new Error(`${binary} ${args.join(" ")} failed (${code})\n${stderr}`));
    });
  });
}

async function waitHealthy(base, exitCode) {
  for (let attempt = 0; attempt < 400; attempt += 1) {
    if (exitCode() !== null) return false;
    try {
      const response = await fetch(`${base}/api/health`);
      if (response.ok) return true;
    } catch {}
    await sleep(50);
  }
  return false;
}

async function startNode(label, dir) {
  const port = await freePort();
  const base = `http://127.0.0.1:${port}`;
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
  child.stdout.on("data", (chunk) => {
    logs += chunk;
  });
  child.stderr.on("data", (chunk) => {
    logs += chunk;
  });
  if (!(await waitHealthy(base, () => child.exitCode))) {
    child.kill("SIGKILL");
    throw new Error(`node ${label} did not become healthy\n${logs}`);
  }
  return { label, base, child, logs: () => logs };
}

async function hardStop(processInfo) {
  if (processInfo.child.exitCode !== null) return;
  processInfo.child.kill("SIGKILL");
  await new Promise((resolve) => processInfo.child.once("close", resolve));
}

function startProxy() {
  let target = null;
  const server = http.createServer((request, response) => {
    if (!target) {
      response.writeHead(503);
      response.end("no control-plane target");
      return;
    }
    const upstreamUrl = new URL(target);
    const upstream = http.request(
      {
        hostname: upstreamUrl.hostname,
        port: upstreamUrl.port,
        path: request.url,
        method: request.method,
        headers: request.headers,
      },
      (upstreamResponse) => {
        response.writeHead(upstreamResponse.statusCode ?? 502, upstreamResponse.headers);
        upstreamResponse.pipe(response);
      },
    );
    upstream.once("error", (error) => {
      if (!response.headersSent) response.writeHead(502, { "content-type": "text/plain" });
      response.end(`control-plane proxy: ${error.message}`);
    });
    request.pipe(upstream);
  });
  return new Promise((resolve) => {
    server.listen(0, "127.0.0.1", () => {
      resolve({
        server,
        base: `http://127.0.0.1:${server.address().port}`,
        setTarget(value) {
          target = value;
        },
      });
    });
  });
}

function startSlowServer() {
  const calls = [];
  const server = http.createServer((request, response) => {
    const url = new URL(request.url, "http://127.0.0.1");
    const id = url.searchParams.get("id") || "";
    const ms = Math.min(Math.max(Number(url.searchParams.get("ms") || "1000"), 0), 20000);
    calls.push({ id, at: Date.now() });
    setTimeout(() => {
      response.writeHead(200, { "content-type": "application/json" });
      response.end(JSON.stringify({ ok: true, id, call: calls.length }));
    }, ms);
  });
  return new Promise((resolve) => {
    server.listen(0, "127.0.0.1", () => {
      resolve({
        server,
        base: `http://127.0.0.1:${server.address().port}`,
        calls,
      });
    });
  });
}

function startWorker(label, token, dir) {
  const child = spawn(
    binary,
    ["--data-dir", dir, "worker", "--control-url", proxy.base, "--max-concurrency", "1"],
    {
      cwd: root,
      env: { ...process.env, HPA_WORKER_TOKEN: token },
      stdio: ["ignore", "pipe", "pipe"],
    },
  );
  let logs = "";
  child.stdout.on("data", (chunk) => {
    logs += chunk;
  });
  child.stderr.on("data", (chunk) => {
    logs += chunk;
  });
  return { label, child, logs: () => logs };
}

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

const workdir = await fs.mkdtemp(path.join(os.tmpdir(), "hpa-worker-rolling-"));
const dirs = {
  nodeA: path.join(workdir, "node-a"),
  nodeB: path.join(workdir, "node-b"),
  nodeA2: path.join(workdir, "node-a2"),
  nodeB2: path.join(workdir, "node-b2"),
  worker1: path.join(workdir, "worker-1"),
  worker1b: path.join(workdir, "worker-1b"),
  worker2: path.join(workdir, "worker-2"),
  worker2b: path.join(workdir, "worker-2b"),
};
for (const dir of Object.values(dirs)) await fs.mkdir(dir, { recursive: true });

let nodeA;
let nodeB;
let worker1;
let worker2;
let proxy;
let slow;
const liveProcesses = [];

try {
  await run(["--data-dir", dirs.nodeA, "db", "migrate"], { HPA_POSTGRES_URL: databaseUrl });
  const init = await run(["--data-dir", dirs.nodeA, "enterprise", "init"], {
    HPA_POSTGRES_URL: databaseUrl,
  });
  const adminToken = JSON.parse(init).token;

  nodeA = await startNode("node-a", dirs.nodeA);
  nodeB = await startNode("node-b", dirs.nodeB);
  liveProcesses.push(nodeA, nodeB);

  proxy = await startProxy();
  proxy.setTarget(nodeA.base);
  slow = await startSlowServer();

  const tenant = await api(nodeA.base, "POST", "/api/enterprise/tenants", {
    token: adminToken,
    body: { name: `worker_rolling_${Date.now()}` },
  });
  const tenantId = tenant.body?.id;
  check("tenant-created", tenant.status === 200 && Boolean(tenantId), tenant.status);
  if (!tenantId) throw new Error("tenant creation failed");

  const tenantToken = await api(nodeA.base, "POST", "/api/enterprise/tokens", {
    token: adminToken,
    body: { tenantId, name: "worker-rolling", role: "owner" },
  });
  const tenantTokenValue = tenantToken.body?.token;
  check(
    "tenant-token-created",
    tenantToken.status === 200 && Boolean(tenantTokenValue),
    tenantToken.status,
  );

  const workflow = {
    $schema: "https://hpa.dev/schema/ir/v0.1/workflow.schema.json",
    schemaVersion: "hpa.dev/v1",
    id: "wf_worker_rolling",
    name: "Worker rolling upgrade",
    workflowVersion: "1.0.0",
    inputs: [{ name: "url", type: "string", required: true }],
    nodes: [
      { id: "start", type: "start", typeVersion: "1.0.0", name: "Start", config: {} },
      {
        id: "http",
        type: "http.request",
        typeVersion: "1.0.0",
        name: "Slow work",
        config: { method: "GET", url: { $expr: ".input.url" } },
        timeoutMs: 20000,
      },
      { id: "end", type: "end", typeVersion: "1.0.0", name: "End", config: {} },
    ],
    edges: [
      { id: "e1", source: "start", sourcePort: "out", target: "http", targetPort: "in" },
      { id: "e2", source: "http", sourcePort: "out", target: "end", targetPort: "in" },
    ],
  };
  const created = await api(nodeA.base, "POST", "/api/workflows", {
    token: tenantTokenValue,
    body: { ir: workflow },
  });
  const published = await api(nodeA.base, "POST", "/api/workflows/wf_worker_rolling/publish", {
    token: tenantTokenValue,
  });
  check(
    "workflow-published",
    created.status === 200 && published.status === 200,
    { create: created.status, publish: published.status, response: created.body },
  );

  const workerOne = await api(nodeA.base, "POST", "/api/enterprise/workers", {
    token: adminToken,
    body: {
      tenantId,
      name: "worker-1",
      kind: "runner",
      capabilities: ["workflow"],
      maxConcurrency: 1,
    },
  });
  const workerTwo = await api(nodeA.base, "POST", "/api/enterprise/workers", {
    token: adminToken,
    body: {
      tenantId,
      name: "worker-2",
      kind: "runner",
      capabilities: ["workflow"],
      maxConcurrency: 1,
    },
  });
  const workerOneId = workerOne.body?.id;
  const workerTwoId = workerTwo.body?.id;
  check(
    "workers-registered",
    workerOne.status === 200 && workerTwo.status === 200 && Boolean(workerOneId) && Boolean(workerTwoId),
    { one: workerOne.status, two: workerTwo.status },
  );

  const submitJob = async (label, ms) => {
    const response = await api(nodeA.base, "POST", "/api/enterprise/worker-jobs", {
      token: adminToken,
      body: {
        tenantId,
        workflowId: "wf_worker_rolling",
        input: { url: `${slow.base}/work?id=${label}&ms=${ms}` },
      },
    });
    return response;
  };

  const listJobs = async (base = nodeA.base) => {
    const response = await api(base, "GET", `/api/enterprise/worker-jobs?tenantId=${tenantId}`, {
      token: adminToken,
    });
    return response.body?.jobs ?? [];
  };

  const waitJob = async (jobId, predicate, timeoutMs = 90000, base = nodeA.base) => {
    const deadline = Date.now() + timeoutMs;
    let last = null;
    while (Date.now() < deadline) {
      last = (await listJobs(base)).find((job) => job.id === jobId) ?? null;
      if (last && predicate(last)) return last;
      await sleep(200);
    }
    throw new Error(`job ${jobId} did not reach the expected state: ${JSON.stringify(last)}`);
  };

  worker1 = startWorker("worker-1", workerOne.body.workerToken, dirs.worker1);
  liveProcesses.push(worker1);
  const job1 = await submitJob("job-1", 6000);
  const job1Id = job1.body?.id;
  const running1 = await waitJob(
    job1Id,
    (job) => job.status === "running" && job.workerId === workerOneId,
    30000,
  );
  check("worker-1-leases-job", running1.workerId === workerOneId, running1);

  worker2 = startWorker("worker-2", workerTwo.body.workerToken, dirs.worker2);
  liveProcesses.push(worker2);
  const job2 = await submitJob("job-2", 6000);
  const job2Id = job2.body?.id;
  const running2 = await waitJob(
    job2Id,
    (job) => job.status === "running" && job.workerId === workerTwoId,
    30000,
  );
  check("worker-2-leases-job", running2.workerId === workerTwoId, running2);

  const job1Execution = running1.executionId;
  summary.events.push("kill worker-1 while job-1 is running");
  await hardStop(worker1);
  const recovered1 = await waitJob(
    job1Id,
    (job) => job.status === "succeeded" && job.attempts >= 2,
    90000,
  );
  check("worker-kill-requeues-job", recovered1.attempts >= 2, recovered1);
  check(
    "worker-kill-completes-on-survivor",
    recovered1.status === "succeeded" && recovered1.workerId === workerTwoId,
    recovered1,
  );
  check("worker-kill-single-execution", recovered1.executionId === job1Execution, {
    before: job1Execution,
    after: recovered1.executionId,
  });
  const recoveredExecution = await api(
    nodeB.base,
    "GET",
    `/api/executions/${recovered1.executionId}`,
    { token: tenantTokenValue },
  );
  check(
    "worker-kill-execution-succeeded",
    recoveredExecution.status === 200 && recoveredExecution.body?.summary?.status === "succeeded",
    recoveredExecution.body?.summary,
  );

  worker1 = startWorker("worker-1", workerOne.body.workerToken, dirs.worker1b);
  liveProcesses.push(worker1);
  const job3 = await submitJob("job-3", 6000);
  const job3Id = job3.body?.id;
  const running3 = await waitJob(
    job3Id,
    (job) => job.status === "running" && job.workerId === workerOneId,
    30000,
  );
  check("worker-1-restarted-and-leases", running3.workerId === workerOneId, running3);

  const job4 = await submitJob("job-4", 6000);
  const job4Id = job4.body?.id;
  const running4 = await waitJob(
    job4Id,
    (job) => job.status === "running" && job.workerId === workerTwoId,
    30000,
  );
  check("worker-2-leases-second-job", running4.workerId === workerTwoId, running4);

  summary.events.push("kill worker-2 while job-4 is running");
  await hardStop(worker2);
  const recovered4 = await waitJob(
    job4Id,
    (job) => job.status === "succeeded" && job.attempts >= 2,
    90000,
  );
  check("worker-2-kill-requeues-job", recovered4.attempts >= 2, recovered4);
  check(
    "worker-2-kill-completes-on-worker-1",
    recovered4.status === "succeeded" && recovered4.workerId === workerOneId,
    recovered4,
  );

  worker2 = startWorker("worker-2", workerTwo.body.workerToken, dirs.worker2b);
  liveProcesses.push(worker2);

  const job5 = await submitJob("job-5", 8000);
  const job5Id = job5.body?.id;
  await waitJob(job5Id, (job) => job.status === "running", 30000);

  summary.events.push("roll node-a while job-5 is running");
  proxy.setTarget(nodeB.base);
  await hardStop(nodeA);
  nodeA = await startNode("node-a", dirs.nodeA2);
  liveProcesses.push(nodeA);
  check("control-plane-a-replaced", Boolean(nodeA.base), nodeA.base);

  summary.events.push("roll node-b while job-5 is running");
  proxy.setTarget(nodeA.base);
  await hardStop(nodeB);
  nodeB = await startNode("node-b", dirs.nodeB2);
  liveProcesses.push(nodeB);
  check("control-plane-b-replaced", Boolean(nodeB.base), nodeB.base);

  const recovered5 = await waitJob(
    job5Id,
    (job) => job.status === "succeeded",
    90000,
    nodeA.base,
  );
  check("job-survives-control-plane-roll", recovered5.status === "succeeded", recovered5);

  const workersAfterRoll = await api(
    nodeA.base,
    "GET",
    `/api/enterprise/workers?tenantId=${tenantId}`,
    { token: adminToken },
  );
  const activeWorkers = (workersAfterRoll.body?.workers ?? []).filter(
    (worker) => worker.status === "active",
  );
  check(
    "workers-active-after-control-plane-roll",
    workersAfterRoll.status === 200 && activeWorkers.length === 2,
    activeWorkers.map((worker) => worker.id),
  );

  const job6 = await submitJob("job-6", 1500);
  const job6Id = job6.body?.id;
  const finished6 = await waitJob(
    job6Id,
    (job) => job.status === "succeeded",
    60000,
    nodeA.base,
  );
  check("job-completes-after-control-plane-roll", finished6.status === "succeeded", finished6);

  const allJobs = await listJobs(nodeA.base);
  const jobs = [job1Id, job2Id, job3Id, job4Id, job5Id, job6Id].map((id) =>
    allJobs.find((job) => job.id === id),
  );
  check(
    "all-worker-jobs-succeeded",
    jobs.every((job) => job?.status === "succeeded"),
    jobs.map((job) => ({ id: job?.id, status: job?.status, attempts: job?.attempts })),
  );
  check(
    "slow-target-was-called",
    slow.calls.length >= 6,
    slow.calls.map((call) => call.id),
  );

  summary.jobs = jobs.map((job) => ({
    id: job?.id,
    status: job?.status,
    attempts: job?.attempts,
    workerId: job?.workerId,
    executionId: job?.executionId,
  }));
} catch (error) {
  fail(error instanceof Error ? error.message : String(error));
} finally {
  for (const processInfo of liveProcesses.reverse()) {
    if (processInfo?.child?.exitCode === null) processInfo.child.kill("SIGKILL");
  }
  proxy?.server.close();
  slow?.server.close();
  await fs
    .rm(workdir, { recursive: true, force: true, maxRetries: 5, retryDelay: 100 })
    .catch(() => {});
}

console.log(JSON.stringify(summary, null, 2));
if (summary.errors.length > 0) process.exit(1);
