import crypto from "node:crypto";
import AxeBuilder from "@axe-core/playwright";
import { chromium } from "playwright";

const baseUrl = (process.env.HPA_BASE_URL || "http://127.0.0.1:8787").replace(/\/+$/, "");
const dataDir = process.env.HPA_DATA_DIR;
const suffix = `${Date.now()}-${process.pid}`;
const sourceId = `ui-source-${suffix}`;
const publisherId = `ui-publisher-${suffix}`;
const sourceLabel = `UI Source ${suffix}`;

async function request(path, body) {
  const response = await fetch(`${baseUrl}${path}`, {
    method: "POST",
    headers: { "content-type": "application/json", accept: "application/json" },
    body: JSON.stringify(body),
    signal: AbortSignal.timeout(10_000),
  });
  if (!response.ok) {
    throw new Error(`${path} returned HTTP ${response.status}: ${(await response.text()).slice(0, 300)}`);
  }
  return response.json();
}

const keyPair = crypto.generateKeyPairSync("ed25519");
const jwk = keyPair.publicKey.export({ format: "jwk" });
const publicKey = Buffer.from(jwk.x, "base64url").toString("base64");

await request("/api/marketplace/publishers", {
  id: publisherId,
  name: "UI Publisher",
});
await request(`/api/marketplace/publishers/${encodeURIComponent(publisherId)}/keys`, {
  keyId: `ui-publisher-key-${suffix}`,
  publicKey,
  label: "UI fixture",
});
await request("/api/marketplace/sources", {
  id: sourceId,
  label: sourceLabel,
  indexUri: "http://127.0.0.1:9/index.json",
  keyId: `ui-source-key-${suffix}`,
  publicKey,
  keyLabel: "UI fixture",
});

const browser = await chromium.launch({ headless: true });
const results = [];
try {
  for (const viewport of [
    { width: 1440, height: 900 },
    { width: 607, height: 900 },
  ]) {
    const context = await browser.newContext({ viewport, locale: "en-US" });
    const page = await context.newPage();
    page.setDefaultTimeout(15_000);
    await page.goto(`${baseUrl}/plugins`, { waitUntil: "networkidle" });
    await page.getByTestId("plugins-tab-marketplace").click();
    await page.getByTestId("marketplace-remote-toggle").click();
    await page.getByText(sourceLabel, { exact: true }).first().waitFor();
    await page.getByText(publisherId, { exact: true }).first().waitFor();

    let withdrawalPush = false;
    if (viewport.width === 1440 && dataDir) {
      await request("/api/plugins/sample", {});
      await request("/api/marketplace/publish", {
        path: `${dataDir}/plugins/samples/plugin_sample_kit-1.0.0.hplugin`,
      });
      await request(
        "/api/marketplace/plugin_sample_kit/versions/1.0.0/review",
        { status: "approved", note: "SSE E2E" },
      );
      await request(
        "/api/marketplace/plugin_sample_kit/versions/1.0.0/withdraw",
        {},
      );
      await page.getByText(/plugin_sample_kit@1\.0\.0/).waitFor({
        timeout: 10_000,
      });
      withdrawalPush = true;
    }

    const overflow = await page.evaluate(
      () => document.documentElement.scrollWidth > window.innerWidth,
    );
    if (overflow) throw new Error(`remote marketplace overflows at ${viewport.width}px`);
    const axe = await new AxeBuilder({ page })
      .withTags(["wcag2a", "wcag2aa", "wcag21a", "wcag21aa", "wcag22aa"])
      .analyze();
    const serious = axe.violations.filter((violation) =>
      ["critical", "serious"].includes(violation.impact || ""),
    );
    if (serious.length) {
      throw new Error(
        `remote marketplace axe failures: ${serious
          .map(
            (violation) =>
              `${violation.id} [${violation.nodes
                .map(
                  (node) =>
                    `${node.target.join(" ")} :: ${node.html.slice(0, 240)} :: ${node.failureSummary ?? ""}`,
                )
                .join(" | ")}]`,
          )
          .join(", ")}`,
      );
    }
    const screenshot = `/tmp/phase13-marketplace-${viewport.width}.png`;
    await page.screenshot({ path: screenshot, animations: "disabled" });
    results.push({
      viewport: viewport.width,
      sourceVisible: true,
      publisherVisible: true,
      horizontalOverflow: false,
      seriousAxeViolations: 0,
      withdrawalPush,
      screenshot,
    });
    await context.close();
  }
} finally {
  await browser.close();
}

console.log(JSON.stringify({ ok: true, sourceId, publisherId, results }, null, 2));
