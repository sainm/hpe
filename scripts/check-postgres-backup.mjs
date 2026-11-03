import { spawnSync } from "node:child_process";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";

const rawUrl = process.env.HPA_POSTGRES_TEST_URL;
if (!rawUrl) {
  console.error("HPA_POSTGRES_TEST_URL is required");
  process.exit(1);
}

const url = new URL(rawUrl);
const sourceDatabase = decodeURIComponent(url.pathname.replace(/^\//, ""));
if (
  !sourceDatabase.startsWith("hpa_") ||
  !/(test|phase13|backup)/i.test(sourceDatabase)
) {
  console.error(
    `refusing backup rehearsal against non-test database ${JSON.stringify(sourceDatabase)}`,
  );
  process.exit(1);
}
const suffix = `${Date.now().toString(36)}_${process.pid.toString(36)}`;
const restoredDatabase = `${sourceDatabase.slice(0, 36)}_restore_${suffix}`;
if (!/^[a-zA-Z0-9_]+$/.test(restoredDatabase) || restoredDatabase.length > 63) {
  console.error("generated restore database name is invalid");
  process.exit(1);
}

const environment = {
  ...process.env,
  PGHOST: url.hostname,
  PGPORT: url.port || "5432",
  PGUSER: decodeURIComponent(url.username),
  PGPASSWORD: decodeURIComponent(url.password),
};
const tools = {
  psql: process.env.PSQL || "psql",
  pgDump: process.env.PG_DUMP || "pg_dump",
  pgRestore: process.env.PG_RESTORE || "pg_restore",
  createdb: process.env.CREATEDB || "createdb",
  dropdb: process.env.DROPDB || "dropdb",
};
const keep = process.argv.includes("--keep");
const temporaryDirectory = fs.mkdtempSync(
  path.join(os.tmpdir(), "hpa-postgres-backup-"),
);
const dumpPath = path.join(temporaryDirectory, "control-plane.dump");
let restored = false;

function run(command, args, options = {}) {
  const result = spawnSync(command, args, {
    env: environment,
    encoding: options.capture ? "utf8" : undefined,
    stdio: options.capture ? "pipe" : "inherit",
  });
  if (result.error) throw result.error;
  if (result.status !== 0) {
    const detail = options.capture
      ? `\nstdout:\n${result.stdout}\nstderr:\n${result.stderr}`
      : "";
    throw new Error(`${command} exited with ${result.status}${detail}`);
  }
  return result.stdout?.trim() || "";
}

function query(database, sql) {
  return run(tools.psql, ["-X", "-q", "-A", "-t", "-v", "ON_ERROR_STOP=1", "-d", database, "-c", sql], {
    capture: true,
  });
}

const schemaInventory = `
WITH relation_counts AS (
  SELECT
    n.nspname || '.' || c.relname AS relation_name,
    COALESCE(
      (
        xpath(
          '/row/count/text()',
          query_to_xml(
            format('SELECT count(*) AS count FROM %I.%I', n.nspname, c.relname),
            false,
            true,
            ''
          )
        )
      )[1]::text::bigint,
      0
    ) AS row_count
  FROM pg_class c
  JOIN pg_namespace n ON n.oid = c.relnamespace
  WHERE n.nspname IN ('hpa_control', 'hpa_tenant')
    AND c.relkind = 'r'
)
SELECT jsonb_build_object(
  'migrations', (
    SELECT COALESCE(
      jsonb_agg(jsonb_build_object(
        'version', version,
        'checksum', encode(checksum, 'hex')
      ) ORDER BY version),
      '[]'::jsonb
    )
    FROM public._sqlx_migrations
    WHERE success
  ),
  'rows', (
    SELECT COALESCE(
      jsonb_object_agg(relation_name, row_count ORDER BY relation_name),
      '{}'::jsonb
    )
    FROM relation_counts
  ),
  'rlsTables', (
    SELECT count(*)
    FROM pg_class c
    JOIN pg_namespace n ON n.oid = c.relnamespace
    WHERE n.nspname IN ('hpa_control', 'hpa_tenant')
      AND c.relkind = 'r'
      AND c.relrowsecurity
      AND c.relforcerowsecurity
  ),
  'rlsWithoutForce', (
    SELECT count(*)
    FROM pg_class c
    JOIN pg_namespace n ON n.oid = c.relnamespace
    WHERE n.nspname IN ('hpa_control', 'hpa_tenant')
      AND c.relkind = 'r'
      AND c.relrowsecurity
      AND NOT c.relforcerowsecurity
  )
)
`;

try {
  run(tools.pgDump, [
    "--format=custom",
    "--no-password",
    "--file",
    dumpPath,
    sourceDatabase,
  ]);
  run(tools.createdb, ["--no-password", restoredDatabase]);
  restored = true;
  run(tools.pgRestore, [
    "--exit-on-error",
    "--no-password",
    "--dbname",
    restoredDatabase,
    dumpPath,
  ]);

  const sourceInventory = JSON.parse(query(sourceDatabase, schemaInventory));
  const restoredInventory = JSON.parse(query(restoredDatabase, schemaInventory));
  if (JSON.stringify(sourceInventory) !== JSON.stringify(restoredInventory)) {
    throw new Error("restored schema/data inventory does not match source");
  }
  if (restoredInventory.rlsWithoutForce !== 0) {
    throw new Error("restored database contains RLS tables without FORCE RLS");
  }
  if (restoredInventory.rlsTables !== 68) {
    throw new Error(
      `expected 68 FORCE RLS tenant tables, got ${restoredInventory.rlsTables}`,
    );
  }
  console.log(
    JSON.stringify(
      {
        source: sourceDatabase,
        restored: restoredDatabase,
        dumpBytes: fs.statSync(dumpPath).size,
        migrations: restoredInventory.migrations.length,
        relations: Object.keys(restoredInventory.rows).length,
        rlsTables: restoredInventory.rlsTables,
        retained: keep,
      },
      null,
      2,
    ),
  );
  if (keep) {
    console.log(`restored database retained: ${restoredDatabase}`);
  }
} finally {
  if (restored && !keep) {
    run(tools.dropdb, ["--if-exists", "--no-password", restoredDatabase]);
  }
  fs.rmSync(temporaryDirectory, { recursive: true, force: true });
}
