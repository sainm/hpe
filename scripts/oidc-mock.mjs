import http from "node:http";
import crypto from "node:crypto";

const supportedAlgs = new Set(["EdDSA", "RS256", "ES256"]);
const alg = (process.env.OIDC_ALG || "EdDSA").trim();

if (!supportedAlgs.has(alg)) {
  console.error(`unsupported OIDC_ALG: ${alg}`);
  process.exit(1);
}

function parsePort(raw, fallback) {
  if (raw === undefined || raw === "") return fallback;
  const value = Number(raw);
  if (!Number.isInteger(value) || value < 0 || value > 65535) {
    console.error(`invalid OIDC_PORT: ${raw}`);
    process.exit(1);
  }
  return value;
}

const port = parsePort(process.env.OIDC_PORT, 18970);
const clientId = process.env.OIDC_CLIENT_ID || "hpa-local-ui";
const audience = process.env.OIDC_AUDIENCE || "hpa-sso";
const subject = process.env.OIDC_SUBJECT || "oidc|browser-admin";
const tenantId = process.env.OIDC_TENANT || "local";
const role = process.env.OIDC_ROLE || "owner";
const email = process.env.OIDC_EMAIL || "admin@example.test";
const audienceClaim = audience === clientId ? clientId : [clientId, audience];

const ed25519 = crypto.generateKeyPairSync("ed25519");
const rsa = crypto.generateKeyPairSync("rsa", { modulusLength: 2048 });
const p256 = crypto.generateKeyPairSync("ec", { namedCurve: "P-256" });

function publicJwk(publicKey, algorithm) {
  return {
    ...publicKey.export({ format: "jwk" }),
    kid: "primary",
    use: "sig",
    alg: algorithm,
  };
}

const signingKeys = {
  EdDSA: {
    privateKey: ed25519.privateKey,
    jwk: publicJwk(ed25519.publicKey, "EdDSA"),
  },
  RS256: {
    privateKey: rsa.privateKey,
    jwk: publicJwk(rsa.publicKey, "RS256"),
  },
  ES256: {
    privateKey: p256.privateKey,
    jwk: publicJwk(p256.publicKey, "ES256"),
  },
};

const activeKey = signingKeys[alg];
const codes = new Map();
const refreshTokens = new Map();
const codeTtlMs = 60_000;
const refreshTtlMs = 24 * 60 * 60 * 1000;
let issuer = "";
let sequence = 0;
let refreshSequence = 0;

function encode(value) {
  return Buffer.from(JSON.stringify(value)).toString("base64url");
}

function signJwt(payload) {
  const header = { alg, kid: "primary", typ: "JWT" };
  const signingInput = `${encode(header)}.${encode(payload)}`;
  let signature;
  if (alg === "EdDSA") {
    signature = crypto.sign(null, Buffer.from(signingInput), activeKey.privateKey);
  } else if (alg === "RS256") {
    signature = crypto.sign("RSA-SHA256", Buffer.from(signingInput), activeKey.privateKey);
  } else {
    signature = crypto.sign("sha256", Buffer.from(signingInput), {
      key: activeKey.privateKey,
      dsaEncoding: "ieee-p1363",
    });
  }
  return `${signingInput}.${signature.toString("base64url")}`;
}

function idToken(nonce) {
  const now = Math.floor(Date.now() / 1000);
  const claims = {
    iss: issuer,
    aud: audienceClaim,
    sub: subject,
    tenant_id: tenantId,
    role,
    email,
    name: "Browser Admin",
    iat: now,
    exp: now + 300,
  };
  if (nonce) claims.nonce = nonce;
  return signJwt(claims);
}

function issueTokenSet(nonce) {
  refreshSequence += 1;
  const refreshToken = `mock-refresh-${refreshSequence}`;
  refreshTokens.set(refreshToken, Date.now() + refreshTtlMs);
  return {
    access_token: `mock-access-${refreshSequence}`,
    token_type: "Bearer",
    id_token: idToken(nonce),
    expires_in: 300,
    refresh_token: refreshToken,
    refresh_expires_in: refreshTtlMs / 1000,
  };
}

function sendJson(res, status, body) {
  const data = JSON.stringify(body);
  res.writeHead(status, {
    "content-type": "application/json; charset=utf-8",
    "content-length": Buffer.byteLength(data),
    "cache-control": "no-store",
    pragma: "no-cache",
    "x-content-type-options": "nosniff",
  });
  res.end(data);
}

function oauthError(res, status, error, description) {
  sendJson(res, status, { error, error_description: description });
}

function isLoopbackRedirect(value) {
  let parsed;
  try {
    parsed = new URL(value);
  } catch {
    return false;
  }
  if (parsed.protocol !== "http:" && parsed.protocol !== "https:") return false;
  if (parsed.username || parsed.password || parsed.hash) return false;
  const host = parsed.hostname.toLowerCase();
  return (
    host === "localhost" ||
    host === "::1" ||
    host === "[::1]" ||
    host === "127.0.0.1" ||
    host.startsWith("127.")
  );
}

function isChallenge(value) {
  if (!/^[A-Za-z0-9_-]{43}$/.test(value)) return false;
  try {
    return Buffer.from(value, "base64url").length === 32;
  } catch {
    return false;
  }
}

function isVerifier(value) {
  return typeof value === "string" && /^[A-Za-z0-9_-]{43,128}$/.test(value);
}

function equal(left, right) {
  const a = Buffer.from(left);
  const b = Buffer.from(right);
  if (a.length !== b.length) return false;
  return crypto.timingSafeEqual(a, b);
}

function purgeCodes() {
  const now = Date.now();
  for (const [code, flow] of codes) {
    if (flow.expiresAt <= now) codes.delete(code);
  }
  for (const [token, expiresAt] of refreshTokens) {
    if (expiresAt <= now) refreshTokens.delete(token);
  }
}

function handleAuthorize(res, url) {
  const params = url.searchParams;
  if (params.get("response_type") !== "code") {
    return oauthError(res, 400, "unsupported_response_type", "response_type must be code");
  }
  if (params.get("client_id") !== clientId) {
    return oauthError(res, 400, "unauthorized_client", "client_id is not recognized");
  }
  const redirectUri = params.get("redirect_uri") || "";
  if (!isLoopbackRedirect(redirectUri)) {
    return oauthError(res, 400, "invalid_request", "redirect_uri must be an absolute loopback URL");
  }
  const scopes = (params.get("scope") || "").split(/\s+/).filter(Boolean);
  if (!scopes.includes("openid")) {
    return oauthError(res, 400, "invalid_scope", "scope must include openid");
  }
  const state = params.get("state") || "";
  if (!state || state.length > 512) {
    return oauthError(res, 400, "invalid_request", "state is required");
  }
  const nonce = params.get("nonce") || "";
  if (!nonce || nonce.length > 512) {
    return oauthError(res, 400, "invalid_request", "nonce is required");
  }
  const challenge = params.get("code_challenge") || "";
  if (params.get("code_challenge_method") !== "S256" || !isChallenge(challenge)) {
    return oauthError(res, 400, "invalid_request", "PKCE S256 code_challenge is required");
  }

  purgeCodes();
  const code = crypto.randomBytes(32).toString("base64url");
  codes.set(code, {
    clientId,
    redirectUri,
    challenge,
    nonce,
    expiresAt: Date.now() + codeTtlMs,
  });

  const target = new URL(redirectUri);
  target.searchParams.set("code", code);
  target.searchParams.set("state", state);
  res.writeHead(302, {
    location: target.toString(),
    "cache-control": "no-store",
    "x-content-type-options": "nosniff",
  });
  res.end();
}

function readBody(req, limit) {
  return new Promise((resolve, reject) => {
    const chunks = [];
    let size = 0;
    let tooLarge = false;
    req.on("data", (chunk) => {
      size += chunk.length;
      if (size > limit) {
        tooLarge = true;
        return;
      }
      chunks.push(chunk);
    });
    req.on("end", () => {
      if (tooLarge) reject(new Error("body too large"));
      else resolve(Buffer.concat(chunks).toString("utf8"));
    });
    req.on("error", reject);
  });
}

async function handleToken(req, res) {
  const contentType = String(req.headers["content-type"] || "")
    .split(";")[0]
    .trim()
    .toLowerCase();
  if (contentType !== "application/x-www-form-urlencoded") {
    return oauthError(res, 400, "invalid_request", "content-type must be application/x-www-form-urlencoded");
  }

  let raw;
  try {
    raw = await readBody(req, 16 * 1024);
  } catch {
    return oauthError(res, 400, "invalid_request", "request body is invalid");
  }

  const form = new URLSearchParams(raw);
  const grantType = form.get("grant_type");
  if (grantType === "refresh_token") {
    const refreshToken = form.get("refresh_token") || "";
    const tokenClientId = form.get("client_id") || "";
    if (!refreshToken || tokenClientId !== clientId) {
      return oauthError(res, 400, "invalid_grant", "refresh token or client_id is invalid");
    }
    const expiresAt = refreshTokens.get(refreshToken);
    if (!expiresAt || expiresAt <= Date.now()) {
      refreshTokens.delete(refreshToken);
      return oauthError(res, 400, "invalid_grant", "refresh token is invalid or already used");
    }
    refreshTokens.delete(refreshToken);
    return sendJson(res, 200, issueTokenSet());
  }
  if (grantType !== "authorization_code") {
    return oauthError(res, 400, "unsupported_grant_type", "grant_type is not supported");
  }
  const code = form.get("code") || "";
  const redirectUri = form.get("redirect_uri") || "";
  const tokenClientId = form.get("client_id") || "";
  const verifier = form.get("code_verifier") || "";

  if (!code) return oauthError(res, 400, "invalid_request", "code is required");
  if (!redirectUri) return oauthError(res, 400, "invalid_request", "redirect_uri is required");
  if (!tokenClientId) return oauthError(res, 400, "invalid_request", "client_id is required");
  if (!verifier) return oauthError(res, 400, "invalid_request", "code_verifier is required");

  const flow = codes.get(code);
  if (!flow) return oauthError(res, 400, "invalid_grant", "code is invalid or already used");
  codes.delete(code);

  if (flow.expiresAt <= Date.now()) {
    return oauthError(res, 400, "invalid_grant", "code has expired");
  }
  if (tokenClientId !== flow.clientId || tokenClientId !== clientId) {
    return oauthError(res, 400, "invalid_grant", "client_id does not match the authorization request");
  }
  if (redirectUri !== flow.redirectUri) {
    return oauthError(res, 400, "invalid_grant", "redirect_uri does not match the authorization request");
  }
  if (!isVerifier(verifier)) {
    return oauthError(res, 400, "invalid_grant", "code_verifier is malformed");
  }
  const computed = crypto.createHash("sha256").update(verifier).digest("base64url");
  if (!equal(computed, flow.challenge)) {
    return oauthError(res, 400, "invalid_grant", "code_verifier does not match code_challenge");
  }

  sendJson(res, 200, issueTokenSet(flow.nonce));
}

function handleLogout(res, url) {
  const params = url.searchParams;
  const state = params.get("state") || "";
  const tokenClientId = params.get("client_id") || "";
  const idTokenHint = params.get("id_token_hint") || "";
  const redirectUri = params.get("post_logout_redirect_uri") || "";
  if (!state || state.length > 512) {
    return oauthError(res, 400, "invalid_request", "state is required");
  }
  if (tokenClientId !== clientId) {
    return oauthError(res, 400, "unauthorized_client", "client_id is not recognized");
  }
  if (idTokenHint.split(".").length !== 3) {
    return oauthError(res, 400, "invalid_request", "id_token_hint is required");
  }
  if (!isLoopbackRedirect(redirectUri)) {
    return oauthError(res, 400, "invalid_request", "post_logout_redirect_uri must be loopback");
  }
  const target = new URL(redirectUri);
  target.searchParams.set("state", state);
  res.writeHead(302, {
    location: target.toString(),
    "cache-control": "no-store",
    "x-content-type-options": "nosniff",
  });
  res.end();
}

function handleRequest(req, res) {
  let url;
  try {
    url = new URL(req.url || "/", "http://127.0.0.1");
  } catch {
    return oauthError(res, 400, "invalid_request", "request URL is invalid");
  }
  const path = url.pathname;

  if (req.method === "GET" && path === "/.well-known/openid-configuration") {
    return sendJson(res, 200, {
      issuer,
      authorization_endpoint: `${issuer}/authorize`,
      token_endpoint: `${issuer}/token`,
      end_session_endpoint: `${issuer}/logout`,
      jwks_uri: `${issuer}/jwks`,
      response_types_supported: ["code"],
      code_challenge_methods_supported: ["S256"],
      id_token_signing_alg_values_supported: ["EdDSA", "RS256", "ES256"],
      subject_types_supported: ["public"],
      scopes_supported: ["openid", "email", "profile", "offline_access"],
      grant_types_supported: ["authorization_code", "refresh_token"],
      token_endpoint_auth_methods_supported: ["none"],
    });
  }

  if (req.method === "GET" && path === "/jwks") {
    return sendJson(res, 200, {
      keys: [
        { ...activeKey.jwk, kid: "primary" },
        { ...activeKey.jwk, kid: "secondary" },
      ],
    });
  }

  if (req.method === "GET" && path === "/authorize") {
    return handleAuthorize(res, url);
  }

  if (req.method === "GET" && path === "/logout") {
    return handleLogout(res, url);
  }

  if (req.method === "POST" && path === "/token") {
    return handleToken(req, res);
  }

  if (req.method === "GET" && path === "/health") {
    return sendJson(res, 200, {
      status: "ok",
      alg,
      issuer,
      clientId,
      audience,
      jwksUri: `${issuer}/jwks`,
      authorizationEndpoint: `${issuer}/authorize`,
      tokenEndpoint: `${issuer}/token`,
      endSessionEndpoint: `${issuer}/logout`,
    });
  }

  if (path === "/authorize" || path === "/token" || path === "/jwks" || path === "/logout") {
    res.setHeader("allow", path === "/token" ? "POST" : "GET");
    return oauthError(res, 405, "invalid_request", "method not allowed");
  }

  return oauthError(res, 404, "not_found", "unknown endpoint");
}

const server = http.createServer((req, res) => {
  const path = (req.url || "/").split("?")[0];
  sequence += 1;
  const requestId = sequence;
  Promise.resolve()
    .then(() => handleRequest(req, res))
    .catch(() => {
      if (!res.headersSent) {
        oauthError(res, 500, "server_error", "request failed");
      } else {
        res.end();
      }
    })
    .finally(() => {
      console.log(`${requestId} ${req.method} ${path} ${res.statusCode} alg=${alg}`);
    });
});

server.on("error", (error) => {
  console.error(`oidc mock failed: ${error.message}`);
  process.exit(1);
});

server.listen(port, "127.0.0.1", () => {
  const address = server.address();
  const actualPort = typeof address === "object" && address ? address.port : port;
  issuer = `http://127.0.0.1:${actualPort}`;
  console.log(`oidc mock listening on ${issuer} alg=${alg}`);
});

const cleanup = setInterval(purgeCodes, 30_000);
cleanup.unref();

let shuttingDown = false;
function shutdown() {
  if (shuttingDown) return;
  shuttingDown = true;
  clearInterval(cleanup);
  server.close(() => process.exit(0));
  setTimeout(() => process.exit(0), 2_000).unref();
}

process.on("SIGINT", shutdown);
process.on("SIGTERM", shutdown);
