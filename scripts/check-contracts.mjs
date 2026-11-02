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
    return;
  }
  if (value.$schema !== "https://json-schema.org/draft/2020-12/schema") {
    errors.push(`${relative}: draft 2020-12 $schema is required`);
  }
  if (!value.$id?.startsWith("https://hpa.dev/schema/")) {
    errors.push(`${relative}: canonical hpa.dev $id is required`);
  }
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
