import fs from "node:fs";
import playwright from "file:///Users/sainm/work/demo/BootDoc/node_modules/playwright/index.js";

const { chromium } = playwright;

const baseUrl = (process.env.HPA_BASE_URL || "http://127.0.0.1:8787").replace(/\/+$/, "");
const baseOrigin = new URL(baseUrl).origin;
const mockPortRaw = process.env.OIDC_MOCK_PORT || "18970";
const mockPort = Number(mockPortRaw);
const issuer = `http://127.0.0.1:${mockPort}`;
const clientId = process.env.OIDC_CLIENT_ID || "hpa-local-ui";
const email = process.env.OIDC_EMAIL || "admin@example.test";
const subject = process.env.OIDC_SUBJECT || "oidc|browser-admin";
const scopes = process.env.OIDC_SCOPES || "openid email profile";
const redirectUri = `${baseUrl}/auth/oidc/callback`;

const screenshots = {
  login1440: "/tmp/phase9-login-1440.png",
  login607: "/tmp/phase9-login-607.png",
  loggedIn1440: "/tmp/phase9-sso-logged-in-1440.png",
  loggedIn607: "/tmp/phase9-sso-logged-in-607.png",
  error1440: "/tmp/phase9-sso-error-1440.png",
};

const summary = {
  login: null,
  session: null,
  logout: null,
  errors: [],
  screenshots: {
    login1440: null,
    login607: null,
    loggedIn1440: null,
    loggedIn607: null,
    error1440: null,
  },
};

function sanitize(value) {
  return String(value ?? "")
    .replace(/([?&](?:code|state|error|code_verifier|access_token|id_token|token)=)[^&\s"'<>]+/gi, "$1[redacted]")
    .replace(/(Bearer\s+)[A-Za-z0-9._~+/-]+=*/gi, "$1[redacted]");
}

function delay(ms) {
  return new Promise((resolve) => setTimeout(resolve, ms));
}

function loadAdminToken() {
  const tokenFile = process.env.HPA_TOKEN_FILE;
  if (tokenFile) {
    const token = fs.readFileSync(tokenFile, "utf8").trim();
    if (!token) throw new Error("HPA_TOKEN_FILE is empty");
    return token;
  }
  const token = (process.env.HPA_TOKEN || "").trim();
  if (!token) throw new Error("HPA_TOKEN_FILE is required");
  return token;
}

function safePath(value) {
  try {
    const parsed = new URL(value);
    return parsed.pathname;
  } catch {
    return String(value);
  }
}

async function screenshot(page, file) {
  await page.screenshot({ path: file, animations: "disabled" });
  return file;
}

async function readSession(api) {
  const response = await api.get(`${baseUrl}/auth/session`, {
    headers: { accept: "application/json" },
    timeout: 10_000,
  });
  let body = null;
  try {
    body = await response.json();
  } catch {
    body = null;
  }
  return { status: response.status(), body };
}

async function waitForSession(api, predicate, timeoutMs) {
  const deadline = Date.now() + timeoutMs;
  let last = null;
  while (Date.now() < deadline) {
    last = await readSession(api);
    if (predicate(last)) return last;
    await delay(250);
  }
  throw new Error(`session condition not met (last status ${last ? last.status : "none"})`);
}

async function checkIssuer() {
  let response;
  try {
    response = await fetch(`${issuer}/.well-known/openid-configuration`, {
      signal: AbortSignal.timeout(5_000),
    });
  } catch (error) {
    const code = error && error.cause && error.cause.code ? ` (${error.cause.code})` : "";
    throw new Error(`mock IdP is unreachable at ${issuer}${code}`);
  }
  if (!response.ok) {
    throw new Error(`mock IdP discovery failed with HTTP ${response.status}`);
  }
  const discovery = await response.json();
  if (discovery.issuer !== issuer) {
    throw new Error("mock IdP issuer does not match OIDC_MOCK_PORT");
  }
}

async function configureOidc(adminToken) {
  const payload = {
    issuer,
    audience: clientId,
    jwksUrl: `${issuer}/jwks`,
    network: [`127.0.0.1:${mockPort}`],
    clientId,
    authorizationEndpoint: `${issuer}/authorize`,
    tokenEndpoint: `${issuer}/token`,
    redirectUri,
    scopes,
    adminSubjects: [email, subject],
    cookieSecure: false,
    sessionTtlSeconds: 3600,
  };
  const response = await fetch(`${baseUrl}/api/enterprise/oidc`, {
    method: "PUT",
    headers: {
      authorization: `Bearer ${adminToken}`,
      "content-type": "application/json",
      accept: "application/json",
    },
    body: JSON.stringify(payload),
    signal: AbortSignal.timeout(10_000),
  });
  if (!response.ok) {
    let detail = "";
    try {
      detail = (await response.text()).slice(0, 300);
    } catch {
      detail = "";
    }
    throw new Error(`OIDC config update failed with HTTP ${response.status}${detail ? `: ${detail}` : ""}`);
  }
}

async function findSsoControl(page) {
  const pattern = /ログイン|Sign in|登录|SSO/i;
  const button = page.getByRole("button", { name: pattern }).first();
  if (await button.count()) return button;
  const link = page.getByRole("link", { name: pattern }).first();
  if (await link.count()) return link;
  const text = page.getByText(pattern).first();
  if (await text.count()) return text;
  throw new Error("SSO login control not found on /login");
}

async function findLogoutControl(page) {
  const pattern = /ログアウト|Sign out|Log out|退出|登出/i;
  const button = page.getByRole("button", { name: pattern }).first();
  if (await button.count()) return button;
  const link = page.getByRole("link", { name: pattern }).first();
  if (await link.count()) return link;
  const text = page.getByText(pattern).first();
  if (await text.count()) return text;
  throw new Error("logout control not found");
}

async function logout(page, context) {
  const control = await findLogoutControl(page);
  await control.click();
  const dialog = page.getByRole("dialog");
  if (await dialog.count()) {
    const confirm = dialog
      .getByRole("button", { name: /確認|Confirm|ログアウト|Sign out|Log out|退出|登出/i })
      .last();
    if (await confirm.count()) await confirm.click();
  }
  const result = await waitForSession(
    context.request,
    (current) => current.body !== null && current.body.authenticated === false,
    15_000,
  );
  return result.body;
}

async function main() {
  if (!Number.isInteger(mockPort) || mockPort < 1 || mockPort > 65535) {
    throw new Error(`invalid OIDC_MOCK_PORT: ${mockPortRaw}`);
  }

  const adminToken = loadAdminToken();
  await checkIssuer();
  await configureOidc(adminToken);

  const browser = await chromium.launch({ headless: true });
  let context = null;
  try {
    context = await browser.newContext({
      viewport: { width: 1440, height: 900 },
      ignoreHTTPSErrors: true,
      locale: "en-US",
    });
    const page = await context.newPage();
    page.setDefaultTimeout(15_000);
    page.setDefaultNavigationTimeout(30_000);
    page.on("dialog", (dialog) => {
      dialog.accept().catch(() => {});
    });

    const unauthorized = [];
    page.on("response", (response) => {
      try {
        const parsed = new URL(response.url());
        if (
          parsed.origin === baseOrigin &&
          parsed.pathname.startsWith("/api/enterprise/") &&
          response.status() === 401
        ) {
          unauthorized.push(parsed.pathname);
        }
      } catch {
        return;
      }
    });

    await page.goto(`${baseUrl}/login`, { waitUntil: "domcontentloaded" });
    const sso = await findSsoControl(page);
    await sso.waitFor({ state: "visible" });
    summary.screenshots.login1440 = await screenshot(page, screenshots.login1440);

    await page.setViewportSize({ width: 607, height: 900 });
    await page.waitForFunction(() => window.innerWidth === 607);
    summary.screenshots.login607 = await screenshot(page, screenshots.login607);

    await page.setViewportSize({ width: 1440, height: 900 });
    await page.waitForFunction(() => window.innerWidth === 1440);

    const started = Date.now();
    const success = page.waitForURL(
      (url) =>
        url.origin === baseOrigin && (url.pathname === "/enterprise" || url.pathname === "/"),
      { timeout: 30_000 },
    );
    const failure = page.waitForURL(
      (url) =>
        url.origin === baseOrigin && url.pathname === "/login" && url.searchParams.has("error"),
      { timeout: 30_000 },
    );
    success.catch(() => {});
    failure.catch(() => {});
    await Promise.all([sso.click(), Promise.race([success, failure])]);
    const landed = new URL(page.url());
    if (landed.origin === baseOrigin && landed.pathname === "/login") {
      throw new Error(`SSO login failed: ${landed.searchParams.get("error") || "login_error"}`);
    }
    await page.waitForLoadState("domcontentloaded").catch(() => {});
    summary.login = {
      ok: true,
      url: safePath(page.url()),
      durationMs: Date.now() - started,
    };

    const sessionResult = await waitForSession(
      context.request,
      (current) => current.body !== null && current.body.authenticated === true,
      15_000,
    );
    const session = sessionResult.body;
    const principal = session && session.principal;
    if (!principal || typeof principal !== "object") {
      throw new Error("authenticated session is missing principal");
    }
    if (String(principal.email || "").toLowerCase() !== email.toLowerCase()) {
      throw new Error("session principal email does not match OIDC_EMAIL");
    }
    const isAdmin = principal.platformAdmin === true || principal.admin === true;
    if (!isAdmin) {
      throw new Error("session principal is not a platform admin");
    }
    summary.session = {
      authenticated: true,
      tenantId: principal.tenantId ?? null,
      role: principal.role ?? null,
      email: principal.email ?? null,
      platformAdmin: true,
    };

    const response = await page.goto(`${baseUrl}/enterprise`, { waitUntil: "domcontentloaded" });
    if (response && response.status() >= 500) {
      throw new Error(`enterprise page failed with HTTP ${response.status()}`);
    }
    await page.waitForFunction(
      () => document.body !== null && document.body.innerText.trim().length > 0,
      null,
      { timeout: 15_000 },
    );
    await page.waitForLoadState("networkidle", { timeout: 5_000 }).catch(() => {});
    const bodyText = await page.locator("body").innerText();
    if (/(?:^|\s)401(?:\s|$)|unauthorized/i.test(bodyText)) {
      throw new Error("enterprise page shows unauthorized text");
    }
    if (unauthorized.length) {
      throw new Error(`enterprise API returned 401: ${unauthorized.join(", ")}`);
    }
    summary.screenshots.loggedIn1440 = await screenshot(page, screenshots.loggedIn1440);

    await page.setViewportSize({ width: 607, height: 900 });
    await page.waitForFunction(() => window.innerWidth === 607);
    summary.screenshots.loggedIn607 = await screenshot(page, screenshots.loggedIn607);

    await page.setViewportSize({ width: 1440, height: 900 });
    await page.waitForFunction(() => window.innerWidth === 1440);

    const loggedOut = await logout(page, context);
    summary.logout = { ok: loggedOut.authenticated === false, authenticated: loggedOut.authenticated };

    await page.goto(`${baseUrl}/auth/oidc/callback?code=bad&state=bad`, {
      waitUntil: "domcontentloaded",
    });
    await page.waitForURL(
      (url) =>
        url.origin === baseOrigin && url.pathname === "/login" && url.searchParams.has("error"),
      { timeout: 15_000 },
    );
    const errorCode = new URL(page.url()).searchParams.get("error");
    if (!errorCode) throw new Error("callback error redirect is missing error code");
    summary.screenshots.error1440 = await screenshot(page, screenshots.error1440);
    if (summary.login) summary.login.callbackError = { ok: true, error: errorCode };
  } finally {
    if (context) await context.close().catch(() => {});
    await browser.close().catch(() => {});
  }
}

try {
  await main();
} catch (error) {
  summary.errors.push(sanitize(error && error.message ? error.message : error));
} finally {
  if (!summary.login) summary.login = { ok: false };
  if (!summary.session) summary.session = { authenticated: false };
  if (!summary.logout) summary.logout = { ok: false };
  console.log(JSON.stringify(summary, null, 2));
  process.exitCode = summary.errors.length ? 1 : 0;
}
