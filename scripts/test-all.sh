#!/bin/sh
# Run every test binary, even after a failure.
#
# `cargo test` stops at the first failing binary, and the lib tests run before
# the integration ones: a "green" count from a plain `cargo test` only proves
# that the first binaries passed. This runs them all and reports the totals.
set -eu

cd "$(git rev-parse --show-toplevel)"

# A guard that has to be invited is not a guard. `pre-push` and `commit-msg`
# only run when `core.hooksPath` points at `.githooks` in THIS repository's
# config — passing it per command (`git -c core.hooksPath=...`) covers only the
# person who remembers to. Measured 2026-10-25: a refused commit went out
# because the push was invoked without the flag, and the guard never ran.
# So the gate checks that the guards are installed, and fails loudly if not.
hooks_missing=no
for hook in commit-msg pre-push; do
    if [ ! -x ".githooks/$hook" ]; then
        echo "gate: .githooks/$hook is not executable — git would skip it silently" >&2
        hooks_missing=yes
    fi
done
configured=$(git config --get core.hooksPath 2>/dev/null || true)
if [ "$configured" != "$(pwd)/.githooks" ] && [ "$configured" != ".githooks" ]; then
    echo "gate: core.hooksPath is '${configured:-<unset>}' — the guards are not installed here." >&2
    echo "gate: run sh scripts/install-hooks.sh in this clone, then re-run." >&2
    hooks_missing=yes
fi

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
# The number alone hides which suites it counts, and the explanation then lives
# in whoever wrote the script. Print the names: a count without its object is a
# claim nobody can check.
awk '
    /^     Running |^   Doc-tests / { name = $0; sub(/^ +/, "", name) }
    /^test result: ok\. 0 passed; 0 failed; 0 ignored/ { print "  empty:   " name }
    /^test result: ok\. 0 passed; 0 failed; [1-9][0-9]* ignored/ { print "  skipped: " name }
' "$log"

if [ "$failed" != "0" ]; then
    printf '\nfailures:\n'
    grep -E '^test .*FAILED' "$log" || true
fi

if [ "$hooks_missing" = yes ]; then
    printf '\nthe gate FAILED for a reason the tests cannot see: the guards are not installed.\n' >&2
    status=1
fi

exit "$status"
