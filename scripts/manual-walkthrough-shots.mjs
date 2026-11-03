import fs from "node:fs";
import path from "node:path";
import { spawn } from "node:child_process";
import net from "node:net";
import os from "node:os";
import playwright from "file:///Users/sainm/work/demo/BootDoc/node_modules/playwright/index.js";

const { chromium } = playwright;
const root = path.resolve(new URL("..", import.meta.url).pathname);
const outDir = process.env.HPA_SHOT_DIR || "docs/assets/manual";
const summary = { shots: {}, errors: [] };

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

const dataDir = fs.mkdtempSync(path.join(os.tmpdir(), "hpa-manual-"));
fs.mkdirSync(outDir, { recursive: true });
const port = await freePort();
const base = `http://127.0.0.1:${port}`;
const env = { ...process.env };
delete env.HPA_POSTGRES_URL;
delete env.DATABASE_URL;
delete env.HPA_POSTGRES_RUNTIME;
delete env.HTTP_PROXY;
delete env.http_proxy;
delete env.ALL_PROXY;
delete env.all_proxy;

const cli = (args) =>
  new Promise((resolve, reject) => {
    const child = spawn(path.join(root, "target/debug/hpa"), ["--data-dir", dataDir, ...args], {
      cwd: root,
      env,
      stdio: ["ignore", "pipe", "pipe"],
    });
    let out = "";
    child.stdout.on("data", (chunk) => (out += chunk));
    child.stderr.on("data", (chunk) => (out += chunk));
    child.on("close", (code) => (code === 0 ? resolve(out) : reject(new Error(out))));
  });

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

const linear = {
  $schema: "https://hpa.dev/schema/ir/v0.1/workflow.schema.json",
  schemaVersion: "hpa.dev/v1",
  id: "wf_invoice",
  name: "发票审核",
  workflowVersion: "1.0.0",
  nodes: [
    { id: "start", type: "start", typeVersion: "1.0.0", name: "开始", config: {}, ui: { x: 0, y: 120 } },
    {
      id: "parse",
      type: "transform",
      typeVersion: "1.0.0",
      name: "解析金额",
      config: { mapping: { amount: { $expr: "1 + 1" } } },
      ui: { x: 240, y: 120 },
    },
    { id: "end", type: "end", typeVersion: "1.0.0", name: "结束", config: {}, ui: { x: 480, y: 120 } },
  ],
  edges: [
    { id: "e1", source: "start", sourcePort: "out", target: "parse", targetPort: "in" },
    { id: "e2", source: "parse", sourcePort: "out", target: "end", targetPort: "in" },
  ],
};

const approval = JSON.parse(fs.readFileSync(path.join(root, "fixtures/08_approval.json"), "utf8"));
approval.id = "wf_approval_demo";
approval.name = "审批演示";

try {
  await api("POST", "/api/workflows", { ir: linear });
  await api("POST", "/api/workflows/wf_invoice/publish", {});
  await api("POST", "/api/workflows", { ir: approval });
  await api("POST", "/api/workflows/wf_approval_demo/publish", {});

  for (let index = 0; index < 3; index += 1) {
    await api("POST", "/api/workflows/wf_invoice/executions", { input: { invoice: `INV-${index}` } });
  }
  const paused = await api("POST", "/api/workflows/wf_approval_demo/executions", { input: {} });
  const approvalExecution = paused.body?.summary?.id;
  await new Promise((resolve) => setTimeout(resolve, 900));

  const browser = await chromium.launch({ headless: true });
  const context = await browser.newContext({
    viewport: { width: 1440, height: 900 },
    locale: "zh-CN",
    serviceWorkers: "block",
  });
  const page = await context.newPage();
  page.on("pageerror", (error) => fail(`page error: ${error.message}`));

  const shot = async (name, route, waitFor) => {
    await page.goto(`${base}${route}`, { waitUntil: "networkidle" });
    if (waitFor) await page.getByText(waitFor).first().waitFor({ timeout: 8000 });
    await page.waitForTimeout(400);
    const file = path.join(outDir, `${name}.png`);
    await page.screenshot({ path: file, fullPage: true });
    summary.shots[name] = file;
  };

  // Force the UI to Chinese so the guide and the screenshots match.
  await page.goto(`${base}/`, { waitUntil: "domcontentloaded" });
  await page.evaluate(() => window.localStorage.setItem("hpa.locale", "zh"));
  await page.reload({ waitUntil: "networkidle" });

  await shot("01-dashboard", "/", null);
  await shot("02-executions", "/executions", null);
  await shot("03-designer", "/designer?id=wf_invoice", null);
  if (approvalExecution) {
    await shot("04-approval", `/executions/${approvalExecution}`, null);
  }
  await shot("05-settings", "/settings", null);

  const languages = await page.evaluate(() => Object.keys(window.localStorage));
  summary.localeKeys = languages;
  await browser.close();
} catch (error) {
  fail(error instanceof Error ? error.message : String(error));
} finally {
  server.kill("SIGKILL");
  fs.rmSync(dataDir, { recursive: true, force: true, maxRetries: 5 });
}

console.log(JSON.stringify(summary, null, 2));
if (summary.errors.length > 0) process.exit(1);
