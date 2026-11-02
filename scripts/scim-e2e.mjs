import fs from "node:fs";

const baseUrl = (process.env.HPA_BASE_URL || "http://127.0.0.1:8787").replace(/\/+$/, "");
const tokenFile = process.env.HPA_TOKEN_FILE;
const token = tokenFile
  ? fs.readFileSync(tokenFile, "utf8").trim()
  : (process.env.HPA_TOKEN || "").trim();
if (!token) throw new Error("HPA_TOKEN_FILE or HPA_TOKEN is required");

const USER_SCHEMA = "urn:ietf:params:scim:schemas:core:2.0:User";
const GROUP_SCHEMA = "urn:ietf:params:scim:schemas:core:2.0:Group";
const PATCH_SCHEMA = "urn:ietf:params:scim:api:messages:2.0:PatchOp";
const ROLE_SCHEMA = "urn:hpa:params:scim:schemas:extension:role:1.0";
const suffix = `${Date.now()}-${process.pid}`;
const userName = `scim-e2e-${suffix}@example.test`;
const userExternalId = `scim-e2e-${suffix}`;
const groupName = `SCIM E2E ${suffix}`;

async function request(path, { method = "GET", body, expected = 200 } = {}) {
  const response = await fetch(`${baseUrl}${path}`, {
    method,
    headers: {
      authorization: `Bearer ${token}`,
      accept: "application/scim+json",
      ...(body ? { "content-type": "application/scim+json" } : {}),
    },
    body: body ? JSON.stringify(body) : undefined,
    signal: AbortSignal.timeout(10_000),
  });
  const text = await response.text();
  let value = null;
  try {
    value = text ? JSON.parse(text) : null;
  } catch {
    value = text;
  }
  if (response.status !== expected) {
    throw new Error(
      `${method} ${path} expected HTTP ${expected}, got ${response.status}: ${String(text).slice(0, 300)}`,
    );
  }
  return { value, headers: response.headers };
}

let userId = null;
let groupId = null;

try {
  const config = await request("/scim/v2/ServiceProviderConfig");
  if (config.value?.patch?.supported !== true || config.value?.etag?.supported !== true) {
    throw new Error("SCIM capabilities do not advertise PATCH and ETag support");
  }

  const createdUser = await request("/scim/v2/Users", {
    method: "POST",
    expected: 201,
    body: {
      schemas: [USER_SCHEMA, ROLE_SCHEMA],
      userName,
      externalId: userExternalId,
      displayName: "SCIM E2E User",
      emails: [{ value: userName, type: "work", primary: true }],
      active: true,
      [ROLE_SCHEMA]: { role: "auditor" },
    },
  });
  userId = createdUser.value.id;
  if (!userId) throw new Error("SCIM User response has no id");

  const filtered = await request(
    `/scim/v2/Users?filter=${encodeURIComponent(`userName eq "${userName}"`)}`,
  );
  if (filtered.value.totalResults !== 1 || filtered.value.Resources[0].id !== userId) {
    throw new Error("SCIM userName filter did not resolve the created User");
  }

  const createdGroup = await request("/scim/v2/Groups", {
    method: "POST",
    expected: 201,
    body: {
      schemas: [GROUP_SCHEMA, ROLE_SCHEMA],
      displayName: groupName,
      externalId: `group-${suffix}`,
      [ROLE_SCHEMA]: { role: "developer" },
      members: [{ value: userId }],
    },
  });
  groupId = createdGroup.value.id;
  if (!groupId) throw new Error("SCIM Group response has no id");

  const mappedUser = await request(`/scim/v2/Users/${encodeURIComponent(userId)}`);
  if (mappedUser.value?.[ROLE_SCHEMA]?.effectiveRole !== "developer") {
    throw new Error("SCIM Group role was not applied to the User");
  }

  const patched = await request(`/scim/v2/Users/${encodeURIComponent(userId)}`, {
    method: "PATCH",
    body: {
      schemas: [PATCH_SCHEMA],
      Operations: [{ op: "replace", path: "title", value: "SCIM E2E Principal" }],
    },
  });
  if (patched.value.title !== "SCIM E2E Principal") {
    throw new Error("SCIM PATCH did not persist a non-indexed attribute");
  }

  await request(`/scim/v2/Groups/${encodeURIComponent(groupId)}`, {
    method: "DELETE",
    expected: 204,
  });
  groupId = null;
  const restored = await request(`/scim/v2/Users/${encodeURIComponent(userId)}`);
  if (restored.value?.[ROLE_SCHEMA]?.effectiveRole !== "auditor") {
    throw new Error("SCIM Group deletion did not restore the direct role");
  }

  await request(`/scim/v2/Users/${encodeURIComponent(userId)}`, {
    method: "DELETE",
    expected: 204,
  });
  userId = null;

  console.log(
    JSON.stringify(
      {
        ok: true,
        baseUrl,
        capabilities: { patch: true, filter: true, etag: true },
        userLifecycle: "create-filter-patch-group-role-delete",
        roleRestoredAfterGroupDelete: true,
      },
      null,
      2,
    ),
  );
} finally {
  if (groupId) {
    await request(`/scim/v2/Groups/${encodeURIComponent(groupId)}`, {
      method: "DELETE",
      expected: 204,
    }).catch(() => {});
  }
  if (userId) {
    await request(`/scim/v2/Users/${encodeURIComponent(userId)}`, {
      method: "DELETE",
      expected: 204,
    }).catch(() => {});
  }
}
