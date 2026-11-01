const assert = require("node:assert/strict");
const http = require("node:http");
const vscode = require("vscode");

const extensionId = "hpa.workbench";
const requiredCommands = [
  "hpa.runtime.refresh",
  "hpa.runtime.connect",
  "hpa.workflow.run",
  "hpa.skill.install",
  "hpa.mcp.register",
  "hpa.settings.open",
];

function runtimeAvailable() {
  return new Promise((resolve) => {
    const request = http.get(
      "http://127.0.0.1:8787/api/health",
      { timeout: 1500 },
      (response) => {
        response.resume();
        resolve(response.statusCode === 200);
      },
    );
    request.on("timeout", () => {
      request.destroy();
      resolve(false);
    });
    request.on("error", () => resolve(false));
  });
}

function delay(milliseconds) {
  return new Promise((resolve) => setTimeout(resolve, milliseconds));
}

function httpJson(method, path) {
  return new Promise((resolve, reject) => {
    const request = http.request(
      `http://127.0.0.1:8787${path}`,
      { method, timeout: 3000 },
      (response) => {
        let body = "";
        response.setEncoding("utf8");
        response.on("data", (chunk) => {
          body += chunk;
        });
        response.on("end", () => {
          try {
            resolve({ status: response.statusCode, body: body === "" ? null : JSON.parse(body) });
          } catch (error) {
            reject(error);
          }
        });
      },
    );
    request.on("timeout", () => request.destroy(new Error("timeout")));
    request.on("error", reject);
    request.end();
  });
}

async function run() {
  console.log("[hpa] extension host suite starting");
  const passed = [];
  const failed = [];
  let skipped = 0;

  async function check(name, fn) {
    try {
      await fn();
      passed.push(name);
      console.log(`[hpa] PASS ${name}`);
    } catch (error) {
      failed.push({ name, error });
      const message = error instanceof Error ? error.message : String(error);
      console.error(`[hpa] FAIL ${name}: ${message}`);
    }
  }

  await check("extension is installed and activates without throwing", async () => {
    const extension = vscode.extensions.getExtension(extensionId);
    assert.ok(extension, `extension ${extensionId} is not installed`);
    if (!extension.isActive) await extension.activate();
    assert.equal(extension.isActive, true, `extension ${extensionId} is not active`);
  });

  await check("required commands are registered", async () => {
    const registered = new Set(await vscode.commands.getCommands(true));
    for (const command of requiredCommands) {
      assert.ok(registered.has(command), `missing command ${command}`);
    }
  });

  await check("hpa.runtime.refresh executes without throwing", async () => {
    await vscode.commands.executeCommand("hpa.runtime.refresh");
  });

  await check("hpa.runtime.status executes without throwing", async () => {
    await vscode.commands.executeCommand("hpa.runtime.status");
  });

  if (await runtimeAvailable()) {
    await check("hpa.runtime.connect executes and reconnects after a short delay", async () => {
      await vscode.commands.executeCommand("hpa.runtime.connect");
      await delay(1000);
      await vscode.commands.executeCommand("hpa.runtime.connect");
    });

    await check("extension exports report an online runtime and safe skill target", async () => {
      const extension = vscode.extensions.getExtension(extensionId);
      const api = extension.exports;
      await api.controller.refreshRuntime();
      assert.equal(api.controller.runtime().status, "online");
      const target = await api.installer.resolveTarget("invoice-audit", "project");
      const root = vscode.workspace.workspaceFolders[0].uri.fsPath;
      assert.equal(target.fsPath, `${root}/.github/skills/invoice-audit`);
      assert.deepEqual(api.installer.records(), []);
    });

    await check("MCP registration mints a scoped token and cleans up", async () => {
      await vscode.commands.executeCommand("hpa.mcp.register");
      const tokens = await httpJson("GET", "/api/mcp/tokens");
      const token = tokens.body.tokens.find((entry) => entry.name === "vscode-extension");
      assert.ok(token, "vscode-extension MCP token was not created");
      assert.deepEqual(token.scopes.sort(), ["mcp:execute", "mcp:read"]);
      const revoked = await httpJson("DELETE", `/api/mcp/tokens/${token.id}`);
      assert.equal(revoked.status, 200);
    });
  } else {
    skipped += 3;
    console.log("[hpa] SKIP runtime-dependent checks: local runtime is not available");
  }

  console.log(
    `[hpa] summary: ${passed.length} passed, ${failed.length} failed, ${skipped} skipped`,
  );

  if (failed.length > 0) {
    for (const failure of failed) {
      console.error(`[hpa] failed check: ${failure.name}`);
      console.error(
        failure.error instanceof Error
          ? (failure.error.stack ?? failure.error.message)
          : String(failure.error),
      );
    }
    throw new Error(`${failed.length} extension host check(s) failed`);
  }

  console.log("[hpa] all extension host checks passed");
}

module.exports = { run };
