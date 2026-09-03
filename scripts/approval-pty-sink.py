#!/usr/bin/env python3
"""Approval-channel pty harness for the smoke test.

`handover approve` with the tty channel injects one keystroke into the
recorded terminal device (`printf %s y > /dev/ttysNNN`) and then verifies
the session resumed by watching its transcript (mtime advance or the
permission marker disappearing). The fake agent has no terminal, so this
harness plays one:

  1. allocate a pty pair,
  2. publish the slave's device path for `handover attach --tty`,
  3. convert every injected keystroke into transcript activity — the same
     resumption a real agent's terminal performs after a permission prompt.

Usage: approval-pty-sink.py <device-path-out> <transcript>

The smoke test kills this process when it finishes; it also exits on its
own when the pty closes.
"""

import os
import pty
import sys


def main() -> int:
    if len(sys.argv) != 3:
        print("usage: approval-pty-sink.py <device-path-out> <transcript>", file=sys.stderr)
        return 2
    out_path, transcript = sys.argv[1], sys.argv[2]

    master, slave = pty.openpty()
    # macOS: /dev/ttysNNN — Linux: /dev/pts/N. Both are the shapes the
    # daemon's tty-channel validation accepts.
    with open(out_path, "w") as f:
        f.write(os.ttyname(slave) + "\n")

    # What the daemon injects, and the transcript activity it stands for.
    keystrokes = {b"y": "assistant: approved", b"n": "assistant: denied"}
    while True:
        try:
            data = os.read(master, 1024)
        except OSError:
            break  # pty closed
        if not data:
            break
        for key, line in keystrokes.items():
            if key in data:
                with open(transcript, "a") as f:
                    f.write(line + "\n")
    return 0


if __name__ == "__main__":
    sys.exit(main())
