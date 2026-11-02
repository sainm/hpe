import { spawn } from "node:child_process";
import fs from "node:fs/promises";
import net from "node:net";
import os from "node:os";
import path from "node:path";

const root = path.resolve(new URL("..", import.meta.url).pathname);
const binary = process.env.HPA_BINARY || path.join(root, "target/debug/hpa");
const dataDir = await fs.mkdtemp(path.join(os.tmpdir(), "hpa-e2e-"));
const shotDir = await fs.mkdtemp(path.join(os.tmpdir(), "hpa-shots-"));
const port = await new Promise((resolve, reject) => {
  const server = net.createServer();
  server.once("error", reject);
  server.listen(0, "127.0.0.1", () => {
    const address = server.address();
    server.close(() => resolve(address.port));
  });
});
const baseUrl = `http://127.0.0.1:${port}`;
const runtime = spawn(binary, ["--data-dir", dataDir, "serve", "--port", String(port)], {
  cwd: root,
  env: {
    ...process.env,
    HPA_SCHEDULER_TICK_MS: "200",
    HPA_APPROVAL_TICK_MS: "100",
  },
  stdio: ["ignore", "pipe", "pipe"],
});
let runtimeLogs = "";
runtime.stdout.on("data", (chunk) => {
  runtimeLogs += chunk;
});
runtime.stderr.on("data", (chunk) => {
  runtimeLogs += chunk;
});

async function waitForReady() {
  for (let attempt = 0; attempt < 200; attempt += 1) {
    if (runtime.exitCode !== null) break;
    try {
      const response = await fetch(`${baseUrl}/api/health`);
      if (response.ok) return;
    } catch {
      // Runtime is still starting.
    }
    await new Promise((resolve) => setTimeout(resolve, 50));
  }
  throw new Error(`runtime did not become ready\n${runtimeLogs}`);
}

function runScript(script) {
  return new Promise((resolve, reject) => {
    const child = spawn(process.execPath, [path.join(root, "scripts", script)], {
      cwd: root,
      env: {
        ...process.env,
        HPA_BASE_URL: baseUrl,
        HPA_SHOT_DIR: shotDir,
      },
      stdio: "inherit",
    });
    child.once("error", reject);
    child.once("exit", (code) => {
      if (code === 0) resolve();
      else reject(new Error(`${script} exited with ${code}`));
    });
  });
}

try {
  await waitForReady();
  await runScript("check-web-a11y.mjs");
  await runScript("phase10-oauth-e2e.mjs");
  await runScript("phase11-triggers-e2e.mjs");
  await runScript("phase12-observability-e2e.mjs");
  console.log(`Local E2E passed against ${baseUrl}`);
} finally {
  runtime.kill("SIGTERM");
  await Promise.race([
    new Promise((resolve) => runtime.once("exit", resolve)),
    new Promise((resolve) => setTimeout(resolve, 3000)),
  ]);
  await fs.rm(dataDir, { recursive: true, force: true });
  await fs.rm(shotDir, { recursive: true, force: true });
}
