import { existsSync, rmSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { runTests } from "@vscode/test-electron";

const here = dirname(fileURLToPath(import.meta.url));
const extensionDevelopmentPath = resolve(here, "../..");
const extensionTestsPath = join(here, "suite/index.cjs");
const workspacePath = join(here, "../fixtures/workspace");
const userDataDir = "/tmp/hpa-vscode-test-user";
const extensionsDir = "/tmp/hpa-vscode-test-ext";

const macElectron = "/Applications/Visual Studio Code.app/Contents/MacOS/Electron";
const macCode = "/Applications/Visual Studio Code.app/Contents/MacOS/Code";
const defaultExecutable = existsSync(macElectron)
  ? macElectron
  : existsSync(macCode)
    ? macCode
    : macElectron;
const vscodeExecutablePath = process.env.HPA_VSCODE_EXECUTABLE ?? defaultExecutable;

rmSync(userDataDir, { recursive: true, force: true });
rmSync(extensionsDir, { recursive: true, force: true });

if (!existsSync(vscodeExecutablePath)) {
  console.error(`VS Code executable not found: ${vscodeExecutablePath}`);
  console.error("Set HPA_VSCODE_EXECUTABLE to the VS Code executable path.");
  process.exit(1);
}

try {
  const exitCode = await runTests({
    vscodeExecutablePath,
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
    console.error(`Extension host tests exited with code ${exitCode}`);
    process.exit(1);
  }
  console.log("Extension host tests passed.");
} catch (error) {
  console.error("Extension host tests failed.");
  console.error(error instanceof Error ? (error.stack ?? error.message) : String(error));
  process.exit(1);
}
