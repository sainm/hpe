import fs from "node:fs";
import path from "node:path";

const root = path.resolve(new URL("..", import.meta.url).pathname);
const skip = new Set([
  ".git",
  ".venv",
  "__pycache__",
  ".pytest_cache",
  ".mypy_cache",
  ".ruff_cache",
  "node_modules",
  "target",
  "dist",
  "build",
  "coverage",
]);
const errors = [];
let markdownFiles = 0;
let links = 0;

function walk(directory) {
  for (const entry of fs.readdirSync(directory, { withFileTypes: true })) {
    if (entry.isDirectory()) {
      if (!skip.has(entry.name)) walk(path.join(directory, entry.name));
      continue;
    }
    if (entry.name.endsWith(".md")) check(path.join(directory, entry.name));
  }
}

function check(file) {
  markdownFiles += 1;
  const text = fs.readFileSync(file, "utf8");
  for (const match of text.matchAll(/(?<!!)\[[^\]]*\]\(([^)]+)\)/g)) {
    const raw = match[1].trim().split(/\s+/)[0].replace(/^<|>$/g, "");
    if (
      raw === "" ||
      raw.startsWith("#") ||
      /^(https?:|mailto:|codex:)/.test(raw)
    ) {
      continue;
    }
    links += 1;
    const target = path.resolve(path.dirname(file), raw.split("#", 1)[0]);
    if (!fs.existsSync(target)) {
      const line = text.slice(0, match.index).split("\n").length;
      errors.push(`${path.relative(root, file)}:${line} -> ${raw}`);
    }
  }
}

walk(root);
if (errors.length > 0) {
  console.error(`Documentation link check failed (${errors.length}):`);
  for (const error of errors) console.error(`- ${error}`);
  process.exit(1);
}
console.log(`Documentation link check passed: ${markdownFiles} files, ${links} local links`);
