#!/usr/bin/env bash
# Runs every PostgreSQL integration test file against its own throwaway database.
#
# Sharing one database across the whole workspace pollutes state that these
# tests mutate on the `local` tenant (quota limits, agent sessions, executions),
# which makes later files fail for reasons unrelated to their subject. Each file
# gets a fresh database here, which is also what CI should do.
#
# Usage:
#   HPA_POSTGRES_ADMIN_URL=postgresql://user@host:5432/postgres scripts/run-postgres-tests.sh
#   HPA_POSTGRES_TEST_URL=postgresql://user@host:5432/db scripts/run-postgres-tests.sh  # reuse one server, derive per-file DBs
#   CARGO=/absolute/path/to/cargo ... scripts/run-postgres-tests.sh                # non-login shell
#
# The account needs CREATEDB. `--keep` keeps the databases for inspection.

set -uo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root" || exit 1
if [ -n "${CARGO:-}" ]; then
  cargo_cmd="$CARGO"
elif command -v cargo >/dev/null 2>&1; then
  cargo_cmd="cargo"
elif [ -x "${HOME}/.cargo/bin/cargo" ]; then
  cargo_cmd="${HOME}/.cargo/bin/cargo"
else
  cargo_cmd="cargo"
fi

keep=0
for arg in "$@"; do
  [ "$arg" = "--keep" ] && keep=1
done

base_url="${HPA_POSTGRES_ADMIN_URL:-${HPA_POSTGRES_TEST_URL:-}}"
if [ -z "$base_url" ]; then
  echo "HPA_POSTGRES_ADMIN_URL (or HPA_POSTGRES_TEST_URL) is required" >&2
  exit 2
fi

# Split the URL into "<scheme>://<credentials>@<host>:<port>" so per-file
# databases can be addressed without guessing the user or socket path.
# postgresql://user@host:5432/somedb -> postgresql://user@host:5432
url_no_query="${base_url%%\?*}"
# Keep scheme://authority and drop the database path.
prefix="$(printf '%s' "$url_no_query" | sed -E 's#^([a-zA-Z][a-zA-Z0-9+.-]*://[^/]+).*$#\1#')"
case "$prefix" in
  *://*) ;;
  *)
    echo "cannot parse PostgreSQL URL: ${base_url}" >&2
    echo "expected form: postgresql://user[:pass]@host[:port]/database" >&2
    exit 2
    ;;
esac
admin_url="${prefix}/postgres"

psql_admin() {
  psql "$admin_url" -v ON_ERROR_STOP=1 -tAc "$1"
}

run_id="$(date +%s)_$$"
passed=0
failed=0
failed_files=()

shopt -s nullglob
files=(crates/hpa-local/tests/postgres_*.rs)
shopt -u nullglob

if [ ${#files[@]} -eq 0 ]; then
  echo "no PostgreSQL test files found" >&2
  exit 1
fi

echo "running ${#files[@]} PostgreSQL test files against dedicated databases"

for file in "${files[@]}"; do
  name="$(basename "$file" .rs)"
  db="hpa_t_${run_id}_${name}"
  if ! psql_admin "CREATE DATABASE ${db}" >/dev/null 2>&1; then
    echo "  ${name}: cannot create database ${db}" >&2
    failed=$((failed + 1))
    failed_files+=("$name (createdb failed)")
    continue
  fi
  printf '  %-36s ' "$name"
  output="$(HPA_POSTGRES_TEST_URL="${prefix}/${db}" "$cargo_cmd" test -q -p hpa-local --test "$name" 2>&1)"
  status=$?
  summary="$(printf '%s\n' "$output" | grep -E '^test result' | tail -1)"
  if [ $status -eq 0 ]; then
    passed=$((passed + 1))
    echo "ok   ${summary:-}"
  else
    failed=$((failed + 1))
    failed_files+=("$name")
    echo "FAIL ${summary:-}"
    printf '%s\n' "$output" | grep -E 'panicked at|assertion' | head -3 | sed 's/^/      /'
  fi
  if [ $keep -eq 1 ]; then
    echo "      kept database: ${db}"
  else
    psql_admin "DROP DATABASE IF EXISTS ${db}" >/dev/null 2>&1
  fi
done

echo
echo "summary: ${passed} file(s) passed, ${failed} failed"
if [ $failed -gt 0 ]; then
  printf 'failed: %s\n' "${failed_files[*]}"
  exit 1
fi
