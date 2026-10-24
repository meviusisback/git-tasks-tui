#!/bin/sh
# One command per clone: point git at the versioned hooks directory.
set -eu
top=$(git rev-parse --show-toplevel); cd "$top"
# Absolute on purpose: in a linked worktree a relative path is resolved against
# the worktree's git dir, not the checkout, so `.githooks` would silently not exist.
git config core.hooksPath "$top/.githooks"
chmod +x .githooks/* 2>/dev/null || true
echo "hooks installed: core.hooksPath=$(git config --get core.hooksPath)"
