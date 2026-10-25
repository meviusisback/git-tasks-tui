#!/usr/bin/env bash
# audit-harness/invariants.sh — ASSERTION SUITE against the gtasks binary.
#
# Each case states an invariant from docs/security.md. Cases whose subject does
# not exist yet (no worktree resolver, no gh wrapper) SKIP loudly instead of
# passing silently — a silent pass is how a suite dies.
#
# Usage: invariants.sh /path/to/gtasks

set -uo pipefail
# shellcheck source=lib.sh
. "$(dirname "${BASH_SOURCE[0]}")/lib.sh"
# shellcheck source=fixtures.sh
. "$(dirname "${BASH_SOURCE[0]}")/fixtures.sh"

BIN="${1:-}"
[ -n "$BIN" ] && [ -x "$BIN" ] || { echo "usage: invariants.sh /path/to/gtasks" >&2; exit 2; }

run_bin() { # run_bin XDG_STATE_HOME
  XDG_STATE_HOME="$1" HOME="${HARNESS_HOME:-$HOME}" "$BIN" >/dev/null 2>&1
  echo $?
}

inv_version() {
  group "INV — the binary reports a version (baseline: the harness talks to the right artefact)"
  local out; out=$("$BIN" --version 2>&1)
  assert_contains "--version prints a gtasks version" "$out" "gtasks "
}

inv_state_dir_symlink_not_followed() {
  group "INV S14/F-1 — a symlinked state dir is refused, never chmod'd or written through"
  local d; d=$(new_dir); trap 'rm -rf "'"$d"'"' RETURN
  local pair state victim
  pair=$(fx_state_dir_symlink "$d"); state=${pair% *}; victim=${pair#* }
  local before; before=$(file_mode "$victim")
  run_bin "$state" >/dev/null
  assert_eq "victim directory mode unchanged" "$before" "$(file_mode "$victim")"
  if [ -e "$victim/gtasks.log" ]; then
    fail "no log written through the symlinked state dir" "found $victim/gtasks.log"
  else
    pass "no log written through the symlinked state dir"
  fi
}

inv_log_symlink_not_followed() {
  group "INV S14/F-2 — a symlinked gtasks.log is never appended to or chmod'd"
  local d; d=$(new_dir); trap 'rm -rf "'"$d"'"' RETURN
  local pair state victim
  pair=$(fx_log_symlink "$d"); state=${pair% *}; victim=${pair#* }
  local size_before mode_before
  size_before=$(file_size "$victim"); mode_before=$(file_mode "$victim")
  run_bin "$state" >/dev/null
  assert_eq "victim file size unchanged" "$size_before" "$(file_size "$victim")"
  assert_eq "victim file mode unchanged" "$mode_before" "$(file_mode "$victim")"
}

inv_state_dir_mode() {
  group "INV S14 — a clean run creates 0700 state dir and 0600 log"
  local d; d=$(new_dir); trap 'rm -rf "'"$d"'"' RETURN
  local state="$d/state"; mkdir -p "$state"; chmod 0700 "$state"
  run_bin "$state" >/dev/null
  assert_eq "state dir is 0700" "700" "$(file_mode "$state/gtasks")"
  assert_eq "log file is 0600" "600" "$(file_mode "$state/gtasks/gtasks.log")"
  # Anti-vacuous: the assertions above are worthless if logging init never ran.
  # Measured trap: `gtasks --version` exits inside clap BEFORE init_file_logging,
  # so a repro driven by --version never reaches the code under test. Assert the
  # path was exercised by the artefact it produces.
  local size; size=$(file_size "$state/gtasks/gtasks.log" 2>/dev/null || echo 0)
  if [ "${size:-0}" -gt 0 ]; then
    pass "log file is non-empty — logging init actually ran"
  else
    fail "log file is non-empty — logging init actually ran" "size=$size: the code path was not exercised"
  fi
}

# --- not wired yet: they need the app's API (Task 2 / 10b) -------------------
inv_gh_wrapper_argv() {
  group "INV S1/S4 — gh wrapper argv (NOT WIRED)"
  skip "argv_never_shell_metachars / argv_forbids_admin_force_auto" "needs task/02 (gh wrapper)"
  skip "child_env_has_no_git_vars" "needs task/02 (env allowlist)"
}


inv_spawn_sites_declared() {
  group "INV S2 — every spawn site is declared, and every declaration is still true"
  local root; root=$(git rev-parse --show-toplevel)
  if out=$(check_spawn_sites "$root" 2>&1); then
    pass "all $(spawn_sites_found "$root" | wc -l) Command::new sites are declared"
  else
    fail "spawn sites match the declared list" "$out"
  fi
}

inv_agent_argv_flags() {
  group "INV S18 — the hermes argv keeps S16: --ignore-rules always, --yolo/--accept-hooks never"
  local root; root=$(git rev-parse --show-toplevel)
  set +e
  out=$(check_agent_argv_flags "$root" 2>&1)
  rc=$?
  set -e
  case "$rc" in
    0) pass "every hermes argv builder carries --ignore-rules and none of the silent flags" ;;
    2) skip "agent_argv_carries_ignore_rules" "needs task 16 (build_command: no file mentions --query-file yet)" ;;
    *) fail "the hermes argv keeps S16" "$out" ;;
  esac
}

inv_argv_sites_declared() {
  group "INV S17 — every CheckedArgv construction is declared (one door per decision)"
  local root; root=$(git rev-parse --show-toplevel)
  if out=$(check_argv_sites "$root" 2>&1); then
    pass "all $(argv_sites_found "$root" | wc -l) from_checked( sites are declared"
  else
    fail "CheckedArgv construction sites match the declared list" "$out"
  fi
}

inv_worktree_resolver() {
  group "INV S5/S6 — worktree path + reuse (NOT WIRED)"
  skip "reuse_only_from_worktree_list_porcelain" "needs task/10b (worktree resolver)"
  skip "outside_root_symlink_blocks_creation" "needs task/10b (worktree resolver)"
  skip "worktree_branch_mismatch_is_not_reused" "needs task/10b (worktree resolver)"
  skip "repo_hook_does_not_run_on_worktree_add" "needs task/10b (git call site)"
  skip "dot_git_file_redirect_is_rejected" "needs task/10b (clone validation)"
}

main() {
  inv_version
  inv_state_dir_symlink_not_followed
  inv_log_symlink_not_followed
  inv_state_dir_mode
  inv_gh_wrapper_argv
  inv_spawn_sites_declared
  inv_argv_sites_declared
  inv_agent_argv_flags
  inv_worktree_resolver
  summary
}

main "$@"
