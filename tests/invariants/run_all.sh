#!/usr/bin/env bash
# audit-harness/run_all.sh — entry point.
#
#   ./run_all.sh                       # controls only (no app needed)
#   ./run_all.sh --bin ./target/debug/gtasks
#   ./run_all.sh --suite controls|invariants|guards|all
#
# Exit code: 0 = every executed case passed (skips are allowed and printed).

set -uo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

BIN=""
SUITE="all"

while [ $# -gt 0 ]; do
  case "$1" in
    --bin) BIN="${2:-}"; shift 2 ;;
    --suite) SUITE="${2:-}"; shift 2 ;;
    -h|--help) sed -n '2,9p' "$0"; exit 0 ;;
    *) echo "unknown argument: $1" >&2; exit 2 ;;
  esac
done

rc=0
case "$SUITE" in
  controls) bash "$HERE/controls.sh"; rc=$? ;;
  guards) bash "$HERE/guards.sh"; rc=$? ;;
  invariants)
    [ -n "$BIN" ] || { echo "--suite invariants needs --bin" >&2; exit 2; }
    bash "$HERE/invariants.sh" "$BIN"; rc=$? ;;
  all)
    # The guards first: cheapest cases, and the ones that police every commit and
    # every push — they should be the first thing a red gate shows.
    bash "$HERE/guards.sh" || rc=1
    bash "$HERE/controls.sh"; rc=$?
    if [ -n "$BIN" ]; then
      bash "$HERE/invariants.sh" "$BIN" || rc=1
    else
      printf '\n(no --bin given: invariants not run)\n'
    fi ;;
  *) echo "unknown suite: $SUITE" >&2; exit 2 ;;
esac
exit "$rc"
