#!/bin/sh
# What a reviewer must start from: the remote tip, the newest **PASS** row in the
# ledger, and how many code commits sit between them.
#
# Three details learned the hard way:
#   * pathspecs are relative to the cwd, so run git from the repository top
#     (S-2): from `docs/` the old version counted the ledger commit as code;
#   * the newest row may be "awaiting verdict" or FAIL, so the reference is the
#     newest row whose verdict is PASS (S-3);
#   * docs-only commits never count as unreviewed code.
set -eu

top=$(git rev-parse --show-toplevel)
cd "$top"

git fetch --quiet origin

main=$(git ls-remote origin refs/heads/main | cut -f1)
ledger=$(git show origin/main:docs/REVIEWS.md 2>/dev/null || true)

field() { printf '%s' "$1" | cut -d'|' -f"$2" | tr -d '`' | sed 's/^ *//; s/ *$//'; }

# Every verdict row counts, whatever its label: `main@<sha>` and `review/<tag>`
# are both ledgers entries, and the sha column is what places them in history.
# For each row whose sha is an ancestor of origin/main, the distance to the tip
# decides which row describes the tree most closely.
tips="$(
    printf '%s\n' "$ledger" | grep '^| `' | while IFS= read -r line; do
        sha=$(printf '%s' "$line" | cut -d'|' -f3 | tr -d '` ' | sed 's/^main@//')
        verdict=$(printf '%s' "$line" | cut -d'|' -f4 | tr -d '*' | sed 's/^ *//; s/ *$//')
        case "$sha" in
            ???????*) ;;
            *) continue ;;
        esac
        git cat-file -e "$sha^{commit}" 2>/dev/null || continue
        git merge-base --is-ancestor "$sha" origin/main 2>/dev/null || continue
        ahead=$(git rev-list --count "$sha..origin/main" -- . ':(exclude)docs' 2>/dev/null || echo 999999)
        printf '%s|%s|%s\n' "$ahead" "$sha" "$verdict"
    done | sort -n -t'|' -k1,1
)"

last_row=$(printf '%s\n' "$tips" | grep -v '^$' | head -1 || true)
last_pass=$(printf '%s\n' "$tips" | grep -v '^$' | awk -F'|' '$3 ~ /PASS/ { print; exit }')

row_sha=$(printf '%s' "$last_row" | cut -d'|' -f2)
row_verdict=$(printf '%s' "$last_row" | cut -d'|' -f3)
pass_sha=$(printf '%s' "$last_pass" | cut -d'|' -f2)

printf 'remote main    : %s\n' "$main"
printf 'newest row     : %s (%s)\n' "${row_sha:-<none>}" "${row_verdict:-<none>}"
printf 'newest PASS    : %s\n' "${pass_sha:-<none>}"

if [ -n "$pass_sha" ] && git cat-file -e "$pass_sha^{commit}" 2>/dev/null; then
    ahead=$(git rev-list --count "$pass_sha..origin/main" -- . ':(exclude)docs' 2>/dev/null || echo '?')
    printf 'commits since  : %s\n' "$ahead"
    if [ "$ahead" = "0" ]; then
        printf 'status         : in sync — the newest PASS covers this tree\n'
    else
        printf 'status         : %s code commit(s) newer than the last PASS; review %s\n' "$ahead" "$main"
    fi
else
    printf 'status         : no PASS row found; read docs/REVIEWS.md\n'
fi
