#!/bin/sh
# One command per clone: point git at the versioned hooks directory.
#
# H-2: an earlier version of this comment blamed relative paths in linked
# worktrees. Measured (git 2.56): a **relative** `core.hooksPath` works in a
# worktree too — in a non-bare repository git runs hooks from the root of the
# working tree. The silent skip came from the **executable bit**: git ignores a
# hook that is not executable and only prints a hint, so "installed" and
# "running" are different states. The `test -x` below is what makes the claim
# checkable instead of promised.
set -eu
top=$(git rev-parse --show-toplevel); cd "$top"
# Absolute for clarity, not necessity: relative works, as measured.
git config core.hooksPath "$top/.githooks"
chmod +x .githooks/* 2>/dev/null || true
if [ ! -x .githooks/commit-msg ]; then
    echo "install-hooks: .githooks/commit-msg is not executable — git would skip it" >&2
    exit 1
fi
echo "hooks installed and executable: core.hooksPath=$(git config --get core.hooksPath)"
