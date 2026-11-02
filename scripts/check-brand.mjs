import fs from "node:fs";
import path from "node:path";

const root = process.cwd();
const forbidden = /\b(hisha|auralis)\b/i;
const skipDirs = new Set([
  ".git",
  "node_modules",
  "target",
  "dist",
  "build",
  ".next",
  "coverage",
  ".qoder",
  ".hpa",
  ".workbuddy",
  ".turbo",
  ".venv",
  "__pycache__",
  ".pytest_cache",
  ".mypy_cache",
  ".ruff_cache",
]);
const binaryExts = new Set([".png", ".jpg", ".jpeg", ".gif", ".webp", ".ico", ".woff", ".woff2", ".ttf", ".zip", ".gz", ".vsix", ".pyc", ".db", ".sqlite"]);
const allowedFiles = new Set([
  "README.md",
  "BRANDING.md",
  "CHANGELOG.md",
  "LICENSE",
  "LICENSE.md",
  "scripts/check-brand.mjs",
  "apps/vscode-extension/package.json",
  "apps/vscode-extension/package-lock.json",
  "apps/vscode-extension/package.nls.json",
  "apps/vscode-extension/package.nls.ja.json",
  "apps/vscode-extension/package.nls.zh-cn.json",
  "apps/vscode-extension/README.md",
]);

function toRel(file) {
  return path.relative(root, file).split(path.sep).join("/");
}

function allowed(rel) {
  return rel.startsWith("docs/") || rel.startsWith("branding/") || allowedFiles.has(rel);
}

function walk(dir, out) {
  for (const entry of fs.readdirSync(dir, { withFileTypes: true })) {
    if (entry.isDirectory()) {
      if (!skipDirs.has(entry.name)) walk(path.join(dir, entry.name), out);
      continue;
    }
    out.push(path.join(dir, entry.name));
  }
  return out;
}

const brandPath = path.join(root, "branding", "brand.json");
const violations = [];

if (!fs.existsSync(brandPath)) {
  violations.push("branding/brand.json missing");
} else {
  const brand = JSON.parse(fs.readFileSync(brandPath, "utf8"));
  if (brand.internalId !== "hpa") violations.push("brand.json internalId must be hpa");
  if (!brand.productName) violations.push("brand.json productName missing");
}

for (const file of walk(root, [])) {
  const rel = toRel(file);
  if (allowed(rel)) continue;
  if (binaryExts.has(path.extname(file).toLowerCase())) continue;
  if (forbidden.test(rel)) violations.push(`${rel}: brand in path`);
  const text = fs.readFileSync(file, "utf8");
  if (forbidden.test(text)) violations.push(`${rel}: brand in content`);
}

if (violations.length) {
  console.error("brand check failed:");
  for (const item of violations) console.error(`- ${item}`);
  process.exitCode = 1;
} else {
  console.log("brand check passed: internalId=hpa, no brand in technical paths/content");
}
