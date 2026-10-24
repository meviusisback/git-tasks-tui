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

# Spawn sites: every `Command::new` in src/ must be declared, and every declared
# site must still exist. Grep alone answers "how many"; this answers "which ones,
# and does each still deserve to be there".
# Declared format: "<file>|<reason>" one per line.
SPAWN_SITES_ALLOWED='
src/gh/mod.rs|run_pipe: the single pipe spawner (process group, PDEATHSIG, caps)
src/actions.rs|open_in_browser: detached launcher, no pipe, no group (B-2 reap)
'

spawn_sites_found() { # spawn_sites_found <root> → files containing Command::new, sorted
  grep -rl 'Command::new' "$1/src" --include='*.rs' 2>/dev/null | sed "s|^$1/||" | sort -u
}

check_spawn_sites() { # check_spawn_sites <root> → 0 ok, 1 undeclared or stale
  local root=$1 rc=0 f reason declared found
  found=$(spawn_sites_found "$root")
  # A detector that finds nothing is not a healthy repo, it is a broken pattern:
  # a rename or a helper wrapper would make every direction below pass in silence.
  if [ -z "$found" ]; then
    printf 'the detector found no Command::new anywhere in %s/src — the pattern is wrong, not the repo\n' "$root" >&2
    return 1
  fi
  declared=$(printf '%s\n' "$SPAWN_SITES_ALLOWED" | sed '/^$/d' | cut -d'|' -f1 | sort -u)
  # direction 1: a found site that is not declared
  while IFS= read -r f; do
    [ -n "$f" ] || continue
    if ! printf '%s\n' "$declared" | grep -qxF "$f"; then
      printf 'undeclared spawn site: %s — add it to SPAWN_SITES_ALLOWED with its reason\n' "$f" >&2
      rc=1
    fi
  done <<EOF
$(spawn_sites_found "$root")
EOF
  # direction 2: a declared site that no longer exists (a stale exemption)
  while IFS= read -r f; do
    [ -n "$f" ] || continue
    if ! grep -q 'Command::new' "$root/$f" 2>/dev/null; then
      printf 'stale exemption: %s is declared but has no Command::new — remove it from the list\n' "$f" >&2
      rc=1
    fi
  done <<EOF
$declared
EOF
  return $rc
}
