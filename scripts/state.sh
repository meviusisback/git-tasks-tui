#!/bin/sh
# The facts every report must start from, printed by one command.
#
# Why this exists: on 2026-10-25 four reports in a row described trees the
# remote did not have. Each one cost a round trip, and two of them nearly
# merged the wrong thing. A report that begins with somebody's local memory is
# a claim; this script makes it a measurement.
#
# Usage:  sh scripts/state.sh          (from any clone of this repo)
#
# Four defects fixed after the reviewer measured them (2026-10-25):
#   1. a failed fetch used to be silent, so stale refs wore the face of fresh
#      ones — the one guarantee this tool exists to give;
#   2. "newest 8 tags" sorted by NAME and therefore showed the oldest series;
#   3. the ledger view cut the line that says WHICH sha to review;
#   4. `repo_filter=13` counted a name, not a fact: a number without a claim.
set -eu
cd "$(git rev-parse --show-toplevel)"

# 1. The fetch result is printed, because a silent failure here is the exact
#    lie this script was built to prevent. Numbers are still printed when the
#    fetch fails — they are useful, they just have to be labelled.
err=$(mktemp)
if ! git fetch -q --prune --tags origin 2>"$err"; then
    echo "STALE — fetch failed: $(head -1 "$err" 2>/dev/null || echo 'no detail')"
fi
rm -f "$err"

echo "remote main   : $(git rev-parse --short origin/main 2>/dev/null || echo '?')"
# A detached HEAD has no branch name: say so, instead of printing "HEAD on HEAD".
branch=$(git rev-parse --abbrev-ref HEAD 2>/dev/null || echo HEAD)
if [ "$branch" = HEAD ]; then
    echo "local HEAD    : $(git rev-parse --short HEAD) (detached)"
else
    echo "local HEAD    : $(git rev-parse --short HEAD) on $branch"
fi

echo
echo "stage branches (remote, 10+):"
git ls-remote --heads origin 'refs/heads/task/1[0-9]*' 'refs/heads/tests/*' 2>/dev/null \
    | sed 's#refs/heads/#  #; s/\t/  /' | sort -k2

echo
# 2. Newest means newest by date, not by name: `ls-remote` cannot sort by date,
#    but the fetch above already updated the local tags, so `for-each-ref` can.
echo "review tags (newest 8, by creation date):"
git for-each-ref --sort=-creatordate --count=8 \
    --format='  %(creatordate:short)  %(refname:short)  %(objectname:short)' \
    refs/tags/review 2>/dev/null || echo "  (none)"

echo
# The ledger view: which PASS covers which tree, and how far main has moved.
# 3. All seven lines: the last one names the sha to review, and cutting it
#    leaves the diagnosis without the prescription.
if [ -f scripts/review-state.sh ]; then
    sh scripts/review-state.sh | sed -n '1,7p' | sed 's/^/ledger: /'
fi

echo
# 4. Facts, each as a claim that can be true or false. A count of occurrences
#    of a name ("RepoFilter=13") is not a fact anybody can act on.
src=src/app.rs
printf 'app.rs   : RepoRow=%s  last_repo=%s\n' \
    "$(git show "origin/main:$src" | grep -c 'enum RepoRow' || true)" \
    "$(git show "origin/main:$src" | grep -c 'last_repo' || true)"
printf 'app.rs   : RepoFilter is a field=%s (0 means it is derived, which is what we want)\n' \
    "$(git show "origin/main:$src" | grep -c 'repo_filter: RepoFilter' || true)"
printf 'main.rs  : selection cleared=%s\n' \
    "$(git show origin/main:src/main.rs | grep -c 'selection cleared' || true)"
