import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";
import YAML from "yaml";

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const configPath = path.join(root, "deploy", "otel", "otel-collector.yaml");
const config = YAML.parse(fs.readFileSync(configPath, "utf8"));
const failures = [];
const processors = config.processors ?? {};
const pipelines = config.service?.pipelines ?? {};

function pipeline(name) {
  const selected = pipelines[name];
  if (!selected) {
    failures.push(`missing ${name} pipeline`);
    return [];
  }
  return selected.processors ?? [];
}

for (const name of ["traces", "metrics", "logs"]) {
  const selected = pipeline(name);
  if (selected[0] !== "memory_limiter") {
    failures.push(`${name} pipeline must start with memory_limiter`);
  }
  if (!selected.includes("attributes/sanitize")) {
    failures.push(`${name} pipeline must include attributes/sanitize`);
  }
  if (!selected.includes("batch")) {
    failures.push(`${name} pipeline must include bounded batch processor`);
  }
}

const tail = processors.tail_sampling;
if (!tail) {
  failures.push("missing tail_sampling processor");
} else {
  if (!Number.isInteger(tail.num_traces) || tail.num_traces < 1 || tail.num_traces > 1_000_000) {
    failures.push("tail_sampling.num_traces must be between 1 and 1000000");
  }
  if (!/^\d+(ms|s|m)$/.test(String(tail.decision_wait))) {
    failures.push("tail_sampling.decision_wait must be a bounded duration");
  }
  const policies = new Map((tail.policies ?? []).map((policy) => [policy.name, policy.type]));
  for (const [name, type] of [
    ["errors", "status_code"],
    ["slow", "latency"],
    ["baseline", "probabilistic"],
  ]) {
    if (policies.get(name) !== type) {
      failures.push(`tail_sampling must include ${name}/${type}`);
    }
  }
  const baseline = (tail.policies ?? []).find((policy) => policy.name === "baseline");
  const percentage = baseline?.probabilistic?.sampling_percentage;
  if (!(percentage > 0 && percentage <= 100)) {
    failures.push("tail_sampling baseline percentage must be within 1..100");
  }
  const tracePipeline = pipeline("traces");
  if (!tracePipeline.includes("tail_sampling")) {
    failures.push("traces pipeline must include tail_sampling");
  } else if (
    tracePipeline.indexOf("tail_sampling") > tracePipeline.indexOf("batch")
  ) {
    failures.push("tail_sampling must run before batch");
  }
}

const sanitizer = processors["attributes/sanitize"];
const deleted = new Set(
  (sanitizer?.actions ?? [])
    .filter((action) => action.action === "delete")
    .map((action) => action.key),
);
for (const key of [
  "authorization",
  "http.request.header.authorization",
  "cookie",
  "set-cookie",
  "api_key",
  "api.key",
  "bearer_token",
  "model.api_key",
  "db.statement",
  "checkpoint",
]) {
  if (!deleted.has(key)) {
    failures.push(`attributes/sanitize must delete sensitive key ${key}`);
  }
}

if (failures.length > 0) {
  console.error(`OTel configuration check failed (${failures.length}):`);
  for (const failure of failures) console.error(`- ${failure}`);
  process.exit(1);
}

console.log(
  "OTel configuration check passed: bounded error/latency/probabilistic tail sampling, " +
    "memory/batch limits, and sensitive attribute deletion across all signals",
);
