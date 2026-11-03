#!/usr/bin/env node
// Builds a reproducible release bundle: binary, web assets, migrations, deploy
// examples, SBOMs, SHA256SUMS and a manifest describing exactly what was built.
//
// Usage:
//   node scripts/release-pack.mjs                         # release build
//   node scripts/release-pack.mjs --profile debug --skip-build
//   node scripts/release-pack.mjs --skip-build --out /tmp/hpa-release

import { spawnSync } from "node:child_process";
import { createHash } from "node:crypto";
import fs from "node:fs";
import path from "node:path";
import os from "node:os";

const root = path.resolve(new URL("..", import.meta.url).pathname);
const localCargo = path.join(os.homedir(), ".cargo", "bin", "cargo");
const cargo =
  process.env.CARGO || (fs.existsSync(localCargo) ? localCargo : "cargo");

function parseArgs(rawArgv) {
  // `pnpm <script> -- --flag` forwards a literal `--`; ignore it.
  const argv = rawArgv.filter((arg) => arg !== "--");
  const options = { profile: "release", build: true, out: path.join(root, "dist-release") };
  for (let index = 0; index < argv.length; index += 1) {
    const arg = argv[index];
    if (arg === "--profile") options.profile = argv[++index] ?? "release";
    else if (arg === "--out") options.out = path.resolve(argv[++index] ?? options.out);
    else if (arg === "--skip-build") options.build = false;
    else if (arg === "--help" || arg === "-h") {
      console.log("usage: release-pack.mjs [--profile release|debug] [--skip-build] [--out DIR]");
      process.exit(0);
    } else {
      console.error(`unknown argument: ${arg}`);
      process.exit(2);
    }
  }
  if (!["release", "debug"].includes(options.profile)) {
    console.error(`unsupported profile: ${options.profile}`);
    process.exit(2);
  }
  return options;
}

function run(command, args, { capture = true } = {}) {
  const result = spawnSync(command, args, {
    cwd: root,
    encoding: "utf8",
    stdio: capture ? ["ignore", "pipe", "pipe"] : "inherit",
  });
  if (result.status !== 0) {
    const detail = capture ? `${result.stdout ?? ""}${result.stderr ?? ""}` : "";
    throw new Error(`command failed: ${command} ${args.join(" ")}\n${detail}`);
  }
  return result;
}

function tryRun(command, args) {
  try {
    return run(command, args).stdout.trim();
  } catch {
    return "unknown";
  }
}

function sha256(file) {
  const hash = createHash("sha256");
  hash.update(fs.readFileSync(file));
  return hash.digest("hex");
}

function copyFile(source, target) {
  fs.mkdirSync(path.dirname(target), { recursive: true });
  fs.copyFileSync(source, target);
}

function copyTree(source, target) {
  if (!fs.existsSync(source)) return 0;
  let count = 0;
  for (const entry of fs.readdirSync(source, { withFileTypes: true })) {
    const from = path.join(source, entry.name);
    const to = path.join(target, entry.name);
    if (entry.isDirectory()) {
      count += copyTree(from, to);
      continue;
    }
    if (!entry.isFile()) continue;
    copyFile(from, to);
    count += 1;
  }
  return count;
}

function walk(dir, base = dir) {
  const files = [];
  for (const entry of fs.readdirSync(dir, { withFileTypes: true })) {
    const full = path.join(dir, entry.name);
    if (entry.isDirectory()) files.push(...walk(full, base));
    else if (entry.isFile()) files.push(path.relative(base, full));
  }
  return files.sort();
}

const options = parseArgs(process.argv.slice(2));
const cargoProfileDir = options.profile === "release" ? "release" : "debug";
const binaryPath = path.join(root, "target", cargoProfileDir, "hpa");

if (options.build) {
  console.log(`building hpa (${options.profile})`);
  run(cargo, ["build", "-p", "hpa-local", "--profile", options.profile], { capture: false });
}
if (!fs.existsSync(binaryPath)) {
  console.error(`missing binary: ${binaryPath} (run without --skip-build)`);
  process.exit(1);
}

const webDist = path.join(root, "apps", "web", "dist");
if (!fs.existsSync(path.join(webDist, "index.html"))) {
  console.error(`missing web build: ${webDist} (run \`pnpm --dir apps/web build\`)`);
  process.exit(1);
}

const brand = (() => {
  try {
    return JSON.parse(fs.readFileSync(path.join(root, "branding", "brand.json"), "utf8"));
  } catch {
    return {};
  }
})();

const version = tryRun(cargo, ["metadata", "--no-deps", "--format-version", "1"]);
let packageVersion = "0.1.0";
try {
  const metadata = JSON.parse(version);
  packageVersion = metadata.packages?.find((entry) => entry.name === "hpa-local")?.version ?? packageVersion;
} catch {
  // keep the fallback
}
const gitSha = tryRun("git", ["rev-parse", "--short", "HEAD"]);
const gitBranch = tryRun("git", ["rev-parse", "--abbrev-ref", "HEAD"]);
const platform = `${process.platform}-${process.arch}`;
const name = `hpa-${packageVersion}-${gitSha}-${platform}`;

const staging = path.join(options.out, name);
fs.rmSync(staging, { recursive: true, force: true });
fs.mkdirSync(staging, { recursive: true });

copyFile(binaryPath, path.join(staging, "bin", "hpa"));
const webFiles = copyTree(webDist, path.join(staging, "web"));
const migrationFiles = copyTree(
  path.join(root, "migrations", "postgres"),
  path.join(staging, "migrations", "postgres"),
);
copyTree(path.join(root, "deploy"), path.join(staging, "deploy"));

for (const doc of ["README.md", "BRANDING.md"]) {
  const source = path.join(root, doc);
  if (fs.existsSync(source)) copyFile(source, path.join(staging, doc));
}
for (const doc of [
  "docs/11-roadmap/phase-13-production-completion.md",
  "docs/10-quality/release-checklist.md",
  "docs/08-platform/postgres-control-plane.md",
  "docs/08-platform/observability.md",
]) {
  const source = path.join(root, doc);
  if (fs.existsSync(source)) copyFile(source, path.join(staging, doc));
}

// SBOMs are produced by the supply-chain gate; regenerate when missing.
const sbomDir = path.join(root, "target", "sbom");
if (!fs.existsSync(path.join(sbomDir, "hpa-sbom.cdx.json"))) {
  console.log("generating SBOM via check:supply-chain");
  tryRun("node", ["scripts/check-supply-chain.mjs"]);
}
let sbomFiles = 0;
if (fs.existsSync(sbomDir)) {
  for (const entry of fs.readdirSync(sbomDir)) {
    if (!entry.endsWith(".cdx.json")) continue;
    copyFile(path.join(sbomDir, entry), path.join(staging, "sbom", entry));
    sbomFiles += 1;
  }
}

// Checksums for every staged file except the checksum file itself.
const stagedFiles = walk(staging);
const sums = stagedFiles.map((relative) => `${sha256(path.join(staging, relative))}  ${relative}`);
fs.writeFileSync(path.join(staging, "SHA256SUMS"), `${sums.join("\n")}\n`);

const manifest = {
  name,
  product: brand.productName ?? "hpa",
  internalId: brand.internalId ?? "hpa",
  version: packageVersion,
  git: { sha: gitSha, branch: gitBranch },
  builtAt: new Date().toISOString(),
  profile: options.profile,
  platform: { os: process.platform, arch: process.arch, release: os.release() },
  toolchain: {
    rustc: tryRun("rustc", ["--version"]),
    node: process.version,
    python: tryRun("python3", ["--version"]),
  },
  contents: {
    binary: "bin/hpa",
    webAssets: webFiles,
    migrations: migrationFiles,
    sboms: sbomFiles,
    files: stagedFiles.length,
  },
  archives: {},
};

const tarball = path.join(options.out, `${name}.tar.gz`);
fs.rmSync(tarball, { force: true });
run("tar", ["-czf", tarball, "-C", options.out, name]);
manifest.archives.tarball = {
  file: path.basename(tarball),
  bytes: fs.statSync(tarball).size,
  sha256: sha256(tarball),
};

fs.writeFileSync(
  path.join(options.out, `${name}.manifest.json`),
  `${JSON.stringify(manifest, null, 2)}\n`,
);
fs.writeFileSync(
  path.join(options.out, `${name}.tar.gz.sha256`),
  `${manifest.archives.tarball.sha256}  ${path.basename(tarball)}\n`,
);

console.log(
  JSON.stringify(
    {
      name,
      out: options.out,
      tarball: manifest.archives.tarball,
      files: manifest.contents.files,
      sboms: sbomFiles,
      migrations: migrationFiles,
      webAssets: webFiles,
    },
    null,
    2,
  ),
);
