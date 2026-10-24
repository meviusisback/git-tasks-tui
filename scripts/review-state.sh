#!/bin/sh
# What a reviewer must start from: the remote tip, and the newest row in the
# review ledger. If the SHA you are about to review is not this one, the review
# is of a state that no longer exists.
set -eu

git fetch --quiet origin

main=$(git ls-remote origin refs/heads/main | cut -f1)
ledger=$(git show origin/main:docs/REVIEWS.md 2>/dev/null \
         | grep '^| `main@' | tail -1 | cut -d'|' -f2 | tr -d ' ' || true)

printf 'remote main : %s\n' "$main"
printf 'ledger row  : %s\n' "${ledger:-<none>}"

if [ "$ledger" = "\`$main\`" ]; then
    printf 'status      : in sync — review this tree\n'
else
    printf 'status      : ledger is behind main; review %s and expect the ledger to catch up\n' "$main"
fi
