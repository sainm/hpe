import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";
import YAML from "yaml";

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const failures = [];

function read(relativePath) {
  return fs.readFileSync(path.join(root, relativePath), "utf8");
}

function requireFragment(relativePath, fragment) {
  if (!read(relativePath).includes(fragment)) {
    failures.push(`${relativePath} must contain ${fragment}`);
  }
}

function walk(relativeDirectory) {
  const absoluteDirectory = path.join(root, relativeDirectory);
  if (!fs.existsSync(absoluteDirectory)) return [];
  return fs.readdirSync(absoluteDirectory, { withFileTypes: true }).flatMap((entry) => {
    const relativePath = path.join(relativeDirectory, entry.name);
    if (entry.isDirectory() && [".git", "node_modules", "target"].includes(entry.name)) {
      return [];
    }
    return entry.isDirectory() ? walk(relativePath) : [relativePath];
  });
}

requireFragment(".cargo/config.toml", 'sparse+https://rsproxy.cn/index/');
requireFragment(".npmrc", "registry=https://registry.npmmirror.com/");
requireFragment("scripts/use-domestic-mirrors.sh", "https://rsproxy.cn");
requireFragment("scripts/use-domestic-mirrors.sh", "https://registry.npmmirror.com");
requireFragment("scripts/use-domestic-mirrors.sh", "https://npmmirror.com/mirrors/node/");
requireFragment(
  "scripts/use-domestic-mirrors.sh",
  "https://pypi.tuna.tsinghua.edu.cn/simple",
);
requireFragment(
  "scripts/use-domestic-mirrors.sh",
  "https://npmmirror.com/mirrors/playwright",
);
requireFragment("scripts/use-domestic-mirrors.sh", "https://goproxy.cn,direct");

const uvLock = read("services/ai-worker/uv.lock");
const uvRegistries = [
  ...uvLock.matchAll(/source = \{ registry = "([^"]+)" \}/g),
].map((match) => match[1]);
if (uvRegistries.length === 0) {
  failures.push("services/ai-worker/uv.lock has no registry sources");
}
for (const registry of new Set(uvRegistries)) {
  if (registry !== "https://pypi.tuna.tsinghua.edu.cn/simple") {
    failures.push(`services/ai-worker/uv.lock uses unexpected registry ${registry}`);
  }
}
for (const upstream of [
  "https://pypi.org/",
  "https://files.pythonhosted.org/",
  "https://registry.npmjs.org/",
  "https://static.crates.io/",
]) {
  if (uvLock.includes(upstream)) {
    failures.push(`services/ai-worker/uv.lock contains upstream download URL ${upstream}`);
  }
}

const containerfiles = walk(".")
  .filter((relativePath) => /(^|\/)(Containerfile|Dockerfile)$/.test(relativePath));

function checkImage(relativePath, line, image) {
  if (!image.startsWith("docker.m.daocloud.io/")) {
    failures.push(`${relativePath} image does not use DaoCloud mirror: ${image}`);
  }
  if (!/@sha256:[a-f0-9]{64}$/.test(image)) {
    failures.push(`${relativePath} image is not pinned by SHA-256 digest: ${line.trim()}`);
  }
}

for (const relativePath of containerfiles) {
  for (const line of read(relativePath).split("\n")) {
    const match = line.match(/^\s*FROM\s+(?:--platform=\S+\s+)?(\S+)/i);
    if (match) checkImage(relativePath, line, match[1]);
  }
}

const compose = YAML.parse(read("deploy/otel/compose.yaml"));
for (const [service, definition] of Object.entries(compose.services ?? {})) {
  if (typeof definition?.image !== "string") {
    failures.push(`deploy/otel/compose.yaml service ${service} has no image`);
    continue;
  }
  checkImage("deploy/otel/compose.yaml", `services.${service}.image`, definition.image);
}

const workflow = YAML.parse(read(".github/workflows/ci.yml"));
for (const [jobName, job] of Object.entries(workflow.jobs ?? {})) {
  for (const [serviceName, service] of Object.entries(job?.services ?? {})) {
    if (typeof service?.image !== "string") {
      failures.push(`.github/workflows/ci.yml ${jobName}.${serviceName} has no image`);
      continue;
    }
    checkImage(
      ".github/workflows/ci.yml",
      `${jobName}.services.${serviceName}.image`,
      service.image,
    );
  }
}

if (failures.length > 0) {
  console.error(`Domestic mirror check failed (${failures.length}):`);
  for (const failure of failures) console.error(`- ${failure}`);
  process.exit(1);
}

console.log(
  `Domestic mirror check passed: ${new Set(uvRegistries).size} Python registry, ` +
    `${containerfiles.length} Containerfile, ${Object.keys(compose.services ?? {}).length} Compose image, ` +
    `and workflow service images are mirrored and pinned`,
);
