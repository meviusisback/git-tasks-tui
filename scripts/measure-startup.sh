#!/bin/sh
# How long until the first screen is worth looking at?
#
# The requirement (docs/ux-redesign.md): with 69 configured repos the list must
# be populated within ten seconds. "Populated" is not "the process started" and
# not "the spinner appeared": it is the first moment the left column shows a
# repo with a non-zero count.
#
# Usage:  sh scripts/measure-startup.sh [seconds_to_wait] [binary]
#         default: 120 seconds, target/release/gtasks
#
# It runs the real binary in a detached tmux session with an isolated
# XDG_STATE_HOME, so a measurement never writes into the real log or state.
set -eu

limit=${1:-120}
bin=${2:-target/release/gtasks}
session="gtasks-measure-$$"
payload=$(mktemp -d)
trap 'tmux kill-session -t "$session" 2>/dev/null || true; rm -rf "$payload"' EXIT INT TERM

cd "$(git rev-parse --show-toplevel)"

if [ ! -x "$bin" ]; then
    echo "no binary at $bin — build it first (cargo build --release)" >&2
    exit 2
fi

XDG_STATE_HOME="$payload/state" XDG_CONFIG_HOME="${XDG_CONFIG_HOME:-$HOME/.config}" \
    tmux new-session -d -s "$session" -x 120 -y 30 "$(pwd)/$bin"

start=$(date +%s)
first=''
while :; do
    now=$(date +%s)
    elapsed=$((now - start))
    # An item row in the left column (`││#12345 …`): that is the list being
    # worth a look. A repo count is not the signal — counts can stay at zero
    # while the items themselves are already drawn under the repo.
    line=$(tmux capture-pane -p -t "$session" 2>/dev/null | grep -oE '││#[0-9]+ ' | head -1 || true)
    if [ -n "$line" ]; then
        first=$elapsed
        break
    fi
    if [ "$elapsed" -ge "$limit" ]; then
        break
    fi
    sleep 1
done

echo "binary            : $bin"
echo "repos configured  : $(grep -c '^ *"meviusisback/\|^ *"[a-zA-Z0-9_.-]*/[a-zA-Z0-9_.-]*"' "${XDG_CONFIG_HOME:-$HOME/.config}/gtasks/config.toml" 2>/dev/null || echo '?')"
if [ -n "$first" ]; then
    echo "first useful row  : ${first}s"
    if [ "$first" -le 10 ]; then
        echo "verdict           : within the ten-second requirement"
    else
        echo "verdict           : too slow — over ten seconds"
    fi
else
    echo "first useful row  : never within ${limit}s"
    echo "verdict           : too slow — nothing on screen"
fi
