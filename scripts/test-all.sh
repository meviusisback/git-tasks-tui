#!/bin/sh
# Run every test binary, even after a failure.
#
# `cargo test` stops at the first failing binary, and the lib tests run before
# the integration ones: a "green" count from a plain `cargo test` only proves
# that the first binaries passed. This runs them all and reports the totals.
set -eu

cd "$(git rev-parse --show-toplevel)"

log=$(mktemp)
trap 'rm -f "$log"' EXIT

status=0
cargo test --locked --no-fail-fast "$@" >"$log" 2>&1 || status=$?

bins=$(grep -c '^     Running' "$log" || true)
ok=$(grep -c 'test result: ok' "$log" || true)
failed=$(grep -c 'test result: FAILED' "$log" || true)

printf 'binaries run : %s\n' "$bins"
printf 'suites ok    : %s\n' "$ok"
printf 'suites failed: %s\n' "$failed"

if [ "$failed" != "0" ]; then
    printf '\nfailures:\n'
    grep -E '^test .*FAILED' "$log" || true
fi

exit "$status"
