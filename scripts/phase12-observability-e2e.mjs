import fs from "node:fs";
import path from "node:path";
import { chromium } from "playwright";

const baseUrl = (process.env.HPA_BASE_URL || "http://127.0.0.1:8787").replace(/\/+$/, "");
const outDir = process.env.HPA_SHOT_DIR || "docs/assets/phase12";

const summary = {
  ready: null,
  metrics: {},
  timeline: {},
  overflow: {},
  screenshots: {},
  errors: [],
};

function fail(message) {
  summary.errors.push(message);
  console.error("FAIL:", message);
}

async function jsonFetch(url, init) {
  const response = await fetch(url, init);
  const text = await response.text();
  let body = null;
  try {
    body = text ? JSON.parse(text) : null;
  } catch {
    body = text;
  }
  return { status: response.status, body, text };
}

function workflowIr() {
  return {
    $schema: "https://hpa.dev/schema/ir/v0.1/workflow.schema.json",
    schemaVersion: "hpa.dev/v1",
    id: "wf_trace",
    name: "Trace demo",
    workflowVersion: "1.0.0",
    nodes: [
      { id: "start", type: "start", typeVersion: "1.0.0", name: "Start", config: {} },
      {
        id: "enrich",
        type: "transform",
        typeVersion: "1.0.0",
        name: "Enrich",
        config: { mapping: { value: { $expr: "1 + 1" } } },
      },
      {
        id: "shape",
        type: "transform",
        typeVersion: "1.0.0",
        name: "Shape",
        config: { mapping: { total: { $expr: "1 + 1" } } },
      },
      { id: "end", type: "end", typeVersion: "1.0.0", name: "End", config: {} },
    ],
    edges: [
      { id: "e1", source: "start", sourcePort: "out", target: "enrich", targetPort: "in" },
      { id: "e2", source: "enrich", sourcePort: "out", target: "shape", targetPort: "in" },
      { id: "e3", source: "shape", sourcePort: "out", target: "end", targetPort: "in" },
    ],
  };
}

async function overflow(page, label) {
  const metrics = await page.evaluate(() => ({
    scrollWidth: document.documentElement.scrollWidth,
    innerWidth: window.innerWidth,
  }));
  const ok = metrics.scrollWidth <= metrics.innerWidth + 1;
  summary.overflow[label] = { ...metrics, ok };
  if (!ok) fail(`${label} overflows horizontally (${metrics.scrollWidth} > ${metrics.innerWidth})`);
}

async function main() {
  fs.mkdirSync(outDir, { recursive: true });

  const ready = await jsonFetch(`${baseUrl}/api/ready`);
  summary.ready = { status: ready.status, body: ready.body };
  if (ready.status !== 200) fail(`readiness failed: ${ready.status} ${JSON.stringify(ready.body)}`);

  const created = await jsonFetch(`${baseUrl}/api/workflows`, {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify({ ir: workflowIr() }),
  });
  if (created.status !== 200) fail(`create failed: ${created.status} ${created.text}`);
  const published = await jsonFetch(`${baseUrl}/api/workflows/wf_trace/publish`, { method: "POST" });
  if (published.status !== 200) fail(`publish failed: ${published.status} ${published.text}`);

  const started = await jsonFetch(`${baseUrl}/api/workflows/wf_trace/executions`, {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify({ input: {} }),
  });
  if (started.status !== 200) fail(`execute failed: ${started.status} ${started.text}`);
  const executionId = started.body?.summary?.id;
  if (!executionId) {
    fail("no execution id returned");
    return;
  }

  let status = "";
  for (let index = 0; index < 200; index += 1) {
    const detail = await jsonFetch(`${baseUrl}/api/executions/${executionId}`);
    status = detail.body?.summary?.status ?? "";
    if (status === "succeeded" || status === "failed") break;
    await new Promise((resolve) => setTimeout(resolve, 50));
  }
  if (status !== "succeeded") fail(`execution finished as ${status}`);

  const metrics = await fetch(`${baseUrl}/metrics`);
  const text = await metrics.text();
  summary.metrics = {
    status: metrics.status,
    contentType: metrics.headers.get("content-type"),
    hasExecutionCounter: /hpa_workflow_executions_total\{status="succeeded"\} \d+/.test(text),
    hasNodeHistogram: /hpa_node_seconds_bucket\{type="transform",le="\+Inf"\}/.test(text),
    hasSchedulerTick: text.includes("hpa_scheduler_last_tick_timestamp_seconds"),
    hasRoute: text.includes(`route="/api/executions/{id}"`),
    workflowGauge: /hpa_workflows_total \d+/.exec(text)?.[0] ?? null,
  };
  for (const [key, value] of Object.entries(summary.metrics)) {
    if (typeof value === "boolean" && !value) fail(`metrics missing ${key}`);
  }

  const browser = await chromium.launch({ headless: true });
  const context = await browser.newContext({ viewport: { width: 1440, height: 900 } });
  const page = await context.newPage();
  page.on("pageerror", (error) => fail(`page error: ${error.message}`));

  await page.goto(`${baseUrl}/executions/${executionId}`, { waitUntil: "networkidle" });
  await page.getByRole("heading", { name: "ステップタイムライン" }).waitFor({ timeout: 8000 });
  await page.waitForTimeout(400);

  summary.timeline = {
    bars: await page.locator("li[title], ol li div[title]").count(),
    nodes: await page.getByText("enrich", { exact: true }).count(),
    slowestBadge: await page.getByText(/最も遅い/).count(),
  };
  if (summary.timeline.nodes === 0) fail("timeline does not list the transform node");
  if (summary.timeline.slowestBadge === 0) fail("timeline does not show the slowest badge");

  await overflow(page, "timeline-1440");
  summary.screenshots.timeline1440 = path.join(outDir, "timeline-1440.png");
  await page.screenshot({ path: summary.screenshots.timeline1440, fullPage: true });

  await page.setViewportSize({ width: 607, height: 900 });
  await page.waitForTimeout(300);
  await overflow(page, "timeline-607");
  summary.screenshots.timeline607 = path.join(outDir, "timeline-607.png");
  await page.screenshot({ path: summary.screenshots.timeline607, fullPage: true });

  // Runtime health panel on the settings page.
  await page.setViewportSize({ width: 1440, height: 900 });
  await page.goto(`${baseUrl}/settings`, { waitUntil: "networkidle" });
  await page.getByRole("heading", { name: "ランタイムの状態" }).waitFor({ timeout: 8000 });
  await page.waitForTimeout(400);
  const readyChips = await page.getByText("ok", { exact: true }).count();
  summary.healthPanel = { okChips: readyChips };
  if (readyChips < 4) fail(`expected 4 healthy checks, saw ${readyChips}`);
  await overflow(page, "settings-1440");
  summary.screenshots.settings1440 = path.join(outDir, "settings-runtime-1440.png");
  await page.screenshot({ path: summary.screenshots.settings1440, fullPage: true });

  await page.setViewportSize({ width: 607, height: 900 });
  await page.waitForTimeout(300);
  await overflow(page, "settings-607");
  summary.screenshots.settings607 = path.join(outDir, "settings-runtime-607.png");
  await page.screenshot({ path: summary.screenshots.settings607, fullPage: true });

  await browser.close();
}

await main();
console.log(JSON.stringify(summary, null, 2));
if (summary.errors.length > 0) process.exit(1);
