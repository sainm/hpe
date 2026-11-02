import { spawn } from "node:child_process";

const runtime = process.env.HPA_CONTAINER_RUNTIME || "podman";
const image = process.env.HPA_AI_WORKER_IMAGE || "localhost/hpa-ai-worker:phase13";

function podman(args, { input, expected = 0 } = {}) {
  return new Promise((resolve, reject) => {
    const child = spawn(runtime, args, {
      stdio: ["pipe", "pipe", "pipe"],
      env: { ...process.env },
    });
    let stdout = "";
    let stderr = "";
    child.stdout.setEncoding("utf8");
    child.stderr.setEncoding("utf8");
    child.stdout.on("data", (chunk) => {
      stdout += chunk;
    });
    child.stderr.on("data", (chunk) => {
      stderr += chunk;
    });
    child.on("error", reject);
    child.on("close", (code) => {
      if (expected !== 0 && code === expected) {
        resolve({ code, stdout, stderr });
      } else if (expected === 0 && code === 0) {
        resolve({ code, stdout, stderr });
      } else {
        reject(new Error(`${runtime} ${args.join(" ")} exited ${code}\n${stderr}${stdout}`));
      }
    });
    child.stdin.end(input || "");
  });
}

const hardened = [
  "run",
  "--rm",
  "--read-only",
  "--tmpfs",
  "/tmp:rw,noexec,nosuid",
  "--network=none",
  "--cap-drop=ALL",
  "--security-opt=no-new-privileges",
  "--user=10001:10001",
  "--memory=512m",
  "--cpus=1",
  "--pids-limit=64",
];

const inspector = `
import json
import socket
from pathlib import Path

network_failed = False
try:
    socket.create_connection(("192.0.2.1", 80), timeout=0.5)
except OSError:
    network_failed = True

print(json.dumps({
    "uid": __import__("os").getuid(),
    "network_failed": network_failed,
    "interfaces": sorted(path.name for path in Path("/sys/class/net").iterdir()),
    "root_read_only": " ro," in Path("/proc/mounts").read_text() or " ro " in Path("/proc/mounts").read_text(),
    "memory_max": Path("/sys/fs/cgroup/memory.max").read_text().strip(),
    "pids_max": Path("/sys/fs/cgroup/pids.max").read_text().strip(),
    "no_new_privs": "NoNewPrivs:\\t1" in Path("/proc/self/status").read_text(),
}))
`;

const inspection = await podman(
  [
    ...hardened,
    "--entrypoint",
    "/app/.venv/bin/python",
    image,
    "-c",
    inspector,
  ],
);
const limits = JSON.parse(inspection.stdout.trim());
if (limits.uid !== 10001) throw new Error(`unexpected container uid ${limits.uid}`);
if (!limits.network_failed) throw new Error("network-isolated container reached the Internet");
if (limits.interfaces.join(",") !== "lo") {
  throw new Error(`network namespace has unexpected interfaces: ${limits.interfaces}`);
}
if (!limits.root_read_only) throw new Error("container root filesystem is writable");
if (limits.memory_max !== "536870912") {
  throw new Error(`unexpected memory limit ${limits.memory_max}`);
}
if (limits.pids_max !== "64") throw new Error(`unexpected PID limit ${limits.pids_max}`);
if (!limits.no_new_privs) throw new Error("no_new_privs is not active");

const writeProbe = await podman(
  [
    ...hardened,
    "--entrypoint",
    "/bin/sh",
    image,
    "-c",
    "printf forbidden > /rootfs-write-probe",
  ],
  { expected: 2 },
).catch(async (error) => {
  if (error.message.includes("exited 1")) return { code: 1 };
  throw error;
});
if (writeProbe.code === 0) throw new Error("read-only rootfs accepted a write");

const requestTools = [
  {
    type: "function",
    function: {
      name: "workflow.execute",
      description: "Deterministic container smoke tool",
      parameters: {
        type: "object",
        properties: { workflowId: { type: "string" } },
        required: ["workflowId"],
      },
      "x-hpa": {
        risk: "read",
        approvalRequired: false,
        idempotencyKeyTemplate: "{sessionId}/{step}/{callId}",
      },
    },
  },
];
const request = {
  type: "start",
  sessionId: "session-container-smoke",
  requestToken: "container-worker-token",
  tenantId: "tenant-container",
  agentVersionId: "agent-container",
  input: { message: "run" },
  instructions: "Execute the deterministic container smoke.",
  tools: requestTools,
  budget: {
    maxSteps: 4,
    maxToolCalls: 2,
    timeoutSeconds: 30,
    maxTokens: 1000,
    maxCostUsd: 0.1,
  },
};
const worker = await podman(
  [
    ...hardened,
    "-i",
    "-e",
    "HPA_AI_WORKER_TOKEN=container-worker-token",
    "-e",
    "HPA_AI_PLANNER=deterministic",
    "-e",
    "HPA_DETERMINISTIC_TOOL=workflow.execute",
    "-e",
    'HPA_DETERMINISTIC_ARGUMENTS={"workflowId":"wf_container"}',
    "-e",
    'HPA_DETERMINISTIC_OUTPUT={"ok":true}',
    "-e",
    'HPA_DETERMINISTIC_TOOL_RESULTS={"workflow.execute":{"callId":"tc_container","status":"succeeded","result":{"container":true}}}',
    image,
  ],
  { input: `${JSON.stringify(request)}\n` },
);
const events = worker.stdout
  .trim()
  .split("\n")
  .filter(Boolean)
  .map((line) => JSON.parse(line));
const types = events.map((event) => event.type);
if (types.at(-1) !== "session.result") {
  throw new Error(`worker did not finish: ${JSON.stringify(types)}`);
}
if (events.at(-1)?.data?.output?.ok !== true) {
  throw new Error(`unexpected worker output: ${JSON.stringify(events.at(-1))}`);
}
if (!types.includes("tool.result")) throw new Error("worker did not round-trip its tool");

console.log(
  JSON.stringify(
    {
      ok: true,
      runtime,
      image,
      isolation: limits,
      workerEvents: types,
      output: events.at(-1).data.output,
    },
    null,
    2,
  ),
);
