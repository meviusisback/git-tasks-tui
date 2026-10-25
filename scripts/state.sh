#!/bin/sh
# The facts every report must start from, printed by one command.
#
# Why this exists: on 2026-10-25 four reports in a row described trees the
# remote did not have. Each one cost a round trip, and two of them nearly
# merged the wrong thing. A report that begins with somebody's local memory is
# a claim; this script makes it a measurement.
#
# Usage:  sh scripts/state.sh          (from any clone of this repo)
#         it fetches first, so the numbers are the remote's, not the shell's.
set -eu
cd "$(git rev-parse --show-toplevel)"
git fetch -q --prune --tags origin 2>/dev/null || true

echo "remote main   : $(git rev-parse --short origin/main)"
echo "local HEAD    : $(git rev-parse --short HEAD)$(git rev-parse --abbrev-ref HEAD | sed 's/^/ on /')"

echo
echo "stage branches (remote, 10+):"
git ls-remote --heads origin 'refs/heads/task/1[0-9]*' 'refs/heads/tests/*' 2>/dev/null \
    | sed 's#refs/heads/#  #; s/\t/  /' | sort -k2

echo
echo "review tags (remote, newest 8):"
git ls-remote --tags origin 'refs/tags/review/*' 2>/dev/null \
    | sed 's#refs/tags/#  #; s/\t/  /' | sort -k2 | tail -8

echo
# The ledger view: which PASS covers which tree, and how far main has moved.
if [ -x scripts/review-state.sh ] || [ -f scripts/review-state.sh ]; then
    sh scripts/review-state.sh | sed -n '1,5p' | sed 's/^/ledger: /'
fi

echo
# Facts about the tree that people keep guessing at. Each line is a number, not
# an opinion; a report that contradicts one of these is wrong, not unlucky.
src=src/app.rs
printf 'app.rs: RepoRow=%s last_repo=%s repo_filter=%s\n' \
    "$(git show "origin/main:$src" | grep -c 'enum RepoRow' || true)" \
    "$(git show "origin/main:$src" | grep -c 'last_repo' || true)" \
    "$(git show "origin/main:$src" | grep -c 'RepoFilter' || true)"
printf 'main.rs: selection cleared=%s\n' \
    "$(git show origin/main:src/main.rs | grep -c 'selection cleared' || true)"
