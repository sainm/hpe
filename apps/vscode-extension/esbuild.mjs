import { build, context } from "esbuild";

const watch = process.argv.includes("--watch");
const options = {
  entryPoints: ["src/extension.ts"],
  bundle: true,
  platform: "node",
  format: "cjs",
  target: "node22",
  outfile: "dist/extension.js",
  external: ["vscode"],
  sourcemap: true,
  minify: false,
  logLevel: "info",
};

if (watch) {
  const watcher = await context(options);
  await watcher.watch();
} else {
  await build(options);
}
