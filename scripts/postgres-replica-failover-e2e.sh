#!/usr/bin/env bash
# End-to-end rehearsal of the PostgreSQL read-replica and failover path.
#
# Creates two throwaway clusters in a temp directory (primary + streaming
# standby), then drives a real `hpa serve` through:
#   1. writes on the primary, reads from the replica
#   2. a paused replica so the console read is provably served by the standby
#   3. replica outage -> listing falls back to the primary
#   4. primary killed, standby promoted, node restarted against the promotion
#
# Requires initdb/pg_ctl/pg_basebackup/psql on PATH (PostgreSQL 16+).
# Usage: scripts/postgres-replica-failover-e2e.sh [--keep]

set -uo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root" || exit 1

keep=0
[ "${1:-}" = "--keep" ] && keep=1

bin="${HPA_BINARY:-$root/target/debug/hpa}"
if [ ! -x "$bin" ]; then
  echo "missing binary: $bin (run cargo build -p hpa-local)" >&2
  exit 1
fi

for tool in initdb pg_ctl pg_basebackup psql; do
  command -v "$tool" >/dev/null 2>&1 || { echo "$tool not found on PATH" >&2; exit 1; }
done

work="$(mktemp -d "${TMPDIR:-/tmp}/hpa-ha-XXXXXX")"
primary="$work/primary"
standby="$work/standby"
appdir="$work/app"
logs="$work/logs"
mkdir -p "$appdir" "$logs"

port=$(( 55000 + RANDOM % 400 ))
failport=$(( port + 400 ))
appport=$(( port + 800 ))
ready=0
server_pid=""

cleanup() {
  [ -n "$server_pid" ] && kill -9 "$server_pid" 2>/dev/null
  pg_ctl -D "$standby" -m immediate stop >/dev/null 2>&1
  pg_ctl -D "$primary" -m immediate stop >/dev/null 2>&1
  if [ $keep -eq 1 ]; then
    echo "kept work dir: $work (logs in $logs)"
  else
    rm -rf "$work"
  fi
}
trap cleanup EXIT

fail() {
  echo "FAIL: $*" >&2
  exit 1
}

psql_primary() { psql -h 127.0.0.1 -p "$port" -U postgres -tAc "$1"; }
psql_standby() { psql -h 127.0.0.1 -p "$((port + 1))" -U postgres -tAc "$1"; }

wait_ready() {
  for _ in $(seq 1 200); do
    if curl -fsS "http://127.0.0.1:$appport/api/health" >/dev/null 2>&1; then return 0; fi
    sleep 0.05
  done
  return 1
}

count_executions() {
  curl -fsS -H "authorization: Bearer $token" \
    "http://127.0.0.1:$appport/api/executions?limit=50" | python3 -c 'import json,sys; print(len(json.load(sys.stdin)))'
}

echo "work dir: $work (primary :$port, standby :$((port + 1)), app :$appport)"

initdb -D "$primary" -U postgres --auth=trust >"$logs/initdb.log" 2>&1 || fail "initdb"
{
  echo "port=$port"
  echo "wal_level=replica"
  echo "max_wal_senders=4"
  echo "hot_standby=on"
  echo "listen_addresses='127.0.0.1'"
} >> "$primary/postgresql.conf"
echo "host replication all 127.0.0.1/32 trust" >> "$primary/pg_hba.conf"
pg_ctl -D "$primary" -l "$logs/primary.log" start >/dev/null 2>&1 || fail "start primary"
sleep 2

pg_basebackup -h 127.0.0.1 -p "$port" -U postgres -D "$standby" -R -X stream >"$logs/basebackup.log" 2>&1 || fail "pg_basebackup"
echo "port=$((port + 1))" >> "$standby/postgresql.conf"
pg_ctl -D "$standby" -l "$logs/standby.log" start >/dev/null 2>&1 || fail "start standby"
sleep 2
[ "$(psql_standby 'select pg_is_in_recovery()')" = "t" ] || fail "standby is not in recovery"

primary_url="postgresql://postgres@127.0.0.1:$port/postgres"
standby_url="postgresql://postgres@127.0.0.1:$((port + 1))/postgres"

HPA_POSTGRES_URL="$primary_url" "$bin" --data-dir "$appdir" db migrate >"$logs/migrate.json" 2>&1 || fail "migrate"
sleep 1
[ "$(psql_standby 'select count(*) from public._sqlx_migrations')" -gt 0 ] || fail "schema did not replicate"

token="$(HPA_POSTGRES_URL="$primary_url" "$bin" --data-dir "$appdir" enterprise init | python3 -c 'import json,sys; print(json.load(sys.stdin)["token"])')" || fail "enterprise init"

start_server() {
  HPA_POSTGRES_URL="$1" HPA_POSTGRES_REPLICA_URL="$2" HPA_POSTGRES_RUNTIME=1 \
    HPA_ENTERPRISE_MODE=required HPA_NODE_ID="ha-probe" \
    "$bin" --data-dir "$appdir" serve --port "$appport" >>"$logs/serve.log" 2>&1 &
  server_pid=$!
  wait_ready || { tail -20 "$logs/serve.log" >&2; fail "server did not become ready"; }
}

start_server "$primary_url" "$standby_url"
echo "  server started with replica"

curl -fsS -o /dev/null -X POST -H "authorization: Bearer $token" -H "content-type: application/json" \
  -d '{"ir":{"$schema":"https://hpa.dev/schema/ir/v0.1/workflow.schema.json","schemaVersion":"hpa.dev/v1","id":"wf_ha","name":"HA","workflowVersion":"1.0.0","nodes":[{"id":"start","type":"start","typeVersion":"1.0.0","name":"S","config":{}},{"id":"end","type":"end","typeVersion":"1.0.0","name":"E","config":{}}],"edges":[{"id":"e1","source":"start","sourcePort":"out","target":"end","targetPort":"in"}]}}' \
  "http://127.0.0.1:$appport/api/workflows" || fail "create workflow"
curl -fsS -o /dev/null -X POST -H "authorization: Bearer $token" -H "content-type: application/json" -d '{}' \
  "http://127.0.0.1:$appport/api/workflows/wf_ha/publish" || fail "publish workflow"

for i in 1 2 3; do
  curl -fsS -o /dev/null -X POST -H "authorization: Bearer $token" -H "content-type: application/json" \
    -d "{\"input\":{\"i\":$i}}" "http://127.0.0.1:$appport/api/workflows/wf_ha/executions" || fail "start execution $i"
done
sleep 1
[ "$(count_executions)" = "3" ] || fail "expected 3 executions, got $(count_executions)"
echo "  baseline: 3 executions visible"

psql_standby "select pg_wal_replay_pause()" >/dev/null
curl -fsS -o /dev/null -X POST -H "authorization: Bearer $token" -H "content-type: application/json" \
  -d '{"input":{"i":4}}' "http://127.0.0.1:$appport/api/workflows/wf_ha/executions" || fail "start execution 4"
sleep 1
stale="$(count_executions)"
[ "$stale" = "3" ] || fail "listing should be served by the paused replica (expected 3, got $stale)"
echo "  replica paused: listing still 3 (proves read routing)"

psql_standby "select pg_wal_replay_resume()" >/dev/null
sleep 2
fresh="$(count_executions)"
[ "$fresh" = "4" ] || fail "after resume expected 4, got $fresh"
echo "  replay resumed: listing 4"

# Replica outage: point the node at a dead replica, listing must still work.
kill -9 "$server_pid" 2>/dev/null
wait "$server_pid" 2>/dev/null
server_pid=""
start_server "$primary_url" "postgresql://postgres@127.0.0.1:$failport/postgres"
fallback="$(count_executions)"
[ "$fallback" = "4" ] || fail "fallback to primary expected 4, got $fallback"
echo "  replica unreachable: listing still 4 (fallback to primary)"

# Failover: kill the primary, promote the standby, restart against it.
kill -9 "$server_pid" 2>/dev/null
wait "$server_pid" 2>/dev/null
server_pid=""
pg_ctl -D "$primary" -m immediate stop >/dev/null 2>&1
pg_ctl -D "$standby" promote >/dev/null 2>&1 || fail "promote standby"
sleep 2
[ "$(psql_standby 'select pg_is_in_recovery()')" = "f" ] || fail "standby was not promoted"

start_server "$standby_url" ""
after_promote="$(count_executions)"
[ "$after_promote" = "4" ] || fail "promoted node expected 4 executions, got $after_promote"
curl -fsS -o /dev/null -X POST -H "authorization: Bearer $token" -H "content-type: application/json" \
  -d '{"input":{"after":"failover"}}' "http://127.0.0.1:$appport/api/workflows/wf_ha/executions" || fail "write after failover"
sleep 1
final="$(count_executions)"
[ "$final" = "5" ] || fail "after failover write expected 5, got $final"
echo "  failover: promoted node serves 4 rows and accepts a write (5)"

echo "PASS: replica routing, lag, fallback and promotion all verified"
