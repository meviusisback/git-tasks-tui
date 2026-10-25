#!/bin/sh
# How long until the first screen is worth looking at?
#
# The requirement (docs/ux-redesign.md): with 69 configured repos the list must
# be populated within ten seconds. "Populated" is not "the process started" and
# not "the spinner appeared": it is the first moment the left column shows an
# item row.
#
# Usage:  sh scripts/measure-startup.sh [seconds_to_wait] [binary]
#         default: 120 seconds, target/release/gtasks
#
# Four things this script refuses to get wrong, each learned by measuring badly
# first:
#
#   1. ENVIRONMENT. `VAR=x tmux new-session ...` does NOT pass VAR to the
#      session when a tmux server is already running — the session inherits the
#      *server's* environment. On a desktop that is always the case, so the
#      isolation was theoretical. `-e` is the only way that works.
#   2. IDENTITY. A duration with no object attached is a rumour: the source sha
#      and the binary's own hash are printed, so "7 s" cannot later belong to a
#      different tree.
#   3. THE CONTROL. If the pattern that recognises an item row matches nothing,
#      "never within Ns" is indistinguishable from "the pattern is wrong". A
#      known-good screen is measured first; if the pattern cannot match there,
#      the script refuses to give a verdict instead of inventing one.
#   4. RESOLUTION. The requirement is ten seconds; sampling at one second
#      rounds every result up by up to a second. Sampling is 250 ms.
set -eu

limit=${1:-120}
bin=${2:-target/release/gtasks}
session="gtasks-measure-$$"
control_session="gtasks-control-$$"
payload=$(mktemp -d)
cleanup() {
    tmux kill-session -t "$session" 2>/dev/null || true
    tmux kill-session -t "$control_session" 2>/dev/null || true
    rm -rf "$payload"
}
trap cleanup EXIT INT TERM

cd "$(git rev-parse --show-toplevel)"

if [ ! -x "$bin" ]; then
    echo "no binary at $bin — build it first (cargo build --release)" >&2
    exit 2
fi

# What is being measured (2).
abs_bin=$(cd "$(dirname "$bin")" && pwd)/$(basename "$bin")
echo "source sha        : $(git rev-parse --short HEAD)"
echo "binary            : $abs_bin"
echo "binary sha256     : $(sha256sum "$abs_bin" | cut -c1-16)"

# A row looks like `││#12345 …`; keep the pattern in one place so the control and
# the measurement cannot disagree about what they are looking for.
pattern='││#[0-9][0-9]* '

wait_for_row() { # wait_for_row <session> <seconds> <step>
    session=$1; wait_s=$2; step=$3
    started=$(date +%s%N)
    while :; do
        line=$(tmux capture-pane -p -t "$session" 2>/dev/null | grep -oE "$pattern" | head -1 || true)
        if [ -n "$line" ]; then
            now=$(date +%s%N)
            echo $(( (now - started) / 1000000 ))
            return 0
        fi
        elapsed_ms=$(( ($(date +%s%N) - started) / 1000000 ))
        [ "$elapsed_ms" -ge $((wait_s * 1000)) ] && return 1
        sleep "$step"
    done
}

# The control (3): one repo, isolated config, must show a row.
mkdir -p "$payload/control/gtasks"
printf 'repos = ["cli/cli"]\nlimit = 5\n' > "$payload/control/gtasks/config.toml"
# Authentication: `gh` inside a detached session sees only what we pass. Without
# the token exported, the run fails for auth and the number says "too slow" —
# a harness failure wearing the face of a measurement. Pass the variables by
# NAME (-e VAR), so the value never appears on any command line or in any log.
auth=""
for var in GH_TOKEN GITHUB_TOKEN; do
    eval "val=\${$var:-}"
    if [ -n "$val" ]; then
        auth="$auth -e $var"
    fi
done

# shellcheck disable=SC2086
tmux new-session -d -s "$control_session" -x 120 -y 30 \
    -e XDG_CONFIG_HOME="$payload/control" -e XDG_STATE_HOME="$payload/control-state" \
    $auth "$abs_bin"
if ! control_ms=$(wait_for_row "$control_session" 60 0.25); then
    echo "control           : the pattern matched no item row on a known-good screen"
    echo "verdict           : unmeasurable — the pattern is wrong, not the program"
    exit 3
fi
echo "control           : row visible after ${control_ms}ms (one repo) — the pattern works"
tmux kill-session -t "$control_session" 2>/dev/null || true

# The measurement (1, 4): the user's own config, isolated state, real sampling.
mkdir -p "$payload/state"
config_home=${XDG_CONFIG_HOME:-$HOME/.config}
repos=$(grep -cE '"[A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+"' "$config_home/gtasks/config.toml" 2>/dev/null || echo 0)
echo "config read by app: $config_home/gtasks/config.toml ($repos repos)"
# shellcheck disable=SC2086
tmux new-session -d -s "$session" -x 120 -y 30 \
    -e XDG_CONFIG_HOME="$config_home" -e XDG_STATE_HOME="$payload/state" \
    $auth "$abs_bin"

# A binary that never started must not be reported as a slow binary: the two
# verdicts are different, and the first one is a defect of the harness.
sleep 1
if ! tmux has-session -t "$session" 2>/dev/null; then
    echo "binary started    : no — the session died within a second"
    echo "verdict           : unmeasurable — the binary did not start" 
    exit 4
fi


if first_ms=$(wait_for_row "$session" "$limit" 0.25); then
    secs=$((first_ms / 1000))
    printf 'first useful row  : %sms (%ss)\n' "$first_ms" "$secs"
    if [ "$first_ms" -le 10000 ]; then
        echo "verdict           : within the ten-second requirement"
    else
        echo "verdict           : too slow — over ten seconds"
    fi
else
    echo "first useful row  : never within ${limit}s"
    echo "verdict           : too slow — nothing on screen (the control proved the pattern works)"
fi
