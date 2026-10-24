#!/bin/sh
# What a reviewer must start from: the remote tip, and the newest row in the
# review ledger. If the SHA you are about to review is not this one, the review
# is of a state that no longer exists.
set -eu

git fetch --quiet origin

main=$(git ls-remote origin refs/heads/main | cut -f1)
row=$(git show origin/main:docs/REVIEWS.md 2>/dev/null \
      | grep '^| `main@' | tail -1 | cut -d'|' -f2 | tr -d ' ' | tr -d '`' || true)
sha=${row#main@}

printf 'remote main  : %s\n' "$main"
printf 'newest row   : %s\n' "${sha:-<none>}"

if [ -n "${sha:-}" ] && git cat-file -e "$sha^{commit}" 2>/dev/null; then
    # Docs commits only carry the ledger row itself: they must not count as
    # "unreviewed code".
    ahead=$(git rev-list --count "$sha..origin/main" -- . ':(exclude)docs' 2>/dev/null || echo '?')
    printf 'commits since: %s\n' "$ahead"
    if [ "$ahead" = "0" ]; then
        printf 'status       : in sync — review this tree\n'
    else
        printf 'status       : %s commit(s) newer than the last row; review %s\n' "$ahead" "$main"
    fi
else
    printf 'status       : no usable ledger row; review %s\n' "$main"
fi
