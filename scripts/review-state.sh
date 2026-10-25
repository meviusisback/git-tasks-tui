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
# A row that cannot be read must be **said out loud**: a verdict row whose column 3 is
# not a commit used to be dropped by `continue`, so the ledger reported "newest is X"
# while a correct row sat in the file and did not count. Silent skipping is the same
# defect as a probe that cannot fail. Diagnostics go to a file because the loop below
# runs inside a subshell (`| while`), where variables do not come back.
diag=$(mktemp "${TMPDIR:-/tmp}/review-state.XXXXXX")
trap 'rm -f "$diag"' EXIT INT TERM

tips="$(
    printf '%s\n' "$ledger" | grep '^| `' | while IFS= read -r line; do
        sha=$(printf '%s' "$line" | cut -d'|' -f3 | tr -d '` ' | sed 's/^main@//')
        verdict=$(printf '%s' "$line" | cut -d'|' -f4 | tr -d '*' | sed 's/^ *//; s/ *$//')
        # Two kinds of row live in this file. A **verdict** row has five pipes
        # (`| label | sha | verdict | evidence |`) and must place ONE commit in column 3.
        # A **note** row (`| sha | prose |`) documents a probe and carries no verdict, so
        # it is counted and skipped — not an error, but not silent either.
        pipes=$(printf '%s' "$line" | tr -cd '|' | wc -c | tr -d ' ')
        if [ "$pipes" -lt 5 ]; then
            printf 'note|%s|%s\n' "$sha" "$line" >> "$diag"
            continue
        fi
        case "$sha" in
            ???????*) ;;
            *) printf 'short|%s|%s\n' "$sha" "$line" >> "$diag"; continue ;;
        esac
        if ! git cat-file -e "$sha^{commit}" 2>/dev/null; then
            printf 'unreadable|%s|%s\n' "$sha" "$line" >> "$diag"
            continue
        fi
        if ! git merge-base --is-ancestor "$sha" origin/main 2>/dev/null; then
            # Legitimate: a row may describe a tree that is not in main yet (an open PR,
            # or a tag on a branch). Counted, not silent, and not an error.
            printf 'unmerged|%s|%s\n' "$sha" "$line" >> "$diag"
            continue
        fi
        ahead=$(git rev-list --count "$sha..origin/main" -- . ':(exclude)docs' 2>/dev/null || echo 999999)
        printf '%s|%s|%s|%s\n' "$ahead" "$sha" "$verdict" "$line"
    done | sort -n -t'|' -k1,1
)"

last_row=$(printf '%s\n' "$tips" | grep -v '^$' | head -1 || true)
last_pass=$(printf '%s\n' "$tips" | grep -v '^$' | awk -F'|' '$3 ~ /PASS/ { print; exit }')
last_pass_row=$(printf '%s' "$last_pass" | cut -d'|' -f4-)

row_sha=$(printf '%s' "$last_row" | cut -d'|' -f2)
row_verdict=$(printf '%s' "$last_row" | cut -d'|' -f3)
pass_sha=$(printf '%s' "$last_pass" | cut -d'|' -f2)

# A verdict row can name the symbol that must exist (or be absent) on the tip for
# the row to be true: `key-symbol: path contains "text"`. A merge that was
# declared but never executed leaves no red test — it leaves a missing symbol,
# and that is checkable by machine.
key_symbol=$(printf '%s' "$last_pass_row" | grep -o 'key-symbol: [^|]*' | head -1 \
    | sed 's/^key-symbol: *//')
ks_status=''
if [ -n "$key_symbol" ]; then
    # K-2: the separator is ` contains ` / ` absent `, so a path may contain
    # spaces, and the first occurrence in the row is the one that counts.
    case "$key_symbol" in
        *" contains "*) ks_file=${key_symbol%% contains *}; ks_verb=contains; ks_text=${key_symbol#* contains } ;;
        *" absent "*)   ks_file=${key_symbol%% absent *};   ks_verb=absent;   ks_text=${key_symbol#* absent } ;;
        *) ks_file='' ;;
    esac
    # K-3: after the closing quote comes prose, not more text — and a note after
    # it must not turn a correct row into a false MISSING.
    ks_text=$(printf '%s' "$ks_text" | sed 's/^"//; s/".*$//')
    # K-1: a file that does not exist cannot satisfy `absent` — a missing file
    # is a broken row, not an absent symbol.
    if [ -z "$ks_file" ] || ! git cat-file -e "origin/main:$ks_file" 2>/dev/null; then
        ks_status=missing
    elif git show "origin/main:$ks_file" | grep -qF -- "$ks_text"; then
        ks_found=yes
    else
        ks_found=no
    fi
    if [ -z "$ks_status" ]; then
        if { [ "$ks_verb" = contains ] && [ "$ks_found" = yes ]; } \
            || { [ "$ks_verb" = absent ] && [ "$ks_found" = no ]; }; then
            ks_status=ok
        else
            ks_status=missing
        fi
    fi
fi

# A stale clone shows the same files and the same green tests, and HEAD looks
# like the tip: say out loud how far behind the local tree is, because a verdict
# on an ancestor describes a tree nobody will use.
local=$(git rev-parse --short HEAD)
behind=$(git rev-list --count HEAD..origin/main 2>/dev/null || echo '?')

printf 'remote main    : %s\n' "$main"
printf 'local HEAD     : %s (behind: %s)\n' "$local" "$behind"
printf 'newest row     : %s (%s)\n' "${row_sha:-<none>}" "${row_verdict:-<none>}"
printf 'newest PASS    : %s\n' "${pass_sha:-<none>}"

if [ -n "$pass_sha" ] && git cat-file -e "$pass_sha^{commit}" 2>/dev/null; then
    ahead=$(git rev-list --count "$pass_sha..origin/main" -- . ':(exclude)docs' 2>/dev/null || echo '?')
    printf 'commits since  : %s\n' "$ahead"
    if [ -n "$ks_status" ]; then
        printf 'key-symbol     : %s %s "%s" -> %s\n' \
            "$ks_file" "$ks_verb" "$ks_text" \
            "$([ "$ks_status" = ok ] && echo held || echo VIOLATED)"
    fi
    if [ "$ks_status" = missing ]; then
        printf 'status         : the newest PASS does not hold on this tree — its key-symbol is missing\n'
    elif [ "$ahead" = "0" ]; then
        printf 'status         : in sync — the newest PASS covers this tree\n'
    else
        printf 'status         : %s code commit(s) newer than the last PASS; review %s\n' "$ahead" "$main"
    fi
else
    printf 'status         : no PASS row found; read docs/REVIEWS.md\n'
fi

# The reading of the file, reported as a count — because "I read it" without a number is
# how a dropped row stays invisible.
read_rows=$(printf '%s\n' "$ledger" | grep -c '^| `' || true)
unreadable=$(awk -F'|' '$1=="unreadable"' "$diag" 2>/dev/null | wc -l)
short=$(awk -F'|' '$1=="short"' "$diag" 2>/dev/null | wc -l)
unmerged=$(awk -F'|' '$1=="unmerged"' "$diag" 2>/dev/null | wc -l)
notes=$(awk -F'|' '$1=="note"' "$diag" 2>/dev/null | wc -l)
printf 'rows           : %s read, %s counted, %s notes (no verdict), %s not in main yet\n' \
    "$read_rows" "$(printf '%s\n' "$tips" | grep -c '[^[:space:]]' || true)" "$notes" "$unmerged"

rc=0
if [ "$unreadable" -gt 0 ] || [ "$short" -gt 0 ]; then
    rc=1
    printf '\nledger: %s row(s) could not be read. Column 3 must be ONE commit sha —\n' \
        "$((unreadable + short))" >&2
    printf 'a row like `| main | 8b433d3 / 8d31705 |` is skipped silently by the sort and\n' >&2
    printf 'never reaches the top of the ledger. Fix the row, do not re-run the script.\n\n' >&2
    while IFS='|' read -r kind sha line; do
        [ -n "${sha:-}${line:-}" ] || continue
        case "$kind" in
            unreadable|short)
                printf '  %-10s column 3 = %-30s %s\n' "$kind" "${sha:-<empty>}" \
                    "$(printf '%s' "$line" | cut -c1-90)" >&2
                ;;
        esac
    done < "$diag"
fi
exit $rc
