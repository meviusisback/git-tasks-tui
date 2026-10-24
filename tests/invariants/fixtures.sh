#!/usr/bin/env bash
# audit-harness/fixtures.sh — fixture builders, sourced by the suites.
#
# Each builder takes the temp dir to build in and echoes the path of the object
# it produced (repo, clone, victim file...). Nothing here asserts; assertions
# live in controls.sh (threat is real) and invariants.sh (app resists).

# fx_repo_with_worktree_symlink DIR
# A repository that commits the exact outside-clone worktree path as a symlink to
# a victim directory. Models the H-2 payload: repo content pre-empting a path the
# app will use.
fx_repo_with_worktree_symlink() {
  local dir="$1" slug="$2" victim="$3"
  mkdir -p "$dir/evil/.worktrees"
  ln -s "$victim" "$dir/evil/.worktrees/$slug"
  ( cd "$dir/evil" && "${GIT_ID[@]}" init -q . && "${GIT_ID[@]}" add -A \
      && "${GIT_ID[@]}" commit -qm "pre-empt worktree path" )
  echo "$dir/evil"
}

# fx_clone_with_symlink DIR SLUG VICTIM -> echoes the clone path
fx_clone_with_symlink() {
  local dir="$1" slug="$2" victim="$3"
  fx_repo_with_worktree_symlink "$dir" "$slug" "$victim" >/dev/null
  "${GIT_ID[@]}" clone -q "$dir/evil" "$dir/clone"
  echo "$dir/clone"
}

# fx_repo_with_hook DIR -> repository whose post-checkout hook writes a marker.
# The marker path is echoed on the last line.
fx_repo_with_hook() {
  local dir="$1"
  local marker="$dir/HOOK-RAN"
  mkdir -p "$dir/victim"
  ( cd "$dir/victim" && "${GIT_ID[@]}" init -q . && echo x > f \
      && "${GIT_ID[@]}" add f && "${GIT_ID[@]}" commit -qm c1
    mkdir -p .git/hooks
    printf '#!/bin/sh\ntouch %s\n' "$marker" > .git/hooks/post-checkout
    chmod +x .git/hooks/post-checkout )
  echo "$dir/victim"
  echo "$marker"
}

# fx_dir_with_dotgit_file DIR -> a directory whose .git is a FILE pointing at a
# foreign git dir (the redirect S16 must reject).
fx_dir_with_dotgit_file() {
  local dir="$1"
  mkdir -p "$dir/other"
  ( cd "$dir/other" && "${GIT_ID[@]}" init -q . && echo y > g \
      && "${GIT_ID[@]}" add g && "${GIT_ID[@]}" commit -qm c2 )
  mkdir -p "$dir/fake-clone"
  printf 'gitdir: %s/other/.git\n' "$dir" > "$dir/fake-clone/.git"
  echo "$dir/fake-clone"
}

# fx_state_dir_symlink DIR -> XDG_STATE_HOME containing `gtasks` as a symlink to a
# victim directory at 0755.
# echoes: "<state_home> <victim_dir>"
fx_state_dir_symlink() {
  local dir="$1"
  local state="$dir/state" victim="$dir/victim-state-dir"
  mkdir -p "$state" "$victim"
  chmod 0755 "$victim"
  ln -s "$victim" "$state/gtasks"
  echo "$state $victim"
}

# fx_log_symlink DIR -> a real state dir whose gtasks.log is a symlink to a victim
# file at 0644 with known content.
# echoes: "<state_home> <victim_file>"
fx_log_symlink() {
  local dir="$1"
  local state="$dir/state-log" victim="$dir/victim.txt"
  mkdir -p "$state/gtasks"
  chmod 0700 "$state/gtasks"
  printf 'VICTIM ORIGINAL CONTENT\n' > "$victim"
  chmod 0644 "$victim"
  ln -s "$victim" "$state/gtasks/gtasks.log"
  echo "$state $victim"
}
