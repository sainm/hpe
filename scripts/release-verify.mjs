#!/usr/bin/env node
// Verifies a release bundle: archive checksum, per-file SHA256SUMS, and a smoke
// run of the bundled binary against a temporary data directory.
//
// Usage: node scripts/release-verify.mjs <path/to/hpa-*.tar.gz> [--keep]

import { spawnSync } from "node:child_process";
import { createHash } from "node:crypto";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";

const args = process.argv.slice(2).filter((arg) => arg !== "--");
const keep = args.includes("--keep");

// Accept a glob from callers that cannot rely on shell expansion.
function expandGlob(pattern) {
  if (!pattern.includes("*")) return pattern;
  const dir = path.dirname(pattern);
  const mask = new RegExp(
    `^${path.basename(pattern).replace(/[.+^${}()|[\]\\]/g, "\\$&").replace(/\*/g, ".*")}$`,
  );
  const match = fs
    .readdirSync(dir)
    .filter((entry) => mask.test(entry))
    .sort()
    .pop();
  return match ? path.join(dir, match) : pattern;
}

const rawTarball = args.find((arg) => !arg.startsWith("--"));
const tarball = rawTarball ? expandGlob(rawTarball) : undefined;

if (!tarball) {
  console.error("usage: release-verify.mjs <hpa-*.tar.gz> [--keep]");
  process.exit(2);
}

const archive = path.resolve(tarball);
if (!fs.existsSync(archive)) {
  console.error(`archive not found: ${archive}`);
  process.exit(1);
}

function sha256File(file) {
  const hash = createHash("sha256");
  hash.update(fs.readFileSync(file));
  return hash.digest("hex");
}

function fail(message) {
  console.error(`FAIL: ${message}`);
  process.exit(1);
}

const expectedArchive = `${archive}.sha256`;
if (!fs.existsSync(expectedArchive)) fail(`missing archive checksum: ${expectedArchive}`);
const archiveLine = fs.readFileSync(expectedArchive, "utf8").trim().split(/\s+/)[0];
const actualArchive = sha256File(archive);
if (archiveLine !== actualArchive) {
  fail(`archive checksum mismatch: expected ${archiveLine}, got ${actualArchive}`);
}

const workdir = fs.mkdtempSync(path.join(os.tmpdir(), "hpa-release-verify-"));
const extract = spawnSync("tar", ["-xzf", archive, "-C", workdir], { encoding: "utf8" });
if (extract.status !== 0) fail(`cannot extract archive: ${extract.stderr}`);

const entries = fs.readdirSync(workdir, { withFileTypes: true }).filter((entry) => entry.isDirectory());
if (entries.length !== 1) fail(`archive must contain exactly one directory, found ${entries.length}`);
const bundle = path.join(workdir, entries[0].name);

const sumsPath = path.join(bundle, "SHA256SUMS");
if (!fs.existsSync(sumsPath)) fail("bundle has no SHA256SUMS");
const sums = fs
  .readFileSync(sumsPath, "utf8")
  .split("\n")
  .map((line) => line.trim())
  .filter(Boolean);
let mismatches = 0;
for (const line of sums) {
  const [expected, relative] = line.split(/\s+/);
  const target = path.join(bundle, relative);
  if (!fs.existsSync(target)) {
    console.error(`missing file listed in manifest: ${relative}`);
    mismatches += 1;
    continue;
  }
  if (sha256File(target) !== expected) {
    console.error(`checksum mismatch: ${relative}`);
    mismatches += 1;
  }
}
if (mismatches > 0) fail(`${mismatches} file checksum problem(s)`);

// The manifest sits next to the archive (not inside it) so it can be read
// without extracting; fall back to a copy inside the bundle if present.
const siblingManifest = path.join(
  path.dirname(archive),
  `${path.basename(archive, ".tar.gz")}.manifest.json`,
);
const bundledManifest = fs
  .readdirSync(bundle)
  .find((entry) => entry.endsWith(".manifest.json"));
let manifest = null;
for (const candidate of [siblingManifest, bundledManifest && path.join(bundle, bundledManifest)]) {
  if (!candidate || !fs.existsSync(candidate)) continue;
  manifest = JSON.parse(fs.readFileSync(candidate, "utf8"));
  break;
}
if (!manifest) fail("no manifest found next to the archive or inside the bundle");

const binary = path.join(bundle, "bin", "hpa");
if (!fs.existsSync(binary)) fail("bundle has no bin/hpa");
fs.chmodSync(binary, 0o755);

const dataDir = path.join(workdir, "data");
const doctor = spawnSync(binary, ["--data-dir", dataDir, "doctor"], { encoding: "utf8" });
if (doctor.status !== 0) {
  fail(`bundled binary failed to run: ${doctor.stderr || doctor.stdout}`);
}
let doctorReport = null;
try {
  doctorReport = JSON.parse(doctor.stdout);
} catch {
  fail(`doctor output is not JSON: ${doctor.stdout.slice(0, 200)}`);
}
if (doctorReport?.status !== "ok") fail(`doctor reported status ${doctorReport?.status}`);

const migrations = path.join(bundle, "migrations", "postgres");
const migrationCount = fs.existsSync(migrations) ? fs.readdirSync(migrations).filter((file) => file.endsWith(".sql")).length : 0;
if (migrationCount === 0) fail("bundle has no PostgreSQL migrations");

const webIndex = path.join(bundle, "web", "index.html");
if (!fs.existsSync(webIndex)) fail("bundle has no web assets");

if (!keep) fs.rmSync(workdir, { recursive: true, force: true });

console.log(
  JSON.stringify(
    {
      archive: path.basename(archive),
      sha256: actualArchive,
      files: sums.length,
      migrations: migrationCount,
      version: manifest?.version ?? doctorReport?.version ?? "unknown",
      git: manifest?.git ?? null,
      doctor: { status: doctorReport?.status, runtime: doctorReport?.runtime },
      extracted: keep ? bundle : null,
    },
    null,
    2,
  ),
);
