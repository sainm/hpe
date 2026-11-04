import { spawn, spawnSync } from "node:child_process";
import fs from "node:fs/promises";
import http from "node:http";
import net from "node:net";
import os from "node:os";
import path from "node:path";
import { fileURLToPath } from "node:url";

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const binary = process.env.HPA_BINARY || path.join(root, "target", "debug", "hpa");
const requirements = process.env.HPA_CONFORMANCE_REQUIREMENTS?.trim();
const conformanceOverride = process.env.HPA_CONFORMANCE_BIN?.trim();
const scenarios = (
  process.env.HPA_CONFORMANCE_SCENARIOS ||
  [
    "server-initialize",
    "ping",
    "logging-set-level",
    "tools-list",
    "tools-call-simple-text",
    "tools-call-error",
    "resources-list",
    "prompts-list",
    "dns-rebinding-protection",
  ].join(",")
)
  .split(",")
  .map((value) => value.trim())
  .filter(Boolean);
const dataDir = await fs.mkdtemp(path.join(os.tmpdir(), "hpa-conformance-"));
const resultRoot = path.join(
  root,
  "target",
  "mcp-conformance",
  new Date().toISOString().replaceAll(":", "-"),
);
await fs.mkdir(resultRoot, { recursive: true });

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

function cli(args) {
  const output = spawnSync(binary, ["--data-dir", dataDir, ...args], {
    cwd: root,
    encoding: "utf8",
  });
  if (output.status !== 0) {
    throw new Error(
      `hpa ${args.join(" ")} failed: ${output.stderr || output.stdout}`,
    );
  }
  return output.stdout;
}

function runProcess(command, args) {
  return new Promise((resolve, reject) => {
    const child = spawn(command, args, {
      cwd: root,
      env: { ...process.env, NO_COLOR: "1" },
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
    child.once("error", reject);
    child.once("exit", (status) => resolve({ status, stdout, stderr }));
  });
}

async function waitForHealth(port, runtime) {
  for (let attempt = 0; attempt < 200; attempt += 1) {
    if (runtime.exitCode !== null) break;
    try {
      const response = await fetch(`http://127.0.0.1:${port}/api/health`);
      if (response.ok) return;
    } catch {
      // Runtime is still starting.
    }
    await new Promise((resolve) => setTimeout(resolve, 50));
  }
  throw new Error("HPA runtime did not become healthy");
}

function tokenProxy(port, targetPort, token, logs) {
  const server = http.createServer((request, response) => {
    const startedAt = performance.now();
    const log = {
      method: request.method,
      path: request.url,
      accept: request.headers.accept,
      protocolVersion: request.headers["mcp-protocol-version"],
      status: null,
      contentType: null,
      contentEncoding: null,
      responseBody: "",
      error: null,
    };
    logs.push(log);
    const headers = {
      ...request.headers,
      authorization: `Bearer ${token}`,
      "accept-encoding": "identity",
    };
    const upstream = http.request(
      {
        hostname: "127.0.0.1",
        port: targetPort,
        method: request.method,
        path: request.url,
        headers,
      },
      (upstreamResponse) => {
        log.status = upstreamResponse.statusCode;
        log.durationMs = Math.round(performance.now() - startedAt);
        log.contentType = upstreamResponse.headers["content-type"];
        log.contentEncoding = upstreamResponse.headers["content-encoding"];
        response.writeHead(
          upstreamResponse.statusCode ?? 502,
          upstreamResponse.headers,
        );
        upstreamResponse.on("data", (chunk) => {
          if (log.responseBody.length < 2000) {
            log.responseBody += chunk.toString("utf8").slice(0, 2000);
          }
        });
        upstreamResponse.pipe(response);
      },
    );
    upstream.once("error", (error) => {
      log.error = error.message;
      if (!response.headersSent) {
        response.writeHead(502, { "content-type": "text/plain" });
      }
      response.end(`conformance proxy error: ${error.message}`);
    });
    request.pipe(upstream);
  });
  return new Promise((resolve, reject) => {
    server.once("error", reject);
    server.listen(port, "127.0.0.1", () => resolve(server));
  });
}

async function stopProcess(runtime) {
  if (runtime.exitCode !== null) return;
  runtime.kill("SIGTERM");
  await Promise.race([
    new Promise((resolve) => runtime.once("exit", resolve)),
    new Promise((resolve) => setTimeout(resolve, 3_000)),
  ]);
  if (runtime.exitCode === null) runtime.kill("SIGKILL");
}

const token = JSON.parse(cli(["mcp", "token", "create", "conformance"])).token;
const runtimePort = await freePort();
const proxyPort = await freePort();
let runtimeLogs = "";
const runtime = spawn(
  binary,
  ["--data-dir", dataDir, "serve", "--port", String(runtimePort)],
  {
    cwd: root,
    env: {
      ...process.env,
      HPA_MCP_CONFORMANCE: "1",
      HPA_MCP_RATE_LIMIT: process.env.HPA_MCP_RATE_LIMIT || "100000",
    },
    stdio: ["ignore", "pipe", "pipe"],
  },
);
runtime.stdout.on("data", (chunk) => {
  runtimeLogs += chunk;
});
runtime.stderr.on("data", (chunk) => {
  runtimeLogs += chunk;
});

let proxy;
const proxyLogs = [];
const results = [];
try {
  await waitForHealth(runtimePort, runtime);
  proxy = await tokenProxy(proxyPort, runtimePort, token, proxyLogs);
  const probe = await fetch(`http://127.0.0.1:${proxyPort}/mcp`, {
    method: "POST",
    headers: {
      accept: "application/json, text/event-stream",
      "content-type": "application/json",
      "MCP-Protocol-Version": "2025-11-25",
    },
    body: JSON.stringify({
      jsonrpc: "2.0",
      id: "proxy-probe",
      method: "initialize",
      params: {
        protocolVersion: "2025-11-25",
        capabilities: {},
        clientInfo: { name: "hpa-conformance-probe", version: "1" },
      },
    }),
    signal: AbortSignal.timeout(10_000),
  });
  if (!probe.ok) {
    throw new Error(
      `conformance proxy probe returned HTTP ${probe.status}: ${await probe.text()}`,
    );
  }
  const probeBody = await probe.json();
  if (probeBody?.result?.protocolVersion !== "2025-11-25") {
    throw new Error(`conformance proxy probe returned invalid initialize: ${JSON.stringify(probeBody)}`);
  }
  const conformance =
    conformanceOverride ||
    path.join(
      root,
      "node_modules",
      ".bin",
      process.platform === "win32" ? "conformance.cmd" : "conformance",
    );
  if (requirements) {
    const label = `requirements-${requirements}`;
    const outputDir = path.join(resultRoot, label);
    const result = await runProcess(conformance, [
      "server",
      "--url",
      `http://127.0.0.1:${proxyPort}/mcp`,
      "--requirements",
      requirements,
      "--output-dir",
      outputDir,
    ]);
    process.stdout.write(result.stdout);
    process.stderr.write(result.stderr);
    results.push({
      scenario: label,
      status: result.status,
      outputDir: path.relative(root, outputDir),
    });
    if (result.status !== 0) {
      throw new Error(`MCP conformance requirements ${requirements} failed`);
    }
  } else {
    for (const scenario of scenarios) {
      const outputDir = path.join(resultRoot, scenario);
      const result = await runProcess(
        conformance,
        [
          "server",
          "--url",
          `http://127.0.0.1:${proxyPort}/mcp`,
          "--scenario",
          scenario,
          "--spec-version",
          process.env.HPA_CONFORMANCE_SPEC_VERSION || "2025-11-25",
          "--output-dir",
          outputDir,
          ...(process.env.HPA_CONFORMANCE_FORCE === "1" ? ["--force"] : []),
        ],
      );
      process.stdout.write(result.stdout);
      process.stderr.write(result.stderr);
      results.push({
        scenario,
        status: result.status,
        outputDir: path.relative(root, outputDir),
      });
      if (result.status !== 0) {
        throw new Error(`MCP conformance scenario ${scenario} failed`);
      }
    }
  }
  console.log(
    JSON.stringify(
      {
        ok: true,
        specVersion:
          requirements ||
          process.env.HPA_CONFORMANCE_SPEC_VERSION ||
          "2025-11-25",
        results,
        resultRoot: path.relative(root, resultRoot),
      },
      null,
      2,
    ),
  );
} catch (error) {
  if (runtimeLogs) {
    console.error("HPA runtime logs:");
    console.error(runtimeLogs.slice(-8000));
  }
  console.error("Conformance proxy logs:");
  console.error(JSON.stringify(proxyLogs, null, 2));
  throw error;
} finally {
  if (proxy) {
    await new Promise((resolve) => proxy.close(resolve));
  }
  await stopProcess(runtime);
  await fs.rm(dataDir, { recursive: true, force: true });
}
