import fs from "node:fs";
import path from "node:path";

const root = path.resolve(new URL("..", import.meta.url).pathname);
const schemaRoot = path.join(root, "schema");
const required = [
  "ir/v0.1/workflow.schema.json",
  "ir/v0.1/node-registry.schema.json",
  "events/v1/event.schema.json",
  "capabilities/v1/tool.schema.json",
  "plugins/v1/vulnerability-db.schema.json",
  "openapi/local-api.json",
];
const errors = [];
let documents = 0;

function walk(directory) {
  for (const entry of fs.readdirSync(directory, { withFileTypes: true })) {
    const file = path.join(directory, entry.name);
    if (entry.isDirectory()) walk(file);
    else if (entry.name.endsWith(".json")) validate(file);
  }
}

function validate(file) {
  const relative = path.relative(schemaRoot, file).split(path.sep).join("/");
  let value;
  try {
    value = JSON.parse(fs.readFileSync(file, "utf8"));
  } catch (error) {
    errors.push(`${relative}: invalid JSON: ${error.message}`);
    return;
  }
  documents += 1;
  if (relative === "openapi/local-api.json") {
    if (value.openapi !== "3.1.0") errors.push(`${relative}: openapi must be 3.1.0`);
    if (!value.info?.title || !value.info?.version) {
      errors.push(`${relative}: info.title and info.version are required`);
    }
    if (!value.paths || Object.keys(value.paths).length === 0) {
      errors.push(`${relative}: paths must not be empty`);
    }
    validateRuntimeRoutes(value, path.join(root, "crates/hpa-local/src/api.rs"));
    return;
  }
  if (value.$schema !== "https://json-schema.org/draft/2020-12/schema") {
    errors.push(`${relative}: draft 2020-12 $schema is required`);
  }
  if (!value.$id?.startsWith("https://hpa.dev/schema/")) {
    errors.push(`${relative}: canonical hpa.dev $id is required`);
  }
}

function validateRuntimeRoutes(openapi, apiSource) {
  const source = fs.readFileSync(apiSource, "utf8");
  for (const [runtimePath, methods] of extractRuntimeRoutes(source)) {
    const openapiPath = `/api${runtimePath}`;
    const documented = openapi.paths[openapiPath];
    if (!documented) {
      errors.push(`openapi/local-api.json: missing runtime path ${openapiPath}`);
      continue;
    }
    for (const method of methods) {
      if (!documented[method]) {
        errors.push(
          `openapi/local-api.json: missing ${method.toUpperCase()} ${openapiPath}`,
        );
      }
    }
  }
}

function extractRuntimeRoutes(source) {
  const routes = [];
  let offset = 0;
  while (true) {
    const start = source.indexOf(".route(", offset);
    if (start === -1) return routes;
    const body = balancedBody(source, start + ".route(".length);
    const match = body.match(/^\s*"([^"]+)"\s*,\s*([\s\S]*)$/);
    if (match) {
      routes.push([match[1], routeMethods(match[2])]);
    }
    offset = start + ".route(".length + body.length + 1;
  }
}

function balancedBody(source, start) {
  let depth = 1;
  let quote;
  let escaped = false;
  for (let index = start; index < source.length; index += 1) {
    const char = source[index];
    if (quote) {
      if (escaped) escaped = false;
      else if (char === "\\") escaped = true;
      else if (char === quote) quote = undefined;
      continue;
    }
    if (char === '"' || char === "'") quote = char;
    else if (char === "(") depth += 1;
    else if (char === ")" && --depth === 0) return source.slice(start, index);
  }
  return source.slice(start);
}

function routeMethods(body) {
  const methods = [];
  let depth = 0;
  let quote;
  let escaped = false;
  for (let index = 0; index < body.length; index += 1) {
    const char = body[index];
    if (quote) {
      if (escaped) escaped = false;
      else if (char === "\\") escaped = true;
      else if (char === quote) quote = undefined;
      continue;
    }
    if (char === '"' || char === "'") quote = char;
    else if (char === "(") depth += 1;
    else if (char === ")") depth -= 1;
    else if (depth === 0) {
      const match = body.slice(index).match(/^(get|post|put|patch|delete)\s*\(/);
      if (match) methods.push(match[1]);
    }
  }
  return methods;
}

walk(schemaRoot);
for (const relative of required) {
  if (!fs.existsSync(path.join(schemaRoot, relative))) {
    errors.push(`missing required contract: schema/${relative}`);
  }
}

if (errors.length > 0) {
  console.error(`Contract check failed (${errors.length}):`);
  for (const error of errors) console.error(`- ${error}`);
  process.exit(1);
}
console.log(`Contract check passed: ${documents} JSON contracts`);
