import { spawn } from "node:child_process";
import { existsSync } from "node:fs";
import { mkdtemp, rm } from "node:fs/promises";
import os from "node:os";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { runTests } from "@vscode/test-electron";

const here = dirname(fileURLToPath(import.meta.url));
const extensionDevelopmentPath = resolve(here, "../..");
const extensionTestsPath = join(here, "suite/index.cjs");
const workspacePath = join(here, "../fixtures/workspace");
const repositoryRoot = resolve(here, "../../../..");
const runtimeBinary = process.env.HPA_BINARY || join(repositoryRoot, "target/debug/hpa");
const userDataDir = await mkdtemp(join(os.tmpdir(), "hpa-vscode-test-user-"));
const extensionsDir = await mkdtemp(join(os.tmpdir(), "hpa-vscode-test-ext-"));
const runtimeDataDir = await mkdtemp(join(os.tmpdir(), "hpa-vscode-runtime-"));

const macElectron = "/Applications/Visual Studio Code.app/Contents/MacOS/Electron";
const macCode = "/Applications/Visual Studio Code.app/Contents/MacOS/Code";
const defaultExecutable = existsSync(macElectron)
  ? macElectron
  : existsSync(macCode)
    ? macCode
    : macElectron;
const vscodeExecutablePath =
  process.env.HPA_VSCODE_EXECUTABLE ?? (existsSync(defaultExecutable) ? defaultExecutable : undefined);

if (!existsSync(runtimeBinary)) {
  console.error(`Runtime binary not found: ${runtimeBinary}`);
  console.error("Run `cargo build -p hpa-local` first or set HPA_BINARY.");
  process.exit(1);
}

async function runtimeReady() {
  try {
    const response = await fetch("http://127.0.0.1:8787/api/health");
    return response.ok;
  } catch {
    return false;
  }
}

let runtime;
if (!(await runtimeReady())) {
  runtime = spawn(runtimeBinary, ["--data-dir", runtimeDataDir, "serve", "--port", "8787"], {
    cwd: repositoryRoot,
    stdio: ["ignore", "ignore", "pipe"],
  });
  let logs = "";
  runtime.stderr.on("data", (chunk) => {
    logs += chunk;
  });
  for (let attempt = 0; attempt < 200; attempt += 1) {
    if (runtime.exitCode !== null) break;
    if (await runtimeReady()) break;
    await new Promise((resolveWait) => setTimeout(resolveWait, 50));
  }
  if (!(await runtimeReady())) {
    runtime.kill("SIGTERM");
    console.error(`Runtime did not become ready on 127.0.0.1:8787\n${logs}`);
    process.exit(1);
  }
}

try {
  const exitCode = await runTests({
    ...(vscodeExecutablePath ? { vscodeExecutablePath } : {}),
    extensionDevelopmentPath,
    extensionTestsPath,
    launchArgs: [
      "--disable-extensions",
      `--user-data-dir=${userDataDir}`,
      `--extensions-dir=${extensionsDir}`,
      workspacePath,
    ],
  });
  if (exitCode !== 0) {
    throw new Error(`Extension host tests exited with code ${exitCode}`);
  }
  console.log("Extension host tests passed.");
} catch (error) {
  console.error("Extension host tests failed.");
  console.error(error instanceof Error ? (error.stack ?? error.message) : String(error));
  process.exitCode = 1;
} finally {
  if (runtime && runtime.exitCode === null) {
    runtime.kill("SIGTERM");
    await Promise.race([
      new Promise((resolveExit) => runtime.once("exit", resolveExit)),
      new Promise((resolveTimeout) => setTimeout(resolveTimeout, 3000)),
    ]);
  }
  await rm(userDataDir, { recursive: true, force: true });
  await rm(extensionsDir, { recursive: true, force: true });
  await rm(runtimeDataDir, { recursive: true, force: true });
}
