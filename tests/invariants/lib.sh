#!/usr/bin/env bash
# audit-harness/lib.sh — shared helpers.
#
# Rules for every fixture in this suite:
#   * isolated git config (the developer's global config must not change a result)
#   * fixed identity (no dependence on ~/.gitconfig)
#   * C locale (git messages are matched literally in a few controls)
#   * every fixture lives in its own mktemp dir and is removed by its caller
# NOTE: no `set -e` on purpose — every assertion here inspects non-zero exit
# statuses, and errexit would abort the suite at the first expected failure.
set -uo pipefail

export GIT_CONFIG_GLOBAL=/dev/null
export GIT_CONFIG_SYSTEM=/dev/null
export GIT_CONFIG_NOSYSTEM=1
export LC_ALL=C

GIT_ID=(git -c user.email=audit@example.invalid -c user.name=audit)

PASSED=0
FAILED=0
SKIPPED=0

pass() { printf '  PASS  %s\n' "$1"; PASSED=$((PASSED + 1)); }
fail() { printf '  FAIL  %s%s\n' "$1" "${2:+ — $2}"; FAILED=$((FAILED + 1)); }
skip() { printf '  SKIP  %s%s\n' "$1" "${2:+ — $2}"; SKIPPED=$((SKIPPED + 1)); }
info() { printf '  note  %s\n' "$1"; }
group() { printf '\n== %s\n' "$1"; }

summary() {
  printf '\n%s\n' "-----------------------------------------------"
  printf 'passed=%d failed=%d skipped=%d\n' "$PASSED" "$FAILED" "$SKIPPED"
  [ "$FAILED" -eq 0 ]
}

# assert_eq NAME EXPECTED ACTUAL
assert_eq() {
  if [ "$2" = "$3" ]; then pass "$1"; else fail "$1" "expected [$2], got [$3]"; fi
}

# assert_contains NAME HAYSTACK NEEDLE
assert_contains() {
  case "$2" in *"$3"*) pass "$1" ;; *) fail "$1" "[$2] does not contain [$3]" ;; esac
}

# file_mode PATH -> octal mode, e.g. 700
file_mode() { stat -c '%a' "$1"; }
file_size() { stat -c '%s' "$1"; }

# new_dir -> echoes a fresh temp dir (caller removes it)
new_dir() { mktemp -d "${TMPDIR:-/tmp}/gtasks-harness.XXXXXX"; }

have() { command -v "$1" >/dev/null 2>&1; }

require_python() { have python3 || { echo "python3 required for the symlink-create assertions" >&2; exit 2; }; }
