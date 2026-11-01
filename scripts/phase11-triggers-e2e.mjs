import crypto from "node:crypto";
import fs from "node:fs";
import path from "node:path";
import playwright from "file:///Users/sainm/work/demo/BootDoc/node_modules/playwright/index.js";

const { chromium } = playwright;

const baseUrl = (process.env.HPA_BASE_URL || "http://127.0.0.1:8787").replace(/\/+$/, "");
const outDir = process.env.HPA_SHOT_DIR || "docs/assets/phase11";
const SECRET = "phase11-webhook-secret";

const summary = {
  workflow: null,
  triggers: null,
  webhook: null,
  manualFire: null,
  ui: {},
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
  return { status: response.status, body };
}

function workflowIr() {
  return {
    $schema: "https://hpa.dev/schema/ir/v0.1/workflow.schema.json",
    schemaVersion: "hpa.dev/v1",
    id: "wf_phase11",
    name: "Phase 11 demo",
    workflowVersion: "1.0.0",
    nodes: [
      { id: "start", type: "start", typeVersion: "1.0.0", name: "Start", config: {} },
      { id: "end", type: "end", typeVersion: "1.0.0", name: "End", config: {} },
    ],
    edges: [
      { id: "e1", source: "start", sourcePort: "out", target: "end", targetPort: "in" },
    ],
    triggers: [
      {
        id: "nightly-report",
        type: "schedule",
        config: { cron: "0 9 * * *", timezone: "+08:00", input: { window: "daily" } },
      },
      {
        id: "github-release",
        type: "webhook",
        config: { secret: SECRET },
      },
      { id: "order-events", type: "event", config: {} },
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

function signedHeaders(body, timestamp, eventId) {
  const signature = crypto
    .createHmac("sha256", SECRET)
    .update(`${timestamp}.${body}`)
    .digest("hex");
  const headers = {
    "content-type": "application/json",
    "x-hpa-timestamp": String(timestamp),
    "x-hpa-signature": `sha256=${signature}`,
  };
  if (eventId) headers["x-hpa-event-id"] = eventId;
  return headers;
}

async function main() {
  fs.mkdirSync(outDir, { recursive: true });

  const created = await jsonFetch(`${baseUrl}/api/workflows`, {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify({ ir: workflowIr() }),
  });
  if (created.status !== 200) {
    fail(`workflow create failed: ${created.status} ${JSON.stringify(created.body)}`);
  }
  const published = await jsonFetch(`${baseUrl}/api/workflows/wf_phase11/publish`, { method: "POST" });
  if (published.status !== 200) {
    fail(`publish failed: ${published.status} ${JSON.stringify(published.body)}`);
  }
  summary.workflow = { create: created.status, publish: published.status };

  const listed = await jsonFetch(`${baseUrl}/api/triggers`);
  const triggers = listed.body?.triggers ?? [];
  summary.triggers = triggers.map((trigger) => ({
    id: trigger.triggerId,
    kind: trigger.kind,
    enabled: trigger.enabled,
    nextFireAt: trigger.nextFireAt,
    hasSecret: trigger.webhook?.hasSecret ?? null,
  }));
  if (triggers.length !== 3) fail(`expected 3 triggers, got ${triggers.length}`);

  const body = JSON.stringify({ action: "published", tag: "v1.2.0" });
  const delivered = await jsonFetch(`${baseUrl}/hooks/wf_phase11/github-release`, {
    method: "POST",
    headers: signedHeaders(body, Math.floor(Date.now() / 1000), "release-1"),
    body,
  });
  summary.webhook = { status: delivered.status, executionId: delivered.body?.executionId ?? null };
  if (delivered.status !== 202) fail(`webhook delivery failed: ${delivered.status}`);

  const manual = await jsonFetch(`${baseUrl}/api/triggers/wf_phase11/nightly-report/fire`, {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify({ input: { window: "manual" } }),
  });
  summary.manualFire = { status: manual.status, executionId: manual.body?.executionId ?? null };
  if (manual.status !== 200) fail(`manual fire failed: ${manual.status}`);

  const browser = await chromium.launch({ headless: true });
  const context = await browser.newContext({ viewport: { width: 1440, height: 900 } });
  const page = await context.newPage();
  page.on("pageerror", (error) => fail(`page error: ${error.message}`));

  await page.goto(`${baseUrl}/triggers`, { waitUntil: "networkidle" });
  await page.getByRole("heading", { name: "トリガー", exact: true }).waitFor({ timeout: 8000 });
  await page.getByText("nightly-report").first().waitFor({ timeout: 8000 });
  await page.getByText("github-release").first().waitFor({ timeout: 8000 });

  summary.ui.triggers = await page.locator("main .rounded-md, main [class*='rounded']").count();
  summary.ui.fireRows = await page.getByText("github-release ›").count();

  await overflow(page, "triggers-1440");
  summary.screenshots.main1440 = path.join(outDir, "triggers-1440.png");
  await page.screenshot({ path: summary.screenshots.main1440, fullPage: true });

  await page.setViewportSize({ width: 607, height: 900 });
  await page.waitForTimeout(300);
  await overflow(page, "triggers-607");
  summary.screenshots.main607 = path.join(outDir, "triggers-607.png");
  await page.screenshot({ path: summary.screenshots.main607, fullPage: true });
  await page.setViewportSize({ width: 1440, height: 900 });

  // Toggle the webhook trigger off and back on.
  const card = page.locator("div").filter({ hasText: "github-release" }).first();
  void card;
  await page.getByRole("button", { name: "無効化" }).first().click();
  await page.getByText("トリガーを無効化しました").waitFor({ timeout: 5000 });
  await page.waitForTimeout(600);
  summary.ui.disabledBadges = await page.getByText("無効", { exact: true }).count();
  if (summary.ui.disabledBadges === 0) fail("disable action did not update the UI");

  await page.getByRole("button", { name: "有効化" }).first().click();
  await page.getByText("トリガーを有効化しました").waitFor({ timeout: 5000 });

  // Fire from the UI.
  await page.getByRole("button", { name: "今すぐ実行" }).first().click();
  await page.getByText(/実行を開始しました/).waitFor({ timeout: 5000 });

  // Reload to pick up the fired history.
  await page.reload({ waitUntil: "networkidle" });
  await page.waitForTimeout(800);
  summary.ui.firesAfterReload = await page.getByText(/exec_/).count();

  await browser.close();
}

await main();
console.log(JSON.stringify(summary, null, 2));
if (summary.errors.length > 0) process.exit(1);
