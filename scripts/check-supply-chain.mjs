import { execFileSync } from "node:child_process";
import crypto from "node:crypto";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";

const root = path.resolve(new URL("..", import.meta.url).pathname);
const allowedLicenses = [
  "0BSD",
  "Apache-2.0",
  "BSD-2-Clause",
  "BSD-3-Clause",
  "CC0-1.0",
  "ISC",
  "MIT",
  "MIT-0",
  "MPL-2.0",
  "BlueOak-1.0.0",
  "BSL-1.0",
  "CDLA-Permissive-2.0",
  "Artistic-2.0",
  "Python-2.0",
  "Unicode-3.0",
  "Unlicense",
  "Zlib",
];
const deniedFragments = ["AGPL", "SSPL", "BUSL", "Commons-Clause"];
const errors = [];
const components = [];

function licenseAllowed(expression) {
  if (!expression) return false;
  const normalized = expression
    .replaceAll("(", " ")
    .replaceAll(")", " ")
    .replaceAll("/", " OR ")
    .trim();
  return normalized.split(/\s+OR\s+/i).some((alternative) => {
    if (deniedFragments.some((fragment) => alternative.includes(fragment))) {
      return false;
    }
    const tokens = alternative
      .replaceAll(" AND ", " ")
      .replaceAll(" WITH ", " ")
      .split(/\s+/)
      .filter(Boolean)
      .filter((token) => token !== "LLVM-exception");
    return tokens.length > 0 && tokens.every((token) => allowedLicenses.includes(token));
  });
}

let rustDependencies = 0;
const bundledCargo = path.join(os.homedir(), ".cargo", "bin", "cargo");
const cargo =
  process.env.CARGO || (fs.existsSync(bundledCargo) ? bundledCargo : "cargo");
const metadata = JSON.parse(
  execFileSync(cargo, ["metadata", "--locked", "--format-version", "1"], {
    cwd: root,
    encoding: "utf8",
    maxBuffer: 32 * 1024 * 1024,
  }),
);

for (const pkg of metadata.packages.filter((candidate) => candidate.source)) {
  const name = pkg.name;
  const version = pkg.version;
  const license = pkg.license;
  const licenseFile = pkg.license_file;
  rustDependencies += 1;
  components.push({
    type: "library",
    name,
    version,
    purl: `pkg:cargo/${encodeURIComponent(name)}@${encodeURIComponent(version)}`,
    licenses: license ? [{ license: { id: license } }] : [],
  });
  if (deniedFragments.some((fragment) => (license ?? "").includes(fragment))) {
    errors.push(`Rust dependency ${name}@${version} uses denied license ${license}`);
  } else if (!licenseAllowed(license) && !licenseFile) {
    errors.push(
      `Rust dependency ${name}@${version} has unapproved license ${license ?? "<missing>"}`,
    );
  }
}

let nodeDependencies = 0;
const licenseChecked = new Set();
const pnpmStore = path.join(root, "node_modules", ".pnpm");
if (fs.existsSync(pnpmStore)) {
  for (const packageDir of fs.readdirSync(pnpmStore, { withFileTypes: true })) {
    if (!packageDir.isDirectory()) continue;
    const nodeModules = path.join(pnpmStore, packageDir.name, "node_modules");
    if (!fs.existsSync(nodeModules)) continue;
    for (const entry of fs.readdirSync(nodeModules, { withFileTypes: true })) {
      const manifests = [];
      if (entry.name.startsWith("@") && entry.isDirectory()) {
        for (const scoped of fs.readdirSync(path.join(nodeModules, entry.name), {
          withFileTypes: true,
        })) {
          manifests.push(path.join(nodeModules, entry.name, scoped.name, "package.json"));
        }
      } else {
        manifests.push(path.join(nodeModules, entry.name, "package.json"));
      }
      for (const manifestPath of manifests) {
        if (!fs.existsSync(manifestPath)) continue;
        const manifest = JSON.parse(fs.readFileSync(manifestPath, "utf8"));
        const identity = `${manifest.name}@${manifest.version}`;
        if (!licenseChecked.has(identity)) {
          licenseChecked.add(identity);
        } else {
          continue;
        }
        nodeDependencies += 1;
        components.push({
          type: "library",
          name: manifest.name,
          version: manifest.version,
          purl: `pkg:npm/${manifest.name.replace("@", "%40")}@${manifest.version}`,
          licenses: manifest.license
            ? [{ license: { id: manifest.license } }]
            : [],
        });
        const buildOnlyMicrosoftSigningTool =
          manifest.name === "@vscode/vsce-sign" ||
          manifest.name.startsWith("@vscode/vsce-sign-");
        if (!licenseAllowed(manifest.license) && !buildOnlyMicrosoftSigningTool) {
          errors.push(
            `Node dependency ${manifest.name}@${manifest.version} has unapproved license ${manifest.license ?? "<missing>"}`,
          );
        }
      }
    }
  }
}

const tracked = execFileSync("git", ["ls-files"], { cwd: root, encoding: "utf8" })
  .split("\n")
  .filter(Boolean);
const forbiddenSource = /(^|\/)(LICENSE_EE|enterprise_edition)(\/|$)/i;
for (const file of tracked) {
  if (forbiddenSource.test(file) && !file.startsWith("docs/")) {
    errors.push(`forbidden enterprise-license source path: ${file}`);
  }
}

if (errors.length > 0) {
  console.error(`Supply-chain check failed (${errors.length}):`);
  for (const error of errors) console.error(`- ${error}`);
  process.exit(1);
}

const sbomPath =
  process.env.HPA_SBOM_PATH ||
  path.join(root, "target", "sbom", "hpa-sbom.cdx.json");
fs.mkdirSync(path.dirname(sbomPath), { recursive: true });
fs.writeFileSync(
  sbomPath,
  `${JSON.stringify(
    {
      bomFormat: "CycloneDX",
      specVersion: "1.6",
      serialNumber: `urn:uuid:${crypto.randomUUID()}`,
      version: 1,
      metadata: {
        timestamp: new Date().toISOString(),
        component: {
          type: "application",
          name: "hpa",
          version: JSON.parse(fs.readFileSync(path.join(root, "package.json"), "utf8")).version,
        },
      },
      components,
    },
    null,
    2,
  )}\n`,
);

console.log(
  `Supply-chain check passed: ${rustDependencies} resolved Rust registry/git packages, ` +
    `${nodeDependencies} installed Node packages, no denied license families; SBOM: ${path.relative(root, sbomPath)}`,
);
