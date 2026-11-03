import fs from "node:fs";
import path from "node:path";
import { spawn } from "node:child_process";
import net from "node:net";
import os from "node:os";
import playwright from "file:///Users/sainm/work/demo/BootDoc/node_modules/playwright/index.js";

const { chromium } = playwright;
const root = path.resolve(new URL("..", import.meta.url).pathname);
const outDir = process.env.HPA_SHOT_DIR || "docs/assets/phase19";
const summary = { checks: {}, screenshots: {}, errors: [] };

const fail = (message) => {
  summary.errors.push(message);
  console.error("FAIL:", message);
};
const check = (name, ok, detail) => {
  summary.checks[name] = { ok: Boolean(ok), detail };
  if (!ok) fail(`${name}: ${JSON.stringify(detail)}`);
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

const dataDir = fs.mkdtempSync(path.join(os.tmpdir(), "hpa-pagination-"));
fs.mkdirSync(outDir, { recursive: true });
const port = await freePort();
const base = `http://127.0.0.1:${port}`;
const env = { ...process.env };
delete env.HPA_POSTGRES_URL;
delete env.DATABASE_URL;
delete env.HPA_POSTGRES_RUNTIME;

const cli = (args) => {
  const result = spawn(path.join(root, "target/debug/hpa"), ["--data-dir", dataDir, ...args], {
    cwd: root,
    env,
    stdio: ["ignore", "pipe", "pipe"],
  });
  let out = "";
  result.stdout.on("data", (chunk) => (out += chunk));
  result.stderr.on("data", (chunk) => (out += chunk));
  return new Promise((resolve, reject) =>
    result.on("close", (code) => (code === 0 ? resolve(out) : reject(new Error(out)))),
  );
};

const admin = JSON.parse(await cli(["enterprise", "init"])).token;
const server = spawn(
  path.join(root, "target/debug/hpa"),
  ["--data-dir", dataDir, "serve", "--port", String(port)],
  { cwd: root, env, stdio: ["ignore", "pipe", "pipe"] },
);
for (let attempt = 0; attempt < 200; attempt += 1) {
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
    headers: { authorization: `Bearer ${admin}`, "content-type": "application/json" },
    body: body === undefined ? undefined : JSON.stringify(body),
  });
  return { status: response.status, body: await response.json().catch(() => null) };
};

const TOTAL = 55;
const ir = {
  $schema: "https://hpa.dev/schema/ir/v0.1/workflow.schema.json",
  schemaVersion: "hpa.dev/v1",
  id: "wf_pagination",
  name: "Pagination",
  workflowVersion: "1.0.0",
  nodes: [
    { id: "start", type: "start", typeVersion: "1.0.0", name: "Start", config: {} },
    { id: "end", type: "end", typeVersion: "1.0.0", name: "End", config: {} },
  ],
  edges: [{ id: "e1", source: "start", sourcePort: "out", target: "end", targetPort: "in" }],
};

await api("POST", "/api/workflows", { ir });
await api("POST", "/api/workflows/wf_pagination/publish", {});
for (let index = 0; index < TOTAL; index += 1) {
  await api("POST", "/api/workflows/wf_pagination/executions", { input: { index } });
}
// Wait until every execution is terminal so the count is stable.
for (let attempt = 0; attempt < 100; attempt += 1) {
  const page = await api("GET", "/api/executions?limit=1");
  if (page.status === 200) {
    const head = await fetch(`${base}/api/executions?limit=1&status=succeeded`, {
      headers: { authorization: `Bearer ${admin}` },
    });
    if (Number(head.headers.get("x-total-count") ?? 0) === TOTAL) break;
  }
  await new Promise((resolve) => setTimeout(resolve, 100));
}

const browser = await chromium.launch({ headless: true });
const context = await browser.newContext({ viewport: { width: 1440, height: 900 } });
const page = await context.newPage();
page.on("pageerror", (error) => fail(`page error: ${error.message}`));

try {
  await page.goto(`${base}/executions`, { waitUntil: "networkidle" });
  await page.getByText(/1\s*\/\s*2|1\/2/).first().waitFor({ timeout: 8000 });
  const firstPageRows = await page.locator("ul.divide-y > li").count();
  const summaryText = (await page.getByText(/1\s*\/\s*2|1\/2/).first().innerText()).trim();
  check("page1-rows", firstPageRows === 50, { rows: firstPageRows });
  check("page1-summary", summaryText.includes("1/2") || summaryText.includes("1 / 2"), summaryText);

  const next = page.getByRole("button", { name: /次へ|Next|下一页/ }).first();
  await next.click();
  await page.waitForTimeout(600);
  const secondPageRows = await page.locator("ul.divide-y > li").count();
  const secondSummary = (await page.getByText(/2\s*\/\s*2|2\/2/).first().innerText()).trim();
  check("page2-rows", secondPageRows === 5, { rows: secondPageRows });
  check("page2-summary", secondSummary.includes("2/2") || secondSummary.includes("2 / 2"), secondSummary);
  check("page2-next-disabled", await next.isDisabled(), "next should be disabled on the last page");

  const overflow = await page.evaluate(() => ({
    scroll: document.documentElement.scrollWidth,
    inner: window.innerWidth,
  }));
  check("no-overflow-1440", overflow.scroll <= overflow.inner + 1, overflow);
  summary.screenshots.page2_1440 = path.join(outDir, "executions-page2-1440.png");
  await page.screenshot({ path: summary.screenshots.page2_1440, fullPage: true });

  await page.setViewportSize({ width: 607, height: 900 });
  await page.waitForTimeout(300);
  const narrow = await page.evaluate(() => ({
    scroll: document.documentElement.scrollWidth,
    inner: window.innerWidth,
  }));
  check("no-overflow-607", narrow.scroll <= narrow.inner + 1, narrow);
  summary.screenshots.page2_607 = path.join(outDir, "executions-page2-607.png");
  await page.screenshot({ path: summary.screenshots.page2_607, fullPage: true });
} catch (error) {
  fail(error instanceof Error ? error.message : String(error));
} finally {
  await browser.close();
  server.kill("SIGKILL");
  fs.rmSync(dataDir, { recursive: true, force: true, maxRetries: 5 });
}

console.log(JSON.stringify(summary, null, 2));
if (summary.errors.length > 0) process.exit(1);
