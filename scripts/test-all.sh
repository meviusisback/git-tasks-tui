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
docs=$(grep -c '^   Doc-tests' "$log" || true)
ok=$(grep -c 'test result: ok' "$log" || true)
failed=$(grep -c 'test result: FAILED' "$log" || true)
# `suites ok` counts result lines, which include Doc-tests sections: three
# numbers, three different things. The fourth is the one nobody looks at — a
# suite that passes having run no tests at all, i.e. one that could be emptied
# without the counter noticing.
empty=$(grep -c '^test result: ok\. 0 passed; 0 failed; 0 ignored' "$log" || true)

printf 'binaries run : %s\n' "$bins"
printf 'doc-tests    : %s\n' "$docs"
printf 'suites ok    : %s\n' "$ok"
printf 'suites failed: %s\n' "$failed"
printf 'empty suites : %s (ok, 0 passed and 0 ignored — a suite that declares nothing)\n' "$empty"

if [ "$failed" != "0" ]; then
    printf '\nfailures:\n'
    grep -E '^test .*FAILED' "$log" || true
fi

exit "$status"
