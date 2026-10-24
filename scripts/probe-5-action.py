#!/usr/bin/env python3
"""Probe 5/5 — **the terminal survives an action that times out**.

The other four probes test the pieces around an action (the argv shown, the run
count, the group kill, the restore on exit). This one tests the moment the user
cares about: pressing `y` runs a write that never answers, and what the terminal
looks like while that happens and after it gives up.

Contract announced by the team lead: `a` opens the confirmation on the selected
item, `y` executes, `n`/`Esc` cancels.

Steps, each one an assertion except where marked:
  1. baseline: the terminal as the shell left it;
  2. the TUI runs and raw mode is observable — otherwise step 6 would be a false
     pass, because "restored" means nothing if it was never changed;
  3. `a` produces **new output** (a repaint) — with the keys unwired the TUI
     ignores the keystroke and draws nothing, which is how this probe tells
     "not implemented" from "implemented and wrong";
  4. (informational) the terminal while the write is in flight;
  5. after `write_timeout_s` the process is still alive;
  5b. the UI answers again — the event loop resumed after the timeout;
  6. after quit, the terminal is byte-identical to the baseline.

Exit 0 = every property held. 1 = a property failed. 2 = the probe could not
reach the state (binary missing, no raw mode, or the keys are not wired yet).
**2 is never a pass**: it means this probe has nothing to say yet.
"""

import fcntl
import os
import pty
import re
import select
import signal
import struct
import shutil
import sys
import tempfile
import termios
import time

BINARY = os.environ.get("GTASKS_BIN", "./target/debug/gtasks")
ANSI = re.compile(r"\x1b\[[0-9;?]*[a-zA-Z]")


def describe(attrs):
    _i, oflag, _c, lflag, _is, _os_, _cc = attrs
    return {
        "ECHO": bool(lflag & termios.ECHO),
        "ICANON": bool(lflag & termios.ICANON),
        "ISIG": bool(lflag & termios.ISIG),
        "OPOST": bool(oflag & termios.OPOST),
    }


def is_raw(attrs):
    d = describe(attrs)
    return not d["ECHO"] and not d["ICANON"]


def setup_home():
    """A scratch HOME: config, and a fake `gh` that lists one PR and never merges."""
    home = tempfile.mkdtemp(prefix="gtasks-5th-")
    os.makedirs(f"{home}/.config/gtasks", exist_ok=True)
    os.makedirs(f"{home}/bin", exist_ok=True)
    os.makedirs(f"{home}/.local/state", exist_ok=True)
    with open(f"{home}/.config/gtasks/config.toml", "w") as f:
        f.write('repos = ["acme/widget"]\ntimeout_s = 2\nwrite_timeout_s = 1\n')
    with open(f"{home}/bin/gh", "w") as f:
        f.write(
            "#!/bin/sh\n"
            f'echo "$@" >> {home}/gh-calls.log\n'
            'case "$1 $2" in\n'
            '  "pr list") printf \'[{"number":7,"title":"A pull request","state":"OPEN"}]\\n\'; exit 0 ;;\n'
            '  "pr view") printf \'{"number":7,"title":"A pull request","state":"OPEN","body":"body"}\\n\'; exit 0 ;;\n'
            '  "pr merge") sleep 30; exit 0 ;;\n'
            "  *) exit 0 ;;\n"
            "esac\n"
        )
    with open(f"{home}/bin/git", "w") as f:
        f.write("#!/bin/sh\nexit 0\n")
    for p in (f"{home}/bin/gh", f"{home}/bin/git"):
        os.chmod(p, 0o755)
    return home


class Session:
    def __init__(self, env):
        self.master, self.slave = pty.openpty()
        # A pty with no window size draws nothing; 40x120 gives the TUI a canvas.
        fcntl.ioctl(self.slave, termios.TIOCSWINSZ, struct.pack("HHHH", 40, 120, 0, 0))
        self.baseline = termios.tcgetattr(self.slave)
        self.pid = os.fork()
        if self.pid == 0:
            os.setsid()
            os.dup2(self.slave, 0)
            os.dup2(self.slave, 1)
            os.dup2(self.slave, 2)
            os.close(self.master)
            os.close(self.slave)
            try:
                os.execvpe(BINARY, [BINARY], env)
            finally:
                os._exit(127)
        self.out = []

    def pump(self, seconds):
        """Read what the TUI draws; return how many bytes arrived."""
        got = 0
        end = time.time() + seconds
        while time.time() < end:
            r, _, _ = select.select([self.master], [], [], 0.05)
            if r:
                try:
                    chunk = os.read(self.master, 8192)
                except OSError:
                    break
                if not chunk:
                    break
                got += len(chunk)
                self.out.append(chunk.decode("utf-8", "replace"))
        return got

    def text(self):
        return ANSI.sub("", "".join(self.out))

    def alive(self):
        return os.waitpid(self.pid, os.WNOHANG)[0] == 0

    def quit(self):
        os.write(self.master, b"\x03")
        for _ in range(100):
            if not self.alive():
                return True
            self.pump(0.05)
        os.kill(self.pid, signal.SIGKILL)
        os.waitpid(self.pid, 0)
        return False


def main():
    if not os.path.exists(BINARY):
        print(f"SKIP: {BINARY} does not exist — build first")
        return 2

    home = setup_home()
    env = dict(os.environ)
    env.update(
        TERM="xterm-256color",
        HOME=home,
        PATH=f"{home}/bin:" + env.get("PATH", ""),
        XDG_CONFIG_HOME=f"{home}/.config",
        XDG_STATE_HOME=f"{home}/.local/state",
        XDG_DATA_HOME=f"{home}/.local/share",
    )

    s = Session(env)
    s.pump(1.5)
    raw_idle = is_raw(termios.tcgetattr(s.slave))
    listed = "A pull request" in s.text()
    print(f"step 2 : raw while idle = {raw_idle} · list rendered = {listed}")

    # Step 3: a repaint is the only honest signal that the key is handled.
    os.write(s.master, b"a")
    repainted = s.pump(0.8)
    print(f"step 3 : 'a' produced {repainted} bytes of repaint")
    if repainted == 0:
        print("UNCLEAR: 'a' is not wired — the TUI ignored the key and drew nothing.")
        print("         This probe cannot test what the code does not implement yet.")
        s.quit()
        os.close(s.master)
        os.close(s.slave)
        shutil.rmtree(home, ignore_errors=True)
        return 2

    os.write(s.master, b"y")
    s.pump(0.6)
    calls = ""
    try:
        with open(f"{home}/gh-calls.log") as f:
            calls = f.read()
    except FileNotFoundError:
        pass
    started = "pr merge" in calls
    print(f"step 3b: the execute key started the write = {started}")
    print(f"step 4 : during the write -> {describe(termios.tcgetattr(s.slave))}  (informational)")
    if not started:
        print("UNCLEAR: 'y' is not wired — no merge was attempted, so there is nothing")
        print("         to time out. Calls seen:", " ".join(calls.split()) or "<none>")
        s.quit()
        os.close(s.master)
        os.close(s.slave)
        shutil.rmtree(home, ignore_errors=True)
        return 2

    s.pump(2.5)
    alive = s.alive()
    print(f"step 5 : after the timeout -> alive = {alive}")

    os.write(s.master, b"a")
    woke = s.pump(0.8)
    print(f"step 5b: the UI drew {woke} bytes after the timeout -> resumed = {woke > 0}")

    s.quit()
    restored = termios.tcgetattr(s.slave) == s.baseline
    print(f"step 6 : after quit -> identical to baseline = {restored}")
    os.close(s.master)
    os.close(s.slave)
    shutil.rmtree(home, ignore_errors=True)

    if not raw_idle:
        print("FAIL: raw mode never observed — 'restored' would be meaningless")
        return 2
    if not alive:
        print("FAIL: the process died during a timed-out write")
        return 1
    if woke == 0:
        print("FAIL: the event loop did not resume after the timeout")
        return 1
    if not restored:
        print("FAIL: the terminal did not come back identical to the baseline")
        return 1
    print("PASS: the terminal survives an action that times out")
    return 0


if __name__ == "__main__":
    sys.exit(main())
