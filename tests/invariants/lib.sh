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

# Every place that turns tokens into a `CheckedArgv` — i.e. every door into the
# transport. `from_checked` is `pub(crate)`, so any module can call it; the point of this
# list is that adding a third door is a decision, not an edit.
#
# Declared as `file|count|reason`. A count is used instead of line numbers so an edit
# above a call site does not fail the invariant, while a **new** call in an existing file
# still does: two doors in one file are two doors.
ARGV_SITES_ALLOWED='src/actions.rs|1|an action becomes an argv here, through the allowlist
src/fetch.rs|1|the batch read is a query the builder vouches for'

argv_sites_found() { # argv_sites_found <root> → "file:count" per file, sorted
  local root=$1
  grep -rn 'from_checked(' "$root/src" 2>/dev/null \
    | grep -v 'fn from_checked' \
    | grep -v '^\s*//' \
    | grep -v '///' \
    | awk -F: '{print $1}' \
    | sed "s|^${root%/}/||; s|^\./||" \
    | sort | uniq -c | awk '{print $2 ":" $1}' | sort
}

check_argv_sites() { # check_argv_sites <root> → 0 ok, 1 undeclared/stale
  local root=$1 rc=0 pair file count declared_f declared_c reason
  local found
  found=$(argv_sites_found "$root")
  # A detector that finds nothing is a broken pattern, not a clean repo: the rename of
  # `from_checked` would make every direction below pass in silence.
  if [ -z "$found" ]; then
    printf 'the detector found no from_checked( anywhere in %s/src — the pattern is wrong, not the repo\n' "$root" >&2
    return 1
  fi
  # direction 1: a found site that is not declared, or whose count disagrees
  while IFS= read -r pair; do
    [ -n "$pair" ] || continue
    file=${pair%:*}; count=${pair##*:}
    declared_c=$(printf '%s\n' "$ARGV_SITES_ALLOWED" | grep -F "$file|" | cut -d'|' -f2)
    if [ -z "$declared_c" ]; then
      printf 'undeclared CheckedArgv construction: %s — add it to ARGV_SITES_ALLOWED with its reason\n' "$file" >&2
      rc=1
    elif [ "$declared_c" != "$count" ]; then
      printf '%s now builds %s argvs, declared %s — a second door in the same file is still a second door\n' \
        "$file" "$count" "$declared_c" >&2
      rc=1
    fi
  done <<EOF
$(printf '%s\n' "$found")
EOF
  # direction 2: a declaration that no longer exists
  while IFS='|' read -r file count reason; do
    [ -n "${file:-}" ] || continue
    if ! printf '%s\n' "$found" | grep -qF "$file:"; then
      printf 'stale declaration: %s no longer builds any argv (reason given: %s)\n' "$file" "$reason" >&2
      rc=1
    fi
  done <<EOF
$(printf '%s\n' "$ARGV_SITES_ALLOWED")
EOF
  return $rc
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
