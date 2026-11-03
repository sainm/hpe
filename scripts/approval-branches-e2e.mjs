import fs from "node:fs";
import path from "node:path";
import { spawn } from "node:child_process";
import net from "node:net";
import os from "node:os";

const root = path.resolve(new URL("..", import.meta.url).pathname);
const summary = { branches: {}, errors: [] };
const fail = (message) => {
  summary.errors.push(message);
  console.error("FAIL:", message);
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

const dataDir = fs.mkdtempSync(path.join(os.tmpdir(), "hpa-approval-"));
const port = await freePort();
const base = `http://127.0.0.1:${port}`;
const env = { ...process.env, HPA_APPROVAL_TICK_MS: "100" };
delete env.HPA_POSTGRES_URL;
delete env.DATABASE_URL;
delete env.HPA_POSTGRES_RUNTIME;
delete env.HTTP_PROXY;
delete env.http_proxy;
delete env.ALL_PROXY;
delete env.all_proxy;

const server = spawn(
  path.join(root, "target/debug/hpa"),
  ["--data-dir", dataDir, "serve", "--port", String(port)],
  { cwd: root, env, stdio: ["ignore", "pipe", "pipe"] },
);
for (let attempt = 0; attempt < 300; attempt += 1) {
  try {
    const response = await fetch(`${base}/api/health`);
    if (response.ok) break;
  } catch {
    // starting
  }
  await new Promise((resolve) => setTimeout(resolve, 50));
}

const api = async (method, route, body) => {
  const response = await fetch(`${base}${route}`, {
    method,
    headers: { "content-type": "application/json" },
    body: body === undefined ? undefined : JSON.stringify(body),
  });
  return { status: response.status, body: await response.json().catch(() => null) };
};

const approvalIr = (id, slaMs) => ({
  $schema: "https://hpa.dev/schema/ir/v0.1/workflow.schema.json",
  schemaVersion: "hpa.dev/v1",
  id,
  name: id,
  workflowVersion: "1.0.0",
  nodes: [
    { id: "start", type: "start", typeVersion: "1.0.0", name: "Start", config: {} },
    {
      id: "approve",
      type: "human.approval",
      typeVersion: "1.0.0",
      name: "Approve",
      config: {
        title: "Approve the invoice",
        formSchema: {
          type: "object",
          properties: { decision: { type: "string", enum: ["approve", "reject"] } },
          required: ["decision"],
        },
        assignees: [{ kind: "role", value: "approver" }],
        slaMs,
        onTimeout: "reject",
      },
    },
    {
      id: "approved",
      type: "transform",
      typeVersion: "1.0.0",
      name: "Approved",
      config: { mapping: { decision: "approved" } },
    },
    {
      id: "rejected",
      type: "transform",
      typeVersion: "1.0.0",
      name: "Rejected",
      config: { mapping: { decision: "rejected" } },
    },
    { id: "end", type: "end", typeVersion: "1.0.0", name: "End", config: {} },
  ],
  edges: [
    { id: "e1", source: "start", sourcePort: "out", target: "approve", targetPort: "in" },
    { id: "e2", source: "approve", sourcePort: "approved", target: "approved", targetPort: "in" },
    { id: "e3", source: "approve", sourcePort: "rejected", target: "rejected", targetPort: "in" },
    { id: "e4", source: "approved", sourcePort: "out", target: "end", targetPort: "in" },
    { id: "e5", source: "rejected", sourcePort: "out", target: "end", targetPort: "in" },
  ],
});

const waitFor = async (id, predicate, timeoutMs = 20_000) => {
  const deadline = Date.now() + timeoutMs;
  let detail = null;
  while (Date.now() < deadline) {
    detail = (await api("GET", `/api/executions/${id}`)).body;
    if (predicate(detail)) return detail;
    await new Promise((resolve) => setTimeout(resolve, 100));
  }
  return detail;
};

const events = async (id) => (await api("GET", `/api/executions/${id}/events`)).body ?? [];

const runCase = async (name, workflowId, decide, expectDecision) => {
  await api("POST", "/api/workflows", { ir: approvalIr(workflowId, name === "sla-timeout" ? 1200 : 600_000) });
  const published = await api("POST", `/api/workflows/${workflowId}/publish`, {});
  if (published.status !== 200) return fail(`${name}: publish ${published.status}`);

  const started = await api("POST", `/api/workflows/${workflowId}/executions`, { input: {} });
  const executionId = started.body?.summary?.id;
  if (!executionId) return fail(`${name}: no execution id`);

  const pending = await waitFor(executionId, (detail) => detail?.summary?.status === "waiting_approval", 10_000);
  if (pending?.summary?.status !== "waiting_approval") {
    return fail(`${name}: never reached waiting_approval (${pending?.summary?.status})`);
  }

  if (decide) {
    const decision = await api("POST", `/api/executions/${executionId}/approvals`, {
      nodeId: "approve",
      decision: decide,
      form: { decision: decide === "approved" ? "approve" : "reject" },
    });
    if (decision.status !== 200) return fail(`${name}: decision ${decision.status}`);
  }

  const finished = await waitFor(executionId, (detail) =>
    ["succeeded", "failed", "cancelled"].includes(detail?.summary?.status),
  );
  const eventList = await events(executionId);
  const kinds = eventList.map((event) => event.kind);
  const nodeOutputs = finished?.state?.nodeOutputs ?? {};
  // The approval port decides which transform runs; the end node only receives it.
  const decidedBy = nodeOutputs.approved?.decision
    ? "approved"
    : nodeOutputs.rejected?.decision
      ? "rejected"
      : null;
  summary.branches[name] = {
    status: finished?.summary?.status,
    finalOutput: finished?.state?.output ?? null,
    branchTaken: decidedBy,
    approvalEvent: kinds.find((kind) => /^approval\.(approved|rejected|timeout)$/.test(kind)) ?? null,
    skippedNodes: Object.keys(nodeOutputs).length,
    seenEvents: [...new Set(kinds)],
  };

  if (finished?.summary?.status !== "succeeded") {
    fail(`${name}: expected succeeded, got ${finished?.summary?.status}`);
  }
  if (expectDecision && decidedBy !== expectDecision) {
    fail(`${name}: expected branch ${expectDecision}, got ${decidedBy}`);
  }
  if (name === "reject" && !kinds.includes("approval.rejected")) fail("reject: missing approval.rejected event");
  if (name === "approve" && !kinds.includes("approval.approved")) fail("approve: missing approval.approved event");
  if (name === "sla-timeout" && !kinds.includes("approval.timeout")) fail("sla: missing approval.timeout event");
};

try {
  await runCase("approve", "wf_ap_approve", "approved", "approved");
  await runCase("reject", "wf_ap_reject", "rejected", "rejected");
  await runCase("sla-timeout", "wf_ap_sla", null, "rejected");
} finally {
  server.kill("SIGKILL");
  fs.rmSync(dataDir, { recursive: true, force: true, maxRetries: 5 });
}

console.log(JSON.stringify(summary, null, 2));
if (summary.errors.length > 0) process.exit(1);
