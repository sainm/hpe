import fs from "node:fs";
import http from "node:http";
import path from "node:path";
import playwright from "file:///Users/sainm/work/demo/BootDoc/node_modules/playwright/index.js";

const { chromium } = playwright;

const baseUrl = (process.env.HPA_BASE_URL || "http://127.0.0.1:8787").replace(/\/+$/, "");
const callbackPort = Number(process.env.HPA_CALLBACK_PORT || "4180");
const redirectUri = `http://127.0.0.1:${callbackPort}/callback`;
const outDir = process.env.HPA_SHOT_DIR || "docs/assets/phase10";

const summary = {
  authorize: null,
  consent: null,
  token: null,
  mcp: null,
  enterprise: null,
  overflow: {},
  errors: [],
  screenshots: {},
};

function fail(message) {
  summary.errors.push(message);
  console.error("FAIL:", message);
}

async function pkcePair() {
  const bytes = new Uint8Array(48);
  crypto.getRandomValues(bytes);
  const verifier = Buffer.from(bytes).toString("base64url");
  const digest = await crypto.subtle.digest("SHA-256", new TextEncoder().encode(verifier));
  return { verifier, challenge: Buffer.from(new Uint8Array(digest)).toString("base64url") };
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
  return { status: response.status, body, headers: response.headers };
}

function startCallbackServer() {
  const state = { code: null, url: null, denied: false };
  const server = http.createServer((request, response) => {
    const url = new URL(request.url, `http://127.0.0.1:${callbackPort}`);
    state.url = url.toString();
    state.code = url.searchParams.get("code");
    state.denied = url.searchParams.get("error") !== null;
    response.writeHead(200, { "content-type": "text/html; charset=utf-8" });
    response.end(
      `<!doctype html><meta charset="utf-8"><title>callback</title>` +
        `<body style="font-family:system-ui;padding:24px">` +
        `<p>code received: ${state.code ? "yes" : "no"}</p></body>`,
    );
  });
  return new Promise((resolve) => {
    server.listen(callbackPort, "127.0.0.1", () => resolve({ server, state }));
  });
}

async function overflow(page, label) {
  const metrics = await page.evaluate(() => ({
    scrollWidth: document.documentElement.scrollWidth,
    innerWidth: window.innerWidth,
  }));
  const ok = metrics.scrollWidth <= metrics.innerWidth + 1;
  summary.overflow[label] = { ...metrics, ok };
  if (!ok) fail(`${label} overflows horizontally (${metrics.scrollWidth} > ${metrics.innerWidth})`);
  return ok;
}

async function main() {
  fs.mkdirSync(outDir, { recursive: true });
  const { server, state } = await startCallbackServer();

  const registration = await jsonFetch(`${baseUrl}/oauth/register`, {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify({
      client_name: "Playwright MCP client",
      redirect_uris: [redirectUri],
      token_endpoint_auth_method: "none",
      scope: "mcp:read mcp:execute offline_access",
    }),
  });
  if (registration.status !== 201) {
    fail(`registration failed: ${registration.status} ${JSON.stringify(registration.body)}`);
    server.close();
    return;
  }
  const clientId = registration.body.client_id;
  summary.authorize = { clientId };

  const { verifier, challenge } = await pkcePair();
  const authorizeUrl = new URL(`${baseUrl}/oauth/authorize`);
  authorizeUrl.searchParams.set("response_type", "code");
  authorizeUrl.searchParams.set("client_id", clientId);
  authorizeUrl.searchParams.set("redirect_uri", redirectUri);
  authorizeUrl.searchParams.set("scope", "mcp:read mcp:execute offline_access");
  authorizeUrl.searchParams.set("state", "playwright-state");
  authorizeUrl.searchParams.set("code_challenge", challenge);
  authorizeUrl.searchParams.set("code_challenge_method", "S256");
  authorizeUrl.searchParams.set("resource", `${baseUrl}/mcp`);

  const browser = await chromium.launch({ headless: true });
  const context = await browser.newContext({ viewport: { width: 1440, height: 900 } });
  const page = await context.newPage();
  page.on("pageerror", (error) => fail(`page error: ${error.message}`));

  await page.goto(authorizeUrl.toString(), { waitUntil: "networkidle" });
  summary.consent = { url: page.url().replace(/request=[^&]+/, "request=[redacted]") };
  if (!page.url().includes("/consent?request=")) {
    fail(`authorize did not reach the consent page: ${page.url()}`);
  }
  await page.getByRole("heading", { name: /アクセスを許可しますか/ }).waitFor({ timeout: 8000 });
  await overflow(page, "consent-1440");
  summary.screenshots.consent1440 = path.join(outDir, "consent-1440.png");
  await page.screenshot({ path: summary.screenshots.consent1440, fullPage: true });

  await page.setViewportSize({ width: 607, height: 900 });
  await page.waitForTimeout(300);
  await overflow(page, "consent-607");
  summary.screenshots.consent607 = path.join(outDir, "consent-607.png");
  await page.screenshot({ path: summary.screenshots.consent607, fullPage: true });
  await page.setViewportSize({ width: 1440, height: 900 });

  await page.getByRole("button", { name: /許可する/ }).click();
  for (let index = 0; index < 100 && !state.code; index += 1) {
    await page.waitForTimeout(100);
  }
  if (!state.code) {
    fail("consent did not redirect back with an authorization code");
  }

  const token = await jsonFetch(`${baseUrl}/oauth/token`, {
    method: "POST",
    headers: { "content-type": "application/x-www-form-urlencoded" },
    body: new URLSearchParams({
      grant_type: "authorization_code",
      code: state.code ?? "",
      redirect_uri: redirectUri,
      client_id: clientId,
      code_verifier: verifier,
    }),
  });
  if (token.status !== 200) {
    fail(`token exchange failed: ${token.status} ${JSON.stringify(token.body)}`);
  }
  summary.token = {
    status: token.status,
    tokenType: token.body?.token_type,
    expiresIn: token.body?.expires_in,
    scope: token.body?.scope,
    hasRefresh: Boolean(token.body?.refresh_token),
  };

  const discover = await jsonFetch(`${baseUrl}/mcp`, {
    method: "POST",
    headers: {
      "content-type": "application/json",
      authorization: `Bearer ${token.body?.access_token ?? ""}`,
      "mcp-protocol-version": "2026-07-28",
      "mcp-method": "server/discover",
    },
    body: JSON.stringify({
      jsonrpc: "2.0",
      id: 1,
      method: "server/discover",
      params: {
        _meta: {
          "io.modelcontextprotocol/protocolVersion": "2026-07-28",
          "io.modelcontextprotocol/clientCapabilities": {},
          "io.modelcontextprotocol/clientInfo": { name: "playwright", version: "1" },
        },
      },
    }),
  });
  summary.mcp = {
    status: discover.status,
    resultType: discover.body?.result?.resultType ?? null,
  };
  if (discover.status !== 200) fail(`MCP call with OAuth token failed: ${discover.status}`);

  await page.goto(`${baseUrl}/enterprise`, { waitUntil: "networkidle" });
  const localButton = page.getByRole("button", { name: /トークンなしで試す/ });
  if (await localButton.count()) {
    await localButton.first().click();
    await page.waitForTimeout(400);
  }
  await page.getByRole("button", { name: "OAuth 2.1" }).click();
  await page.getByRole("heading", { name: /OAuth 2\.1 認可サーバー/ }).waitFor({ timeout: 8000 });
  await page.waitForTimeout(500);
  summary.enterprise = { clientRows: await page.getByText("Playwright MCP client").count() };
  if (summary.enterprise.clientRows === 0) fail("registered client is not listed in the console");
  await overflow(page, "enterprise-oauth-1440");
  summary.screenshots.enterprise1440 = path.join(outDir, "enterprise-oauth-1440.png");
  await page.screenshot({ path: summary.screenshots.enterprise1440, fullPage: true });

  await page.setViewportSize({ width: 607, height: 900 });
  await page.waitForTimeout(300);
  await overflow(page, "enterprise-oauth-607");
  summary.screenshots.enterprise607 = path.join(outDir, "enterprise-oauth-607.png");
  await page.screenshot({ path: summary.screenshots.enterprise607, fullPage: true });

  await browser.close();
  server.close();
}

await main();
console.log(JSON.stringify(summary, null, 2));
if (summary.errors.length > 0) process.exit(1);
