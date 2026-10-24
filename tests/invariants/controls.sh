#!/usr/bin/env bash
# audit-harness/controls.sh — CONTROL CASES.
#
# A control proves the THREAT is real, so that the matching invariant in
# invariants.sh is not vacuous: a test that passes both with and without the bug
# is a decoration, not a test. Every case here must PASS on any healthy host —
# if one ever fails, the platform changed and the invariants have lost their
# teeth (fix the control, not the invariant).

set -uo pipefail
# shellcheck source=lib.sh
. "$(dirname "${BASH_SOURCE[0]}")/lib.sh"
# shellcheck source=fixtures.sh
. "$(dirname "${BASH_SOURCE[0]}")/fixtures.sh"

ctl_worktree_symlink_in_clone() {
  group "CTL A1 — a committed symlink survives the clone and blocks worktree creation"
  local d; d=$(new_dir); trap 'rm -rf "'"$d"'"' RETURN
  local clone; clone=$(fx_clone_with_symlink "$d" "acme-widget-5" "$HOME/.ssh")

  local lt; lt=$(stat -c '%F' "$clone/.worktrees/acme-widget-5" 2>/dev/null || echo missing)
  assert_contains "symlink is present after clone" "$lt" "symbolic link"

  local out rc
  out=$(git -C "$clone" worktree add ".worktrees/acme-widget-5" -b feat HEAD 2>&1); rc=$?
  assert_eq "git refuses the pre-empted path (rc)" "128" "$rc"
  assert_contains "git says it already exists" "$out" "already exists"

  local listed; listed=$(git -C "$clone" worktree list --porcelain)
  case "$listed" in
    *acme-widget-5*) fail "unregistered path is not listed by worktree list" "listed: $listed" ;;
    *) pass "unregistered path is not listed by worktree list (reuse must use this)";;
  esac
}

ctl_outside_root_symlink() {
  group "CTL A2 — create_dir refuses an existing symlink; metadata() would lie"
  require_python
  local d; d=$(new_dir); trap 'rm -rf "'"$d"'"' RETURN
  local victim="$d/victim" p="$d/root/acme-widget-5"
  mkdir -p "$victim" "$d/root"; chmod 0755 "$victim"; ln -s "$victim" "$p"

  local r
  r=$(python3 - "$p" <<'PY'
import errno, os, sys
p = sys.argv[1]
try:
    os.mkdir(p); print("created")
except OSError as e:
    print("mkdir", errno.errorcode.get(e.errno, e.errno))
print("islink", os.path.islink(p))
print("isdir-follows", os.path.isdir(p))
try:
    os.close(os.open(p, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW)); print("open01 ok")
except OSError as e:
    print("open01", errno.errorcode.get(e.errno, e.errno))
PY
)
  assert_contains "mkdir on an existing symlink → EEXIST" "$r" "mkdir EEXIST"
  assert_contains "symlink_metadata sees a link" "$r" "islink True"
  assert_contains "metadata() follows it and reports a dir" "$r" "isdir-follows True"
  assert_contains "O_NOFOLLOW|O_DIRECTORY refuses" "$r" "open01 ENOTDIR"
}

ctl_chmod_follows_symlink() {
  group "CTL F-1/F-2 primitive — chmod through a symlink hits the target"
  local d; d=$(new_dir); trap 'rm -rf "'"$d"'"' RETURN
  local victim="$d/victim"; mkdir -p "$victim"; chmod 0755 "$victim"
  ln -s "$victim" "$d/link"
  chmod 0700 "$d/link"
  assert_eq "chmod on the link changed the target to 700" "700" "$(file_mode "$victim")"
  info "so set_permissions(path) after create_dir_all is a chmod-clobber primitive"
}

ctl_git_config_env() {
  group "CTL B1 — GIT_CONFIG_COUNT injects arbitrary git config"
  local d; d=$(new_dir); trap 'rm -rf "'"$d"'"' RETURN
  mkdir -p "$d/repo"; ( cd "$d/repo" && "${GIT_ID[@]}" init -q . )
  local val
  val=$(GIT_CONFIG_COUNT=1 GIT_CONFIG_KEY_0=core.hooksPath GIT_CONFIG_VALUE_0="$d/evil-hooks" \
        git -C "$d/repo" config --get core.hooksPath)
  assert_eq "injected core.hooksPath is visible to git" "$d/evil-hooks" "$val"
  info "hooksPath is command execution: fsmonitor/post-checkout/pager all resolve through it"
}

ctl_git_dir_env() {
  group "CTL B2 — GIT_DIR overrides git -C"
  local d; d=$(new_dir); trap 'rm -rf "'"$d"'"' RETURN
  mkdir -p "$d/repo"; ( cd "$d/repo" && "${GIT_ID[@]}" init -q . && echo z > f \
      && "${GIT_ID[@]}" add f && "${GIT_ID[@]}" commit -qm c )
  local out rc
  out=$(GIT_DIR="$d/notarepo" git -C "$d/repo" rev-parse --absolute-git-dir 2>&1); rc=$?
  [ "$rc" -ne 0 ] && pass "command fails with GIT_DIR pointing elsewhere (so -C is not isolation)" \
    || fail "GIT_DIR was ignored" "$out"
  assert_contains "fatal: not a git repository" "$out" "not a git repository"
}

ctl_repo_hook() {
  group "CTL C1 — git worktree add executes the repo's post-checkout hook"
  local d; d=$(new_dir); trap 'rm -rf "'"$d"'"' RETURN
  local repo marker
  { read -r repo; read -r marker; } < <(fx_repo_with_hook "$d")
  rm -f "$marker"

  git -C "$repo" worktree add "$d/wt-plain" -b hb HEAD >/dev/null 2>&1
  [ -f "$marker" ] && pass "hook RUNS on plain worktree add (the finding)" \
    || fail "hook did not run" "the control is broken, re-check the fixture"

  rm -f "$marker"
  git -C "$repo" -c core.hooksPath=/nonexistent-hooks \
      worktree add "$d/wt-kept" -b hb2 HEAD >/dev/null 2>&1
  [ -f "$marker" ] && fail "-c core.hooksPath suppression did NOT work" \
    || pass "-c core.hooksPath=/nonexistent-hooks suppresses it (the fix)"
}

ctl_dotgit_file() {
  group "CTL C2 — a .git FILE redirects the git dir; --show-toplevel does not notice"
  local d; d=$(new_dir); trap 'rm -rf "'"$d"'"' RETURN
  local fake; fake=$(fx_dir_with_dotgit_file "$d")
  local gd top
  gd=$(git -C "$fake" rev-parse --absolute-git-dir)
  top=$(git -C "$fake" rev-parse --show-toplevel)
  assert_eq "absolute-git-dir points at the foreign repo" "$d/other/.git" "$gd"
  assert_eq "show-toplevel still names the scanned dir" "$fake" "$top"
  info "so clone validation must compare the git dir, not the toplevel"
}

main() {
  have git || { echo "git required" >&2; exit 2; }
  ctl_worktree_symlink_in_clone
  ctl_outside_root_symlink
  ctl_chmod_follows_symlink
  ctl_git_config_env
  ctl_git_dir_env
  ctl_repo_hook
  ctl_dotgit_file
  summary
}

main "$@"
