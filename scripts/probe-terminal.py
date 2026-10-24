#!/usr/bin/env python3
"""Probe 3/4 for the execution block — **the terminal must be usable again**.

Property: a TUI that puts the terminal into raw mode must leave it as it found it,
including when it exits because of a signal, and including after an action has run.
This measures the terminal state itself (termios on the pty), not what the program
says about it — the difference matters, because "I restored the terminal" is
exactly the kind of statement a program makes while the user's shell has no echo.

Checks, in order:
  1. before spawning: cooked mode (the baseline);
  2. while the TUI runs: ECHO off and ICANON off — so the probe CAN see raw mode,
     which is what stops step 3 from being a false positive;
  3. after Ctrl-C and exit: the attributes are identical to the baseline.

Exit 0 = the terminal came back. 1 = it did not. 2 = the probe could not run
(binary missing, or raw mode never observable) — never a pass.
"""

import os
import pty
import select
import signal
import sys
import termios
import time
import shutil
import tempfile

BINARY = os.environ.get("GTASKS_BIN", "./target/debug/gtasks")


def describe(attrs):
    _iflag, oflag, _cflag, lflag, _is, _os, _cc = attrs
    return {
        "ECHO": bool(lflag & termios.ECHO),
        "ICANON": bool(lflag & termios.ICANON),
        "ISIG": bool(lflag & termios.ISIG),
        "IEXTEN": bool(lflag & termios.IEXTEN),
        "OPOST": bool(oflag & termios.OPOST),
    }


def is_raw(attrs):
    d = describe(attrs)
    return not d["ECHO"] and not d["ICANON"]


def child_is_raw(pid):
    """Look at the terminal the child actually has open."""
    try:
        for entry in os.listdir(f"/proc/{pid}/fd"):
            path = os.path.realpath(os.path.join(f"/proc/{pid}/fd", entry))
            if path.startswith("/dev/pts/"):
                fd = os.open(path, os.O_RDWR | os.O_NOCTTY)
                try:
                    raw = is_raw(termios.tcgetattr(fd))
                finally:
                    os.close(fd)
                if raw:
                    return True
    except OSError:
        return False
    return False


def main():
    if not os.path.exists(BINARY):
        print(f"SKIP: {BINARY} does not exist — build first")
        return 2

    workdir = tempfile.mkdtemp(prefix="gtasks-term-")
    env = dict(os.environ)
    env.update(
        TERM="xterm-256color",
        HOME=workdir,
        XDG_CONFIG_HOME=os.path.join(workdir, "config"),
        XDG_STATE_HOME=os.path.join(workdir, "state"),
        XDG_DATA_HOME=os.path.join(workdir, "data"),
    )

    master, slave = pty.openpty()
    baseline = termios.tcgetattr(slave)

    pid = os.fork()
    if pid == 0:  # child
        os.setsid()
        os.dup2(slave, 0)
        os.dup2(slave, 1)
        os.dup2(slave, 2)
        os.close(master)
        os.close(slave)
        try:
            os.execvpe(BINARY, [BINARY], env)
        finally:
            os._exit(127)

    # Drain the child's output and watch for raw mode.
    raw_seen = False
    deadline = time.time() + 5.0
    while time.time() < deadline and not raw_seen:
        r, _, _ = select.select([master], [], [], 0.1)
        if r:
            try:
                os.read(master, 8192)
            except OSError:
                break
        raw_seen = child_is_raw(pid)

    # Ctrl-C: the TUI must restore on the way out.
    os.write(master, b"\x03")
    status = None
    for _ in range(80):
        wpid, st = os.waitpid(pid, os.WNOHANG)
        if wpid:
            status = st
            break
        r, _, _ = select.select([master], [], [], 0.05)
        if r:
            try:
                os.read(master, 8192)
            except OSError:
                pass
        time.sleep(0.02)
    if status is None:
        os.kill(pid, signal.SIGKILL)
        os.waitpid(pid, 0)
        print("FAIL: the process did not exit on Ctrl-C")
        os.close(master)
        os.close(slave)
        shutil.rmtree(workdir, ignore_errors=True)
        return 1

    after = termios.tcgetattr(slave)
    restored = after == baseline

    print(f"baseline     : {describe(baseline)}")
    print(f"raw observed : {raw_seen}")
    print(f"after exit   : {describe(after)}")
    print(f"identical    : {restored}")

    os.close(master)
    os.close(slave)
    shutil.rmtree(workdir, ignore_errors=True)

    if not raw_seen:
        print("FAIL: raw mode never observed — the probe would be a false positive")
        return 2
    if not restored:
        print("FAIL: the terminal did not come back identical to the baseline")
        return 1
    print("PASS: the terminal is usable again")
    return 0


if __name__ == "__main__":
    sys.exit(main())
